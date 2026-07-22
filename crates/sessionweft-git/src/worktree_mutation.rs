use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{GitOperationError, GitRepositoryError, GitWorkspaceError, GitWorktreeRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCommitRequest {
    pub worktree_id: Uuid,
    pub paths: Vec<String>,
    pub message: String,
}

impl WorktreeCommitRequest {
    pub fn validate(&self) -> Result<(), GitWorkspaceError> {
        let message = self.message.trim();
        if message.is_empty() || message.len() > 8_192 {
            return Err(GitWorkspaceError::Validation(
                "commit message must contain between 1 and 8192 bytes".into(),
            ));
        }
        let paths = normalize_paths(self.paths.clone())?;
        if paths.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "commit requires at least one repository-relative path".into(),
            ));
        }
        Ok(())
    }

    pub fn normalized_paths(&self) -> Result<Vec<String>, GitWorkspaceError> {
        normalize_paths(self.paths.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCommitResult {
    pub worktree: GitWorktreeRecord,
    pub previous_head: String,
    pub commit: String,
    pub paths: Vec<String>,
}

#[async_trait]
pub trait GitWorktreeMutationRepository: Send + Sync {
    async fn validate_worktree_fence(
        &self,
        worktree_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn record_worktree_commit(
        &self,
        worktree_id: Uuid,
        expected_head: &str,
        commit: &str,
        paths: &[String],
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;
}

#[async_trait]
pub trait GitWorktreeCommitter: Send + Sync {
    async fn stage(
        &self,
        worktree: &GitWorktreeRecord,
        paths: &[String],
    ) -> Result<(), GitOperationError>;

    async fn unstage(
        &self,
        worktree: &GitWorktreeRecord,
        paths: &[String],
    ) -> Result<(), GitOperationError>;

    async fn commit(
        &self,
        worktree: &GitWorktreeRecord,
        message: &str,
    ) -> Result<String, GitOperationError>;

    async fn rollback_commit(
        &self,
        worktree: &GitWorktreeRecord,
        expected_current: &str,
        restore_head: &str,
    ) -> Result<(), GitOperationError>;
}

#[derive(Clone)]
pub struct GitWorktreeMutationService<R, C>
where
    R: GitWorktreeMutationRepository,
    C: GitWorktreeCommitter,
{
    repository: Arc<R>,
    committer: Arc<C>,
}

impl<R, C> GitWorktreeMutationService<R, C>
where
    R: GitWorktreeMutationRepository,
    C: GitWorktreeCommitter,
{
    #[must_use]
    pub fn new(repository: Arc<R>, committer: Arc<C>) -> Self {
        Self {
            repository,
            committer,
        }
    }

    pub async fn commit_changes(
        &self,
        request: &WorktreeCommitRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<WorktreeCommitResult, GitWorkspaceError> {
        request.validate()?;
        let paths = request.normalized_paths()?;
        let before_stage = self
            .repository
            .validate_worktree_fence(request.worktree_id, Utc::now())
            .await
            .map_err(GitWorkspaceError::Repository)?;
        let previous_head = before_stage.head_commit.clone().ok_or_else(|| {
            GitWorkspaceError::InvalidTransition(
                "ready worktree is missing its durable HEAD".into(),
            )
        })?;
        self.committer
            .stage(&before_stage, &paths)
            .await
            .map_err(GitWorkspaceError::Operation)?;

        let before_commit = match self
            .repository
            .validate_worktree_fence(request.worktree_id, Utc::now())
            .await
        {
            Ok(worktree) => worktree,
            Err(error) => {
                let _ = self.committer.unstage(&before_stage, &paths).await;
                return Err(GitWorkspaceError::Repository(error));
            }
        };
        if before_commit.head_commit.as_deref() != Some(previous_head.as_str()) {
            let _ = self.committer.unstage(&before_stage, &paths).await;
            return Err(GitWorkspaceError::InvalidTransition(
                "worktree HEAD changed between stage and commit".into(),
            ));
        }

        let commit = self
            .committer
            .commit(&before_commit, request.message.trim())
            .await
            .map_err(GitWorkspaceError::Operation)?;
        match self
            .repository
            .record_worktree_commit(
                request.worktree_id,
                &previous_head,
                &commit,
                &paths,
                Utc::now(),
                correlation_id,
                actor_id,
            )
            .await
        {
            Ok(worktree) => Ok(WorktreeCommitResult {
                worktree,
                previous_head,
                commit,
                paths,
            }),
            Err(error) => {
                let rollback = self
                    .committer
                    .rollback_commit(&before_commit, &commit, &previous_head)
                    .await;
                match rollback {
                    Ok(()) => Err(GitWorkspaceError::Repository(error)),
                    Err(rollback_error) => Err(GitWorkspaceError::Operation(
                        GitOperationError::Command(format!(
                            "commit persistence failed: {error}; rollback failed: {rollback_error}"
                        )),
                    )),
                }
            }
        }
    }
}

fn normalize_paths(paths: Vec<String>) -> Result<Vec<String>, GitWorkspaceError> {
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        let path = path.trim().replace('\\', "/");
        if path.is_empty()
            || path.len() > 4_096
            || path.starts_with('/')
            || path.contains('\0')
            || path.contains('\n')
            || path.contains('\r')
            || path.split('/').any(|part| part == "..")
        {
            return Err(GitWorkspaceError::Validation(
                "commit paths must be safe repository-relative paths".into(),
            ));
        }
        normalized.push(path);
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_request_normalizes_and_rejects_unsafe_paths() {
        let request = WorktreeCommitRequest {
            worktree_id: Uuid::new_v4(),
            paths: vec!["src\\lib.rs".into(), "src/lib.rs".into()],
            message: "commit".into(),
        };
        assert_eq!(
            request.normalized_paths().expect("paths"),
            vec!["src/lib.rs"]
        );
        let unsafe_request = WorktreeCommitRequest {
            paths: vec!["../secret".into()],
            ..request
        };
        assert!(unsafe_request.validate().is_err());
    }
}
