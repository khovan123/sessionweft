include!("lib.rs");

mod merge_execution;

pub use merge_execution::GitCliMergeExecutor;

#[cfg(test)]
mod merge_execution_tests;
