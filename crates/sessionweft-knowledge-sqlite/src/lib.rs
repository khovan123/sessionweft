use std::{str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::{EventEnvelope, SessionId};
use sessionweft_knowledge::{MemoryClass, MemoryRecord, MemoryRepository, RepositoryError};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteMemoryRepository {
    pool: SqlitePool,
}

impl SqliteMemoryRepository {
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
            CREATE TABLE IF NOT EXISTS memory_records (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                class TEXT NOT NULL,
                valid_from TEXT NOT NULL,
                valid_until TEXT,
                superseded_by TEXT,
                deleted_at TEXT,
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
            CREATE INDEX IF NOT EXISTS idx_memory_active
            ON memory_records (
                session_id, class, deleted_at, superseded_by, valid_from, valid_until, updated_at
            )
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

    async fn insert_record(
        transaction: &mut Transaction<'_, Sqlite>,
        record: &MemoryRecord,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO memory_records (
                id, session_id, class, valid_from, valid_until,
                superseded_by, deleted_at, updated_at, data_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(record.id.to_string())
        .bind(record.session_id.to_string())
        .bind(memory_class_name(record.class))
        .bind(record.valid_from.to_rfc3339())
        .bind(record.valid_until.map(|value| value.to_rfc3339()))
        .bind(record.superseded_by.map(|value| value.to_string()))
        .bind(record.deleted_at.map(|value| value.to_rfc3339()))
        .bind(record.updated_at.to_rfc3339())
        .bind(serde_json::to_string(record).map_err(backend)?)
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn update_record(
        transaction: &mut Transaction<'_, Sqlite>,
        record: &MemoryRecord,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            r#"
            UPDATE memory_records
            SET valid_until = ?, superseded_by = ?, deleted_at = ?, updated_at = ?, data_json = ?
            WHERE id = ? AND session_id = ?
            "#,
        )
        .bind(record.valid_until.map(|value| value.to_rfc3339()))
        .bind(record.superseded_by.map(|value| value.to_string()))
        .bind(record.deleted_at.map(|value| value.to_rfc3339()))
        .bind(record.updated_at.to_rfc3339())
        .bind(serde_json::to_string(record).map_err(backend)?)
        .bind(record.id.to_string())
        .bind(record.session_id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::MemoryNotFound(record.id));
        }
        Ok(())
    }

    async fn load_record(
        transaction: &mut Transaction<'_, Sqlite>,
        session_id: SessionId,
        memory_id: Uuid,
    ) -> Result<MemoryRecord, RepositoryError> {
        let row =
            sqlx::query("SELECT data_json FROM memory_records WHERE session_id = ? AND id = ?")
                .bind(session_id.to_string())
                .bind(memory_id.to_string())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(backend)?
                .ok_or(RepositoryError::MemoryNotFound(memory_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
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
impl MemoryRepository for SqliteMemoryRepository {
    async fn put(
        &self,
        record: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        Self::insert_record(&mut transaction, record).await?;
        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(record.clone())
    }

    async fn get(
        &self,
        session_id: SessionId,
        memory_id: Uuid,
    ) -> Result<Option<MemoryRecord>, RepositoryError> {
        let row =
            sqlx::query("SELECT data_json FROM memory_records WHERE session_id = ? AND id = ?")
                .bind(session_id.to_string())
                .bind(memory_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn active_candidates(
        &self,
        session_id: SessionId,
        classes: &std::collections::BTreeSet<MemoryClass>,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT data_json
            FROM memory_records
            WHERE session_id = ?
              AND deleted_at IS NULL
              AND superseded_by IS NULL
              AND valid_from <= ?
              AND (valid_until IS NULL OR valid_until > ?)
            ORDER BY updated_at DESC
            LIMIT ?
            "#,
        )
        .bind(session_id.to_string())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(i64::try_from(limit.clamp(1, 10_000)).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;

        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<MemoryRecord>(row.get::<&str, _>("data_json"))
                    .map_err(backend)
            })
            .filter(|result| match result {
                Ok(record) => classes.is_empty() || classes.contains(&record.class),
                Err(_) => true,
            })
            .collect()
    }

    async fn mark_superseded(
        &self,
        session_id: SessionId,
        old_memory_id: Uuid,
        replacement: &MemoryRecord,
        events: &[EventEnvelope],
    ) -> Result<MemoryRecord, RepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut old = Self::load_record(&mut transaction, session_id, old_memory_id).await?;
        if !old.is_active_at(Utc::now()) {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::MemoryInactive(old_memory_id));
        }
        let now = Utc::now();
        old.superseded_by = Some(replacement.id);
        old.valid_until = Some(now);
        old.updated_at = now;
        Self::update_record(&mut transaction, &old).await?;
        Self::insert_record(&mut transaction, replacement).await?;
        Self::insert_events(&mut transaction, events).await?;
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
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut record = Self::load_record(&mut transaction, session_id, memory_id).await?;
        if record.deleted_at.is_some() {
            transaction.rollback().await.map_err(backend)?;
            return Err(RepositoryError::MemoryInactive(memory_id));
        }
        record.deleted_at = Some(deleted_at);
        record.updated_at = deleted_at;
        Self::update_record(&mut transaction, &record).await?;
        Self::insert_events(&mut transaction, events).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(())
    }
}

const fn memory_class_name(class: MemoryClass) -> &'static str {
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use sessionweft_knowledge::{MemoryQuery, MemoryService, MemorySource};

    use super::*;

    fn memory(session_id: SessionId, content: &str) -> MemoryRecord {
        MemoryRecord::new(
            session_id,
            MemoryClass::Decision,
            content,
            MemorySource {
                kind: "adr".into(),
                locator: "ADR-TEST".into(),
                revision: Some("1".into()),
            },
            ["architecture".into()],
        )
        .expect("memory")
    }

    #[tokio::test]
    async fn memory_and_outbox_mutation_is_atomic_and_searchable() {
        let repository = Arc::new(
            SqliteMemoryRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = MemoryService::new(Arc::clone(&repository));
        let session_id = SessionId::new();
        service
            .remember(
                memory(session_id, "SQLite WAL is used for local mode"),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("remember");
        let hits = service
            .search(&MemoryQuery {
                session_id,
                text: "SQLite local".into(),
                classes: BTreeSet::new(),
                tags: BTreeSet::new(),
                limit: 10,
            })
            .await
            .expect("search");
        assert_eq!(hits.len(), 1);
        let outbox_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbox")
            .fetch_one(&repository.pool)
            .await
            .expect("outbox count");
        assert_eq!(outbox_count, 1);
    }

    #[tokio::test]
    async fn superseded_and_deleted_memories_disappear_from_search() {
        let repository = Arc::new(
            SqliteMemoryRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let service = MemoryService::new(Arc::clone(&repository));
        let session_id = SessionId::new();
        let original = service
            .remember(
                memory(session_id, "Use Qdrant for every deployment"),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("remember");
        let replacement = service
            .supersede(
                session_id,
                original.id,
                memory(session_id, "Vector storage is optional and rebuildable"),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("supersede");

        let old_hits = service
            .search(&MemoryQuery {
                session_id,
                text: "Qdrant every deployment".into(),
                classes: BTreeSet::new(),
                tags: BTreeSet::new(),
                limit: 10,
            })
            .await
            .expect("search old");
        assert!(old_hits.is_empty());

        service
            .forget(session_id, replacement.id, Uuid::new_v4(), Some("test"))
            .await
            .expect("forget");
        let replacement_hits = service
            .search(&MemoryQuery {
                session_id,
                text: "optional rebuildable".into(),
                classes: BTreeSet::new(),
                tags: BTreeSet::new(),
                limit: 10,
            })
            .await
            .expect("search replacement");
        assert!(replacement_hits.is_empty());
    }
}
