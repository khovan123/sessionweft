use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_git::{
    GitMergeQueueRepository, GitRepositoryError, GitWorktreeRecord, GitWorktreeStatus,
    MergeQueueEntry, MergeQueueRequest, MergeQueueStatus,
};
use sessionweft_orchestration::LockLease;
use sqlx::{Row, Sqlite, Transaction};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{SqliteGitWorktreeRepository, backend};

static MERGE_CLAIM_GUARD: Mutex<()> = Mutex::const_new(());

impl SqliteGitWorktreeRepository {
    async fn ensure_merge_queue_tables(&self) -> Result<(), GitRepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS git_merge_sequence (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                value INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query("INSERT OR IGNORE INTO git_merge_sequence (id, value) VALUES (1, 0)")
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS git_merge_queue (
                queue_id TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL UNIQUE,
                priority INTEGER NOT NULL,
                worktree_id TEXT NOT NULL UNIQUE,
                session_id TEXT NOT NULL,
                claim_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                source_branch TEXT NOT NULL,
                target_branch TEXT NOT NULL,
                head_commit TEXT NOT NULL,
                status TEXT NOT NULL,
                review_status TEXT NOT NULL,
                test_status TEXT NOT NULL,
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
            CREATE INDEX IF NOT EXISTS idx_git_merge_queue_order
            ON git_merge_queue (status, review_status, test_status, priority DESC, sequence ASC)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_git_merge_queue_single_active
            ON git_merge_queue ((1))
            WHERE status IN ('claimed', 'merging')
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn next_merge_sequence(
        transaction: &mut Transaction<'_, Sqlite>,
    ) -> Result<u64, GitRepositoryError> {
        let value = sqlx::query_scalar::<_, i64>(
            "UPDATE git_merge_sequence SET value = value + 1 WHERE id = 1 RETURNING value",
        )
        .fetch_one(&mut **transaction)
        .await
        .map_err(backend)?;
        u64::try_from(value).map_err(backend)
    }

    async fn load_merge_entry(
        transaction: &mut Transaction<'_, Sqlite>,
        queue_id: Uuid,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_merge_queue WHERE queue_id = ?")
            .bind(queue_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or_else(|| {
                GitRepositoryError::Conflict(format!("merge queue {queue_id} not found"))
            })?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn save_merge_entry(
        transaction: &mut Transaction<'_, Sqlite>,
        entry: &MergeQueueEntry,
    ) -> Result<(), GitRepositoryError> {
        let result = sqlx::query(
            r#"
            UPDATE git_merge_queue
            SET head_commit = ?, status = ?, review_status = ?, test_status = ?,
                data_json = ?, updated_at = ?
            WHERE queue_id = ?
            "#,
        )
        .bind(&entry.head_commit)
        .bind(queue_status(entry.status))
        .bind(gate_status(entry.review.status))
        .bind(gate_status(entry.tests.status))
        .bind(serde_json::to_string(entry).map_err(backend)?)
        .bind(entry.updated_at.to_rfc3339())
        .bind(entry.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(GitRepositoryError::Conflict(format!(
                "merge queue {} changed concurrently",
                entry.id
            )));
        }
        Ok(())
    }

    async fn insert_merge_event(
        transaction: &mut Transaction<'_, Sqlite>,
        entry: &MergeQueueEntry,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(), GitRepositoryError> {
        let event = EventEnvelope::new(
            event_type,
            Some(entry.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "queue_id": entry.id,
                "sequence": entry.sequence,
                "priority": entry.priority,
                "worktree_id": entry.worktree_id,
                "claim_id": entry.claim_id,
                "source_branch": entry.source_branch,
                "target_branch": entry.target_branch,
                "head_commit": entry.head_commit,
                "merge_commit": entry.merge_commit,
                "fencing_token": entry.fence.fencing_token,
                "review_status": entry.review.status,
                "test_status": entry.tests.status,
                "status": entry.status,
                "conflict": entry.conflict,
                "cancellation_reason": entry.cancellation_reason,
                "last_error": entry.last_error,
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

    async fn live_merge_fence(
        transaction: &mut Transaction<'_, Sqlite>,
        entry: &MergeQueueEntry,
        now: DateTime<Utc>,
    ) -> Result<(), GitRepositoryError> {
        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(entry.fence.lock_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::StaleFence)?;
        let lease = serde_json::from_str::<LockLease>(lease_row.get::<&str, _>("data_json"))
            .map_err(backend)?;
        if lease.session_id != entry.session_id
            || lease.owner_id != entry.agent_id.to_string()
            || lease.resource.workspace_id() != entry.workspace_id
            || lease.fencing_token != entry.fence.fencing_token
            || lease.expires_at <= now
        {
            return Err(GitRepositoryError::StaleFence);
        }
        let worktree = Self::load(transaction, entry.worktree_id).await?;
        if worktree.status != GitWorktreeStatus::Ready
            || worktree.head_commit.as_deref() != Some(entry.head_commit.as_str())
            || worktree.fence.fencing_token != entry.fence.fencing_token
        {
            return Err(GitRepositoryError::Conflict(
                "merge queue worktree no longer matches its durable snapshot".into(),
            ));
        }
        Ok(())
    }

    async fn mutate_merge<F>(
        &self,
        queue_id: Uuid,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        operation: F,
    ) -> Result<MergeQueueEntry, GitRepositoryError>
    where
        F: FnOnce(&mut MergeQueueEntry) -> Result<(), sessionweft_git::GitWorkspaceError>,
    {
        self.ensure_merge_queue_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut entry = Self::load_merge_entry(&mut transaction, queue_id).await?;
        operation(&mut entry).map_err(domain)?;
        Self::save_merge_entry(&mut transaction, &entry).await?;
        Self::insert_merge_event(
            &mut transaction,
            &entry,
            event_type,
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(entry)
    }
}

#[async_trait]
impl GitMergeQueueRepository for SqliteGitWorktreeRepository {
    async fn enqueue(
        &self,
        request: MergeQueueRequest,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.ensure_merge_queue_tables().await?;
        request.validate().map_err(domain)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        if let Some(row) =
            sqlx::query("SELECT data_json FROM git_merge_queue WHERE worktree_id = ?")
                .bind(request.worktree_id.to_string())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(backend)?
        {
            let existing = serde_json::from_str::<MergeQueueEntry>(row.get::<&str, _>("data_json"))
                .map_err(backend)?;
            if existing.target_branch == request.target_branch
                && existing.priority == request.priority
            {
                return Ok(existing);
            }
            return Err(GitRepositoryError::Conflict(
                "worktree is already enqueued with different merge parameters".into(),
            ));
        }
        let worktree = Self::load(&mut transaction, request.worktree_id).await?;
        Self::live_worktree_fence(&mut transaction, &worktree, now).await?;
        let sequence = Self::next_merge_sequence(&mut transaction).await?;
        let entry = MergeQueueEntry::new(request, &worktree, sequence, now).map_err(domain)?;
        sqlx::query(
            r#"
            INSERT INTO git_merge_queue (
                queue_id, sequence, priority, worktree_id, session_id, claim_id,
                agent_id, workspace_id, source_branch, target_branch, head_commit,
                status, review_status, test_status, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(entry.id.to_string())
        .bind(to_i64(entry.sequence)?)
        .bind(i64::from(entry.priority))
        .bind(entry.worktree_id.to_string())
        .bind(entry.session_id.to_string())
        .bind(entry.claim_id.to_string())
        .bind(entry.agent_id.to_string())
        .bind(&entry.workspace_id)
        .bind(&entry.source_branch)
        .bind(&entry.target_branch)
        .bind(&entry.head_commit)
        .bind(queue_status(entry.status))
        .bind(gate_status(entry.review.status))
        .bind(gate_status(entry.tests.status))
        .bind(serde_json::to_string(&entry).map_err(backend)?)
        .bind(entry.created_at.to_rfc3339())
        .bind(entry.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::insert_merge_event(
            &mut transaction,
            &entry,
            "git.merge_enqueued",
            correlation_id,
            actor_id,
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(entry)
    }

    async fn get_merge_entry(
        &self,
        queue_id: Uuid,
    ) -> Result<Option<MergeQueueEntry>, GitRepositoryError> {
        self.ensure_merge_queue_tables().await?;
        let row = sqlx::query("SELECT data_json FROM git_merge_queue WHERE queue_id = ?")
            .bind(queue_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn record_review(
        &self,
        queue_id: Uuid,
        reviewer_id: &str,
        approved: bool,
        note: Option<&str>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.mutate_merge(
            queue_id,
            "git.merge_review_recorded",
            correlation_id,
            actor_id,
            |entry| entry.record_review(reviewer_id, approved, note.map(str::to_owned), now),
        )
        .await
    }

    async fn record_tests(
        &self,
        queue_id: Uuid,
        suite: &str,
        passed: bool,
        summary: Option<&str>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.mutate_merge(
            queue_id,
            "git.merge_tests_recorded",
            correlation_id,
            actor_id,
            |entry| entry.record_tests(suite, passed, summary.map(str::to_owned), now),
        )
        .await
    }

    async fn cancel_merge(
        &self,
        queue_id: Uuid,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.mutate_merge(
            queue_id,
            "git.merge_cancelled",
            correlation_id,
            actor_id,
            |entry| entry.cancel(reason, now),
        )
        .await
    }

    async fn claim_next_merge(
        &self,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<MergeQueueEntry>, GitRepositoryError> {
        self.ensure_merge_queue_tables().await?;
        let _guard = MERGE_CLAIM_GUARD.lock().await;
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM git_merge_queue WHERE status IN ('claimed', 'merging')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        if active > 0 {
            return Ok(None);
        }
        let queue_ids = sqlx::query(
            r#"
            SELECT queue_id FROM git_merge_queue
            WHERE status = 'queued' AND review_status = 'passed' AND test_status = 'passed'
            ORDER BY priority DESC, sequence ASC
            LIMIT 100
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        for row in queue_ids {
            let queue_id = Uuid::parse_str(row.get::<&str, _>("queue_id")).map_err(backend)?;
            let mut transaction = self.pool.begin().await.map_err(backend)?;
            let mut entry = Self::load_merge_entry(&mut transaction, queue_id).await?;
            if entry.status != MergeQueueStatus::Queued || !entry.gates_passed() {
                transaction.rollback().await.map_err(backend)?;
                continue;
            }
            if let Err(error) = Self::live_merge_fence(&mut transaction, &entry, now).await {
                entry.status = MergeQueueStatus::Failed;
                entry.last_error = Some(error.to_string());
                entry.updated_at = now;
                Self::save_merge_entry(&mut transaction, &entry).await?;
                Self::insert_merge_event(
                    &mut transaction,
                    &entry,
                    "git.merge_fence_rejected",
                    correlation_id,
                    actor_id,
                )
                .await?;
                transaction.commit().await.map_err(backend)?;
                continue;
            }
            entry.claim(now).map_err(domain)?;
            let result = sqlx::query(
                r#"
                UPDATE git_merge_queue
                SET status = 'claimed', data_json = ?, updated_at = ?
                WHERE queue_id = ? AND status = 'queued'
                "#,
            )
            .bind(serde_json::to_string(&entry).map_err(backend)?)
            .bind(entry.updated_at.to_rfc3339())
            .bind(entry.id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
            if result.rows_affected() != 1 {
                transaction.rollback().await.map_err(backend)?;
                continue;
            }
            Self::insert_merge_event(
                &mut transaction,
                &entry,
                "git.merge_claimed",
                correlation_id,
                actor_id,
            )
            .await?;
            transaction.commit().await.map_err(backend)?;
            return Ok(Some(entry));
        }
        Ok(None)
    }

    async fn validate_merge_fence(
        &self,
        queue_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), GitRepositoryError> {
        self.ensure_merge_queue_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let entry = Self::load_merge_entry(&mut transaction, queue_id).await?;
        Self::live_merge_fence(&mut transaction, &entry, now).await
    }

    async fn begin_merge(
        &self,
        queue_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.validate_merge_fence(queue_id, now).await?;
        self.mutate_merge(
            queue_id,
            "git.merge_started",
            correlation_id,
            actor_id,
            |entry| entry.begin_merge(now),
        )
        .await
    }

    async fn record_rebased_head(
        &self,
        queue_id: Uuid,
        head_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.validate_merge_fence(queue_id, now).await?;
        self.mutate_merge(
            queue_id,
            "git.merge_rebased",
            correlation_id,
            actor_id,
            |entry| entry.record_rebased_head(head_commit, now),
        )
        .await
    }

    async fn mark_merge_conflict(
        &self,
        queue_id: Uuid,
        paths: Vec<String>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.mutate_merge(
            queue_id,
            "git.merge_conflict",
            correlation_id,
            actor_id,
            |entry| entry.mark_conflict(paths, now),
        )
        .await
    }

    async fn mark_merged(
        &self,
        queue_id: Uuid,
        merge_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.validate_merge_fence(queue_id, now).await?;
        self.mutate_merge(
            queue_id,
            "git.merge_completed",
            correlation_id,
            actor_id,
            |entry| entry.mark_merged(merge_commit, now),
        )
        .await
    }

    async fn mark_merge_failed(
        &self,
        queue_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.mutate_merge(
            queue_id,
            "git.merge_failed",
            correlation_id,
            actor_id,
            |entry| entry.mark_failed(sanitized_error, now),
        )
        .await
    }
}

impl SqliteGitWorktreeRepository {
    async fn live_worktree_fence(
        transaction: &mut Transaction<'_, Sqlite>,
        worktree: &GitWorktreeRecord,
        now: DateTime<Utc>,
    ) -> Result<(), GitRepositoryError> {
        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(worktree.fence.lock_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::StaleFence)?;
        let lease = serde_json::from_str::<LockLease>(lease_row.get::<&str, _>("data_json"))
            .map_err(backend)?;
        if lease.session_id != worktree.session_id
            || lease.owner_id != worktree.agent_id.to_string()
            || lease.resource.workspace_id() != worktree.workspace_id
            || lease.fencing_token != worktree.fence.fencing_token
            || lease.expires_at <= now
        {
            return Err(GitRepositoryError::StaleFence);
        }
        Ok(())
    }
}

const fn queue_status(status: MergeQueueStatus) -> &'static str {
    match status {
        MergeQueueStatus::Queued => "queued",
        MergeQueueStatus::Claimed => "claimed",
        MergeQueueStatus::Merging => "merging",
        MergeQueueStatus::Conflict => "conflict",
        MergeQueueStatus::Merged => "merged",
        MergeQueueStatus::Cancelled => "cancelled",
        MergeQueueStatus::Failed => "failed",
    }
}

const fn gate_status(status: sessionweft_git::MergeGateStatus) -> &'static str {
    match status {
        sessionweft_git::MergeGateStatus::Pending => "pending",
        sessionweft_git::MergeGateStatus::Passed => "passed",
        sessionweft_git::MergeGateStatus::Failed => "failed",
    }
}

fn to_i64(value: u64) -> Result<i64, GitRepositoryError> {
    i64::try_from(value).map_err(backend)
}

fn domain(error: sessionweft_git::GitWorkspaceError) -> GitRepositoryError {
    GitRepositoryError::Conflict(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sessionweft_core::SessionId;
    use sessionweft_git::{
        GitFence, GitMergeQueueRepository, GitWorktreeRepository, MergeQueueRequest,
        WorktreeAllocationRequest,
    };
    use sessionweft_orchestration::{LockMode, LockRequest, LockResource, OrchestrationService};
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;

    use super::*;

    struct Fixture {
        orchestration: OrchestrationService<SqliteOrchestrationRepository>,
        repository: SqliteGitWorktreeRepository,
        session_id: SessionId,
        agent_id: Uuid,
        lease: LockLease,
    }

    async fn fixture() -> Fixture {
        let path =
            std::env::temp_dir().join(format!("sessionweft-git-merge-queue-{}.db", Uuid::new_v4()));
        let database_url = format!("sqlite://{}", path.display());
        let orchestration_repository = Arc::new(
            SqliteOrchestrationRepository::connect(&database_url)
                .await
                .expect("orchestration repository"),
        );
        let orchestration = OrchestrationService::new(orchestration_repository);
        let repository = SqliteGitWorktreeRepository::connect(&database_url)
            .await
            .expect("Git repository");
        let session_id = SessionId::new();
        let agent_id = Uuid::new_v4();
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
            .expect("lease");
        Fixture {
            orchestration,
            repository,
            session_id,
            agent_id,
            lease,
        }
    }

    async fn ready_worktree(fixture: &Fixture) -> GitWorktreeRecord {
        let record = fixture
            .repository
            .reserve(
                WorktreeAllocationRequest {
                    session_id: fixture.session_id,
                    claim_id: Uuid::new_v4(),
                    agent_id: fixture.agent_id,
                    workspace_id: "workspace".into(),
                    repository_root: "/tmp/repository".into(),
                    branch_name: format!("sessionweft/{}", Uuid::new_v4()),
                    worktree_path: format!("/tmp/worktrees/{}", Uuid::new_v4()),
                    base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                    fence: GitFence {
                        lock_id: fixture.lease.lock_id,
                        fencing_token: fixture.lease.fencing_token,
                        expires_at: fixture.lease.expires_at,
                    },
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("reserve");
        fixture
            .repository
            .mark_ready(
                record.id,
                "abcdef0123456789abcdef0123456789abcdef01",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("ready")
    }

    async fn pass_gates(repository: &SqliteGitWorktreeRepository, queue_id: Uuid) {
        repository
            .record_review(
                queue_id,
                "reviewer",
                true,
                None,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("review");
        repository
            .record_tests(
                queue_id,
                "workspace",
                true,
                None,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("tests");
    }

    #[tokio::test]
    async fn queue_requires_gates_and_orders_by_priority_then_sequence() {
        let fixture = fixture().await;
        let first = ready_worktree(&fixture).await;
        let second = ready_worktree(&fixture).await;
        let low = fixture
            .repository
            .enqueue(
                MergeQueueRequest {
                    worktree_id: first.id,
                    target_branch: "main".into(),
                    priority: 0,
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("low priority");
        let high = fixture
            .repository
            .enqueue(
                MergeQueueRequest {
                    worktree_id: second.id,
                    target_branch: "main".into(),
                    priority: 10,
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("high priority");
        assert!(
            fixture
                .repository
                .claim_next_merge(Utc::now(), Uuid::new_v4(), Some("test"))
                .await
                .expect("blocked queue")
                .is_none()
        );
        pass_gates(&fixture.repository, low.id).await;
        pass_gates(&fixture.repository, high.id).await;
        let claimed = fixture
            .repository
            .claim_next_merge(Utc::now(), Uuid::new_v4(), Some("test"))
            .await
            .expect("claim")
            .expect("eligible entry");
        assert_eq!(claimed.id, high.id);
        assert_eq!(claimed.status, MergeQueueStatus::Claimed);
        fixture
            .repository
            .cancel_merge(
                low.id,
                "superseded",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("cancel low priority");
    }

    #[tokio::test]
    async fn stale_fence_is_failed_instead_of_claimed() {
        let fixture = fixture().await;
        let worktree = ready_worktree(&fixture).await;
        let entry = fixture
            .repository
            .enqueue(
                MergeQueueRequest {
                    worktree_id: worktree.id,
                    target_branch: "main".into(),
                    priority: 0,
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("enqueue");
        pass_gates(&fixture.repository, entry.id).await;
        fixture
            .orchestration
            .release_lock(
                fixture.lease.lock_id,
                &fixture.lease.owner_id,
                fixture.lease.fencing_token,
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("release fence");
        assert!(
            fixture
                .repository
                .claim_next_merge(Utc::now(), Uuid::new_v4(), Some("test"))
                .await
                .expect("queue scan")
                .is_none()
        );
        let failed = fixture
            .repository
            .get_merge_entry(entry.id)
            .await
            .expect("load")
            .expect("entry");
        assert_eq!(failed.status, MergeQueueStatus::Failed);
        assert!(failed.last_error.is_some());
    }
}
