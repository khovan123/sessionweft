use std::{env, sync::Arc, time::Duration};

use anyhow::Context;
use chrono::{Duration as ChronoDuration, Utc};
use sessionweft_git::{GitMergeCoordinator, GitMergeQueueRepository, MergeExecutionResult};
use sessionweft_git_local::GitCliMergeExecutor;
use sessionweft_git_sqlite::SqliteGitWorktreeRepository;
use sessionweft_scheduler::ExponentialBackoff;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let database_url =
        env::var("SESSIONWEFT_DATABASE_URL").unwrap_or_else(|_| "sqlite://sessionweft.db".into());
    let cancellation = CancellationToken::new();
    let shutdown = cancellation.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(operation = "git_merge_shutdown_signal", error = %error, "failed to listen for Ctrl+C");
        }
        shutdown.cancel();
    });
    let result = run(database_url, cancellation).await;
    signal_task.abort();
    result
}

async fn run(database_url: String, cancellation: CancellationToken) -> anyhow::Result<()> {
    let batch_limit = env_usize("SESSIONWEFT_GIT_MERGE_BATCH_LIMIT", 100)?;
    let stale_seconds = env_u64("SESSIONWEFT_GIT_MERGE_STALE_SECONDS", 300)?;
    let minimum_backoff = env_u64("SESSIONWEFT_GIT_MERGE_MIN_BACKOFF_MS", 250)?;
    let maximum_backoff = env_u64("SESSIONWEFT_GIT_MERGE_MAX_BACKOFF_MS", 5_000)?;
    if batch_limit == 0 || batch_limit > 1_000 {
        anyhow::bail!("SESSIONWEFT_GIT_MERGE_BATCH_LIMIT must be between 1 and 1000");
    }
    if stale_seconds == 0 || stale_seconds > 86_400 {
        anyhow::bail!("SESSIONWEFT_GIT_MERGE_STALE_SECONDS must be between 1 and 86400");
    }

    let repository = Arc::new(
        SqliteGitWorktreeRepository::connect(&database_url)
            .await
            .context("failed to initialize Git merge repository")?,
    );
    let coordinator = GitMergeCoordinator::new(
        Arc::clone(&repository),
        Arc::new(GitCliMergeExecutor::default()),
    );
    let mut backoff = ExponentialBackoff::new(minimum_backoff, maximum_backoff)
        .context("invalid Git merge worker backoff configuration")?;

    info!(
        operation = "git_merge_worker_start",
        batch_limit,
        stale_seconds,
        minimum_backoff,
        maximum_backoff,
        "durable Git merge worker started"
    );

    loop {
        if cancellation.is_cancelled() {
            break;
        }
        let correlation_id = Uuid::new_v4();
        let stale_before = Utc::now()
            - ChronoDuration::seconds(
                i64::try_from(stale_seconds).context("stale seconds exceed i64")?,
            );
        let mut made_progress = false;
        match coordinator
            .reconcile_stale(
                stale_before,
                batch_limit,
                correlation_id,
                Some("git-merge-worker"),
            )
            .await
        {
            Ok(recovered) => {
                if !recovered.is_empty() {
                    made_progress = true;
                    info!(
                        operation = "git_merge_reconcile",
                        %correlation_id,
                        recovered = recovered.len(),
                        "reconciled interrupted Git merges"
                    );
                }
            }
            Err(error) => {
                warn!(
                    operation = "git_merge_reconcile",
                    %correlation_id,
                    error = %error,
                    "Git merge reconciliation failed"
                );
            }
        }

        match repository
            .claim_next_merge(Utc::now(), correlation_id, Some("git-merge-worker"))
            .await
        {
            Ok(Some(entry)) => {
                made_progress = true;
                match coordinator
                    .execute_claimed(entry.id, correlation_id, Some("git-merge-worker"))
                    .await
                {
                    Ok(result) => log_result(correlation_id, &result),
                    Err(error) => {
                        warn!(
                            operation = "git_merge_execute",
                            %correlation_id,
                            queue_id = %entry.id,
                            error = %error,
                            "Git merge execution failed"
                        );
                        if let Err(mark_error) = repository
                            .mark_merge_failed(
                                entry.id,
                                &error.to_string(),
                                Utc::now(),
                                correlation_id,
                                Some("git-merge-worker"),
                            )
                            .await
                        {
                            warn!(
                                operation = "git_merge_fail_record",
                                %correlation_id,
                                queue_id = %entry.id,
                                error = %mark_error,
                                "failed to persist Git merge failure"
                            );
                        }
                    }
                }
            }
            Ok(None) => {
                debug!(
                    operation = "git_merge_poll",
                    %correlation_id,
                    "Git merge queue has no eligible entry"
                );
            }
            Err(error) => {
                warn!(
                    operation = "git_merge_claim",
                    %correlation_id,
                    error = %error,
                    "failed to claim Git merge queue entry"
                );
            }
        }

        let delay = Duration::from_millis(backoff.observe(made_progress));
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = tokio::time::sleep(delay) => {}
        }
    }

    info!(
        operation = "git_merge_worker_stop",
        "durable Git merge worker stopped"
    );
    Ok(())
}

fn log_result(correlation_id: Uuid, result: &MergeExecutionResult) {
    match result {
        MergeExecutionResult::Merged(entry) => info!(
            operation = "git_merge_execute",
            %correlation_id,
            queue_id = %entry.id,
            merge_commit = ?entry.merge_commit,
            "Git merge completed"
        ),
        MergeExecutionResult::Requeued(entry) => info!(
            operation = "git_merge_execute",
            %correlation_id,
            queue_id = %entry.id,
            head_commit = %entry.head_commit,
            "Git merge entry requeued for renewed gates"
        ),
        MergeExecutionResult::Conflict { entry, task } => warn!(
            operation = "git_merge_execute",
            %correlation_id,
            queue_id = %entry.id,
            conflict_task_id = %task.id,
            paths = ?task.paths,
            "Git merge conflict created an explicit resolution task"
        ),
        MergeExecutionResult::Failed(entry) => warn!(
            operation = "git_merge_execute",
            %correlation_id,
            queue_id = %entry.id,
            error = ?entry.last_error,
            "Git merge entry failed"
        ),
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .init();
}

fn env_usize(name: &str, default: usize) -> anyhow::Result<usize> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u64(name: &str, default: u64) -> anyhow::Result<u64> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("invalid {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}
