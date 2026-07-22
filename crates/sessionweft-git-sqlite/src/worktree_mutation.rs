use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sessionweft_core::EventEnvelope;
use sessionweft_git::{
    GitRepositoryError, GitWorktreeMutationRepository, GitWorktreeRecord, GitWorktreeStatus,
};
use sessionweft_orchestration::LockLease;
use sqlx::Row;
use uuid::Uuid;

use super::{SqliteGitWorktreeRepository, backend};

#[async_trait]
impl GitWorktreeMutationRepository for SqliteGitWorktreeRepository {
    async fn validate_worktree_fence(
        &self,
        worktree_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        let row = sqlx::query("SELECT data_json FROM git_worktrees WHERE worktree_id = ?")
            .bind(worktree_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::NotFound(worktree_id))?;
        let worktree = serde_json::from_str::<GitWorktreeRecord>(row.get::<&str, _>("data_json"))
            .map_err(backend)?;
        if worktree.status != GitWorktreeStatus::Ready {
            return Err(GitRepositoryError::Conflict(format!(
                "worktree {worktree_id} is not ready for mutation"
            )));
        }
        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(worktree.fence.lock_id.to_string())
            .fetch_optional(&self.pool)
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
        Ok(worktree)
    }

    async fn record_worktree_commit(
        &self,
        worktree_id: Uuid,
        expected_head: &str,
        commit: &str,
        paths: &[String],
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError> {
        validate_commit(expected_head)?;
        validate_commit(commit)?;
        if paths.is_empty() {
            return Err(GitRepositoryError::Conflict(
                "committed worktree update requires at least one path".into(),
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let row = sqlx::query("SELECT data_json FROM git_worktrees WHERE worktree_id = ?")
            .bind(worktree_id.to_string())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?
            .ok_or(GitRepositoryError::NotFound(worktree_id))?;
        let mut worktree =
            serde_json::from_str::<GitWorktreeRecord>(row.get::<&str, _>("data_json"))
                .map_err(backend)?;
        if worktree.status != GitWorktreeStatus::Ready
            || worktree.head_commit.as_deref() != Some(expected_head)
        {
            return Err(GitRepositoryError::Conflict(
                "durable worktree HEAD changed before commit persistence".into(),
            ));
        }
        let lease_row = sqlx::query("SELECT data_json FROM lock_leases WHERE lock_id = ?")
            .bind(worktree.fence.lock_id.to_string())
            .fetch_optional(&mut *transaction)
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
        worktree.head_commit = Some(commit.to_owned());
        worktree.updated_at = now;
        let result = sqlx::query(
            r#"
            UPDATE git_worktrees
            SET head_commit = ?, data_json = ?, updated_at = ?
            WHERE worktree_id = ? AND head_commit = ? AND status = 'ready'
            "#,
        )
        .bind(commit)
        .bind(serde_json::to_string(&worktree).map_err(backend)?)
        .bind(now.to_rfc3339())
        .bind(worktree_id.to_string())
        .bind(expected_head)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        if result.rows_affected() != 1 {
            return Err(GitRepositoryError::Conflict(
                "worktree changed concurrently while persisting commit".into(),
            ));
        }
        let event = EventEnvelope::new(
            "git.worktree_committed",
            Some(worktree.session_id),
            correlation_id,
            actor_id,
            serde_json::json!({
                "worktree_id": worktree.id,
                "claim_id": worktree.claim_id,
                "agent_id": worktree.agent_id,
                "branch_name": worktree.branch_name,
                "previous_head": expected_head,
                "commit": commit,
                "paths": paths,
                "fencing_token": worktree.fence.fencing_token,
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
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        Ok(worktree)
    }
}

fn validate_commit(value: &str) -> Result<(), GitRepositoryError> {
    let value = value.trim();
    if !(7..=64).contains(&value.len())
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(GitRepositoryError::Conflict(
            "commit must be a 7 to 64 character hexadecimal object ID".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sessionweft_core::SessionId;
    use sessionweft_git::{
        GitFence, GitWorktreeMutationService, GitWorktreeProvisioner, GitWorktreeRepository,
        WorktreeAllocationRequest, WorktreeCommitRequest,
    };
    use sessionweft_git_local::{GitCliWorktreeCommitter, GitCliWorktreeProvisioner};
    use sessionweft_orchestration::{LockMode, LockRequest, LockResource, OrchestrationService};
    use sessionweft_orchestration_sqlite::SqliteOrchestrationRepository;
    use tokio::process::Command;

    use super::*;

    #[tokio::test]
    async fn fenced_commit_updates_git_and_durable_head_then_rejects_stale_lease() {
        let root = std::env::temp_dir().join(format!("sessionweft-commit-root-{}", Uuid::new_v4()));
        let worker =
            std::env::temp_dir().join(format!("sessionweft-commit-worker-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root)
            .await
            .expect("repository directory");
        run_git(&root, &["init", "-b", "main"]).await;
        run_git(
            &root,
            &["config", "user.email", "sessionweft@example.invalid"],
        )
        .await;
        run_git(&root, &["config", "user.name", "SessionWeft Test"]).await;
        tokio::fs::write(root.join("README.md"), "initial\n")
            .await
            .expect("fixture");
        run_git(&root, &["add", "README.md"]).await;
        run_git(&root, &["commit", "-m", "initial"]).await;
        let initial = git_output(&root, &["rev-parse", "HEAD"]).await;

        let database =
            std::env::temp_dir().join(format!("sessionweft-commit-{}.db", Uuid::new_v4()));
        let database_url = format!("sqlite://{}", database.display());
        let orchestration_repository = Arc::new(
            SqliteOrchestrationRepository::connect(&database_url)
                .await
                .expect("orchestration repository"),
        );
        let orchestration = OrchestrationService::new(orchestration_repository);
        let repository = Arc::new(
            SqliteGitWorktreeRepository::connect(&database_url)
                .await
                .expect("Git repository"),
        );
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
        let reserved = repository
            .reserve(
                WorktreeAllocationRequest {
                    session_id,
                    claim_id: Uuid::new_v4(),
                    agent_id,
                    workspace_id: "workspace".into(),
                    repository_root: root.display().to_string(),
                    branch_name: format!("sessionweft/commit-{}", Uuid::new_v4()),
                    worktree_path: worker.display().to_string(),
                    base_commit: initial.clone(),
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
        let provisioner = GitCliWorktreeProvisioner::default();
        let head = provisioner
            .create(&reserved)
            .await
            .expect("create worktree");
        repository
            .mark_ready(reserved.id, &head, Utc::now(), Uuid::new_v4(), Some("test"))
            .await
            .expect("ready");
        tokio::fs::write(worker.join("feature.txt"), "feature\n")
            .await
            .expect("feature file");
        let service = GitWorktreeMutationService::new(
            Arc::clone(&repository),
            Arc::new(GitCliWorktreeCommitter::default()),
        );
        let committed = service
            .commit_changes(
                &WorktreeCommitRequest {
                    worktree_id: reserved.id,
                    paths: vec!["feature.txt".into()],
                    message: "add feature".into(),
                },
                Uuid::new_v4(),
                Some("test"),
            )
            .await
            .expect("commit changes");
        assert_ne!(committed.commit, initial);
        assert_eq!(
            git_output(&worker, &["rev-parse", "HEAD"]).await,
            committed.commit
        );
        assert_eq!(
            repository
                .get(reserved.id)
                .await
                .expect("load worktree")
                .expect("worktree")
                .head_commit
                .as_deref(),
            Some(committed.commit.as_str())
        );

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
        tokio::fs::write(worker.join("blocked.txt"), "blocked\n")
            .await
            .expect("blocked file");
        assert!(
            service
                .commit_changes(
                    &WorktreeCommitRequest {
                        worktree_id: reserved.id,
                        paths: vec!["blocked.txt".into()],
                        message: "must not commit".into(),
                    },
                    Uuid::new_v4(),
                    Some("test"),
                )
                .await
                .is_err()
        );
        let cached = Command::new("git")
            .arg("-C")
            .arg(&worker)
            .args(["diff", "--cached", "--quiet", "--exit-code"])
            .status()
            .await
            .expect("cached status");
        assert!(cached.success());

        let _ = provisioner.remove(&reserved).await;
        let _ = tokio::fs::remove_dir_all(&worker).await;
        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_file(&database).await;
    }

    async fn run_git(root: &std::path::Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .await
            .expect("run git");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn git_output(root: &std::path::Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .await
            .expect("run git");
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .expect("UTF-8")
            .trim()
            .to_owned()
    }
}
