use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_orchestration::{
    LockLease, LockMode, LockRequest, LockResource, OrchestrationRepository, RepositoryError,
    WorkflowExecution, WorkflowStatus,
};
use sqlx::Row;
use uuid::Uuid;

use crate::PostgresServiceDatabase;

#[derive(Clone)]
pub struct PostgresOrchestrationRepository {
    database: PostgresServiceDatabase,
}

impl PostgresOrchestrationRepository {
    #[must_use]
    pub fn new(database: PostgresServiceDatabase) -> Self {
        Self { database }
    }
}

#[async_trait]
impl OrchestrationRepository for PostgresOrchestrationRepository {
    async fn create_workflow(
        &self,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_workflows (
                id, session_id, version, status, data_json, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(execution.id)
        .bind(execution.session_id.to_string())
        .bind(as_i64(execution.version)?)
        .bind(workflow_status(execution.status))
        .bind(serde_json::to_value(execution).map_err(backend)?)
        .bind(execution.created_at)
        .bind(execution.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution.clone())
    }

    async fn get_workflow(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowExecution>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM sessionweft_workflows WHERE id = $1")
            .bind(workflow_id)
            .fetch_optional(&self.database.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .transpose()
    }

    async fn save_workflow(
        &self,
        expected_version: u64,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_workflows
            SET version = $1, status = $2, data_json = $3, updated_at = $4
            WHERE id = $5 AND version = $6
            "#,
        )
        .bind(as_i64(execution.version)?)
        .bind(workflow_status(execution.status))
        .bind(serde_json::to_value(execution).map_err(backend)?)
        .bind(execution.updated_at)
        .bind(execution.id)
        .bind(as_i64(expected_version)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM sessionweft_workflows WHERE id = $1",
            )
            .bind(execution.id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?;
            transaction.rollback().await.map_err(backend)?;
            return match actual {
                Some(actual) => Err(RepositoryError::VersionConflict {
                    expected: expected_version,
                    actual: as_u64(actual)?,
                }),
                None => Err(RepositoryError::WorkflowNotFound(execution.id)),
            };
        }
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution.clone())
    }

    async fn acquire_lock(
        &self,
        request: &LockRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, RepositoryError> {
        request.validate().map_err(backend)?;
        let workspace_id = request.resource.workspace_id();
        let now = Utc::now();
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_lock_guards (workspace_id, next_fencing_token)
            VALUES ($1, 1)
            ON CONFLICT (workspace_id) DO NOTHING
            "#,
        )
        .bind(workspace_id)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        let next_token = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT next_fencing_token
            FROM sessionweft_lock_guards
            WHERE workspace_id = $1
            FOR UPDATE
            "#,
        )
        .bind(workspace_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(backend)?;
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM sessionweft_locks
            WHERE workspace_id = $1 AND expires_at > $2
            FOR UPDATE
            "#,
        )
        .bind(workspace_id)
        .bind(now)
        .fetch_all(&mut *transaction)
        .await
        .map_err(backend)?;
        for row in rows {
            let lease: LockLease = serde_json::from_value(row.get("data_json")).map_err(backend)?;
            if lease.conflicts_with(request, now) {
                transaction.rollback().await.map_err(backend)?;
                return Err(RepositoryError::LockConflict {
                    owner_id: lease.owner_id,
                    resource: lease.resource,
                });
            }
        }
        let fencing_token = as_u64(next_token)?;
        let expires_at = now + Duration::seconds(i64::from(request.ttl_seconds));
        let lease = LockLease {
            lock_id: Uuid::new_v4(),
            session_id: request.session_id,
            owner_id: request.owner_id.clone(),
            resource: request.resource.clone(),
            mode: request.mode,
            fencing_token,
            acquired_at: now,
            expires_at,
        };
        sqlx::query(
            r#"
            UPDATE sessionweft_lock_guards
            SET next_fencing_token = next_fencing_token + 1
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_locks (
                lock_id, session_id, workspace_id, owner_id, mode,
                fencing_token, resource_json, data_json, acquired_at, expires_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(lease.lock_id)
        .bind(lease.session_id.to_string())
        .bind(workspace_id)
        .bind(&lease.owner_id)
        .bind(lock_mode(lease.mode))
        .bind(as_i64(lease.fencing_token)?)
        .bind(serde_json::to_value(&lease.resource).map_err(backend)?)
        .bind(serde_json::to_value(&lease).map_err(backend)?)
        .bind(lease.acquired_at)
        .bind(lease.expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        let event = lock_event("lock.acquired", &lease, correlation_id, actor_id);
        PostgresServiceDatabase::insert_events(&mut transaction, &[event])
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(lease)
    }

    async fn get_lock(&self, lock_id: Uuid) -> Result<Option<LockLease>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM sessionweft_locks WHERE lock_id = $1")
            .bind(lock_id)
            .fetch_optional(&self.database.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .transpose()
    }

    async fn list_session_locks(
        &self,
        session_id: SessionId,
        workspace_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<LockLease>, RepositoryError> {
        read_locks(
            &self.database,
            "session_id = $1 AND workspace_id = $2 AND expires_at > $3",
            session_id.to_string(),
            workspace_id,
            now,
        )
        .await
    }

    async fn heartbeat_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        ttl_seconds: u32,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, RepositoryError> {
        if !(1..=3_600).contains(&ttl_seconds) {
            return Err(RepositoryError::Backend(
                "lock TTL must be between 1 and 3600 seconds".into(),
            ));
        }
        let now = Utc::now();
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let row =
            sqlx::query("SELECT data_json FROM sessionweft_locks WHERE lock_id = $1 FOR UPDATE")
                .bind(lock_id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(backend)?
                .ok_or(RepositoryError::LockNotFound(lock_id))?;
        let mut lease: LockLease = serde_json::from_value(row.get("data_json")).map_err(backend)?;
        if lease.owner_id != owner_id
            || lease.fencing_token != fencing_token
            || lease.expires_at <= now
        {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::StaleFence);
        }
        lease.expires_at = now + Duration::seconds(i64::from(ttl_seconds));
        sqlx::query(
            r#"
            UPDATE sessionweft_locks
            SET data_json = $1, expires_at = $2
            WHERE lock_id = $3 AND owner_id = $4 AND fencing_token = $5
            "#,
        )
        .bind(serde_json::to_value(&lease).map_err(backend)?)
        .bind(lease.expires_at)
        .bind(lock_id)
        .bind(owner_id)
        .bind(as_i64(fencing_token)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        let event = lock_event("lock.heartbeat", &lease, correlation_id, actor_id);
        PostgresServiceDatabase::insert_events(&mut transaction, &[event])
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(lease)
    }

    async fn release_lock(
        &self,
        lock_id: Uuid,
        owner_id: &str,
        fencing_token: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let row =
            sqlx::query("SELECT data_json FROM sessionweft_locks WHERE lock_id = $1 FOR UPDATE")
                .bind(lock_id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(backend)?
                .ok_or(RepositoryError::LockNotFound(lock_id))?;
        let lease: LockLease = serde_json::from_value(row.get("data_json")).map_err(backend)?;
        if lease.owner_id != owner_id || lease.fencing_token != fencing_token {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::StaleFence);
        }
        sqlx::query(
            "DELETE FROM sessionweft_locks WHERE lock_id = $1 AND owner_id = $2 AND fencing_token = $3",
        )
        .bind(lock_id)
        .bind(owner_id)
        .bind(as_i64(fencing_token)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        let event = lock_event("lock.released", &lease, correlation_id, actor_id);
        PostgresServiceDatabase::insert_events(&mut transaction, &[event])
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }

    async fn validate_fence(
        &self,
        resource: &LockResource,
        owner_id: &str,
        fencing_token: u64,
        now: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM sessionweft_locks
            WHERE workspace_id = $1 AND owner_id = $2
              AND fencing_token = $3 AND expires_at > $4
            "#,
        )
        .bind(resource.workspace_id())
        .bind(owner_id)
        .bind(as_i64(fencing_token)?)
        .bind(now)
        .fetch_all(&self.database.pool)
        .await
        .map_err(backend)?;
        let valid = rows.into_iter().any(|row| {
            serde_json::from_value::<LockLease>(row.get("data_json"))
                .is_ok_and(|lease| lease.resource == *resource)
        });
        if valid {
            Ok(())
        } else {
            Err(RepositoryError::StaleFence)
        }
    }

    async fn list_locks(
        &self,
        workspace_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<LockLease>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM sessionweft_locks
            WHERE workspace_id = $1 AND expires_at > $2
            ORDER BY acquired_at ASC
            "#,
        )
        .bind(workspace_id)
        .bind(now)
        .fetch_all(&self.database.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .collect()
    }
}

async fn read_locks(
    database: &PostgresServiceDatabase,
    predicate: &str,
    session_id: String,
    workspace_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<LockLease>, RepositoryError> {
    let query = format!(
        "SELECT data_json FROM sessionweft_locks WHERE {predicate} ORDER BY acquired_at ASC"
    );
    let rows = sqlx::query(&query)
        .bind(session_id)
        .bind(workspace_id)
        .bind(now)
        .fetch_all(&database.pool)
        .await
        .map_err(backend)?;
    rows.into_iter()
        .map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
        .collect()
}

fn lock_event(
    event_type: &str,
    lease: &LockLease,
    correlation_id: Uuid,
    actor_id: Option<&str>,
) -> EventEnvelope {
    EventEnvelope::new(
        event_type,
        Some(lease.session_id),
        correlation_id,
        actor_id,
        serde_json::json!({
            "lock_id": lease.lock_id,
            "owner_id": lease.owner_id,
            "resource": lease.resource,
            "mode": lease.mode,
            "fencing_token": lease.fencing_token,
            "expires_at": lease.expires_at,
        }),
    )
}

const fn workflow_status(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Running => "running",
        WorkflowStatus::Succeeded => "succeeded",
        WorkflowStatus::Failed => "failed",
        WorkflowStatus::Cancelled => "cancelled",
    }
}

const fn lock_mode(mode: LockMode) -> &'static str {
    match mode {
        LockMode::Shared => "shared",
        LockMode::Exclusive => "exclusive",
    }
}

fn as_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(backend)
}

fn as_u64(value: i64) -> Result<u64, RepositoryError> {
    u64::try_from(value).map_err(backend)
}

fn backend(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Backend(error.to_string())
}
