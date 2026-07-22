use std::{sync::Arc, time::Duration};

use sessionweft_core::MessageRole;
use sessionweft_provider::{OllamaProvider, ProviderRegistry};
use sessionweft_runtime::{RuntimeError, RuntimeService};
use sessionweft_storage::{SessionRepository, SqliteSessionRepository};
use uuid::Uuid;

#[tokio::test]
async fn provider_outage_preserves_committed_user_input_for_recovery() {
    let repository = Arc::new(
        SqliteSessionRepository::connect("sqlite::memory:")
            .await
            .expect("repository"),
    );
    let mut providers = ProviderRegistry::new();
    providers.register(
        OllamaProvider::new("http://127.0.0.1:9", Duration::from_millis(100)).expect("provider"),
    );
    let runtime = RuntimeService::new(Arc::clone(&repository), Arc::new(providers));
    let correlation_id = Uuid::new_v4();
    let session = runtime
        .create_session("provider outage", Some("hardening"), correlation_id)
        .await
        .expect("create");
    let session = runtime
        .select_provider(
            session.id,
            session.version,
            "ollama",
            "unreachable-model",
            Some("hardening"),
            correlation_id,
        )
        .await
        .expect("select provider");

    let error = runtime
        .run_provider(
            session.id,
            session.version,
            "recoverable input",
            Some("hardening"),
            correlation_id,
        )
        .await
        .expect_err("provider must be unavailable");
    assert!(matches!(
        error,
        RuntimeError::ProviderAfterCommit {
            committed_version: 2,
            ..
        }
    ));

    let persisted = repository
        .get(session.id)
        .await
        .expect("load")
        .expect("session");
    assert_eq!(persisted.version, 2);
    assert_eq!(persisted.messages.len(), 1);
    assert_eq!(persisted.messages[0].role, MessageRole::User);
    assert_eq!(persisted.messages[0].content, "recoverable input");
}
