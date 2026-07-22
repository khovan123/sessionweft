use std::{str::FromStr, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::Utc;
use sessionweft_core::EventEnvelope;
use sessionweft_mcp::{
    ConsumeApprovalCommand, IssueApprovalCommand, McpApprovalRecord, McpApprovalRepository,
    McpApprovalRepositoryError,
};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteMcpApprovalRepository {
    pool: SqlitePool,
}

impl SqliteMcpApprovalRepository {
    pub async fn connect(database_url: &str) -> Result<Self, McpApprovalRepositoryError> {
        let is_memory = database_url.contains(":memory:");
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;
        let repository = Self { pool };
        repository.migrate().await?;
        Ok(repository)
    }

    async fn migrate(&self) -> Result<(), McpApprovalRepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS mcp_approval_grants (
                grant_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                consumed_at TEXT,
                consumed_by_invocation TEXT,
                data_json TEXT NOT NULL,
                issued_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_mcp_approval_scope
            ON mcp_approval_grants (session_id, agent_id, tool_name, expires_at)
            "#,
        )
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
        Ok(())
    }

    async fn load(
        transaction: &mut Transaction<'_, Sqlite>,
        grant_id: Uuid,
    ) -> Result<McpApprovalRecord, McpApprovalRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM mcp_approval_grants WHERE grant_id = ?")
            .bind(grant_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(McpApprovalRepositoryError::NotFound(grant_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn insert_event(
        transaction: &mut Transaction<'_, Sqlite>,
        record: &McpApprovalRecord,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), McpApprovalRepositoryError> {
        let event = EventEnvelope::new(
            event_type,
            Some(record.grant.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "grant_id": record.grant.id,
                "agent_id": record.grant.agent_id,
                "tool_name": record.grant.tool_name,
                "expires_at": record.grant.expires_at,
                "consumed_at": record.consumed_at,
                "consumed_by_invocation": record.consumed_by_invocation,
            }),
        );
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
        .bind(serde_json::to_string(&event).map_err(backend)?)
        .bind(event.correlation_id.to_string())
        .bind(event.occurred_at.to_rfc3339())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl McpApprovalRepository for SqliteMcpApprovalRepository {
    async fn issue(
        &self,
        command: &IssueApprovalCommand,
    ) -> Result<McpApprovalRecord, McpApprovalRepositoryError> {
        let record = McpApprovalRecord::new(
            command.grant.clone(),
            command.issued_at,
            command.actor_id.clone(),
        )
        .map_err(|error| McpApprovalRepositoryError::Conflict(error.to_string()))?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let result = sqlx::query(
            r#"
            INSERT INTO mcp_approval_grants (
                grant_id, session_id, agent_id, tool_name, expires_at,
                consumed_at, consumed_by_invocation, data_json, issued_at
            ) VALUES (?, ?, ?, ?, ?, NULL, NULL, ?, ?)
            "#,
        )
        .bind(record.grant.id.to_string())
        .bind(record.grant.session_id.to_string())
        .bind(record.grant.agent_id.to_string())
        .bind(&record.grant.tool_name)
        .bind(record.grant.expires_at.to_rfc3339())
        .bind(serde_json::to_string(&record).map_err(backend)?)
        .bind(record.issued_at.to_rfc3339())
        .execute(&mut *transaction)
        .await;
        match result {
            Ok(_) => {}
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                transaction.rollback().await.map_err(backend)?;
                return Err(McpApprovalRepositoryError::Conflict(format!(
                    "approval {} already exists",
                    record.grant.id
                )));
            }
            Err(error) => return Err(backend(error)),
        }
        Self::insert_event(
            &mut transaction,
            &record,
            "mcp.approval_issued",
            command.correlation_id,
            command.actor_id.as_deref(),
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(record)
    }

    async fn consume(
        &self,
        command: &ConsumeApprovalCommand,
    ) -> Result<McpApprovalRecord, McpApprovalRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut record = Self::load(&mut transaction, command.grant_id).await?;
        if record.consumed_at.is_some() {
            transaction.rollback().await.map_err(backend)?;
            return Err(McpApprovalRepositoryError::AlreadyConsumed(
                command.grant_id,
            ));
        }
        if record.grant.expires_at <= command.consumed_at {
            transaction.rollback().await.map_err(backend)?;
            return Err(McpApprovalRepositoryError::Expired(command.grant_id));
        }
        if record.grant.session_id != command.session_id
            || record.grant.agent_id != command.agent_id
            || record.grant.tool_name != command.tool_name
        {
            transaction.rollback().await.map_err(backend)?;
            return Err(McpApprovalRepositoryError::ScopeMismatch(command.grant_id));
        }
        record.consumed_at = Some(command.consumed_at);
        record.consumed_by_invocation = Some(command.invocation_correlation_id);
        let result = sqlx::query(
            r#"
            UPDATE mcp_approval_grants
            SET consumed_at = ?, consumed_by_invocation = ?, data_json = ?
            WHERE grant_id = ? AND consumed_at IS NULL
            "#,
        )
        .bind(command.consumed_at.to_rfc3339())
        .bind(command.invocation_correlation_id.to_string())
        .bind(serde_json::to_string(&record).map_err(backend)?)
        .bind(command.grant_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(backend)?;
            return Err(McpApprovalRepositoryError::AlreadyConsumed(
                command.grant_id,
            ));
        }
        Self::insert_event(
            &mut transaction,
            &record,
            "mcp.approval_consumed",
            command.correlation_id,
            command.actor_id.as_deref(),
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(record)
    }

    async fn get(
        &self,
        grant_id: Uuid,
    ) -> Result<Option<McpApprovalRecord>, McpApprovalRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM mcp_approval_grants WHERE grant_id = ?")
            .bind(grant_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }
}

fn backend(error: impl std::fmt::Display) -> McpApprovalRepositoryError {
    McpApprovalRepositoryError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use sessionweft_core::SessionId;
    use sessionweft_execution::ApprovalGrant;
    use sessionweft_mcp::{
        ConsumeApprovalCommand, IssueApprovalCommand, McpApprovalRepository,
        McpApprovalRepositoryError,
    };

    use super::*;

    #[tokio::test]
    async fn consumes_grant_once_and_persists_audit_state() {
        let repository = SqliteMcpApprovalRepository::connect("sqlite::memory:")
            .await
            .expect("repository");
        let now = Utc::now();
        let grant = ApprovalGrant {
            id: Uuid::new_v4(),
            session_id: SessionId::new(),
            agent_id: Uuid::new_v4(),
            tool_name: "mcp.write".into(),
            expires_at: now + Duration::minutes(5),
        };
        repository
            .issue(&IssueApprovalCommand {
                grant: grant.clone(),
                issued_at: now,
                actor_id: Some("reviewer".into()),
                correlation_id: Uuid::new_v4(),
            })
            .await
            .expect("issue");
        let command = ConsumeApprovalCommand {
            grant_id: grant.id,
            session_id: grant.session_id,
            agent_id: grant.agent_id,
            tool_name: grant.tool_name.clone(),
            invocation_correlation_id: Uuid::new_v4(),
            consumed_at: now + Duration::seconds(1),
            actor_id: Some("runtime".into()),
            correlation_id: Uuid::new_v4(),
        };
        let consumed = repository.consume(&command).await.expect("consume");
        assert_eq!(consumed.consumed_at, Some(command.consumed_at));
        assert_eq!(
            repository.consume(&command).await.expect_err("single use"),
            McpApprovalRepositoryError::AlreadyConsumed(grant.id)
        );
        let stored = repository
            .get(grant.id)
            .await
            .expect("read")
            .expect("record");
        assert_eq!(
            stored.consumed_by_invocation,
            Some(command.invocation_correlation_id)
        );
    }

    #[tokio::test]
    async fn rejects_scope_mismatch_before_consumption() {
        let repository = SqliteMcpApprovalRepository::connect("sqlite::memory:")
            .await
            .expect("repository");
        let now = Utc::now();
        let grant = ApprovalGrant {
            id: Uuid::new_v4(),
            session_id: SessionId::new(),
            agent_id: Uuid::new_v4(),
            tool_name: "mcp.read".into(),
            expires_at: now + Duration::minutes(5),
        };
        repository
            .issue(&IssueApprovalCommand {
                grant: grant.clone(),
                issued_at: now,
                actor_id: None,
                correlation_id: Uuid::new_v4(),
            })
            .await
            .expect("issue");
        let result = repository
            .consume(&ConsumeApprovalCommand {
                grant_id: grant.id,
                session_id: grant.session_id,
                agent_id: Uuid::new_v4(),
                tool_name: grant.tool_name.clone(),
                invocation_correlation_id: Uuid::new_v4(),
                consumed_at: now + Duration::seconds(1),
                actor_id: None,
                correlation_id: Uuid::new_v4(),
            })
            .await;
        assert!(matches!(
            result,
            Err(McpApprovalRepositoryError::ScopeMismatch(id)) if id == grant.id
        ));
        assert!(
            repository
                .get(grant.id)
                .await
                .expect("read")
                .expect("record")
                .consumed_at
                .is_none()
        );
    }
}
