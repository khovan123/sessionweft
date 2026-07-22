include!("lib.rs");

mod merge_execution;

#[expect(
    clippy::too_many_arguments,
    reason = "merge queue transitions carry durable gate, time and audit context"
)]
mod merge_queue;
mod worktree_mutation;

pub use merge_execution::{
    ConflictResolutionTask, ConflictTaskStatus, FastForwardOutcome, GIT_CONFLICT_TASK_SCHEMA_VERSION,
    GitMergeCoordinator, GitMergeExecutor, GitMergeRecoveryRepository, MergeExecutionResult,
    MergeInspection, MergeQueueRecoveryTransition, MergeRecoveryObservation, RebaseOutcome,
    RollbackOutcome,
};
pub use merge_queue::{
    GIT_MERGE_QUEUE_SCHEMA_VERSION, GitMergeQueueRepository, GitMergeQueueService, MergeConflict,
    MergeGateStatus, MergeQueueEntry, MergeQueueRequest, MergeQueueStatus, ReviewGate, TestGate,
};
pub use worktree_mutation::{
    GitWorktreeCommitter, GitWorktreeMutationRepository, GitWorktreeMutationService,
    WorktreeCommitRequest, WorktreeCommitResult,
};
