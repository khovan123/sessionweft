use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::EventEnvelope;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresServiceDatabase {
    pub(crate) pool: PgPool,
    pub(crate) instance_id: String,
    outbox_claim_ttl: Duration,
}

impl PostgresServiceDatabase {
    pub async fn connect(
        database_url: &str,
        instance_id: impl Into<String>,
    ) -> Result<Self, ServiceDatabaseError> {
        let instance_id = validate_instance_id(instance_id.into())?;
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        let database = Self {
            pool,
            instance_id,
            outbox_claim_ttl: Duration::from_secs(30),
        };
        database.migrate().await?;
        Ok(database)
    }

    pub async fn connect_in_schema(
        database_url: &str,
        instance_id: impl Into<String>,
        schema: &str,
    ) -> Result<Self, ServiceDatabaseError> {
        let instance_id = validate_instance_id(instance_id.into())?;
        validate_schema_name(schema)?;
        let quoted_schema = format!("\"{schema}\"");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {quoted_schema}"))
            .execute(&admin)
            .await?;
        admin.close().await;

        let search_path = format!("SET search_path TO {quoted_schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |connection, _metadata| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(&mut *connection, search_path.as_str()).await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;
        let database = Self {
            pool,
            instance_id,
            outbox_claim_ttl: Duration::from_secs(30),
        };
        database.migrate().await?;
        Ok(database)
    }

    pub async fn connect_in_schema(
        database_url: &str,
        instance_id: impl Into<String>,
        schema: &str,
    ) -> Result<Self, ServiceDatabaseError> {
        let instance_id = validate_instance_id(instance_id.into())?;
        validate_schema_name(schema)?;
        let quoted_schema = format!("\"{schema}\"");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await?;
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {quoted_schema}"))
            .execute(&admin)
            .await?;
        admin.close().await;

        let search_path = format!("SET search_path TO {quoted_schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(move |connection, _metadata| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(&mut *connection, search_path.as_str()).await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;
        let database = Self {
            pool,
            instance_id,
            outbox_claim_ttl: Duration::from_secs(30),
        };
        database.migrate().await?;
        Ok(database)
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub async fn migrate(&self) -> Result<(), ServiceDatabaseError> {
        for statement in MIGRATIONS {
            sqlx::query(statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub(crate) async fn insert_events(
        transaction: &mut Transaction<'_, Postgres>,
        events: &[EventEnvelope],
    ) -> Result<(), ServiceDatabaseError> {
        for event in events {
            sqlx::query(
                r#"
                INSERT INTO sessionweft_outbox (
                    event_id, session_id, event_type, schema_version,
                    payload_json, correlation_id, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (event_id) DO NOTHING
                "#,
            )
            .bind(event.event_id)
            .bind(event.session_id.map(|value| value.to_string()))
            .bind(&event.event_type)
            .bind(i32::try_from(event.schema_version).map_err(|_| {
                ServiceDatabaseError::Validation("event schema version exceeds i32".into())
            })?)
            .bind(serde_json::to_value(event)?)
            .bind(event.correlation_id)
            .bind(event.occurred_at)
            .execute(&mut **transaction)
            .await?;
        }
        Ok(())
    }

    pub async fn claim_outbox(
        &self,
        limit: u32,
    ) -> Result<Vec<ClaimedOutboxEvent>, ServiceDatabaseError> {
        let ttl = chrono::Duration::from_std(self.outbox_claim_ttl)
            .map_err(|error| ServiceDatabaseError::Validation(error.to_string()))?;
        let claimed_until = Utc::now() + ttl;
        let rows = sqlx::query_as::<_, ClaimedOutboxRow>(
            r#"
            WITH candidates AS (
                SELECT event_id
                FROM sessionweft_outbox
                WHERE published_at IS NULL
                  AND (claimed_until IS NULL OR claimed_until < NOW())
                ORDER BY created_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT $1
            )
            UPDATE sessionweft_outbox AS outbox
            SET claimed_by = $2, claimed_until = $3
            FROM candidates
            WHERE outbox.event_id = candidates.event_id
            RETURNING outbox.event_id, outbox.payload_json, outbox.publish_attempts
            "#,
        )
        .bind(i64::from(limit.clamp(1, 500)))
        .bind(&self.instance_id)
        .bind(claimed_until)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ClaimedOutboxEvent {
                    event_id: row.event_id,
                    envelope: serde_json::from_value(row.payload_json)?,
                    publish_attempts: u32::try_from(row.publish_attempts).map_err(|_| {
                        ServiceDatabaseError::Validation(
                            "negative or oversized outbox attempt count".into(),
                        )
                    })?,
                })
            })
            .collect()
    }

    pub async fn mark_outbox_published(&self, event_id: Uuid) -> Result<(), ServiceDatabaseError> {
        sqlx::query(
            r#"
            UPDATE sessionweft_outbox
            SET published_at = NOW(), claimed_by = NULL, claimed_until = NULL, last_error = NULL
            WHERE event_id = $1 AND claimed_by = $2
            "#,
        )
        .bind(event_id)
        .bind(&self.instance_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_outbox_failed(
        &self,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<(), ServiceDatabaseError> {
        sqlx::query(
            r#"
            UPDATE sessionweft_outbox
            SET publish_attempts = publish_attempts + 1,
                last_error = $3,
                claimed_by = NULL,
                claimed_until = NULL
            WHERE event_id = $1 AND claimed_by = $2
            "#,
        )
        .bind(event_id)
        .bind(&self.instance_id)
        .bind(sanitized_error.chars().take(4_096).collect::<String>())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn try_begin_event(
        &self,
        event: &EventEnvelope,
        consumer: &str,
    ) -> Result<bool, ServiceDatabaseError> {
        let result = sqlx::query(
            r#"
            INSERT INTO sessionweft_inbox (
                consumer_name, event_id, event_type, schema_version, payload_json, received_at
            ) VALUES ($1, $2, $3, $4, $5, NOW())
            ON CONFLICT (consumer_name, event_id) DO NOTHING
            "#,
        )
        .bind(consumer)
        .bind(event.event_id)
        .bind(&event.event_type)
        .bind(i32::try_from(event.schema_version).map_err(|_| {
            ServiceDatabaseError::Validation("event schema version exceeds i32".into())
        })?)
        .bind(serde_json::to_value(event)?)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn complete_event(
        &self,
        consumer: &str,
        event_id: Uuid,
    ) -> Result<(), ServiceDatabaseError> {
        sqlx::query(
            r#"
            UPDATE sessionweft_inbox
            SET consumed_at = NOW(), last_error = NULL
            WHERE consumer_name = $1 AND event_id = $2
            "#,
        )
        .bind(consumer)
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_event(
        &self,
        consumer: &str,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<u32, ServiceDatabaseError> {
        let attempts = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE sessionweft_inbox
            SET attempts = attempts + 1, last_error = $3
            WHERE consumer_name = $1 AND event_id = $2
            RETURNING attempts
            "#,
        )
        .bind(consumer)
        .bind(event_id)
        .bind(sanitized_error.chars().take(4_096).collect::<String>())
        .fetch_one(&self.pool)
        .await?;
        u32::try_from(attempts)
            .map_err(|_| ServiceDatabaseError::Validation("invalid inbox attempts".into()))
    }

    pub async fn release_failed_event(
        &self,
        consumer: &str,
        event_id: Uuid,
    ) -> Result<(), ServiceDatabaseError> {
        sqlx::query(
            "DELETE FROM sessionweft_inbox WHERE consumer_name = $1 AND event_id = $2 AND consumed_at IS NULL",
        )
        .bind(consumer)
        .bind(event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn claim_task(
        &self,
        task_id: &str,
        owner_id: &str,
        ttl: Duration,
    ) -> Result<Option<TaskClaim>, ServiceDatabaseError> {
        if task_id.trim().is_empty() || owner_id.trim().is_empty() || ttl.is_zero() {
            return Err(ServiceDatabaseError::Validation(
                "task ID, owner ID and positive TTL are required".into(),
            ));
        }
        let expires_at = Utc::now()
            + chrono::Duration::from_std(ttl)
                .map_err(|error| ServiceDatabaseError::Validation(error.to_string()))?;
        let claim = TaskClaim {
            task_id: task_id.to_owned(),
            owner_id: owner_id.to_owned(),
            claim_token: Uuid::new_v4(),
            expires_at,
        };
        let row = sqlx::query_as::<_, TaskClaimRow>(
            r#"
            INSERT INTO sessionweft_task_claims (task_id, owner_id, claim_token, expires_at)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (task_id) DO UPDATE
            SET owner_id = EXCLUDED.owner_id,
                claim_token = EXCLUDED.claim_token,
                expires_at = EXCLUDED.expires_at
            WHERE sessionweft_task_claims.expires_at < NOW()
            RETURNING task_id, owner_id, claim_token, expires_at
            "#,
        )
        .bind(&claim.task_id)
        .bind(&claim.owner_id)
        .bind(claim.claim_token)
        .bind(claim.expires_at)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(TaskClaim::from))
    }

    pub async fn release_task(&self, claim: &TaskClaim) -> Result<bool, ServiceDatabaseError> {
        let result = sqlx::query(
            "DELETE FROM sessionweft_task_claims WHERE task_id = $1 AND owner_id = $2 AND claim_token = $3",
        )
        .bind(&claim.task_id)
        .bind(&claim.owner_id)
        .bind(claim.claim_token)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedOutboxEvent {
    pub event_id: Uuid,
    pub envelope: EventEnvelope,
    pub publish_attempts: u32,
}

#[derive(Debug)]
struct ClaimedOutboxRow {
    event_id: Uuid,
    payload_json: serde_json::Value,
    publish_attempts: i32,
}

impl<'row> sqlx::FromRow<'row, sqlx::postgres::PgRow> for ClaimedOutboxRow {
    fn from_row(row: &'row sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            event_id: row.try_get("event_id")?,
            payload_json: row.try_get("payload_json")?,
            publish_attempts: row.try_get("publish_attempts")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClaim {
    pub task_id: String,
    pub owner_id: String,
    pub claim_token: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug)]
struct TaskClaimRow {
    task_id: String,
    owner_id: String,
    claim_token: Uuid,
    expires_at: DateTime<Utc>,
}

impl<'row> sqlx::FromRow<'row, sqlx::postgres::PgRow> for TaskClaimRow {
    fn from_row(row: &'row sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            task_id: row.try_get("task_id")?,
            owner_id: row.try_get("owner_id")?,
            claim_token: row.try_get("claim_token")?,
            expires_at: row.try_get("expires_at")?,
        })
    }
}

impl From<TaskClaimRow> for TaskClaim {
    fn from(value: TaskClaimRow) -> Self {
        Self {
            task_id: value.task_id,
            owner_id: value.owner_id,
            claim_token: value.claim_token,
            expires_at: value.expires_at,
        }
    }
}

fn validate_instance_id(value: String) -> Result<String, ServiceDatabaseError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > 256 {
        return Err(ServiceDatabaseError::Validation(
            "runtime instance ID must be between 1 and 256 bytes".into(),
        ));
    }
    Ok(value)
}

fn validate_schema_name(value: &str) -> Result<(), ServiceDatabaseError> {
    if value.len() < 3
        || value.len() > 63
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
    {
        return Err(ServiceDatabaseError::Validation(
            "PostgreSQL schema name must be 3-63 lowercase letters, numbers or underscores and start with a letter".into(),
        ));
    }
    Ok(())
}

fn validate_instance_id(value: String) -> Result<String, ServiceDatabaseError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > 256 {
        return Err(ServiceDatabaseError::Validation(
            "runtime instance ID must be between 1 and 256 bytes".into(),
        ));
    }
    Ok(value)
}

fn validate_schema_name(value: &str) -> Result<(), ServiceDatabaseError> {
    if value.len() < 3
        || value.len() > 63
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
    {
        return Err(ServiceDatabaseError::Validation(
            "PostgreSQL schema name must be 3-63 lowercase letters, numbers or underscores and start with a letter".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ServiceDatabaseError {
    #[error("service database validation failed: {0}")]
    Validation(String),
    #[error("PostgreSQL error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("service database serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

const MIGRATIONS: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS sessionweft_sessions (
        id TEXT PRIMARY KEY,
        version BIGINT NOT NULL,
        status TEXT NOT NULL,
        title TEXT NOT NULL,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_workflows (
        id UUID PRIMARY KEY,
        session_id TEXT NOT NULL,
        version BIGINT NOT NULL,
        status TEXT NOT NULL,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_agents (
        id UUID PRIMARY KEY,
        session_id TEXT NOT NULL,
        version BIGINT NOT NULL,
        status TEXT NOT NULL,
        heartbeat_at TIMESTAMPTZ NOT NULL,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_memories (
        id UUID PRIMARY KEY,
        session_id TEXT NOT NULL,
        class TEXT NOT NULL,
        active BOOLEAN NOT NULL,
        valid_from TIMESTAMPTZ NOT NULL,
        valid_until TIMESTAMPTZ,
        data_json JSONB NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_sessionweft_memories_active
        ON sessionweft_memories (session_id, active, class, valid_from)"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_lock_guards (
        workspace_id TEXT PRIMARY KEY,
        next_fencing_token BIGINT NOT NULL DEFAULT 1
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_locks (
        lock_id UUID PRIMARY KEY,
        session_id TEXT NOT NULL,
        workspace_id TEXT NOT NULL,
        owner_id TEXT NOT NULL,
        mode TEXT NOT NULL,
        fencing_token BIGINT NOT NULL,
        resource_json JSONB NOT NULL,
        data_json JSONB NOT NULL,
        acquired_at TIMESTAMPTZ NOT NULL,
        expires_at TIMESTAMPTZ NOT NULL
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_sessionweft_locks_active
        ON sessionweft_locks (workspace_id, expires_at)"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_outbox (
        event_id UUID PRIMARY KEY,
        session_id TEXT,
        event_type TEXT NOT NULL,
        schema_version INTEGER NOT NULL,
        payload_json JSONB NOT NULL,
        correlation_id UUID NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        published_at TIMESTAMPTZ,
        publish_attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        claimed_by TEXT,
        claimed_until TIMESTAMPTZ
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_sessionweft_outbox_pending
        ON sessionweft_outbox (published_at, claimed_until, created_at)"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_inbox (
        consumer_name TEXT NOT NULL,
        event_id UUID NOT NULL,
        event_type TEXT NOT NULL,
        schema_version INTEGER NOT NULL,
        payload_json JSONB NOT NULL,
        received_at TIMESTAMPTZ NOT NULL,
        consumed_at TIMESTAMPTZ,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        PRIMARY KEY (consumer_name, event_id)
    )"#,
    r#"CREATE TABLE IF NOT EXISTS sessionweft_task_claims (
        task_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL,
        claim_token UUID NOT NULL,
        expires_at TIMESTAMPTZ NOT NULL
    )"#,
];
