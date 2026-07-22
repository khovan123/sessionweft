use std::{str::FromStr, time::Duration};

use async_trait::async_trait;
use sessionweft_client_protocol::{
    CLIENT_PROTOCOL_VERSION, ClientEventRecord, EventBatch, EventCursor, EventJournal,
    EventJournalError, validate_event_limit,
};
use sessionweft_core::EventEnvelope;
use sqlx::{Row, SqlitePool, sqlite::{SqliteConnectOptions, SqlitePoolOptions}};

#[derive(Clone)]
pub struct SqliteClientEventJournal {
    pool: SqlitePool,
}

impl SqliteClientEventJournal {
    pub async fn connect(database_url: &str) -> Result<Self, EventJournalError> {
        let is_memory = database_url.contains(":memory:");
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await
            .map_err(backend)?;
        let journal = Self { pool };
        journal.migrate().await?;
        Ok(journal)
    }

    async fn migrate(&self) -> Result<(), EventJournalError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS client_event_journal (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                session_id TEXT,
                event_type TEXT NOT NULL,
                protocol_version INTEGER NOT NULL,
                event_schema_version INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_client_event_session_sequence
            ON client_event_journal (session_id, sequence)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl EventJournal for SqliteClientEventJournal {
    async fn append(
        &self,
        envelope: &EventEnvelope,
    ) -> Result<ClientEventRecord, EventJournalError> {
        let data_json = serde_json::to_string(envelope)
            .map_err(|error| EventJournalError::Serialization(error.to_string()))?;
        let sequence = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO client_event_journal (
                event_id, session_id, event_type, protocol_version,
                event_schema_version, data_json, occurred_at, recorded_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (event_id) DO UPDATE SET event_id = excluded.event_id
            RETURNING sequence
            "#,
        )
        .bind(envelope.event_id.to_string())
        .bind(envelope.session_id.map(|value| value.to_string()))
        .bind(&envelope.event_type)
        .bind(i64::from(CLIENT_PROTOCOL_VERSION))
        .bind(i64::from(envelope.schema_version))
        .bind(data_json)
        .bind(envelope.occurred_at.to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        Ok(ClientEventRecord {
            cursor: EventCursor(to_u64(sequence)?),
            envelope: envelope.clone(),
        })
    }

    async fn list_after(
        &self,
        after: EventCursor,
        limit: u32,
    ) -> Result<EventBatch, EventJournalError> {
        let limit = validate_event_limit(limit)?;
        let fetch_limit = i64::from(limit) + 1;
        let rows = sqlx::query(
            r#"
            SELECT sequence, data_json
            FROM client_event_journal
            WHERE sequence > ?
            ORDER BY sequence ASC
            LIMIT ?
            "#,
        )
        .bind(to_i64(after.0)?)
        .bind(fetch_limit)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let has_more = rows.len() > limit as usize;
        let records = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| {
                let sequence = row.get::<i64, _>("sequence");
                let envelope = serde_json::from_str::<EventEnvelope>(row.get("data_json"))
                    .map_err(|error| EventJournalError::Serialization(error.to_string()))?;
                Ok(ClientEventRecord {
                    cursor: EventCursor(to_u64(sequence)?),
                    envelope,
                })
            })
            .collect::<Result<Vec<_>, EventJournalError>>()?;
        let latest = self.latest_cursor().await?;
        let next = records.last().map_or(after, |record| record.cursor);
        Ok(EventBatch {
            protocol_version: CLIENT_PROTOCOL_VERSION,
            after,
            next,
            latest,
            events: records,
            has_more,
        })
    }

    async fn latest_cursor(&self) -> Result<EventCursor, EventJournalError> {
        let sequence = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence), 0) FROM client_event_journal",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        Ok(EventCursor(to_u64(sequence)?))
    }
}

fn to_i64(value: u64) -> Result<i64, EventJournalError> {
    i64::try_from(value)
        .map_err(|_| EventJournalError::Validation("event cursor exceeds i64".into()))
}

fn to_u64(value: i64) -> Result<u64, EventJournalError> {
    u64::try_from(value)
        .map_err(|_| EventJournalError::Backend("negative event sequence".into()))
}

fn backend(error: impl std::fmt::Display) -> EventJournalError {
    EventJournalError::Backend(error.to_string())
}
