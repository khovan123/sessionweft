use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    GitRepositoryError, GitWorkspaceError, GitWorktreeRecord, GitWorktreeStatus,
};

pub const GIT_MERGE_QUEUE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeGateStatus {
    #[default]
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewGate {
    pub status: MergeGateStatus,
    pub reviewer_id: Option<String>,
    pub note: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
}

impl Default for ReviewGate {
    fn default() -> Self {
        Self {
            status: MergeGateStatus::Pending,
            reviewer_id: None,
            note: None,
            decided_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestGate {
    pub status: MergeGateStatus,
    pub suite: Option<String>,
    pub summary: Option<String>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Default for TestGate {
    fn default() -> Self {
        Self {
            status: MergeGateStatus::Pending,
            suite: None,
            summary: None,
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeQueueStatus {
    Queued,
    Claimed,
    Merging,
    Conflict,
    Merged,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    pub paths: Vec<String>,
    pub detected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeQueueRequest {
    pub worktree_id: Uuid,
    pub target_branch: String,
    pub priority: i32,
}

impl MergeQueueRequest {
    pub fn validate(&self) -> Result<(), GitWorkspaceError> {
        validate_branch(&self.target_branch)?;
        if !(-1_000..=1_000).contains(&self.priority) {
            return Err(GitWorkspaceError::Validation(
                "merge queue priority must be between -1000 and 1000".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueueEntry {
    pub schema_version: u32,
    pub id: Uuid,
    pub sequence: u64,
    pub priority: i32,
    pub worktree_id: Uuid,
    pub session_id: sessionweft_core::SessionId,
    pub claim_id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: String,
    pub repository_root: String,
    pub source_branch: String,
    pub target_branch: String,
    pub base_commit: String,
    pub head_commit: String,
    pub merge_commit: Option<String>,
    pub fence: crate::GitFence,
    pub review: ReviewGate,
    pub tests: TestGate,
    pub status: MergeQueueStatus,
    pub conflict: Option<MergeConflict>,
    pub cancellation_reason: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MergeQueueEntry {
    pub fn new(
        request: MergeQueueRequest,
        worktree: &GitWorktreeRecord,
        sequence: u64,
        now: DateTime<Utc>,
    ) -> Result<Self, GitWorkspaceError> {
        request.validate()?;
        if worktree.id != request.worktree_id {
            return Err(GitWorkspaceError::Validation(
                "merge request references a different worktree".into(),
            ));
        }
        if worktree.status != GitWorktreeStatus::Ready {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "worktree {} must be ready before enqueue",
                worktree.id
            )));
        }
        if request.target_branch == worktree.branch_name {
            return Err(GitWorkspaceError::Validation(
                "source and target branches must differ".into(),
            ));
        }
        let head_commit = worktree.head_commit.clone().ok_or_else(|| {
            GitWorkspaceError::Validation("ready worktree is missing its head commit".into())
        })?;
        validate_commit(&head_commit)?;
        if sequence == 0 {
            return Err(GitWorkspaceError::Validation(
                "merge queue sequence must be non-zero".into(),
            ));
        }
        Ok(Self {
            schema_version: GIT_MERGE_QUEUE_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            sequence,
            priority: request.priority,
            worktree_id: worktree.id,
            session_id: worktree.session_id,
            claim_id: worktree.claim_id,
            agent_id: worktree.agent_id,
            workspace_id: worktree.workspace_id.clone(),
            repository_root: worktree.repository_root.clone(),
            source_branch: worktree.branch_name.clone(),
            target_branch: request.target_branch,
            base_commit: worktree.base_commit.clone(),
            head_commit,
            merge_commit: None,
            fence: worktree.fence.clone(),
            review: ReviewGate::default(),
            tests: TestGate::default(),
            status: MergeQueueStatus::Queued,
            conflict: None,
            cancellation_reason: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
    }

    #[must_use]
    pub const fn gates_passed(&self) -> bool {
        matches!(self.review.status, MergeGateStatus::Passed)
            && matches!(self.tests.status, MergeGateStatus::Passed)
    }

    pub fn record_review(
        &mut self,
        reviewer_id: impl Into<String>,
        approved: bool,
        note: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_queued()?;
        let reviewer_id = reviewer_id.into().trim().to_owned();
        if reviewer_id.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "review decision requires a reviewer ID".into(),
            ));
        }
        self.review = ReviewGate {
            status: if approved {
                MergeGateStatus::Passed
            } else {
                MergeGateStatus::Failed
            },
            reviewer_id: Some(reviewer_id),
            note: note.filter(|value| !value.trim().is_empty()),
            decided_at: Some(now),
        };
        self.updated_at = now;
        Ok(())
    }

    pub fn record_tests(
        &mut self,
        suite: impl Into<String>,
        passed: bool,
        summary: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_queued()?;
        let suite = suite.into().trim().to_owned();
        if suite.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "test decision requires a suite name".into(),
            ));
        }
        self.tests = TestGate {
            status: if passed {
                MergeGateStatus::Passed
            } else {
                MergeGateStatus::Failed
            },
            suite: Some(suite),
            summary: summary.filter(|value| !value.trim().is_empty()),
            completed_at: Some(now),
        };
        self.updated_at = now;
        Ok(())
    }

    pub fn cancel(
        &mut self,
        reason: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        if !matches!(self.status, MergeQueueStatus::Queued | MergeQueueStatus::Claimed) {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} cannot be cancelled from {:?}",
                self.id, self.status
            )));
        }
        let reason = reason.into().trim().to_owned();
        if reason.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "merge cancellation requires a reason".into(),
            ));
        }
        self.status = MergeQueueStatus::Cancelled;
        self.cancellation_reason = Some(reason);
        self.updated_at = now;
        Ok(())
    }

    pub fn claim(&mut self, now: DateTime<Utc>) -> Result<(), GitWorkspaceError> {
        self.ensure_queued()?;
        if !self.gates_passed() {
            return Err(GitWorkspaceError::InvalidTransition(
                "merge entry cannot be claimed until review and tests pass".into(),
            ));
        }
        self.status = MergeQueueStatus::Claimed;
        self.updated_at = now;
        Ok(())
    }

    pub fn begin_merge(&mut self, now: DateTime<Utc>) -> Result<(), GitWorkspaceError> {
        if self.status != MergeQueueStatus::Claimed {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} must be claimed before merge",
                self.id
            )));
        }
        self.status = MergeQueueStatus::Merging;
        self.updated_at = now;
        Ok(())
    }

    pub fn record_rebased_head(
        &mut self,
        head_commit: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_merging()?;
        let head_commit = head_commit.into();
        validate_commit(&head_commit)?;
        self.head_commit = head_commit;
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_conflict(
        &mut self,
        paths: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_merging()?;
        let mut paths = paths
            .into_iter()
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "merge conflict requires at least one path".into(),
            ));
        }
        self.status = MergeQueueStatus::Conflict;
        self.conflict = Some(MergeConflict {
            paths,
            detected_at: now,
        });
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_merged(
        &mut self,
        merge_commit: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        self.ensure_merging()?;
        let merge_commit = merge_commit.into();
        validate_commit(&merge_commit)?;
        self.status = MergeQueueStatus::Merged;
        self.merge_commit = Some(merge_commit);
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_failed(
        &mut self,
        sanitized_error: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        if !matches!(self.status, MergeQueueStatus::Claimed | MergeQueueStatus::Merging) {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} cannot fail from {:?}",
                self.id, self.status
            )));
        }
        let sanitized_error = sanitized_error.into().trim().to_owned();
        if sanitized_error.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "failed merge requires a sanitized error".into(),
            ));
        }
        self.status = MergeQueueStatus::Failed;
        self.last_error = Some(sanitized_error);
        self.updated_at = now;
        Ok(())
    }

    fn ensure_queued(&self) -> Result<(), GitWorkspaceError> {
        if self.status != MergeQueueStatus::Queued {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} is not queued",
                self.id
            )));
        }
        Ok(())
    }

    fn ensure_merging(&self) -> Result<(), GitWorkspaceError> {
        if self.status != MergeQueueStatus::Merging {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} is not merging",
                self.id
            )));
        }
        Ok(())
    }
}

#[async_trait]
pub trait GitMergeQueueRepository: Send + Sync {
    async fn enqueue(
        &self,
        request: MergeQueueRequest,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn get_merge_entry(
        &self,
        queue_id: Uuid,
    ) -> Result<Option<MergeQueueEntry>, GitRepositoryError>;

    async fn record_review(
        &self,
        queue_id: Uuid,
        reviewer_id: &str,
        approved: bool,
        note: Option<&str>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn record_tests(
        &self,
        queue_id: Uuid,
        suite: &str,
        passed: bool,
        summary: Option<&str>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn cancel_merge(
        &self,
        queue_id: Uuid,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn claim_next_merge(
        &self,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Option<MergeQueueEntry>, GitRepositoryError>;

    async fn validate_merge_fence(
        &self,
        queue_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), GitRepositoryError>;

    async fn begin_merge(
        &self,
        queue_id: Uuid,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn record_rebased_head(
        &self,
        queue_id: Uuid,
        head_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn mark_merge_conflict(
        &self,
        queue_id: Uuid,
        paths: Vec<String>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn mark_merged(
        &self,
        queue_id: Uuid,
        merge_commit: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn mark_merge_failed(
        &self,
        queue_id: Uuid,
        sanitized_error: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;
}

#[derive(Clone)]
pub struct GitMergeQueueService<R>
where
    R: GitMergeQueueRepository,
{
    repository: Arc<R>,
}

impl<R> GitMergeQueueService<R>
where
    R: GitMergeQueueRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn enqueue(
        &self,
        request: MergeQueueRequest,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitWorkspaceError> {
        request.validate()?;
        self.repository
            .enqueue(request, Utc::now(), correlation_id, actor_id)
            .await
            .map_err(GitWorkspaceError::Repository)
    }

    pub async fn approve(
        &self,
        queue_id: Uuid,
        reviewer_id: &str,
        note: Option<&str>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitWorkspaceError> {
        self.repository
            .record_review(
                queue_id,
                reviewer_id,
                true,
                note,
                Utc::now(),
                correlation_id,
                actor_id,
            )
            .await
            .map_err(GitWorkspaceError::Repository)
    }

    pub async fn record_test_success(
        &self,
        queue_id: Uuid,
        suite: &str,
        summary: Option<&str>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitWorkspaceError> {
        self.repository
            .record_tests(
                queue_id,
                suite,
                true,
                summary,
                Utc::now(),
                correlation_id,
                actor_id,
            )
            .await
            .map_err(GitWorkspaceError::Repository)
    }
}

fn validate_branch(value: &str) -> Result<(), GitWorkspaceError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 512
        || value.starts_with('-')
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
            "target branch is not a safe Git reference".into(),
        ));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), GitWorkspaceError> {
    let value = value.trim();
    if !(7..=64).contains(&value.len())
        || !value
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(GitWorkspaceError::Validation(
            "commit must be a 7 to 64 character hexadecimal object ID".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use sessionweft_core::SessionId;

    use super::*;
    use crate::{GitFence, WorktreeAllocationRequest};

    fn ready_worktree() -> GitWorktreeRecord {
        let now = Utc::now();
        let mut worktree = GitWorktreeRecord::new(
            WorktreeAllocationRequest {
                session_id: SessionId::new(),
                claim_id: Uuid::new_v4(),
                agent_id: Uuid::new_v4(),
                workspace_id: "workspace".into(),
                repository_root: "/tmp/repository".into(),
                branch_name: "sessionweft/task".into(),
                worktree_path: "/tmp/worktrees/task".into(),
                base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                fence: GitFence {
                    lock_id: Uuid::new_v4(),
                    fencing_token: 1,
                    expires_at: now + Duration::minutes(5),
                },
            },
            now,
        )
        .expect("worktree");
        worktree
            .mark_ready("abcdef0123456789abcdef0123456789abcdef01", now)
            .expect("ready");
        worktree
    }

    #[test]
    fn queue_claim_requires_both_gates() {
        let now = Utc::now();
        let worktree = ready_worktree();
        let mut entry = MergeQueueEntry::new(
            MergeQueueRequest {
                worktree_id: worktree.id,
                target_branch: "main".into(),
                priority: 0,
            },
            &worktree,
            1,
            now,
        )
        .expect("queue entry");
        assert!(entry.claim(now).is_err());
        entry
            .record_review("reviewer", true, None, now)
            .expect("review");
        assert!(entry.claim(now).is_err());
        entry
            .record_tests("workspace", true, None, now)
            .expect("tests");
        entry.claim(now).expect("claim");
        assert_eq!(entry.status, MergeQueueStatus::Claimed);
    }

    #[test]
    fn cancellation_is_terminal_before_merge() {
        let now = Utc::now();
        let worktree = ready_worktree();
        let mut entry = MergeQueueEntry::new(
            MergeQueueRequest {
                worktree_id: worktree.id,
                target_branch: "main".into(),
                priority: 0,
            },
            &worktree,
            1,
            now,
        )
        .expect("queue entry");
        entry.cancel("superseded", now).expect("cancel");
        assert_eq!(entry.status, MergeQueueStatus::Cancelled);
        assert!(entry.record_tests("workspace", true, None, now).is_err());
    }
}
