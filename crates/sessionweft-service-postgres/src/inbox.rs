use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use sessionweft_core::EventEnvelope;
use sessionweft_jetstream::{EventInbox, InboxClaim, JetStreamError};
use uuid::Uuid;

use crate::PostgresServiceDatabase;

#[derive(Clone)]
pub struct PostgresEventInbox {
    database: PostgresServiceDatabase,
    processing_ttl: Duration,
}

impl PostgresEventInbox {
    pub async fn new(database: PostgresServiceDatabase) -> Result<Self, JetStreamError> {
        sqlx::query("ALTER TABLE sessionweft_inbox ADD COLUMN IF NOT EXISTS processing_by TEXT")
            .execute(&database.pool)
            .await
            .map_err(inbox_error)?;
        sqlx::query(
            "ALTER TABLE sessionweft_inbox ADD COLUMN IF NOT EXISTS processing_until TIMESTAMPTZ",
        )
        .execute(&database.pool)
        .await
        .map_err(inbox_error)?;
        Ok(Self {
            database,
            processing_ttl: Duration::from_secs(30),
        })
    }

    #[must_use]
    pub fn with_processing_ttl(mut self, processing_ttl: Duration) -> Self {
        if !processing_ttl.is_zero() {
            self.processing_ttl = processing_ttl;
        }
        self
    }
}

#[async_trait]
impl EventInbox for PostgresEventInbox {
    async fn claim(
        &self,
        consumer: &str,
        event: &EventEnvelope,
    ) -> Result<InboxClaim, JetStreamError> {
        let processing_until = Utc::now()
            + chrono::Duration::from_std(self.processing_ttl)
                .map_err(|error| JetStreamError::Inbox(error.to_string()))?;
        let attempts = sqlx::query_scalar::<_, i32>(
            r#"
            INSERT INTO sessionweft_inbox (
                consumer_name, event_id, event_type, schema_version,
                payload_json, received_at, processing_by, processing_until
            ) VALUES ($1, $2, $3, $4, $5, NOW(), $6, $7)
            ON CONFLICT (consumer_name, event_id) DO UPDATE
            SET processing_by = EXCLUDED.processing_by,
                processing_until = EXCLUDED.processing_until
            WHERE sessionweft_inbox.consumed_at IS NULL
              AND (
                sessionweft_inbox.processing_until IS NULL
                OR sessionweft_inbox.processing_until < NOW()
              )
            RETURNING attempts
            "#,
        )
        .bind(consumer)
        .bind(event.event_id)
        .bind(&event.event_type)
        .bind(i32::try_from(event.schema_version).map_err(inbox_error)?)
        .bind(serde_json::to_value(event).map_err(inbox_error)?)
        .bind(&self.database.instance_id)
        .bind(processing_until)
        .fetch_optional(&self.database.pool)
        .await
        .map_err(inbox_error)?;
        if let Some(attempts) = attempts {
            return Ok(InboxClaim::Acquired {
                attempts: u32::try_from(attempts).map_err(inbox_error)?,
            });
        }
        let consumed = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT consumed_at IS NOT NULL
            FROM sessionweft_inbox
            WHERE consumer_name = $1 AND event_id = $2
            "#,
        )
        .bind(consumer)
        .bind(event.event_id)
        .fetch_optional(&self.database.pool)
        .await
        .map_err(inbox_error)?;
        Ok(match consumed {
            Some(true) => InboxClaim::Completed,
            Some(false) => InboxClaim::Busy,
            None => InboxClaim::Busy,
        })
    }

    async fn complete(&self, consumer: &str, event_id: Uuid) -> Result<(), JetStreamError> {
        let result = sqlx::query(
            r#"
            UPDATE sessionweft_inbox
            SET consumed_at = NOW(), processing_by = NULL,
                processing_until = NULL, last_error = NULL
            WHERE consumer_name = $1 AND event_id = $2 AND processing_by = $3
            "#,
        )
        .bind(consumer)
        .bind(event_id)
        .bind(&self.database.instance_id)
        .execute(&self.database.pool)
        .await
        .map_err(inbox_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(JetStreamError::Inbox(
                "event inbox claim was lost before completion".into(),
            ))
        }
    }

    async fn fail(
        &self,
        consumer: &str,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<u32, JetStreamError> {
        let attempts = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE sessionweft_inbox
            SET attempts = attempts + 1,
                last_error = $4,
                processing_by = NULL,
                processing_until = NULL
            WHERE consumer_name = $1 AND event_id = $2 AND processing_by = $3
            RETURNING attempts
            "#,
        )
        .bind(consumer)
        .bind(event_id)
        .bind(&self.database.instance_id)
        .bind(sanitized_error.chars().take(4_096).collect::<String>())
        .fetch_one(&self.database.pool)
        .await
        .map_err(inbox_error)?;
        u32::try_from(attempts).map_err(inbox_error)
    }
}

fn inbox_error(error: impl std::fmt::Display) -> JetStreamError {
    JetStreamError::Inbox(error.to_string())
}
