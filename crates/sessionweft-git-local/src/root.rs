include!("lib.rs");

mod merge_execution;
mod worktree_mutation;

pub use merge_execution::GitCliMergeExecutor;
pub use worktree_mutation::GitCliWorktreeCommitter;

#[cfg(test)]
mod merge_execution_tests;
