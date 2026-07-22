use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    GitMergeQueueRepository, GitOperationError, GitRepositoryError, GitWorkspaceError,
    MergeQueueEntry, MergeQueueStatus,
};

pub const GIT_CONFLICT_TASK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeInspection {
    pub target_commit: String,
    pub source_commit: String,
    pub worktree_clean: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    Rebased {
        target_commit: String,
        head_commit: String,
    },
    Conflict {
        target_commit: String,
        paths: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastForwardOutcome {
    Applied { merge_commit: String },
    TargetMoved { actual_target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackOutcome {
    Applied,
    TargetMoved { actual_target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeRecoveryObservation {
    Merged {
        merge_commit: String,
    },
    Rebased {
        target_commit: String,
        head_commit: String,
    },
    InterruptedRebase {
        target_commit: String,
        source_commit: String,
    },
    Diverged {
        target_commit: String,
        source_commit: String,
    },
    MissingWorktree,
}

#[async_trait]
pub trait GitMergeExecutor: Send + Sync {
    async fn inspect(&self, entry: &MergeQueueEntry) -> Result<MergeInspection, GitOperationError>;

    async fn rebase(
        &self,
        entry: &MergeQueueEntry,
        target_commit: &str,
    ) -> Result<RebaseOutcome, GitOperationError>;

    async fn fast_forward(
        &self,
        entry: &MergeQueueEntry,
        expected_target: &str,
        source_head: &str,
    ) -> Result<FastForwardOutcome, GitOperationError>;

    async fn rollback(
        &self,
        entry: &MergeQueueEntry,
        expected_current: &str,
        restore_target: &str,
    ) -> Result<RollbackOutcome, GitOperationError>;

    async fn recover(
        &self,
        entry: &MergeQueueEntry,
    ) -> Result<MergeRecoveryObservation, GitOperationError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictTaskStatus {
    Open,
    Resolved,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictResolutionTask {
    pub schema_version: u32,
    pub id: Uuid,
    pub queue_id: Uuid,
    pub worktree_id: Uuid,
    pub session_id: sessionweft_core::SessionId,
    pub claim_id: Uuid,
    pub workspace_id: String,
    pub source_branch: String,
    pub target_branch: String,
    pub target_commit: String,
    pub source_commit: String,
    pub paths: Vec<String>,
    pub status: ConflictTaskStatus,
    pub resolution_commit: Option<String>,
    pub assigned_agent_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConflictResolutionTask {
    pub fn new(
        entry: &MergeQueueEntry,
        target_commit: impl Into<String>,
        source_commit: impl Into<String>,
        paths: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<Self, GitWorkspaceError> {
        let target_commit = target_commit.into();
        let source_commit = source_commit.into();
        validate_commit(&target_commit)?;
        validate_commit(&source_commit)?;
        let mut paths = paths
            .into_iter()
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Err(GitWorkspaceError::Validation(
                "conflict task requires at least one path".into(),
            ));
        }
        Ok(Self {
            schema_version: GIT_CONFLICT_TASK_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            queue_id: entry.id,
            worktree_id: entry.worktree_id,
            session_id: entry.session_id,
            claim_id: entry.claim_id,
            workspace_id: entry.workspace_id.clone(),
            source_branch: entry.source_branch.clone(),
            target_branch: entry.target_branch.clone(),
            target_commit,
            source_commit,
            paths,
            status: ConflictTaskStatus::Open,
            resolution_commit: None,
            assigned_agent_id: None,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeExecutionResult {
    Merged(MergeQueueEntry),
    Requeued(MergeQueueEntry),
    Conflict {
        entry: MergeQueueEntry,
        task: ConflictResolutionTask,
    },
    Failed(MergeQueueEntry),
}

#[async_trait]
pub trait GitMergeRecoveryRepository: GitMergeQueueRepository {
    async fn requeue_after_rebase(
        &self,
        queue_id: Uuid,
        target_commit: &str,
        head_commit: &str,
        reason: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn requeue_after_target_move(
        &self,
        queue_id: Uuid,
        actual_target: &str,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeQueueEntry, GitRepositoryError>;

    async fn create_conflict_task(
        &self,
        queue_id: Uuid,
        target_commit: &str,
        source_commit: &str,
        paths: Vec<String>,
        now: DateTime<Utc>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<(MergeQueueEntry, ConflictResolutionTask), GitRepositoryError>;

    async fn get_conflict_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<ConflictResolutionTask>, GitRepositoryError>;

    async fn stale_merging_entries(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MergeQueueEntry>, GitRepositoryError>;
}

#[derive(Clone)]
pub struct GitMergeCoordinator<R, E>
where
    R: GitMergeRecoveryRepository,
    E: GitMergeExecutor,
{
    repository: Arc<R>,
    executor: Arc<E>,
}

impl<R, E> GitMergeCoordinator<R, E>
where
    R: GitMergeRecoveryRepository,
    E: GitMergeExecutor,
{
    #[must_use]
    pub fn new(repository: Arc<R>, executor: Arc<E>) -> Self {
        Self {
            repository,
            executor,
        }
    }

    pub async fn execute_claimed(
        &self,
        queue_id: Uuid,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeExecutionResult, GitWorkspaceError> {
        let entry = self
            .repository
            .begin_merge(queue_id, Utc::now(), correlation_id, actor_id)
            .await
            .map_err(GitWorkspaceError::Repository)?;
        let inspection = match self.executor.inspect(&entry).await {
            Ok(inspection) => inspection,
            Err(error) => {
                let failed = self
                    .repository
                    .mark_merge_failed(
                        queue_id,
                        &error.to_string(),
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                    .map_err(GitWorkspaceError::Repository)?;
                return Ok(MergeExecutionResult::Failed(failed));
            }
        };
        if !inspection.worktree_clean {
            let failed = self
                .repository
                .mark_merge_failed(
                    queue_id,
                    "source worktree contains uncommitted changes",
                    Utc::now(),
                    correlation_id,
                    actor_id,
                )
                .await
                .map_err(GitWorkspaceError::Repository)?;
            return Ok(MergeExecutionResult::Failed(failed));
        }
        if inspection.source_commit != entry.head_commit {
            let failed = self
                .repository
                .mark_merge_failed(
                    queue_id,
                    "source branch HEAD no longer matches the queued snapshot",
                    Utc::now(),
                    correlation_id,
                    actor_id,
                )
                .await
                .map_err(GitWorkspaceError::Repository)?;
            return Ok(MergeExecutionResult::Failed(failed));
        }

        if inspection.target_commit != entry.base_commit {
            return self
                .execute_rebase(
                    &entry,
                    &inspection.target_commit,
                    correlation_id,
                    actor_id,
                )
                .await;
        }

        match self
            .executor
            .fast_forward(&entry, &inspection.target_commit, &inspection.source_commit)
            .await
            .map_err(GitWorkspaceError::Operation)?
        {
            FastForwardOutcome::Applied { merge_commit } => {
                match self
                    .repository
                    .mark_merged(
                        queue_id,
                        &merge_commit,
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                {
                    Ok(merged) => Ok(MergeExecutionResult::Merged(merged)),
                    Err(repository_error) => {
                        let rollback = self
                            .executor
                            .rollback(
                                &entry,
                                &merge_commit,
                                &inspection.target_commit,
                            )
                            .await;
                        let message = match rollback {
                            Ok(RollbackOutcome::Applied) => format!(
                                "merge persistence failed and target was rolled back: {repository_error}"
                            ),
                            Ok(RollbackOutcome::TargetMoved { actual_target }) => format!(
                                "merge persistence failed; rollback rejected because target moved to {actual_target}: {repository_error}"
                            ),
                            Err(error) => format!(
                                "merge persistence failed and rollback failed: {repository_error}; {error}"
                            ),
                        };
                        let failed = self
                            .repository
                            .mark_merge_failed(
                                queue_id,
                                &message,
                                Utc::now(),
                                correlation_id,
                                actor_id,
                            )
                            .await
                            .map_err(GitWorkspaceError::Repository)?;
                        Ok(MergeExecutionResult::Failed(failed))
                    }
                }
            }
            FastForwardOutcome::TargetMoved { actual_target } => {
                let requeued = self
                    .repository
                    .requeue_after_target_move(
                        queue_id,
                        &actual_target,
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                    .map_err(GitWorkspaceError::Repository)?;
                Ok(MergeExecutionResult::Requeued(requeued))
            }
        }
    }

    async fn execute_rebase(
        &self,
        entry: &MergeQueueEntry,
        target_commit: &str,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<MergeExecutionResult, GitWorkspaceError> {
        self.repository
            .validate_merge_fence(entry.id, Utc::now())
            .await
            .map_err(GitWorkspaceError::Repository)?;
        match self
            .executor
            .rebase(entry, target_commit)
            .await
            .map_err(GitWorkspaceError::Operation)?
        {
            RebaseOutcome::Rebased {
                target_commit,
                head_commit,
            } => {
                let requeued = self
                    .repository
                    .requeue_after_rebase(
                        entry.id,
                        &target_commit,
                        &head_commit,
                        "source branch rebased; review and tests must run again",
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                    .map_err(GitWorkspaceError::Repository)?;
                Ok(MergeExecutionResult::Requeued(requeued))
            }
            RebaseOutcome::Conflict {
                target_commit,
                paths,
            } => {
                let (entry, task) = self
                    .repository
                    .create_conflict_task(
                        entry.id,
                        &target_commit,
                        &entry.head_commit,
                        paths,
                        Utc::now(),
                        correlation_id,
                        actor_id,
                    )
                    .await
                    .map_err(GitWorkspaceError::Repository)?;
                Ok(MergeExecutionResult::Conflict { entry, task })
            }
        }
    }

    pub async fn reconcile_stale(
        &self,
        stale_before: DateTime<Utc>,
        limit: usize,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<Vec<MergeExecutionResult>, GitWorkspaceError> {
        if limit == 0 || limit > 1_000 {
            return Err(GitWorkspaceError::Validation(
                "merge reconciliation limit must be between 1 and 1000".into(),
            ));
        }
        let entries = self
            .repository
            .stale_merging_entries(stale_before, limit)
            .await
            .map_err(GitWorkspaceError::Repository)?;
        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let result = match self.executor.recover(&entry).await {
                Ok(MergeRecoveryObservation::Merged { merge_commit }) => {
                    let merged = self
                        .repository
                        .mark_merged(
                            entry.id,
                            &merge_commit,
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Merged(merged)
                }
                Ok(MergeRecoveryObservation::Rebased {
                    target_commit,
                    head_commit,
                }) => {
                    let requeued = self
                        .repository
                        .requeue_after_rebase(
                            entry.id,
                            &target_commit,
                            &head_commit,
                            "interrupted merge recovered after completed rebase",
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Requeued(requeued)
                }
                Ok(MergeRecoveryObservation::InterruptedRebase { .. }) => {
                    let failed = self
                        .repository
                        .mark_merge_failed(
                            entry.id,
                            "interrupted rebase was aborted during recovery",
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Failed(failed)
                }
                Ok(MergeRecoveryObservation::Diverged {
                    target_commit,
                    source_commit,
                }) => {
                    let failed = self
                        .repository
                        .mark_merge_failed(
                            entry.id,
                            &format!(
                                "merge recovery found divergent target {target_commit} and source {source_commit}"
                            ),
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Failed(failed)
                }
                Ok(MergeRecoveryObservation::MissingWorktree) => {
                    let failed = self
                        .repository
                        .mark_merge_failed(
                            entry.id,
                            "merge recovery could not find the source worktree",
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Failed(failed)
                }
                Err(error) => {
                    let failed = self
                        .repository
                        .mark_merge_failed(
                            entry.id,
                            &error.to_string(),
                            Utc::now(),
                            correlation_id,
                            actor_id,
                        )
                        .await
                        .map_err(GitWorkspaceError::Repository)?;
                    MergeExecutionResult::Failed(failed)
                }
            };
            results.push(result);
        }
        Ok(results)
    }
}

pub trait MergeQueueRecoveryTransition {
    fn requeue_after_rebase(
        &mut self,
        target_commit: &str,
        head_commit: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError>;

    fn requeue_after_target_move(
        &mut self,
        actual_target: &str,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError>;
}

impl MergeQueueRecoveryTransition for MergeQueueEntry {
    fn requeue_after_rebase(
        &mut self,
        target_commit: &str,
        head_commit: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        if self.status != MergeQueueStatus::Merging {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} must be merging before requeue",
                self.id
            )));
        }
        validate_commit(target_commit)?;
        validate_commit(head_commit)?;
        self.base_commit = target_commit.to_owned();
        self.head_commit = head_commit.to_owned();
        self.status = MergeQueueStatus::Queued;
        self.review = crate::ReviewGate::default();
        self.tests = crate::TestGate::default();
        self.merge_commit = None;
        self.conflict = None;
        self.last_error = Some(reason.to_owned());
        self.updated_at = now;
        Ok(())
    }

    fn requeue_after_target_move(
        &mut self,
        actual_target: &str,
        now: DateTime<Utc>,
    ) -> Result<(), GitWorkspaceError> {
        if self.status != MergeQueueStatus::Merging {
            return Err(GitWorkspaceError::InvalidTransition(format!(
                "merge queue entry {} must be merging before retry",
                self.id
            )));
        }
        validate_commit(actual_target)?;
        self.status = MergeQueueStatus::Queued;
        self.review = crate::ReviewGate::default();
        self.tests = crate::TestGate::default();
        self.last_error = Some(format!(
            "target branch moved to {actual_target}; rebase and gates are required"
        ));
        self.updated_at = now;
        Ok(())
    }
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
    use crate::{
        GitFence, GitWorktreeRecord, MergeQueueRequest, WorktreeAllocationRequest,
    };

    fn merging_entry() -> MergeQueueEntry {
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
        .expect("entry");
        entry
            .record_review("reviewer", true, None, now)
            .expect("review");
        entry
            .record_tests("workspace", true, None, now)
            .expect("tests");
        entry.claim(now).expect("claim");
        entry.begin_merge(now).expect("begin");
        entry
    }

    #[test]
    fn successful_rebase_resets_gates_and_returns_to_queue() {
        let mut entry = merging_entry();
        entry
            .requeue_after_rebase(
                "1111111111111111111111111111111111111111",
                "2222222222222222222222222222222222222222",
                "retest",
                Utc::now(),
            )
            .expect("requeue");
        assert_eq!(entry.status, MergeQueueStatus::Queued);
        assert!(!entry.gates_passed());
        assert_eq!(
            entry.base_commit,
            "1111111111111111111111111111111111111111"
        );
    }

    #[test]
    fn conflict_task_normalizes_paths() {
        let entry = merging_entry();
        let task = ConflictResolutionTask::new(
            &entry,
            "1111111111111111111111111111111111111111",
            &entry.head_commit,
            vec!["src/lib.rs".into(), "src/lib.rs".into()],
            Utc::now(),
        )
        .expect("task");
        assert_eq!(task.paths, vec!["src/lib.rs"]);
        assert_eq!(task.status, ConflictTaskStatus::Open);
    }
}
