include!("lib.rs");

#[expect(
    clippy::too_many_arguments,
    reason = "merge queue transitions carry durable gate, time and audit context"
)]
mod merge_queue;

pub use merge_queue::{
    GIT_MERGE_QUEUE_SCHEMA_VERSION, GitMergeQueueRepository, GitMergeQueueService, MergeConflict,
    MergeGateStatus, MergeQueueEntry, MergeQueueRequest, MergeQueueStatus, ReviewGate, TestGate,
};
