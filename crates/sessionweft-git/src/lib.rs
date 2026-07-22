use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::SessionId;
use thiserror::Error;
use uuid::Uuid;

pub const GIT_WORKTREE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFence {
    pub lock_id: Uuid,
    pub fencing_token: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAllocationRequest {
    pub session_id: SessionId,
    pub claim_id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: String,
    pub repository_root: String,
    pub branch_name: String,
    pub worktree_path: String,
    pub base_commit: String,
    pub fence: GitFence,
}

impl WorktreeAllocationRequest {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), GitWorkspaceError> {
        validate_identifier("workspace ID", &self.workspace_id, 256)?;
        validate_path("repository root", &self.repository_root)?;
        validate_path("worktree path", &self.worktree_path)?;
        validate_branch(&self.branch_name)?;
        validate_commit(&self.base_commit)?;
        if self.repository_root == self.worktree_path {
            return Err(GitWorkspaceError::Validation(
                "worktree path must differ from repository root".into(),
            ));
        }
        if self.fence.fencing_token == 0 {
            return Err(GitWorkspaceError::Validation(
                "worktree allocation requires a non-zero fencing token".into(),
            ));
        }
        if self.fence.expires_at <= now {
            return Err(GitWorkspaceError::Validation(
                "worktree allocation fence is already expired".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitWorktreeStatus {
    Provisioning,
    Ready,
    Failed,
    Abandoned,
    Cleaned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub session_id: SessionId,
    pub claim_id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: String,
    pub repository_root: String,
    pub branch_name: String,
    pub worktree_path: String,
    pub base_commit: String,
    pub head_commit: Option<String>,
    pub fence: GitFence,
    pub status: GitWorktreeStatus,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl GitWorktreeRecord {
    pub fn new(
        request: WorktreeAllocationRequest,
        now: DateTime<Utc>,
    ) -> Result<Self, GitWorkspaceError> {
        request.validate(now)?;
        Ok(Self {
            schema_version: GIT_WORKTREE_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            session_id: request.session_id,
            claim_id: request.claim_id,
            agent_id: request.agent_id,
            workspace_id: request.workspace_id,
            repository_root: request.repository_root,
            branch_name: request.branch_name,
            worktree_path: request.worktree_path,
            base_commit: request.base_commit,
            head_commit: None,
            fence: request.fence,
            status: GitWorktreeStatus::Provisioning,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn mark_ready(
        &mut self,
        head_commit: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_status(GitWorktreeStatus::Provisioning)?;
        let head_commit = head_commit.into();
        validate_commit(&head_commit)?;
        self.head_commit = Some(head_commit);
        self.status = GitWorktreeStatus::Ready;
        self.last_error = None;
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_failed(
        &mut self,
        sanitized_error: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_status(GitWorktreeStatus::Provisioning)?;
        let error = sanitized_error.into().trim().to_owned();
        if error.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "failed worktree requires a sanitized error".into(),
            ));
        }
        self.status = GitWorktreeStatus::Failed;
        self.last_error = Some(error);
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_abandoned(
        &mut self,
        reason: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        if !matches!(
            self.status,
            GitWorktreeStatus::Provisioning | GitWorktreeStatus::Ready | GitWorktreeStatus::Failed
        ) {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "worktree {} cannot be abandoned from {:?}",
                self.id, self.status
            )));
        }
        let reason = reason.into().trim().to_owned();
        if reason.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "abandoned worktree requires a reason".into(),
            ));
        }
        self.status = GitWorktreeStatus::Abandoned;
        self.last_error = Some(reason);
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_cleaned(&mut self, now: DateTime<Utc>) -> Result<(), GitWorkspaceError> {
        if !matches!(
            self.status,
            GitWorktreeStatus::Abandoned | GitWorktreeStatus::Failed
        ) {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "worktree {} cannot be cleaned from {:?}",
                self.id, self.status
            )));
        }
        self.status = GitWorktreeStatus::Cleaned;
        self.updated_at = now;
        Ok(())
    }

    fn ensure_status(&self, expected: GitWorktreeStatus) -> Result<(), GitWorkspaceError> {
        if self.status != expected {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "worktree {} expected {expected:?}, found {:?}",
                self.id, self.status
            )));
        }
        Ok(())
    }
}

#[async_trait]
pub trait GitWorktreeRepository: Send + Sync {
    async fn reserve(
        &self,
        request: WorktreeAllocationRequest,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn get(&self, worktree_id: Uuid)
    -> Result<Option<GitWorktreeRecord>, GitRepositoryError>;

    async fn mark_ready(
        &self,
        worktree_id: Uuid,
        head_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn mark_failed(
        &self,
        worktree_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn mark_abandoned(
        &self,
        worktree_id: Uuid,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn mark_cleaned(
        &self,
        worktree_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitRepositoryError>;

    async fn stale_provisioning(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<GitWorktreeRecord>, GitRepositoryError>;
}

#[async_trait]
pub trait GitWorktreeProvisioner: Send + Sync {
    async fn create(&self, record: &GitWorktreeRecord) -> Result<String, GitOperationError>;

    async fn inspect_head(
        &self,
        record: &GitWorktreeRecord,
    ) -> Result<Option<String>, GitOperationError>;

    async fn remove(&self, record: &GitWorktreeRecord) -> Result<(), GitOperationError>;
}

#[derive(Clone)]
pub struct GitWorktreeService<R, P>
where
    R: GitWorktreeRepository,
    P: GitWorktreeProvisioner,
{
    repository: Arc<R>,
    provisioner: Arc<P>,
}

impl<R, P> GitWorktreeService<R, P>
where
    R: GitWorktreeRepository,
    P: GitWorktreeProvisioner,
{
    #[must_use]
    pub fn new(repository: Arc<R>, provisioner: Arc<P>) -> Self {
        Self {
            repository,
            provisioner,
        }
    }

    pub async fn allocate(
        &self,
        request: WorktreeAllocationRequest,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitWorkspaceError> {
        let reserved = self
            .repository
            .reserve(request, now, correlation_id, actor_id)
            .await
            .map_err(GitWorkspaceError::Repository)?;
        match self.provisioner.create(&reserved).await {
            Ok(head_commit) => self
                .repository
                .mark_ready(
                    reserved.id,
                    &head_commit,
                    Utc::now(),
                    correlation_id,
                    actor_id,
                )
                .await
                .map_err(GitWorkspaceError::Repository),
            Err(error) => {
                let sanitized = error.to_string();
                self.repository
                    .mark_failed(
                        reserved.id,
                        &sanitized,
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                    .map_err(GitWorkspaceError::Repository)?;
                Err(GitWorkspaceError::Operation(error))
            }
        }
    }

    pub async fn abandon(
        &self,
        worktree_id: Uuid,
        reason: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitWorkspaceError> {
        self.repository
            .mark_abandoned(worktree_id, reason, Utc::now(), correlation_id, actor_id)
            .await
            .map_err(GitWorkspaceError::Repository)
    }

    pub async fn cleanup(
        &self,
        worktree_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<GitWorktreeRecord, GitWorkspaceError> {
        let record = self
            .repository
            .get(worktree_id)
            .await
            .map_err(GitWorkspaceError::Repository)?
            .ok_or(GitRepositoryError::NotFound(worktree_id))?;
        if !matches!(
            record.status,
            GitWorktreeStatus::Abandoned | GitWorktreeStatus::Failed
        ) {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "worktree {worktree_id} must be abandoned or failed before cleanup"
            )));
        }
        self.provisioner
            .remove(&record)
            .await
            .map_err(GitWorkspaceError::Operation)?;
        self.repository
            .mark_cleaned(worktree_id, Utc::now(), correlation_id, actor_id)
            .await
            .map_err(GitWorkspaceError::Repository)
    }

    pub async fn reconcile_stale_provisioning(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<GitWorktreeRecord>, GitWorkspaceError> {
        if limit == 0 || limit > 1_000 {
            return Err(GitWorkspaceError::Validation(
                "worktree reconciliation limit must be between 1 and 1000".into(),
            ));
        }
        let candidates = self
            .repository
            .stale_provisioning(stale_before, limit)
            .await
            .map_err(GitWorkspaceError::Repository)?;
        let mut reconciled = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            match self.provisioner.inspect_head(&candidate).await {
                Ok(Some(head_commit)) => reconciled.push(
                    self.repository
                        .mark_ready(
                            candidate.id,
                            &head_commit,
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?,
                ),
                Ok(None) => reconciled.push(
                    self.repository
                        .mark_failed(
                            candidate.id,
                            "provisioning interrupted before worktree creation",
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?,
                ),
                Err(error) => reconciled.push(
                    self.repository
                        .mark_failed(
                            candidate.id,
                            &error.to_string(),
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?,
                ),
            }
        }
        Ok(reconciled)
    }
}

fn validate_identifier(name: &str, value: &str, maximum: usize) -> Result<(), GitWorkspaceError> {
    let value = value.trim();
    if value.is_empty() || value.len() > maximum {
        return Err(GitWorkspaceError::Validation(format!(
            "{name} must contain between 1 and {maximum} bytes"
        )));
    }
    Ok(())
}

fn validate_path(name: &str, value: &str) -> Result<(), GitWorkspaceError> {
    validate_identifier(name, value, 4_096)?;
    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(GitWorkspaceError::Validation(format!(
            "{name} contains an invalid control character"
        )));
    }
    Ok(())
}

fn validate_branch(value: &str) -> Result<(), GitWorkspaceError> {
    validate_identifier("branch name", value, 512)?;
    if value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("@{")
        || value.chars().any(|character| {
            character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
    {
        return Err(GitWorkspaceError::Validation(
            "branch name is not a safe Git reference".into(),
        ));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), GitWorkspaceError> {
    let value = value.trim();
    if !(7..=64).contains(&value.len())
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(GitWorkspaceError::Validation(
            "commit must be a 7 to 64 character hexadecimal object ID".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum GitWorkspaceError {
    #[error("Git workspace validation failed: {0}")]
    Validation(String),
    #[error("Git workspace transition failed: {0}")]
    InvalidTransition(String),
    #[error("Git workspace repository failed: {0}")]
    Repository(#[from] GitRepositoryError),
    #[error("Git worktree operation failed: {0}")]
    Operation(#[from] GitOperationError),
}

#[derive(Debug, Error)]
pub enum GitRepositoryError {
    #[error("Git worktree {0} not found")]
    NotFound(Uuid),
    #[error("Git worktree fence is stale or no longer owned by the Agent")]
    StaleFence,
    #[error("Git worktree conflict: {0}")]
    Conflict(String),
    #[error("Git worktree backend failed: {0}")]
    Backend(String),
}

#[derive(Debug, Error)]
pub enum GitOperationError {
    #[error("Git command failed: {0}")]
    Command(String),
    #[error("Git command returned invalid output: {0}")]
    InvalidOutput(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> WorktreeAllocationRequest {
        WorktreeAllocationRequest {
            session_id: SessionId::new(),
            claim_id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            workspace_id: "workspace".into(),
            repository_root: "/tmp/repository".into(),
            branch_name: "sessionweft/task-1".into(),
            worktree_path: "/tmp/worktrees/task-1".into(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            fence: GitFence {
                lock_id: Uuid::new_v4(),
                fencing_token: 1,
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            },
        }
    }

    #[test]
    fn worktree_state_machine_requires_ordered_transitions() {
        let now = Utc::now();
        let mut record = GitWorktreeRecord::new(request(), now).expect("record");
        assert_eq!(record.status, GitWorktreeStatus::Provisioning);
        record
            .mark_ready("abcdef0123456789abcdef0123456789abcdef01", now)
            .expect("ready");
        assert!(record.mark_cleaned(now).is_err());
        record
            .mark_abandoned("task cancelled", now)
            .expect("abandon");
        record.mark_cleaned(now).expect("clean");
        assert_eq!(record.status, GitWorktreeStatus::Cleaned);
    }

    #[test]
    fn unsafe_branch_name_is_rejected() {
        let mut request = request();
        request.branch_name = "../main".into();
        assert!(request.validate(Utc::now()).is_err());
    }
}
