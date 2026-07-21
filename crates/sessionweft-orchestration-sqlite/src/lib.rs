use std::{str::FromStr, sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_orchestration::{
    LockLease, LockRequest, LockResource, OrchestrationRepository, RepositoryError,
    WorkflowExecution,
};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteOrchestrationRepository {
    pool: SqlitePool,
    lock_guard: Arc<Mutex<()>>,
}

impl SqliteOrchestrationRepository {
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;
        let repository = Self {
            pool,
            lock_guard: Arc::new(Mutex::new(())),
        };
        repository.migrate().await?;
        Ok(repository)
    }

    async fn migrate(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS workflow_executions (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                status TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lock_leases (
                lock_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                fencing_token INTEGER NOT NULL UNIQUE,
                expires_at TEXT NOT NULL,
                data_json TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lock_sequence (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                value INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query("INSERT OR IGNORE INTO lock_sequence (id, value) VALUES (1, 0)")
            .execute(&self.pool)
            .await
            .map_err(backend)?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS outbox (
                event_id TEXT PRIMARY KEY,
                session_id TEXT,
                event_type TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                correlation_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                published_at TEXT,
                publish_attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_workflows_session ON workflow_executions (session_id, updated_at)",
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_locks_workspace_expiry ON lock_leases (workspace_id, expires_at)",
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn insert_events(
        transaction: &mut Transaction<'_, Sqlite>,
        events: &[EventEnvelope],
    ) -> Result<(), RepositoryError> {
        for event in events {
            let payload_json = serde_json::to_string(event).map_err(backend)?;
            sqlx::query(
                r#"
                INSERT INTO outbox (
                    event_id, session_id, event_type, schema_version,
                    payload_json, correlation_id, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(event.event_id.to_string())
            .bind(event.session_id.map(|value| value.to_string()))
            .bind(&event.event_type)
            .bind(i64::from(event.schema_version))
            .bind(payload_json)
            .bind(event.correlation_id.to_string())
            .bind(event.occurred_at.to_rfc3339())
            .execute(&mut **transaction)
            .await
            .map_err(backend)?;
        }
        Ok(())
    }

    async fn active_locks(
        transaction: &mut Transaction<'_, Sqlite>,
        workspace_id: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<LockLease>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM lock_leases
            WHERE workspace_id = ? AND expires_at > ?
            ORDER BY fencing_token ASC
            "#,
        )
        .bind(workspace_id)
        .bind(now.to_rfc3339())
        .fetch_all(&mut **transaction)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .collect()
    }

    async fn load_lock(
        transaction: &mut Transaction<'_, Sqlite>,
        lock_id: Uuid,
    ) -> Result<LockLease, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(lock_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(RepositoryError::LockNotFound(lock_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }
}

#[async_trait]
impl OrchestrationRepository for SqliteOrchestrationRepository {
    async fn create_workflow(
        &self,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError> {
        let data_json = serde_json::to_string(execution).map_err(backend)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO workflow_executions (
                id, session_id, version, status, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(execution.id.to_string())
        .bind(execution.session_id.to_string())
        .bind(to_i64(execution.version)?)
        .bind(format!("{:?}", execution.status).to_lowercase())
        .bind(data_json)
        .bind(execution.created_at.to_rfc3339())
        .bind(execution.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution.clone())
    }

    async fn get_workflow(
        &self,
        workflow_id: Uuid,
    ) -> Result<Option<WorkflowExecution>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM workflow_executions WHERE id = ?")
            .bind(workflow_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|value| serde_json::from_str(value.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn save_workflow(
        &self,
        expected_version: u64,
        execution: &WorkflowExecution,
        events: &[EventEnvelope],
    ) -> Result<WorkflowExecution, RepositoryError> {
        let data_json = serde_json::to_string(execution).map_err(backend)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let result = sqlx::query(
            r#"
            UPDATE workflow_executions
            SET version = ?, status = ?, data_json = ?, updated_at = ?
            WHERE id = ? AND version = ?
            "#,
        )
        .bind(to_i64(execution.version)?)
        .bind(format!("{:?}", execution.status).to_lowercase())
        .bind(data_json)
        .bind(execution.updated_at.to_rfc3339())
        .bind(execution.id.to_string())
        .bind(to_i64(expected_version)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;

        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM workflow_executions WHERE id = ?",
            )
            .bind(execution.id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?;
            transaction.rollback().await.map_err(backend)?;
            return match actual {
                Some(actual) => Err(RepositoryError::VersionConflict {
                    expected: expected_version,
                    actual: to_u64(actual)?,
                }),
                None => Err(RepositoryError::WorkflowNotFound(execution.id)),
            };
        }

        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(execution.clone())
    }

    async fn acquire_lock(
        &self,
        request: &LockRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<LockLease, RepositoryError> {
        let _guard = self.lock_guard.lock().await;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        sqlx::query("DELETE FROM lock_leases WHERE expires_at <= ?")
            .bind(now.to_rfc3339())
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;

        for existing in
            Self::active_locks(&mut transaction, request.resource.workspace_id(), now).await?
        {
            if existing.conflicts_with(request, now) {
                transaction.rollback().await.map_err(backend)?;
                return Err(RepositoryError::LockConflict {
                    owner_id: existing.owner_id,
                    resource: existing.resource,
                });
            }
        }

        sqlx::query("UPDATE lock_sequence SET value = value + 1 WHERE id = 1")
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
        let fencing_token =
            sqlx::query_scalar::<_, i64>("SELECT value FROM lock_sequence WHERE id = 1")
                .fetch_one(&mut *transaction)
                .await
                .map_err(backend)
                .and_then(to_u64)?;

        let lease = LockLease {
            lock_id: Uuid::new_v4(),
            session_id: request.session_id,
            owner_id: request.owner_id.clone(),
            resource: request.resource.clone(),
            mode: request.mode,
            fencing_token,
            acquired_at: now,
            expires_at: now + Duration::seconds(i64::from(request.ttl_seconds)),
        };
        let data_json = serde_json::to_string(&lease).map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO lock_leases (
                lock_id, session_id, workspace_id, owner_id,
                fencing_token, expires_at, data_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(lease.lock_id.to_string())
        .bind(lease.session_id.to_string())
        .bind(lease.resource.workspace_id())
        .bind(&lease.owner_id)
        .bind(to_i64(lease.fencing_token)?)
        .bind(lease.expires_at.to_rfc3339())
        .bind(data_json)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;

        let event = EventEnvelope::new(
            "lock.acquired",
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
        );
        Self::insert_events(&mut transaction, &[event]).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(lease)
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
        let _guard = self.lock_guard.lock().await;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut lease = Self::load_lock(&mut transaction, lock_id).await?;
        if lease.owner_id != owner_id
            || lease.fencing_token != fencing_token
            || lease.expires_at <= now
        {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::StaleFence);
        }
        lease.expires_at = now + Duration::seconds(i64::from(ttl_seconds));
        let data_json = serde_json::to_string(&lease).map_err(backend)?;
        sqlx::query("UPDATE lock_leases SET expires_at = ?, data_json = ? WHERE lock_id = ?")
            .bind(lease.expires_at.to_rfc3339())
            .bind(data_json)
            .bind(lock_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
        let event = EventEnvelope::new(
            "lock.heartbeat",
            Some(lease.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "lock_id": lease.lock_id,
                "fencing_token": lease.fencing_token,
                "expires_at": lease.expires_at,
            }),
        );
        Self::insert_events(&mut transaction, &[event]).await?;
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
        let _guard = self.lock_guard.lock().await;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let lease = Self::load_lock(&mut transaction, lock_id).await?;
        if lease.owner_id != owner_id || lease.fencing_token != fencing_token {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::StaleFence);
        }
        sqlx::query("DELETE FROM lock_leases WHERE lock_id = ?")
            .bind(lock_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
        let event = EventEnvelope::new(
            "lock.released",
            Some(lease.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "lock_id": lease.lock_id,
                "owner_id": lease.owner_id,
                "resource": lease.resource,
                "fencing_token": lease.fencing_token,
            }),
        );
        Self::insert_events(&mut transaction, &[event]).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }

    async fn validate_fence(
        &self,
        resource: &LockResource,
        owner_id: &str,
        fencing_token: u64,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM lock_leases
            WHERE workspace_id = ? AND owner_id = ? AND fencing_token = ? AND expires_at > ?
            "#,
        )
        .bind(resource.workspace_id())
        .bind(owner_id)
        .bind(to_i64(fencing_token)?)
        .bind(now.to_rfc3339())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let valid = rows.into_iter().any(|row| {
            serde_json::from_str::<LockLease>(row.get::<&str, _>("data_json"))
                .is_ok_and(|lease| lease.resource.overlaps(resource))
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
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<LockLease>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM lock_leases
            WHERE workspace_id = ? AND expires_at > ?
            ORDER BY fencing_token ASC
            "#,
        )
        .bind(workspace_id)
        .bind(now.to_rfc3339())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .collect()
    }
}

fn backend(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Backend(error.to_string())
}

fn to_i64(value: u64) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(backend)
}

fn to_u64(value: i64) -> Result<u64, RepositoryError> {
    u64::try_from(value).map_err(backend)
}

#[cfg(test)]
mod tests {
    use sessionweft_orchestration::{
        LockMode, OrchestrationService, WorkflowDefinition, WorkflowNodeDefinition,
        WorkflowNodeKind,
    };

    use super::*;

    fn workflow_definition() -> WorkflowDefinition {
        WorkflowDefinition {
            name: "test".into(),
            version: 1,
            nodes: vec![
                WorkflowNodeDefinition {
                    id: "plan".into(),
                    kind: WorkflowNodeKind::Task,
                    dependencies: vec![],
                    max_attempts: 1,
                    continue_on_failure: false,
                    fallback: None,
                },
                WorkflowNodeDefinition {
                    id: "review".into(),
                    kind: WorkflowNodeKind::Approval,
                    dependencies: vec!["plan".into()],
                    max_attempts: 1,
                    continue_on_failure: false,
                    fallback: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn workflow_state_and_events_commit_together() {
        let repository = Arc::new(
            SqliteOrchestrationRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = OrchestrationService::new(Arc::clone(&repository));
        let execution = service
            .create_workflow(
                sessionweft_core::SessionId::new(),
                workflow_definition(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("create");
        let execution = service
            .start_node(
                execution.id,
                execution.version,
                "plan",
                "planner",
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("start");
        let execution = service
            .complete_node(
                execution.id,
                execution.version,
                "plan",
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("complete");
        assert_eq!(
            execution.nodes["review"].status,
            sessionweft_orchestration::WorkflowNodeStatus::WaitingApproval
        );
    }

    #[tokio::test]
    async fn exclusive_parent_lock_blocks_child_and_fence_expires() {
        let repository = Arc::new(
            SqliteOrchestrationRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = OrchestrationService::new(Arc::clone(&repository));
        let session_id = sessionweft_core::SessionId::new();
        let parent = LockRequest {
            session_id,
            owner_id: "worker-a".into(),
            resource: LockResource::Directory {
                workspace_id: "workspace".into(),
                path: "src".into(),
            },
            mode: LockMode::Exclusive,
            ttl_seconds: 1,
        };
        let lease = service
            .acquire_lock(&parent, Uuid::new_v4(), Some("test"))
            .await
            .expect("parent lock");
        let child = LockRequest {
            session_id,
            owner_id: "worker-b".into(),
            resource: LockResource::File {
                workspace_id: "workspace".into(),
                path: "src/lib.rs".into(),
            },
            mode: LockMode::Shared,
            ttl_seconds: 30,
        };
        let conflict = service
            .acquire_lock(&child, Uuid::new_v4(), Some("test"))
            .await
            .expect_err("lock conflict");
        assert!(matches!(
            conflict,
            sessionweft_orchestration::OrchestrationError::Repository(
                RepositoryError::LockConflict { .. }
            )
        ));

        let expired = repository
            .validate_fence(
                &parent.resource,
                "worker-a",
                lease.fencing_token,
                lease.expires_at + Duration::seconds(1),
            )
            .await;
        assert!(matches!(expired, Err(RepositoryError::StaleFence)));
    }
}
