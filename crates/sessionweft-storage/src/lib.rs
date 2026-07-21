use std::{str::FromStr, time::Duration};

use async_trait::async_trait;
use sessionweft_core::{EventEnvelope, Session, SessionId};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct OutboxRecord {
    pub envelope: EventEnvelope,
    pub publish_attempts: u32,
}

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(
        &self,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError>;

    async fn get(&self, session_id: SessionId) -> Result<Option<Session>, StorageError>;

    async fn list(&self, limit: u32) -> Result<Vec<Session>, StorageError>;

    async fn save(
        &self,
        expected_version: u64,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError>;

    async fn pending_outbox(&self, limit: u32) -> Result<Vec<OutboxRecord>, StorageError>;

    async fn mark_outbox_published(&self, event_id: Uuid) -> Result<(), StorageError>;

    async fn mark_outbox_failed(
        &self,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<(), StorageError>;
}

#[derive(Clone)]
pub struct SqliteSessionRepository {
    pool: SqlitePool,
}

impl SqliteSessionRepository {
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await?;

        let repository = Self { pool };
        repository.migrate().await?;
        Ok(repository)
    }

    pub async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                version INTEGER NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

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
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_outbox_pending ON outbox (published_at, created_at)",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_events(
        transaction: &mut Transaction<'_, Sqlite>,
        events: &[EventEnvelope],
    ) -> Result<(), StorageError> {
        for event in events {
            let payload_json = serde_json::to_string(event)?;
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
            .await?;
        }
        Ok(())
    }

    fn serialize_session(session: &Session) -> Result<String, StorageError> {
        serde_json::to_string(session).map_err(StorageError::from)
    }

    fn deserialize_session(value: &str) -> Result<Session, StorageError> {
        serde_json::from_str(value).map_err(StorageError::from)
    }
}

#[async_trait]
impl SessionRepository for SqliteSessionRepository {
    async fn create(
        &self,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError> {
        let data_json = Self::serialize_session(session)?;
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO sessions (
                id, version, status, title, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session.id.to_string())
        .bind(as_i64(session.version)?)
        .bind(format!("{:?}", session.status).to_lowercase())
        .bind(&session.title)
        .bind(data_json)
        .bind(session.created_at.to_rfc3339())
        .bind(session.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await?;

        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await?;
        Ok(session.clone())
    }

    async fn get(&self, session_id: SessionId) -> Result<Option<Session>, StorageError> {
        let row = sqlx::query("SELECT data_json FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

        row.map(|value| Self::deserialize_session(value.get::<&str, _>("data_json")))
            .transpose()
    }

    async fn list(&self, limit: u32) -> Result<Vec<Session>, StorageError> {
        let rows = sqlx::query("SELECT data_json FROM sessions ORDER BY updated_at DESC LIMIT ?")
            .bind(i64::from(limit.clamp(1, 500)))
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|row| Self::deserialize_session(row.get::<&str, _>("data_json")))
            .collect()
    }

    async fn save(
        &self,
        expected_version: u64,
        session: &Session,
        events: &[EventEnvelope],
    ) -> Result<Session, StorageError> {
        let data_json = Self::serialize_session(session)?;
        let mut transaction = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE sessions
            SET version = ?, status = ?, title = ?, data_json = ?, updated_at = ?
            WHERE id = ? AND version = ?
            "#,
        )
        .bind(as_i64(session.version)?)
        .bind(format!("{:?}", session.status).to_lowercase())
        .bind(&session.title)
        .bind(data_json)
        .bind(session.updated_at.to_rfc3339())
        .bind(session.id.to_string())
        .bind(as_i64(expected_version)?)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() != 1 {
            let actual = sqlx::query_scalar::<_, i64>("SELECT version FROM sessions WHERE id = ?")
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

        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await?;
        Ok(session.clone())
    }

    async fn pending_outbox(&self, limit: u32) -> Result<Vec<OutboxRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT payload_json, publish_attempts
            FROM outbox
            WHERE published_at IS NULL
            ORDER BY created_at ASC
            LIMIT ?
            "#,
        )
        .bind(i64::from(limit.clamp(1, 500)))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let envelope = serde_json::from_str(row.get::<&str, _>("payload_json"))?;
                let attempts = row.get::<i64, _>("publish_attempts");
                Ok(OutboxRecord {
                    envelope,
                    publish_attempts: u32::try_from(attempts)
                        .map_err(|_| StorageError::CorruptNumericValue(attempts))?,
                })
            })
            .collect()
    }

    async fn mark_outbox_published(&self, event_id: Uuid) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE outbox SET published_at = CURRENT_TIMESTAMP, last_error = NULL WHERE event_id = ?",
        )
        .bind(event_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_outbox_failed(
        &self,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            UPDATE outbox
            SET publish_attempts = publish_attempts + 1, last_error = ?
            WHERE event_id = ?
            "#,
        )
        .bind(sanitized_error)
        .bind(event_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn as_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::VersionOverflow(value))
}

fn as_u64(value: i64) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::CorruptNumericValue(value))
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid database URL: {0}")]
    InvalidDatabaseUrl(#[from] sqlx::sqlite::SqliteError),
    #[error("session {0} not found")]
    NotFound(SessionId),
    #[error("session version conflict: expected {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("session version {0} exceeds database range")]
    VersionOverflow(u64),
    #[error("corrupt numeric value {0}")]
    CorruptNumericValue(i64),
}

impl From<sqlx::error::BoxDynError> for StorageError {
    fn from(error: sqlx::error::BoxDynError) -> Self {
        Self::Database(sqlx::Error::Configuration(error))
    }
}

#[cfg(test)]
mod tests {
    use sessionweft_core::{EventEnvelope, MessageRole};

    use super::*;

    #[tokio::test]
    async fn session_and_outbox_commit_together() {
        let repository = SqliteSessionRepository::connect("sqlite::memory:")
            .await
            .expect("repository");
        let mut session = Session::new("atomic").expect("session");
        let correlation_id = Uuid::new_v4();
        let event = EventEnvelope::new(
            "session.created",
            Some(session.id),
            correlation_id,
            Some("test"),
            serde_json::json!({"version": session.version}),
        );
        repository
            .create(&session, std::slice::from_ref(&event))
            .await
            .expect("create");

        let message_event = session
            .append_message(0, MessageRole::User, "hello", correlation_id, Some("test"))
            .expect("message");
        repository
            .save(0, &session, std::slice::from_ref(&message_event))
            .await
            .expect("save");

        let loaded = repository
            .get(session.id)
            .await
            .expect("get")
            .expect("session exists");
        assert_eq!(loaded.version, 1);
        assert_eq!(
            repository.pending_outbox(10).await.expect("outbox").len(),
            2
        );
    }

    #[tokio::test]
    async fn stale_write_returns_conflict() {
        let repository = SqliteSessionRepository::connect("sqlite::memory:")
            .await
            .expect("repository");
        let session = Session::new("conflict").expect("session");
        repository.create(&session, &[]).await.expect("create");

        let mut first = session.clone();
        let first_event = first
            .append_message(0, MessageRole::User, "one", Uuid::new_v4(), None)
            .expect("append");
        repository
            .save(0, &first, &[first_event])
            .await
            .expect("first save");

        let mut stale = session;
        let stale_event = stale
            .append_message(0, MessageRole::User, "two", Uuid::new_v4(), None)
            .expect("append");
        let error = repository
            .save(0, &stale, &[stale_event])
            .await
            .expect_err("conflict");
        assert!(matches!(
            error,
            StorageError::Conflict {
                expected: 0,
                actual: 1
            }
        ));
    }
}
