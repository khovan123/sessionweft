use std::{env, sync::Arc, time::Duration};

use anyhow::Context;
use chrono::{Duration as ChronoDuration, Utc};
use sessionweft_provider::{EchoProvider, OllamaProvider, ProviderRegistry};
use sessionweft_scheduler::{
    ExponentialBackoff, SchedulerService, TaskExecutionQueueRepository, TaskExecutionRepository,
    TaskExecutionService,
};
use sessionweft_scheduler_sqlite::SqliteSchedulerRepository;
use sessionweft_task_runner::{EchoTool, ProviderToolRunner, ToolRegistry};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cancellation = CancellationToken::new();
    let shutdown = cancellation.clone();
    let signal_task = tokio::spawn(async move {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(operation = "execution_shutdown_signal", error = %error, "failed to listen for Ctrl+C");
        }
        shutdown.cancel();
    });
    let result = run(cancellation).await;
    signal_task.abort();
    result
}

async fn run(cancellation: CancellationToken) -> anyhow::Result<()> {
    let database_url =
        env::var("SESSIONWEFT_DATABASE_URL").unwrap_or_else(|_| "sqlite://sessionweft.db".into());
    let batch_limit = env_usize("SESSIONWEFT_EXECUTION_BATCH_LIMIT", 50)?;
    let running_timeout_seconds = env_i64("SESSIONWEFT_EXECUTION_RUNNING_TIMEOUT_SECONDS", 300)?;
    let minimum_backoff = env_u64("SESSIONWEFT_EXECUTION_MIN_BACKOFF_MS", 100)?;
    let maximum_backoff = env_u64("SESSIONWEFT_EXECUTION_MAX_BACKOFF_MS", 5_000)?;
    if running_timeout_seconds <= 0 {
        anyhow::bail!("SESSIONWEFT_EXECUTION_RUNNING_TIMEOUT_SECONDS must be positive");
    }

    let repository = Arc::new(
        SqliteSchedulerRepository::connect(&database_url)
            .await
            .context("failed to initialize task execution repository")?,
    );
    let execution_service = TaskExecutionService::new(Arc::clone(&repository));
    let scheduler_service = SchedulerService::new(Arc::clone(&repository));
    let runner = provider_tool_runner()?;
    let mut backoff = ExponentialBackoff::new(minimum_backoff, maximum_backoff)
        .context("invalid execution worker backoff")?;

    info!(
        operation = "execution_worker_start",
        batch_limit,
        running_timeout_seconds,
        minimum_backoff,
        maximum_backoff,
        "Provider and Tool execution worker started"
    );

    loop {
        if cancellation.is_cancelled() {
            break;
        }
        let correlation_id = Uuid::new_v4();
        let mut progress = false;
        let stale_before = Utc::now() - ChronoDuration::seconds(running_timeout_seconds);
        match repository
            .mark_stale_running_uncertain(
                stale_before,
                batch_limit,
                correlation_id,
                Some("execution-worker"),
            )
            .await
        {
            Ok(records) => {
                if !records.is_empty() {
                    progress = true;
                    warn!(
                        operation = "execution_uncertain",
                        %correlation_id,
                        count = records.len(),
                        "stale running executions require reconciliation"
                    );
                }
            }
            Err(error) => {
                warn!(operation = "execution_uncertain_scan", %correlation_id, error = %error);
            }
        }

        match repository.executable_claim_ids(batch_limit).await {
            Ok(claim_ids) => {
                for claim_id in claim_ids {
                    match execution_service
                        .prepare_claim(
                            claim_id,
                            Utc::now(),
                            correlation_id,
                            Some("execution-worker"),
                        )
                        .await
                    {
                        Ok(Some(_)) => progress = true,
                        Ok(None) => {}
                        Err(error) => warn!(
                            operation = "execution_prepare",
                            %correlation_id,
                            %claim_id,
                            error = %error,
                            "claim execution preparation failed"
                        ),
                    }
                }
            }
            Err(error) => warn!(
                operation = "execution_discover",
                %correlation_id,
                error = %error,
                "failed to discover executable claims"
            ),
        }

        match repository.prepared_executions(batch_limit).await {
            Ok(executions) => {
                for execution in executions {
                    match execution_service
                        .execute_prepared(
                            execution.id,
                            &runner,
                            Utc::now(),
                            correlation_id,
                            Some("execution-worker"),
                        )
                        .await
                    {
                        Ok(terminal) => {
                            progress = true;
                            info!(
                                operation = "execution_terminal",
                                %correlation_id,
                                execution_id = %terminal.id,
                                claim_id = %terminal.claim_id,
                                status = ?terminal.status,
                                "task action reached a terminal ledger state"
                            );
                        }
                        Err(error) => warn!(
                            operation = "execution_run",
                            %correlation_id,
                            execution_id = %execution.id,
                            error = %error,
                            "task action execution failed before terminal persistence"
                        ),
                    }
                }
            }
            Err(error) => {
                warn!(operation = "execution_prepared_scan", %correlation_id, error = %error)
            }
        }

        match repository
            .succeeded_unfinalized_executions(batch_limit)
            .await
        {
            Ok(executions) => {
                for execution in executions {
                    match scheduler_service
                        .complete_claim(
                            execution.claim_id,
                            correlation_id,
                            Some("execution-worker"),
                        )
                        .await
                    {
                        Ok(_) => {
                            repository
                                .mark_claim_finalized(
                                    execution.id,
                                    correlation_id,
                                    Some("execution-worker"),
                                )
                                .await
                                .context("failed to persist successful claim finalization")?;
                            progress = true;
                        }
                        Err(error) => warn!(
                            operation = "execution_finalize_success",
                            %correlation_id,
                            execution_id = %execution.id,
                            error = %error
                        ),
                    }
                }
            }
            Err(error) => {
                warn!(operation = "execution_success_scan", %correlation_id, error = %error)
            }
        }

        match repository.failed_unfinalized_executions(batch_limit).await {
            Ok(executions) => {
                for execution in executions {
                    let error = execution
                        .sanitized_error
                        .as_deref()
                        .unwrap_or("task action failed");
                    match scheduler_service
                        .fail_claim(
                            execution.claim_id,
                            error,
                            correlation_id,
                            Some("execution-worker"),
                        )
                        .await
                    {
                        Ok(_) => {
                            repository
                                .mark_claim_finalized(
                                    execution.id,
                                    correlation_id,
                                    Some("execution-worker"),
                                )
                                .await
                                .context("failed to persist failed claim finalization")?;
                            progress = true;
                        }
                        Err(error) => warn!(
                            operation = "execution_finalize_failure",
                            %correlation_id,
                            execution_id = %execution.id,
                            error = %error
                        ),
                    }
                }
            }
            Err(error) => {
                warn!(operation = "execution_failure_scan", %correlation_id, error = %error)
            }
        }

        let delay = Duration::from_millis(backoff.observe(progress));
        if !progress {
            debug!(
                operation = "execution_idle",
                %correlation_id,
                delay_millis = delay.as_millis(),
                "execution worker is idle"
            );
        }
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = tokio::time::sleep(delay) => {}
        }
    }

    info!(
        operation = "execution_worker_stop",
        "Provider and Tool execution worker stopped"
    );
    Ok(())
}

fn provider_tool_runner() -> anyhow::Result<ProviderToolRunner> {
    let mut providers = ProviderRegistry::new();
    providers.register(EchoProvider);
    let ollama_url =
        env::var("SESSIONWEFT_OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    providers.register(
        OllamaProvider::new(ollama_url, Duration::from_secs(120))
            .context("failed to initialize Ollama provider")?,
    );
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    Ok(ProviderToolRunner::new(
        Arc::new(providers),
        Arc::new(tools),
    ))
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

fn env_i64(name: &str, default: i64) -> anyhow::Result<i64> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<i64>()
                .with_context(|| format!("invalid {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}
