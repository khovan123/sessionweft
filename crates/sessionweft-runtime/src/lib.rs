use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use sessionweft_core::{
    EventEnvelope, MessageRole, ProviderMessage, ProviderRequest, Session, SessionId,
};
use sessionweft_provider::{ProviderError, ProviderRegistry};
use sessionweft_storage::{SessionRepository, StorageError};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct RuntimeService<R>
where
    R: SessionRepository,
{
    repository: Arc<R>,
    providers: Arc<ProviderRegistry>,
}

impl<R> Clone for RuntimeService<R>
where
    R: SessionRepository,
{
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            providers: Arc::clone(&self.providers),
        }
    }
}

impl<R> RuntimeService<R>
where
    R: SessionRepository,
{
    #[must_use]
    pub fn new(repository: Arc<R>, providers: Arc<ProviderRegistry>) -> Self {
        Self {
            repository,
            providers,
        }
    }

    pub async fn create_session(
        &self,
        title: impl Into<String>,
        actor_id: Option<&str>,
        correlation_id: Uuid,
    ) -> Result<Session, RuntimeError> {
        let session = Session::new(title).map_err(RuntimeError::Domain)?;
        let event = EventEnvelope::new(
            "session.created",
            Some(session.id),
            correlation_id,
            actor_id,
            json!({
                "session_version": session.version,
                "title": session.title,
            }),
        );
        self.repository
            .create(&session, &[event])
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn get_session(&self, session_id: SessionId) -> Result<Session, RuntimeError> {
        self.repository
            .get(session_id)
            .await?
            .ok_or(RuntimeError::NotFound(session_id))
    }

    pub async fn list_sessions(&self, limit: u32) -> Result<Vec<Session>, RuntimeError> {
        self.repository.list(limit).await.map_err(RuntimeError::from)
    }

    pub async fn append_message(
        &self,
        session_id: SessionId,
        expected_version: u64,
        role: MessageRole,
        content: impl Into<String>,
        actor_id: Option<&str>,
        correlation_id: Uuid,
    ) -> Result<Session, RuntimeError> {
        let mut session = self.get_session(session_id).await?;
        let event = session
            .append_message(
                expected_version,
                role,
                content,
                correlation_id,
                actor_id,
            )
            .map_err(RuntimeError::Domain)?;
        self.repository
            .save(expected_version, &session, &[event])
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn select_provider(
        &self,
        session_id: SessionId,
        expected_version: u64,
        provider: impl Into<String>,
        model: impl Into<String>,
        actor_id: Option<&str>,
        correlation_id: Uuid,
    ) -> Result<Session, RuntimeError> {
        let provider = provider.into();
        if self.providers.get(&provider).is_none() {
            return Err(RuntimeError::Provider(ProviderError::NotRegistered(provider)));
        }

        let mut session = self.get_session(session_id).await?;
        let event = session
            .select_provider(
                expected_version,
                provider,
                model,
                correlation_id,
                actor_id,
            )
            .map_err(RuntimeError::Domain)?;
        self.repository
            .save(expected_version, &session, &[event])
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn archive_session(
        &self,
        session_id: SessionId,
        expected_version: u64,
        actor_id: Option<&str>,
        correlation_id: Uuid,
    ) -> Result<Session, RuntimeError> {
        let mut session = self.get_session(session_id).await?;
        let event = session
            .archive(expected_version, correlation_id, actor_id)
            .map_err(RuntimeError::Domain)?;
        self.repository
            .save(expected_version, &session, &[event])
            .await
            .map_err(RuntimeError::from)
    }

    pub async fn run_provider(
        &self,
        session_id: SessionId,
        expected_version: u64,
        input: impl Into<String>,
        actor_id: Option<&str>,
        correlation_id: Uuid,
    ) -> Result<Session, RuntimeError> {
        let committed_input = self
            .append_message(
                session_id,
                expected_version,
                MessageRole::User,
                input,
                actor_id,
                correlation_id,
            )
            .await?;

        let selection = committed_input
            .provider
            .clone()
            .ok_or(RuntimeError::ProviderNotSelected)?;
        let provider = self
            .providers
            .get(&selection.provider)
            .ok_or_else(|| {
                RuntimeError::Provider(ProviderError::NotRegistered(selection.provider.clone()))
            })?;

        let response = provider
            .complete(ProviderRequest {
                session_id,
                model: selection.model,
                messages: committed_input
                    .messages
                    .iter()
                    .map(ProviderMessage::from)
                    .collect(),
            })
            .await
            .map_err(|source| RuntimeError::ProviderAfterCommit {
                committed_version: committed_input.version,
                source,
            })?;

        let mut session = committed_input;
        let assistant_event = session
            .append_message(
                session.version,
                MessageRole::Assistant,
                response.text,
                correlation_id,
                Some("provider"),
            )
            .map_err(RuntimeError::Domain)?;
        let usage_event = EventEnvelope::new(
            "provider.usage_recorded",
            Some(session.id),
            correlation_id,
            Some("provider"),
            json!({
                "provider": selection.provider,
                "session_version": session.version,
                "provider_request_id": response.provider_request_id,
                "input_tokens": response.usage.input_tokens,
                "output_tokens": response.usage.output_tokens,
            }),
        );

        self.repository
            .save(
                session.version.saturating_sub(1),
                &session,
                &[assistant_event, usage_event],
            )
            .await
            .map_err(RuntimeError::from)
    }
}

#[async_trait]
pub trait EventTransport: Send + Sync {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<(), TransportError>;
}

#[derive(Clone)]
pub struct LocalEventTransport {
    sender: broadcast::Sender<EventEnvelope>,
}

impl LocalEventTransport {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }
}

#[async_trait]
impl EventTransport for LocalEventTransport {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<(), TransportError> {
        let _ = self.sender.send(envelope.clone());
        Ok(())
    }
}

pub async fn run_outbox_publisher<R, T>(
    repository: Arc<R>,
    transport: Arc<T>,
    cancellation: CancellationToken,
    poll_interval: Duration,
) where
    R: SessionRepository + 'static,
    T: EventTransport + 'static,
{
    info!(operation = "outbox_publisher", "outbox publisher started");
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                info!(operation = "outbox_publisher", "outbox publisher stopped");
                return;
            }
            () = tokio::time::sleep(poll_interval) => {}
        }

        let records = match repository.pending_outbox(100).await {
            Ok(records) => records,
            Err(error) => {
                error!(operation = "outbox_read", error = %error, "failed to read outbox");
                continue;
            }
        };

        for record in records {
            let event_id = record.envelope.event_id;
            match transport.publish(&record.envelope).await {
                Ok(()) => {
                    if let Err(error) = repository.mark_outbox_published(event_id).await {
                        error!(
                            operation = "outbox_mark_published",
                            event_id = %event_id,
                            error = %error,
                            "published event could not be marked; duplicate delivery is possible"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        operation = "outbox_publish",
                        event_id = %event_id,
                        attempts = record.publish_attempts,
                        error = %error,
                        "event publication failed"
                    );
                    let sanitized_error = error.to_string();
                    if let Err(storage_error) = repository
                        .mark_outbox_failed(event_id, &sanitized_error)
                        .await
                    {
                        error!(
                            operation = "outbox_mark_failed",
                            event_id = %event_id,
                            error = %storage_error,
                            "failed to record outbox error"
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("domain error: {0}")]
    Domain(#[from] sessionweft_core::DomainError),
    #[error("storage error: {0}")]
    Storage(StorageError),
    #[error("session {0} not found")]
    NotFound(SessionId),
    #[error("provider is not selected for this session")]
    ProviderNotSelected,
    #[error("provider error: {0}")]
    Provider(ProviderError),
    #[error("provider failed after input was committed at version {committed_version}: {source}")]
    ProviderAfterCommit {
        committed_version: u64,
        source: ProviderError,
    },
}

impl From<StorageError> for RuntimeError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound(session_id) => Self::NotFound(session_id),
            other => Self::Storage(other),
        }
    }
}

#[derive(Debug, Error)]
#[error("event transport error: {0}")]
pub struct TransportError(pub String);

#[cfg(test)]
mod tests {
    use sessionweft_provider::EchoProvider;
    use sessionweft_storage::SqliteSessionRepository;

    use super::*;

    async fn test_runtime() -> RuntimeService<SqliteSessionRepository> {
        let repository = Arc::new(
            SqliteSessionRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let mut registry = ProviderRegistry::new();
        registry.register(EchoProvider);
        RuntimeService::new(repository, Arc::new(registry))
    }

    #[tokio::test]
    async fn provider_switch_and_run_preserve_session_identity() {
        let runtime = test_runtime().await;
        let correlation_id = Uuid::new_v4();
        let session = runtime
            .create_session("runtime", Some("test"), correlation_id)
            .await
            .expect("create");
        let id = session.id;
        let session = runtime
            .select_provider(
                id,
                session.version,
                "echo",
                "test-model",
                Some("test"),
                correlation_id,
            )
            .await
            .expect("select");
        let session = runtime
            .run_provider(
                id,
                session.version,
                "hello",
                Some("test"),
                correlation_id,
            )
            .await
            .expect("run");

        assert_eq!(session.id, id);
        assert_eq!(session.version, 3);
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].content, "[echo:test-model] hello");
    }

    #[tokio::test]
    async fn outbox_publisher_marks_events_as_published() {
        let repository = Arc::new(
            SqliteSessionRepository::connect("sqlite::memory:")
                .await
                .expect("repository"),
        );
        let mut registry = ProviderRegistry::new();
        registry.register(EchoProvider);
        let runtime = RuntimeService::new(Arc::clone(&repository), Arc::new(registry));
        runtime
            .create_session("outbox", None, Uuid::new_v4())
            .await
            .expect("create");

        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_outbox_publisher(
            Arc::clone(&repository),
            Arc::new(LocalEventTransport::new(16)),
            cancellation.clone(),
            Duration::from_millis(5),
        ));
        tokio::time::sleep(Duration::from_millis(25)).await;
        cancellation.cancel();
        task.await.expect("publisher task");

        assert!(repository.pending_outbox(10).await.expect("outbox").is_empty());
    }
}
