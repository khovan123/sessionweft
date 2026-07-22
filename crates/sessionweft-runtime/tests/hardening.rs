use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use sessionweft_core::{ProviderRequest, ProviderResponse, ProviderUsage};
use sessionweft_provider::{Provider, ProviderError, ProviderRegistry};
use sessionweft_runtime::{RuntimeError, RuntimeService};
use sessionweft_storage::SqliteSessionRepository;
use uuid::Uuid;

struct DelayedOutageProvider;

#[async_trait]
impl Provider for DelayedOutageProvider {
    fn name(&self) -> &'static str {
        "delayed-outage"
    }

    async fn complete(&self, _request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Err(ProviderError::InvalidResponse(
            "simulated provider outage".into(),
        ))
    }
}

struct DelayedHealthyProvider;

#[async_trait]
impl Provider for DelayedHealthyProvider {
    fn name(&self) -> &'static str {
        "delayed-healthy"
    }

    async fn complete(&self, _request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(ProviderResponse {
            text: "healthy response".into(),
            provider_request_id: Some("hardening-request".into()),
            usage: ProviderUsage::default(),
        })
    }
}

async fn runtime() -> RuntimeService<SqliteSessionRepository> {
    let repository = Arc::new(
        SqliteSessionRepository::connect("sqlite::memory:")
            .await
            .expect("repository"),
    );
    let mut registry = ProviderRegistry::new();
    registry.register(DelayedOutageProvider);
    registry.register(DelayedHealthyProvider);
    RuntimeService::new(repository, Arc::new(registry))
}

#[tokio::test]
async fn provider_outage_preserves_committed_input_and_returns_within_timeout_budget() {
    let runtime = runtime().await;
    let correlation = Uuid::new_v4();
    let session = runtime
        .create_session("provider-outage", Some("hardening"), correlation)
        .await
        .expect("session");
    let selected = runtime
        .select_provider(
            session.id,
            session.version,
            "delayed-outage",
            "fixture",
            Some("hardening"),
            correlation,
        )
        .await
        .expect("provider selection");

    let started = Instant::now();
    let error = runtime
        .run_provider(
            selected.id,
            selected.version,
            "persist before external call",
            Some("hardening"),
            correlation,
        )
        .await
        .expect_err("provider must fail");
    assert!(started.elapsed() <= Duration::from_secs(2));
    assert!(matches!(
        error,
        RuntimeError::ProviderAfterCommit {
            committed_version: 2,
            ..
        }
    ));
    let recovered = runtime
        .get_session(selected.id)
        .await
        .expect("recovered Session");
    assert_eq!(recovered.version, 2);
    assert_eq!(recovered.messages.len(), 1);
    assert_eq!(
        recovered.messages[0].content,
        "persist before external call"
    );
}

#[tokio::test]
async fn healthy_provider_stays_within_synthetic_latency_budget() {
    let runtime = runtime().await;
    let correlation = Uuid::new_v4();
    let session = runtime
        .create_session("provider-latency", Some("hardening"), correlation)
        .await
        .expect("session");
    let selected = runtime
        .select_provider(
            session.id,
            session.version,
            "delayed-healthy",
            "fixture",
            Some("hardening"),
            correlation,
        )
        .await
        .expect("provider selection");
    let started = Instant::now();
    let completed = runtime
        .run_provider(
            selected.id,
            selected.version,
            "latency fixture",
            Some("hardening"),
            correlation,
        )
        .await
        .expect("provider completion");
    assert!(started.elapsed() <= Duration::from_secs(1));
    assert_eq!(
        completed.messages.last().expect("assistant").content,
        "healthy response"
    );
}
