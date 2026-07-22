use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_git::{
    ConflictResolutionTask, GitMergeRecoveryRepository, GitRepositoryError, GitWorktreeRecord,
    GitWorktreeStatus, MergeGateStatus, MergeQueueEntry, MergeQueueRecoveryTransition,
    MergeQueueStatus,
};
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

use super::{SqliteGitWorktreeRepository, backend};

impl SqliteGitWorktreeRepository {
    async fn ensure_merge_execution_tables(&self) -> Result<(), GitRepositoryError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS git_conflict_tasks (
                task_id TEXT PRIMARY KEY,
                queue_id TEXT NOT NULL UNIQUE,
                worktree_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                claim_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
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
            CREATE INDEX IF NOT EXISTS idx_git_conflict_tasks_status
            ON git_conflict_tasks (status, updated_at, task_id)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn recovery_load_entry(
        transaction: &mut Transaction<'_, Sqlite>,
        queue_id: Uuid,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_merge_queue WHERE queue_id = ?")
            .bind(queue_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?
            .ok_or_else(|| GitRepositoryError::Conflict(format!("merge queue {queue_id} not found")))?;
        serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend)
    }

    async fn recovery_save_entry(
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

    async fn recovery_load_worktree(
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

    async fn recovery_save_worktree_head(
        transaction: &mut Transaction<'_, Sqlite>,
        worktree: &mut GitWorktreeRecord,
        head_commit: &str,
        now: DateTime<Utc>,
    ) -> Result<(), GitRepositoryError> {
        if worktree.status != GitWorktreeStatus::Ready {
            return Err(GitRepositoryError::Conflict(format!(
                "worktree {} is not ready for a rebased head",
                worktree.id
            )));
        }
        worktree.head_commit = Some(head_commit.to_owned());
        worktree.updated_at = now;
        let result = sqlx::query(
            r#"
            UPDATE git_worktrees
            SET head_commit = ?, data_json = ?, updated_at = ?
            WHERE worktree_id = ? AND status = 'ready'
            "#,
        )
        .bind(head_commit)
        .bind(serde_json::to_string(worktree).map_err(backend)?)
        .bind(now.to_rfc3339())
        .bind(worktree.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(GitRepositoryError::Conflict(format!(
                "worktree {} changed while recording rebased head",
                worktree.id
            )));
        }
        Ok(())
    }

    async fn recovery_insert_event(
        transaction: &mut Transaction<'_, Sqlite>,
        entry: &MergeQueueEntry,
        event_type: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        details: serde_json::Value,
    ) -> Result<(), GitRepositoryError> {
        let event = EventEnvelope::new(
            event_type,
            Some(entry.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "queue_id": entry.id,
                "worktree_id": entry.worktree_id,
                "claim_id": entry.claim_id,
                "source_branch": entry.source_branch,
                "target_branch": entry.target_branch,
                "base_commit": entry.base_commit,
                "head_commit": entry.head_commit,
                "status": entry.status,
                "details": details,
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

    async fn recovery_existing_conflict_task(
        transaction: &mut Transaction<'_, Sqlite>,
        queue_id: Uuid,
    ) -> Result<Option<ConflictResolutionTask>, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_conflict_tasks WHERE queue_id = ?")
            .bind(queue_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }
}

#[async_trait]
impl GitMergeRecoveryRepository for SqliteGitWorktreeRepository {
    async fn requeue_after_rebase(
        &self,
        queue_id: Uuid,
        target_commit: &str,
        head_commit: &str,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.ensure_merge_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut entry = Self::recovery_load_entry(&mut transaction, queue_id).await?;
        let mut worktree = Self::recovery_load_worktree(&mut transaction, entry.worktree_id).await?;
        entry
            .requeue_after_rebase(target_commit, head_commit, reason, now)
            .map_err(domain)?;
        Self::recovery_save_worktree_head(&mut transaction, &mut worktree, head_commit, now).await?;
        Self::recovery_save_entry(&mut transaction, &entry).await?;
        Self::recovery_insert_event(
            &mut transaction,
            &entry,
            "git.merge_requeued_after_rebase",
            correlation_id,
            actor_id,
            serde_json::json!({
                "target_commit": target_commit,
                "head_commit": head_commit,
                "reason": reason,
                "gates_reset": true,
            }),
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(entry)
    }

    async fn requeue_after_target_move(
        &self,
        queue_id: Uuid,
        actual_target: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError> {
        self.ensure_merge_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut entry = Self::recovery_load_entry(&mut transaction, queue_id).await?;
        entry
            .requeue_after_target_move(actual_target, now)
            .map_err(domain)?;
        Self::recovery_save_entry(&mut transaction, &entry).await?;
        Self::recovery_insert_event(
            &mut transaction,
            &entry,
            "git.merge_requeued_target_moved",
            correlation_id,
            actor_id,
            serde_json::json!({
                "actual_target": actual_target,
                "gates_reset": true,
            }),
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok(entry)
    }

    async fn create_conflict_task(
        &self,
        queue_id: Uuid,
        target_commit: &str,
        source_commit: &str,
        paths: Vec<String>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(MergeQueueEntry, ConflictResolutionTask), GitRepositoryError> {
        self.ensure_merge_execution_tables().await?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut entry = Self::recovery_load_entry(&mut transaction, queue_id).await?;
        if let Some(task) = Self::recovery_existing_conflict_task(&mut transaction, queue_id).await? {
            return Ok((entry, task));
        }
        let task = ConflictResolutionTask::new(
            &entry,
            target_commit,
            source_commit,
            paths.clone(),
            now,
        )
        .map_err(domain)?;
        entry.mark_conflict(paths, now).map_err(domain)?;
        Self::recovery_save_entry(&mut transaction, &entry).await?;
        sqlx::query(
            r#"
            INSERT INTO git_conflict_tasks (
                task_id, queue_id, worktree_id, session_id, claim_id,
                workspace_id, status, data_json, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?, ?)
            "#,
        )
        .bind(task.id.to_string())
        .bind(task.queue_id.to_string())
        .bind(task.worktree_id.to_string())
        .bind(task.session_id.to_string())
        .bind(task.claim_id.to_string())
        .bind(&task.workspace_id)
        .bind(serde_json::to_string(&task).map_err(backend)?)
        .bind(task.created_at.to_rfc3339())
        .bind(task.updated_at.to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        Self::recovery_insert_event(
            &mut transaction,
            &entry,
            "git.conflict_task_created",
            correlation_id,
            actor_id,
            serde_json::json!({
                "task_id": task.id,
                "target_commit": target_commit,
                "source_commit": source_commit,
                "paths": task.paths,
            }),
        )
        .await?;
        transaction.commit().await.map_err(backend)?;
        Ok((entry, task))
    }

    async fn get_conflict_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<ConflictResolutionTask>, GitRepositoryError> {
        self.ensure_merge_execution_tables().await?;
        let row = sqlx::query("SELECT data_json FROM git_conflict_tasks WHERE task_id = ?")
            .bind(task_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        row.map(|row| serde_json::from_str(row.get::<&str, _>("data_json")).map_err(backend))
            .transpose()
    }

    async fn stale_merging_entries(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MergeQueueEntry>, GitRepositoryError> {
        self.ensure_merge_execution_tables().await?;
        if limit == 0 || limit > 1_000 {
            return Err(GitRepositoryError::Conflict(
                "stale merge query limit must be between 1 and 1000".into(),
            ));
        }
        let rows = sqlx::query(
            r#"
            SELECT data_json FROM git_merge_queue
            WHERE status = 'merging' AND updated_at < ?
            ORDER BY updated_at ASC, sequence ASC
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

const fn gate_status(status: MergeGateStatus) -> &'static str {
    match status {
        MergeGateStatus::Pending => "pending",
        MergeGateStatus::Passed => "passed",
        MergeGateStatus::Failed => "failed",
    }
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
    use sessionweft_orchestration::{
        LockMode, LockRequest, LockResource, OrchestrationService,
    };
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;

    use super::*;

    struct Fixture {
        repository: SqliteGitWorktreeRepository,
        session_id: SessionId,
        agent_id: Uuid,
        lease: sessionweft_orchestration::LockLease,
    }

    async fn fixture() -> Fixture {
        let path = std::env::temp_dir().join(format!(
            "sessionweft-git-merge-recovery-{}.db",
            Uuid::new_v4()
        ));
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
                    ttl_seconds: 300,
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("lease");
        Fixture {
            repository,
            session_id,
            agent_id,
            lease,
        }
    }

    async fn merging_entry(fixture: &Fixture) -> MergeQueueEntry {
        let worktree = fixture
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
        let ready = fixture
            .repository
            .mark_ready(
                worktree.id,
                "abcdef0123456789abcdef0123456789abcdef01",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("ready");
        let queued = fixture
            .repository
            .enqueue(
                MergeQueueRequest {
                    worktree_id: ready.id,
                    target_branch: "main".into(),
                    priority: 0,
                },
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("enqueue");
        fixture
            .repository
            .record_review(
                queued.id,
                "reviewer",
                true,
                None,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("review");
        fixture
            .repository
            .record_tests(
                queued.id,
                "workspace",
                true,
                None,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("tests");
        let claimed = fixture
            .repository
            .claim_next_merge(Utc::now(), Uuid::new_v4(), Some("test"))
            .await
            .expect("claim")
            .expect("entry");
        fixture
            .repository
            .begin_merge(
                claimed.id,
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("begin merge")
    }

    #[tokio::test]
    async fn rebase_requeue_updates_worktree_head_and_resets_gates() {
        let fixture = fixture().await;
        let entry = merging_entry(&fixture).await;
        let rebased = fixture
            .repository
            .requeue_after_rebase(
                entry.id,
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222",
                "retest",
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("requeue");
        assert_eq!(rebased.status, MergeQueueStatus::Queued);
        assert!(!rebased.gates_passed());
        let worktree = fixture
            .repository
            .get(rebased.worktree_id)
            .await
            .expect("worktree lookup")
            .expect("worktree");
        assert_eq!(
            worktree.head_commit.as_deref(),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[tokio::test]
    async fn conflict_task_is_created_once_with_queue_transition() {
        let fixture = fixture().await;
        let entry = merging_entry(&fixture).await;
        let (conflict, task) = fixture
            .repository
            .create_conflict_task(
                entry.id,
                "1111111111111111111111111111111111111111",
                &entry.head_commit,
                vec!["src/lib.rs".into(), "src/lib.rs".into()],
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("conflict");
        assert_eq!(conflict.status, MergeQueueStatus::Conflict);
        assert_eq!(task.paths, vec!["src/lib.rs"]);
        let (_, replay) = fixture
            .repository
            .create_conflict_task(
                entry.id,
                "1111111111111111111111111111111111111111",
                &entry.head_commit,
                vec!["other.rs".into()],
                Utc::now(),
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("idempotent conflict");
        assert_eq!(task.id, replay.id);
    }
}
