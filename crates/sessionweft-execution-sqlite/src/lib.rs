use std::{str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_execution::{AgentRecord, AgentRepository, RepositoryError};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteAgentRepository {
    pool: SqlitePool,
}

impl SqliteAgentRepository {
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;
        let repository = Self { pool };
        repository.migrate().await?;
        Ok(repository)
    }

    async fn migrate(&self) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS agent_records (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                status TEXT NOT NULL,
                heartbeat_at TEXT NOT NULL,
                current_task_id TEXT,
                updated_at TEXT NOT NULL,
                data_json TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_agents_stale
            ON agent_records (status, heartbeat_at, updated_at)
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

    async fn insert_events(
        transaction: &mut Transaction<'_, Sqlite>,
        events: &[EventEnvelope],
    ) -> Result<(), RepositoryError> {
        for event in events {
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
            .bind(serde_json::to_string(event).map_err(backend)?)
            .bind(event.correlation_id.to_string())
            .bind(event.occurred_at.to_rfc3339())
            .execute(&mut **transaction)
            .await
            .map_err(backend)?;
        }
        Ok(())
    }
}

#[async_trait]
impl AgentRepository for SqliteAgentRepository {
    async fn create(
        &self,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO agent_records (
                id, session_id, version, status, heartbeat_at,
                current_task_id, updated_at, data_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(agent.id.to_string())
        .bind(agent.session_id.to_string())
        .bind(to_i64(agent.version)?)
        .bind(format!("{:?}", agent.status).to_lowercase())
        .bind(agent.heartbeat_at.to_rfc3339())
        .bind(&agent.current_task_id)
        .bind(agent.updated_at.to_rfc3339())
        .bind(serde_json::to_string(agent).map_err(backend)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(agent.clone())
    }

    async fn get(&self, agent_id: Uuid) -> Result<Option<AgentRecord>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM agent_records WHERE id = ?")
            .bind(agent_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn save(
        &self,
        expected_version: u64,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let result = sqlx::query(
            r#"
            UPDATE agent_records
            SET version = ?, status = ?, heartbeat_at = ?, current_task_id = ?,
                updated_at = ?, data_json = ?
            WHERE id = ? AND version = ?
            "#,
        )
        .bind(to_i64(agent.version)?)
        .bind(format!("{:?}", agent.status).to_lowercase())
        .bind(agent.heartbeat_at.to_rfc3339())
        .bind(&agent.current_task_id)
        .bind(agent.updated_at.to_rfc3339())
        .bind(serde_json::to_string(agent).map_err(backend)?)
        .bind(agent.id.to_string())
        .bind(to_i64(expected_version)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            let actual =
                sqlx::query_scalar::<_, i64>("SELECT version FROM agent_records WHERE id = ?")
                    .bind(agent.id.to_string())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(backend)?;
            transaction.rollback().await.map_err(backend)?;
            return match actual {
                Some(actual) => Err(RepositoryError::VersionConflict {
                    expected: expected_version,
                    actual: to_u64(actual)?,
                }),
                None => Err(RepositoryError::AgentNotFound(agent.id)),
            };
        }
        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(agent.clone())
    }

    async fn stale_agents(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<AgentRecord>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM agent_records
            WHERE status = 'running'
            ORDER BY heartbeat_at ASC
            LIMIT ?
            "#,
        )
        .bind(i64::try_from(limit.clamp(1, 10_000)).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<AgentRecord>(row.get::<&str, _>("data_json"))
                    .map_err(backend)
            })
            .filter(|record| match record {
                Ok(agent) => agent.is_stale_at(now),
                Err(_) => true,
            })
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
    use std::{collections::BTreeSet, sync::Arc};

    use chrono::Duration;
    use sessionweft_core::SessionId;
    use sessionweft_execution::{AgentManifest, AgentRole, AgentService, AgentStatus};

    use super::*;

    fn manifest() -> AgentManifest {
        AgentManifest {
            name: "worker".into(),
            role: AgentRole::Worker,
            capabilities: BTreeSet::new(),
            heartbeat_timeout_seconds: 5,
        }
    }

    #[tokio::test]
    async fn agent_state_and_outbox_commit_atomically() {
        let repository = Arc::new(
            SqliteAgentRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = AgentService::new(Arc::clone(&repository));
        let agent = service
            .register(SessionId::new(), manifest(), Uuid::new_v4(), Some("test"))
            .await
            .expect("register");
        let agent = service
            .mutate(agent.id, agent.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start");
        assert_eq!(agent.status, AgentStatus::Running);
        assert_eq!(agent.version, 1);
        let outbox_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbox")
            .fetch_one(&repository.pool)
            .await
            .expect("outbox count");
        assert_eq!(outbox_count, 2);
    }

    #[tokio::test]
    async fn stale_agents_are_discoverable_for_handover() {
        let repository = Arc::new(
            SqliteAgentRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = AgentService::new(Arc::clone(&repository));
        let agent = service
            .register(SessionId::new(), manifest(), Uuid::new_v4(), Some("test"))
            .await
            .expect("register");
        let agent = service
            .mutate(agent.id, agent.version, |agent| {
                Ok(vec![agent.start(
                    agent.version,
                    Uuid::new_v4(),
                    Some("test"),
                )?])
            })
            .await
            .expect("start");
        let stale = repository
            .stale_agents(agent.heartbeat_at + Duration::seconds(6), 10)
            .await
            .expect("stale agents");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, agent.id);
    }
}
