use std::{str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_git::{
    GitRepositoryError, GitWorktreeRecord, GitWorktreeRepository, GitWorktreeStatus,
    WorktreeAllocationRequest,
};
use sessionweft_orchestration::LockLease;
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteGitWorktreeRepository {
    pool: SqlitePool,
}

impl SqliteGitWorktreeRepository {
    pub async fn connect(database_url: &str) -> Result<Self, GitRepositoryError> {
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

    async fn migrate(&self) -> Result<(), GitRepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS git_worktrees (
                worktree_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                claim_id TEXT NOT NULL UNIQUE,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                repository_root TEXT NOT NULL,
                branch_name TEXT NOT NULL UNIQUE,
                worktree_path TEXT NOT NULL UNIQUE,
                base_commit TEXT NOT NULL,
                head_commit TEXT,
                lock_id TEXT NOT NULL,
                fencing_token INTEGER NOT NULL,
                status TEXT NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_git_worktrees_status
            ON git_worktrees (status, updated_at, worktree_id)
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
        worktree_id: Uuid,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_worktrees WHERE worktree_id = ?")
            .bind(worktree_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::NotFound(worktree_id))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn save(
        transaction: &mut Transaction<'_, Sqlite>,
        record: &GitWorktreeRecord,
    ) -> Result<(), GitRepositoryError> {
        let result = sqlx::query(
            r#"
            UPDATE git_worktrees
            SET head_commit = ?, status = ?, data_json = ?, updated_at = ?
            WHERE worktree_id = ?
            "#,
        )
        .bind(&record.head_commit)
        .bind(status_name(record.status))
        .bind(serde_json::to_string(record).map_err(backend)?)
        .bind(record.updated_at.to_rfc3339())
        .bind(record.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(GitRepositoryError::NotFound(record.id));
        }
        Ok(())
    }

    async fn insert_event(
        transaction: &mut Transaction<'_, Sqlite>,
        record: &GitWorktreeRecord,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), GitRepositoryError> {
        let event = EventEnvelope::new(
            event_type,
            Some(record.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "worktree_id": record.id,
                "claim_id": record.claim_id,
                "agent_id": record.agent_id,
                "workspace_id": record.workspace_id,
                "branch_name": record.branch_name,
                "worktree_path": record.worktree_path,
                "base_commit": record.base_commit,
                "head_commit": record.head_commit,
                "fencing_token": record.fence.fencing_token,
                "status": record.status,
                "last_error": record.last_error,
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

    async fn mutate<F>(
        &self,
        worktree_id: Uuid,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        operation: F,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>
    where
        F: FnOnce(&mut GitWorktreeRecord) -> Result<(), sessionweft_git::GitWorkspaceError>,
    {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut record = Self::load(&mut transaction, worktree_id).await?;
        operation(&mut record).map_err(domain)?;
        Self::save(&mut transaction, &record).await?;
        Self::insert_event(
            &mut transaction,
            &record,
            event_type,
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(record)
    }
}

#[async_trait]
impl GitWorktreeRepository for SqliteGitWorktreeRepository {
    async fn reserve(
        &self,
        mut request: WorktreeAllocationRequest,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        request.validate(now).map_err(domain)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        if let Some(row) = sqlx::query("SELECT data_json FROM git_worktrees WHERE claim_id = ?")
            .bind(request.claim_id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?
        {
            let existing = serde_json::from_str::<GitWorktreeRecord>(
                row.get::<&str, _>("data_json"),
            )
            .map_err(backend)?;
            if existing.session_id == request.session_id
                && existing.agent_id == request.agent_id
                && existing.workspace_id == request.workspace_id
                && existing.base_commit == request.base_commit
            {
                return Ok(existing);
            }
            return Err(GitRepositoryError::Conflict(
                "scheduler Claim is already assigned to a different Git worktree".into(),
            ));
        }

        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(request.fence.lock_id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::StaleFence)?;
        let lease = serde_json::from_str::<LockLease>(lease_row.get::<&str, _>("data_json"))
            .map_err(backend)?;
        if lease.session_id != request.session_id
            || lease.owner_id != request.agent_id.to_string()
            || lease.resource.workspace_id() != request.workspace_id
            || lease.fencing_token != request.fence.fencing_token
            || lease.expires_at <= now
        {
            return Err(GitRepositoryError::StaleFence);
        }
        request.fence.expires_at = lease.expires_at;
        let record = GitWorktreeRecord::new(request, now).map_err(domain)?;
        sqlx::query(
            r#"
            INSERT INTO git_worktrees (
                worktree_id, session_id, claim_id, agent_id, workspace_id,
                repository_root, branch_name, worktree_path, base_commit,
                head_commit, lock_id, fencing_token, status, data_json,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(record.id.to_string())
        .bind(record.session_id.to_string())
        .bind(record.claim_id.to_string())
        .bind(record.agent_id.to_string())
        .bind(&record.workspace_id)
        .bind(&record.repository_root)
        .bind(&record.branch_name)
        .bind(&record.worktree_path)
        .bind(&record.base_commit)
        .bind(&record.head_commit)
        .bind(record.fence.lock_id.to_string())
        .bind(to_i64(record.fence.fencing_token)?)
        .bind(status_name(record.status))
        .bind(serde_json::to_string(&record).map_err(backend)?)
        .bind(record.created_at.to_rfc3339())
        .bind(record.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::insert_event(
            &mut transaction,
            &record,
            "git.worktree_reserved",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(record)
    }

    async fn get(
        &self,
        worktree_id: Uuid,
    ) -> Result<Option<GitWorktreeRecord>, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_worktrees WHERE worktree_id = ?")
            .bind(worktree_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn mark_ready(
        &self,
        worktree_id: Uuid,
        head_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        self.mutate(
            worktree_id,
            "git.worktree_ready",
            correlation_id,
            actor_id,
            |record| record.mark_ready(head_commit, now),
        )
        .await
    }

    async fn mark_failed(
        &self,
        worktree_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        self.mutate(
            worktree_id,
            "git.worktree_failed",
            correlation_id,
            actor_id,
            |record| record.mark_failed(sanitized_error, now),
        )
        .await
    }

    async fn mark_abandoned(
        &self,
        worktree_id: Uuid,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        self.mutate(
            worktree_id,
            "git.worktree_abandoned",
            correlation_id,
            actor_id,
            |record| record.mark_abandoned(reason, now),
        )
        .await
    }

    async fn mark_cleaned(
        &self,
        worktree_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        self.mutate(
            worktree_id,
            "git.worktree_cleaned",
            correlation_id,
            actor_id,
            |record| record.mark_cleaned(now),
        )
        .await
    }

    async fn stale_provisioning(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<GitWorktreeRecord>, GitRepositoryError> {
        if limit == 0 || limit > 1_000 {
            return Err(GitRepositoryError::Conflict(
                "worktree query limit must be between 1 and 1000".into(),
            ));
        }
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM git_worktrees
            WHERE status = 'provisioning' AND updated_at < ?
            ORDER BY updated_at ASC, worktree_id ASC
            LIMIT ?
            "#,
        )
        .bind(stale_before.to_rfc3339())
        .bind(i64::try_from(limit).map_err(backend)?)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.into_iter()
            .map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .collect()
    }
}

const fn status_name(status: GitWorktreeStatus) -> &'static str {
    match status {
        GitWorktreeStatus::Provisioning => "provisioning",
        GitWorktreeStatus::Ready => "ready",
        GitWorktreeStatus::Failed => "failed",
        GitWorktreeStatus::Abandoned => "abandoned",
        GitWorktreeStatus::Cleaned => "cleaned",
    }
}

fn to_i64(value: u64) -> Result<i64, GitRepositoryError> {
    i64::try_from(value).map_err(backend)
}

fn domain(error: sessionweft_git::GitWorkspaceError) -> GitRepositoryError {
    GitRepositoryError::Conflict(error.to_string())
}

fn backend(error: impl std::fmt::Display) -> GitRepositoryError {
    GitRepositoryError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sessionweft_core::SessionId;
    use sessionweft_git::{GitFence, GitWorktreeRepository, WorktreeAllocationRequest};
    use sessionweft_orchestration::{
        LockMode, LockRequest, LockResource, OrchestrationService,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;

    use super::*;

    async fn setup() -> (
        OrchestrationService<SqliteOrchestrationRepository>,
        SqliteGitWorktreeRepository,
        SessionId,
        Uuid,
    ) {
        let path = std::env::temp_dir().join(format!(
            "sessionweft-git-worktree-{}.db",
            Uuid::new_v4()
        ));
        let database_url = format!("sqlite://{}", path.display());
        let orchestration_repository = Arc::new(
            SqliteOrchestrationRepository::connect(&database_url)
                .await
                .expect("orchestration repository"),
        );
        let orchestration = OrchestrationService::new(orchestration_repository);
        let git = SqliteGitWorktreeRepository::connect(&database_url)
            .await
            .expect("Git repository");
        (orchestration, git, SessionId::new(), Uuid::new_v4())
    }

    #[tokio::test]
    async fn reservation_is_fenced_and_idempotent_per_claim() {
        let (orchestration, git, session_id, agent_id) = setup().await;
        let lease = orchestration
            .acquire_lock(
                &LockRequest {
                    session_id,
                    owner_id: agent_id.to_string(),
                    resource: LockResource::Workspace {
                        workspace_id: "workspace".into(),
                    },
                    mode: LockMode::Exclusive,
                    ttl_seconds: 60,
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("lock lease");
        let request = WorktreeAllocationRequest {
            session_id,
            claim_id: Uuid::new_v4(),
            agent_id,
            workspace_id: "workspace".into(),
            repository_root: "/tmp/repository".into(),
            branch_name: format!("sessionweft/{}", Uuid::new_v4()),
            worktree_path: format!("/tmp/worktrees/{}", Uuid::new_v4()),
            base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            fence: GitFence {
                lock_id: lease.lock_id,
                fencing_token: lease.fencing_token,
                expires_at: lease.expires_at,
            },
        };
        let first = git
            .reserve(
                request.clone(),
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("reserve");
        let replay = git
            .reserve(request, Utc::now(), Uuid::new_v4(), Some("test"))
            .await
            .expect("idempotent reserve");
        assert_eq!(first.id, replay.id);

        orchestration
            .release_lock(
                lease.lock_id,
                &lease.owner_id,
                lease.fencing_token,
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("release lock");
        let stale = WorktreeAllocationRequest {
            claim_id: Uuid::new_v4(),
            branch_name: format!("sessionweft/{}", Uuid::new_v4()),
            worktree_path: format!("/tmp/worktrees/{}", Uuid::new_v4()),
            ..replay_request(&first)
        };
        assert!(matches!(
            git.reserve(stale, Utc::now(), Uuid::new_v4(), Some("test"))
                .await,
            Err(GitRepositoryError::StaleFence)
        ));
    }

    #[tokio::test]
    async fn lifecycle_persists_ready_abandoned_and_cleaned_states() {
        let (orchestration, git, session_id, agent_id) = setup().await;
        let lease = orchestration
            .acquire_lock(
                &LockRequest {
                    session_id,
                    owner_id: agent_id.to_string(),
                    resource: LockResource::Workspace {
                        workspace_id: "workspace".into(),
                    },
                    mode: LockMode::Exclusive,
                    ttl_seconds: 60,
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("lock lease");
        let record = git
            .reserve(
                WorktreeAllocationRequest {
                    session_id,
                    claim_id: Uuid::new_v4(),
                    agent_id,
                    workspace_id: "workspace".into(),
                    repository_root: "/tmp/repository".into(),
                    branch_name: format!("sessionweft/{}", Uuid::new_v4()),
                    worktree_path: format!("/tmp/worktrees/{}", Uuid::new_v4()),
                    base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                    fence: GitFence {
                        lock_id: lease.lock_id,
                        fencing_token: lease.fencing_token,
                        expires_at: lease.expires_at,
                    },
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("reserve");
        let ready = git
            .mark_ready(
                record.id,
                "abcdef0123456789abcdef0123456789abcdef01",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("ready");
        assert_eq!(ready.status, GitWorktreeStatus::Ready);
        let abandoned = git
            .mark_abandoned(
                record.id,
                "claim cancelled",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("abandoned");
        assert_eq!(abandoned.status, GitWorktreeStatus::Abandoned);
        let cleaned = git
            .mark_cleaned(
                record.id,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("cleaned");
        assert_eq!(cleaned.status, GitWorktreeStatus::Cleaned);
    }

    fn replay_request(record: &GitWorktreeRecord) -> WorktreeAllocationRequest {
        WorktreeAllocationRequest {
            session_id: record.session_id,
            claim_id: record.claim_id,
            agent_id: record.agent_id,
            workspace_id: record.workspace_id.clone(),
            repository_root: record.repository_root.clone(),
            branch_name: record.branch_name.clone(),
            worktree_path: record.worktree_path.clone(),
            base_commit: record.base_commit.clone(),
            fence: record.fence.clone(),
        }
    }
}
