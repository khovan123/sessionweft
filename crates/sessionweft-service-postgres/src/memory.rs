use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_knowledge::{MemoryClass, MemoryRecord, MemoryRepository, RepositoryError};
use sqlx::Row;
use uuid::Uuid;

use crate::PostgresServiceDatabase;

#[derive(Clone)]
pub struct PostgresMemoryRepository {
    database: PostgresServiceDatabase,
}

impl PostgresMemoryRepository {
    #[must_use]
    pub fn new(database: PostgresServiceDatabase) -> Self {
        Self { database }
    }
}

#[async_trait]
impl MemoryRepository for PostgresMemoryRepository {
    async fn put(
        &self,
        record: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        insert_memory(&mut transaction, record).await?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(record.clone())
    }

    async fn get(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
    ) -> Result<Option<MemoryRecord>, RepositoryError> {
        let row = sqlx::query(
            "SELECT data_json FROM sessionweft_memories WHERE id = $1 AND session_id = $2",
        )
        .bind(memory_id)
        .bind(session_id.to_string())
        .fetch_optional(&self.database.pool)
        .await
        .map_err(backend)?;
        row.map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .transpose()
    }

    async fn active_candidates(
        &self,
        session_id: SessionId,
        classes: &BTreeSet<MemoryClass>,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM sessionweft_memories
            WHERE session_id = $1
              AND active = TRUE
              AND valid_from <= $2
              AND (valid_until IS NULL OR valid_until > $2)
            ORDER BY updated_at DESC
            LIMIT $3
            "#,
        )
        .bind(session_id.to_string())
        .bind(now)
        .bind(i64::try_from(limit.clamp(1, 5_000)).map_err(backend)?)
        .fetch_all(&self.database.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row.get("data_json")).map_err(backend))
            .collect::<Result<Vec<MemoryRecord>, _>>()
            .map(|records| {
                records
                    .into_iter()
                    .filter(|record| classes.is_empty() || classes.contains(&record.class))
                    .filter(|record| record.is_active_at(now))
                    .collect()
            })
    }

    async fn mark_superseded(
        &self,
        session_id: SessionId,
        old_memory_id: Uuid,
        replacement: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let row = sqlx::query(
            r#"
            SELECT data_json
            FROM sessionweft_memories
            WHERE id = $1 AND session_id = $2
            FOR UPDATE
            "#,
        )
        .bind(old_memory_id)
        .bind(session_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?
        .ok_or(RepositoryError::MemoryNotFound(old_memory_id))?;
        let mut old: MemoryRecord =
            serde_json::from_value(row.get("data_json")).map_err(backend)?;
        if !old.is_active_at(Utc::now()) {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::MemoryInactive(old_memory_id));
        }
        old.superseded_by = Some(replacement.id);
        old.updated_at = Utc::now();
        sqlx::query(
            r#"
            UPDATE sessionweft_memories
            SET active = FALSE, data_json = $1, updated_at = $2
            WHERE id = $3 AND active = TRUE
            "#,
        )
        .bind(serde_json::to_value(&old).map_err(backend)?)
        .bind(old.updated_at)
        .bind(old_memory_id)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        insert_memory(&mut transaction, replacement).await?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(replacement.clone())
    }

    async fn delete(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
        deleted_at: DateTime<Utc>,
        events: &[EventEnvelope],
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.database.pool.begin().await.map_err(backend)?;
        let row = sqlx::query(
            r#"
            SELECT data_json
            FROM sessionweft_memories
            WHERE id = $1 AND session_id = $2
            FOR UPDATE
            "#,
        )
        .bind(memory_id)
        .bind(session_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?
        .ok_or(RepositoryError::MemoryNotFound(memory_id))?;
        let mut record: MemoryRecord =
            serde_json::from_value(row.get("data_json")).map_err(backend)?;
        if !record.is_active_at(deleted_at) {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::MemoryInactive(memory_id));
        }
        record.deleted_at = Some(deleted_at);
        record.updated_at = deleted_at;
        sqlx::query(
            r#"
            UPDATE sessionweft_memories
            SET active = FALSE, data_json = $1, updated_at = $2
            WHERE id = $3 AND active = TRUE
            "#,
        )
        .bind(serde_json::to_value(&record).map_err(backend)?)
        .bind(deleted_at)
        .bind(memory_id)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        PostgresServiceDatabase::insert_events(&mut transaction, events)
            .await
            .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }
}

async fn insert_memory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &MemoryRecord,
) -> Result<(), RepositoryError> {
    sqlx::query(
        r#"
        INSERT INTO sessionweft_memories (
            id, session_id, class, active, valid_from, valid_until,
            data_json, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(record.id)
    .bind(record.session_id.to_string())
    .bind(class_name(record.class))
    .bind(record.is_active_at(Utc::now()))
    .bind(record.valid_from)
    .bind(record.valid_until)
    .bind(serde_json::to_value(record).map_err(backend)?)
    .bind(record.created_at)
    .bind(record.updated_at)
    .execute(&mut **transaction)
    .await
    .map_err(backend)?;
    Ok(())
}

const fn class_name(class: MemoryClass) -> &'static str {
    match class {
        MemoryClass::Conversation => "conversation",
        MemoryClass::Repository => "repository",
        MemoryClass::Decision => "decision",
        MemoryClass::Preference => "preference",
        MemoryClass::Error => "error",
    }
}

fn backend(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Backend(error.to_string())
}
