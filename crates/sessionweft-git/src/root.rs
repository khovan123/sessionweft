include!("lib.rs");

mod merge_queue;

pub use merge_queue::{
    GIT_MERGE_QUEUE_SCHEMA_VERSION, GitMergeQueueRepository, GitMergeQueueService, MergeConflict,
    MergeGateStatus, MergeQueueEntry, MergeQueueRequest, MergeQueueStatus, ReviewGate, TestGate,
};
