use async_trait::async_trait;
use sessionweft_core::{EventEnvelope, Session, SessionId};
use sessionweft_storage::{OutboxRecord, SessionRepository, StorageError};
use sqlx::Row;
use uuid::Uuid;

use crate::{PostgresServiceDatabase, ServiceDatabaseError};

#[derive(Clone)]
pub struct PostgresSessionRepository {
    database: PostgresServiceDatabase,
}

impl PostgresSessionRepository {
    #[must_use]
    pub fn new(database: PostgresServiceDatabase) -> Self {
        Self { database }
    }

    #[must_use]
    pub fn database(&self) -> &PostgresServiceDatabase {
        &self.database
    }
}

#[async_trait]
impl SessionRepository for PostgresSessionRepository {
    async fn create(
        &self,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError> {
        let mut transaction = self.database.pool.begin().await?;
        sqlx::query(
            r#"
            INSERT INTO sessionweft_sessions (
                id, version, status, title, data_json, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(session.id.to_string())
        .bind(as_i64(session.version)?)
        .bind(format!("{:?}", session.status).to_lowercase())
        .bind(&session.title)
        .bind(serde_json::to_value(session)?)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&mut *transaction)
        .await?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(map_service)?;
        transaction.commit().await?;
        Ok(session.clone())
    }

    async fn get(&self, session_id: SessionId) -> Result<Option<Session>, StorageError> {
        let row = sqlx::query("SELECT data_json FROM sessionweft_sessions WHERE id = $1")
            .bind(session_id.to_string())
            .fetch_optional(&self.database.pool)
            .await?;
        row.map(|row| serde_json::from_value(row.get("data_json")).map_err(StorageError::from))
            .transpose()
    }

    async fn list(&self, limit: u32) -> Result<Vec<Session>, StorageError> {
        let rows = sqlx::query(
            "SELECT data_json FROM sessionweft_sessions ORDER BY updated_at DESC LIMIT $1",
        )
        .bind(i64::from(limit.clamp(1, 500)))
        .fetch_all(&self.database.pool)
        .await?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row.get("data_json")).map_err(StorageError::from))
            .collect()
    }

    async fn save(
        &self,
        expected_version: u64,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError> {
        let mut transaction = self.database.pool.begin().await?;
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_sessions
            SET version = $1, status = $2, title = $3, data_json = $4, updated_at = $5
            WHERE id = $6 AND version = $7
            "#,
        )
        .bind(as_i64(session.version)?)
        .bind(format!("{:?}", session.status).to_lowercase())
        .bind(&session.title)
        .bind(serde_json::to_value(session)?)
        .bind(session.updated_at)
        .bind(session.id.to_string())
        .bind(as_i64(expected_version)?)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar::<_, i64>(
                "SELECT version FROM sessionweft_sessions WHERE id = $1",
            )
            .bind(session.id.to_string())
            .fetch_optional(&mut *transaction)
            .await?;
            transaction.rollback().await?;
            return match actual {
                Some(actual) => Err(StorageError::Conflict {
                    expected: expected_version,
                    actual: as_u64(actual)?,
                }),
                None => Err(StorageError::NotFound(session.id)),
            };
        }
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(map_service)?;
        transaction.commit().await?;
        Ok(session.clone())
    }

    async fn pending_outbox(&self, limit: u32) -> Result<Vec<OutboxRecord>, StorageError> {
        self.database
            .claim_outbox(limit)
            .await
            .map_err(map_service)
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| OutboxRecord {
                        envelope: record.envelope,
                        publish_attempts: record.publish_attempts,
                    })
                    .collect()
            })
    }

    async fn mark_outbox_published(&self, event_id: Uuid) -> Result<(), StorageError> {
        self.database
            .mark_outbox_published(event_id)
            .await
            .map_err(map_service)
    }

    async fn mark_outbox_failed(
        &self,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<(), StorageError> {
        self.database
            .mark_outbox_failed(event_id, sanitized_error)
            .await
            .map_err(map_service)
    }
}

fn as_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::VersionOverflow(value))
}

fn as_u64(value: i64) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::CorruptNumericValue(value))
}

fn map_service(error: ServiceDatabaseError) -> StorageError {
    match error {
        ServiceDatabaseError::Database(error) => StorageError::Database(error),
        ServiceDatabaseError::Serialization(error) => StorageError::Serialization(error),
        ServiceDatabaseError::Validation(message) => {
            StorageError::Database(sqlx::Error::Protocol(message))
        }
    }
}
