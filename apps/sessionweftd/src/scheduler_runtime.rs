use std::{env, sync::Arc, time::Duration};

use anyhow::Context;
use chrono::Utc;
use sessionweft_scheduler::{
    ExponentialBackoff, PollingConfig, SchedulerPollingService,
};
use sessionweft_scheduler_sqlite::SqliteSchedulerRepository;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

pub async fn run(
    database_url: String,
    cancellation: CancellationToken,
) -> anyhow::Result<()> {
    let batch_limit = env_usize("SESSIONWEFT_SCHEDULER_BATCH_LIMIT", 100)?;
    let minimum_backoff = env_u64("SESSIONWEFT_SCHEDULER_MIN_BACKOFF_MS", 100)?;
    let maximum_backoff = env_u64("SESSIONWEFT_SCHEDULER_MAX_BACKOFF_MS", 5_000)?;
    let repository = Arc::new(
        SqliteSchedulerRepository::connect(&database_url)
            .await
            .context("failed to initialize Scheduler repository")?,
    );
    let polling = SchedulerPollingService::new(
        repository,
        PollingConfig { batch_limit },
    )
    .context("invalid scheduler polling configuration")?;
    let mut backoff = ExponentialBackoff::new(minimum_backoff, maximum_backoff)
        .context("invalid scheduler backoff configuration")?;

    info!(
        operation = "scheduler_start",
        batch_limit,
        minimum_backoff,
        maximum_backoff,
        "durable scheduler polling started"
    );
    loop {
        if cancellation.is_cancelled() {
            break;
        }
        let correlation_id = Uuid::new_v4();
        let made_progress = match polling
            .tick(Utc::now(), correlation_id, Some("scheduler"))
            .await
        {
            Ok(report) => {
                if report.made_progress() {
                    info!(
                        operation = "scheduler_tick",
                        %correlation_id,
                        stale_claims_recovered = report.stale_claims_recovered,
                        claims_handed_over = report.claims_handed_over,
                        ready_claims_created = report.ready_claims_created,
                        "scheduler tick committed transitions"
                    );
                } else {
                    debug!(
                        operation = "scheduler_tick",
                        %correlation_id,
                        "scheduler tick was idle"
                    );
                }
                report.made_progress()
            }
            Err(error) => {
                warn!(
                    operation = "scheduler_tick",
                    %correlation_id,
                    error = %error,
                    "scheduler tick failed; retrying with backoff"
                );
                false
            }
        };
        let delay = Duration::from_millis(backoff.observe(made_progress));
        tokio::select! {
            () = cancellation.cancelled() => break,
            () = tokio::time::sleep(delay) => {}
        }
    }
    info!(operation = "scheduler_stop", "durable scheduler polling stopped");
    Ok(())
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
