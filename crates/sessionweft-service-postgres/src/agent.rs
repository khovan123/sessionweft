use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_execution::{AgentRecord, AgentRepository, AgentStatus, RepositoryError};
use sqlx::Row;
use uuid::Uuid;

use crate::PostgresServiceDatabase;

#[derive(Clone)]
pub struct PostgresAgentRepository {
    database: PostgresServiceDatabase,
}

impl PostgresAgentRepository {
    #[must_use]
    pub fn new(database: PostgresServiceDatabase) -> Self {
        Self { database }
    }
}

#[async_trait]
impl AgentRepository for PostgresAgentRepository {
    async fn create(
        &self,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_agents (
                id, session_id, version, status, heartbeat_at,
                data_json, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(agent.id)
        .bind(agent.session_id.to_string())
        .bind(as_i64(agent.version)?)
        .bind(status_name(agent.status))
        .bind(agent.heartbeat_at)
        .bind(serde_json::to_value(agent).map_err(backend)?)
        .bind(agent.created_at)
        .bind(agent.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(agent.clone())
    }

    async fn get(&self, agent_id: Uuid) -> Result<Option<AgentRecord>, RepositoryError> {
        let row = sqlx::query("SELECT data_json FROM sessionweft_agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&self.database.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .transpose()
    }

    async fn save(
        &self,
        expected_version: u64,
        agent: &AgentRecord,
        events: &[EventEnvelope],
    ) -> Result<AgentRecord, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_agents
            SET version = $1, status = $2, heartbeat_at = $3,
                data_json = $4, updated_at = $5
            WHERE id = $6 AND version = $7
            "#,
        )
        .bind(as_i64(agent.version)?)
        .bind(status_name(agent.status))
        .bind(agent.heartbeat_at)
        .bind(serde_json::to_value(agent).map_err(backend)?)
        .bind(agent.updated_at)
        .bind(agent.id)
        .bind(as_i64(expected_version)?)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM sessionweft_agents WHERE id = $1",
            )
            .bind(agent.id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?;
            transaction.rollback().await.map_err(backend)?;
            return match actual {
                Some(actual) => Err(RepositoryError::VersionConflict {
                    expected: expected_version,
                    actual: as_u64(actual)?,
                }),
                None => Err(RepositoryError::AgentNotFound(agent.id)),
            };
        }
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
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
            FROM sessionweft_agents
            WHERE status = 'running' AND heartbeat_at < $1
            ORDER BY heartbeat_at ASC
            LIMIT $2
            "#,
        )
        .bind(now)
        .bind(i64::try_from(limit.clamp(1, 1_000)).map_err(backend)?)
        .fetch_all(&self.database.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .collect::<Result<Vec<AgentRecord>, _>>()
            .map(|agents| {
                agents
                    .into_iter()
                    .filter(|agent| agent.is_stale_at(now))
                    .collect()
            })
    }
}

const fn status_name(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Registered => "registered",
        AgentStatus::Running => "running",
        AgentStatus::Stopped => "stopped",
        AgentStatus::Failed => "failed",
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
