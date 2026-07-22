use std::{sync::Arc, time::Duration};

use async_nats::jetstream::{
    self, AckKind,
    consumer::{AckPolicy, DeliverPolicy, pull},
    stream,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use sessionweft_core::EventEnvelope;
use sessionweft_runtime::{EventTransport, TransportError};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct JetStreamConfig {
    pub server_url: String,
    pub stream_name: String,
    pub subject_prefix: String,
    pub max_messages: i64,
    pub duplicate_window: Duration,
    pub supported_schema_version: u32,
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            server_url: "nats://127.0.0.1:4222".into(),
            stream_name: "SESSIONWEFT_EVENTS".into(),
            subject_prefix: "sessionweft.events".into(),
            max_messages: 1_000_000,
            duplicate_window: Duration::from_secs(120),
            supported_schema_version: 1,
        }
    }
}

impl JetStreamConfig {
    fn validate(&self) -> Result<(), JetStreamError> {
        if self.server_url.trim().is_empty()
            || self.stream_name.trim().is_empty()
            || self.subject_prefix.trim().is_empty()
        {
            return Err(JetStreamError::Configuration(
                "server URL, stream name and subject prefix are required".into(),
            ));
        }
        if self.max_messages <= 0 || self.duplicate_window.is_zero() {
            return Err(JetStreamError::Configuration(
                "positive max messages and duplicate window are required".into(),
            ));
        }
        Ok(())
    }

    fn wildcard_subject(&self) -> String {
        format!("{}.>", self.subject_prefix)
    }

    fn event_subject(&self, event_type: &str) -> String {
        format!("{}.{}", self.subject_prefix, normalize_subject_token(event_type))
    }
}

#[derive(Clone)]
pub struct JetStreamEventTransport {
    context: jetstream::Context,
    config: Arc<JetStreamConfig>,
}

impl JetStreamEventTransport {
    pub async fn connect(config: JetStreamConfig) -> Result<Self, JetStreamError> {
        config.validate()?;
        let client = async_nats::connect(&config.server_url)
            .await
            .map_err(|error| JetStreamError::Connection(error.to_string()))?;
        let context = jetstream::new(client);
        context
            .get_or_create_stream(stream::Config {
                name: config.stream_name.clone(),
                subjects: vec![config.wildcard_subject()],
                max_messages: config.max_messages,
                duplicate_window: config.duplicate_window,
                ..Default::default()
            })
            .await
            .map_err(|error| JetStreamError::Api(error.to_string()))?;
        Ok(Self {
            context,
            config: Arc::new(config),
        })
    }

    #[must_use]
    pub fn context(&self) -> &jetstream::Context {
        &self.context
    }

    pub async fn publish_event(&self, event: &EventEnvelope) -> Result<(), JetStreamError> {
        if event.schema_version > self.config.supported_schema_version {
            return Err(JetStreamError::UnsupportedSchema {
                event_id: event.event_id,
                actual: event.schema_version,
                supported: self.config.supported_schema_version,
            });
        }
        let payload = Bytes::from(serde_json::to_vec(event)?);
        self.context
            .publish(self.config.event_subject(&event.event_type), payload)
            .await
            .map_err(|error| JetStreamError::Publish(error.to_string()))?
            .await
            .map_err(|error| JetStreamError::Publish(error.to_string()))?;
        Ok(())
    }

    pub async fn durable_consumer<I, H>(
        &self,
        config: DurableConsumerConfig,
        inbox: Arc<I>,
        handler: Arc<H>,
        cancellation: CancellationToken,
    ) -> Result<(), JetStreamError>
    where
        I: EventInbox + 'static,
        H: EventHandler + 'static,
    {
        config.validate()?;
        let stream = self
            .context
            .get_stream(&self.config.stream_name)
            .await
            .map_err(|error| JetStreamError::Api(error.to_string()))?;
        let consumer = stream
            .get_or_create_consumer(
                &config.durable_name,
                pull::Config {
                    durable_name: Some(config.durable_name.clone()),
                    filter_subject: config
                        .filter_subject
                        .clone()
                        .unwrap_or_else(|| self.config.wildcard_subject()),
                    deliver_policy: DeliverPolicy::All,
                    ack_policy: AckPolicy::Explicit,
                    ack_wait: config.ack_wait,
                    max_deliver: i64::from(config.max_deliveries),
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| JetStreamError::Api(error.to_string()))?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
        loop {
            let message = tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                message = messages.next() => message,
            };
            let Some(message) = message else {
                return Ok(());
            };
            let message = message.map_err(|error| JetStreamError::Consumer(error.to_string()))?;
            let event: EventEnvelope = match serde_json::from_slice(&message.payload) {
                Ok(event) => event,
                Err(error) => {
                    self.publish_dead_letter(&config, None, &message.payload, &error.to_string())
                        .await?;
                    message
                        .ack()
                        .await
                        .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
                    continue;
                }
            };
            if event.schema_version > self.config.supported_schema_version {
                self.publish_dead_letter(
                    &config,
                    Some(event.event_id),
                    &message.payload,
                    "unsupported event schema version",
                )
                .await?;
                message
                    .ack()
                    .await
                    .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
                continue;
            }
            match inbox.claim(&config.durable_name, &event).await? {
                InboxClaim::Completed => {
                    message
                        .ack()
                        .await
                        .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
                }
                InboxClaim::Busy => {
                    message
                        .ack_with(AckKind::Nak(Some(Duration::from_secs(1))))
                        .await
                        .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
                }
                InboxClaim::Acquired { attempts } => {
                    match handler.handle(&event).await {
                        Ok(()) => {
                            inbox.complete(&config.durable_name, event.event_id).await?;
                            message
                                .ack()
                                .await
                                .map_err(|error| JetStreamError::Consumer(error.to_string()))?;
                        }
                        Err(error) => {
                            let next_attempt = inbox
                                .fail(&config.durable_name, event.event_id, &error)
                                .await?
                                .max(attempts.saturating_add(1));
                            if next_attempt >= config.max_deliveries {
                                self.publish_dead_letter(
                                    &config,
                                    Some(event.event_id),
                                    &message.payload,
                                    &error,
                                )
                                .await?;
                                message
                                    .ack()
                                    .await
                                    .map_err(|error| {
                                        JetStreamError::Consumer(error.to_string())
                                    })?;
                            } else {
                                message
                                    .ack_with(AckKind::Nak(Some(config.retry_delay)))
                                    .await
                                    .map_err(|error| {
                                        JetStreamError::Consumer(error.to_string())
                                    })?;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn publish_dead_letter(
        &self,
        config: &DurableConsumerConfig,
        event_id: Option<Uuid>,
        payload: &[u8],
        error: &str,
    ) -> Result<(), JetStreamError> {
        let body = serde_json::json!({
            "event_id": event_id,
            "consumer": config.durable_name,
            "error": error.chars().take(4096).collect::<String>(),
            "payload": String::from_utf8_lossy(payload),
        });
        self.context
            .publish(
                config.dead_letter_subject.clone(),
                Bytes::from(serde_json::to_vec(&body)?),
            )
            .await
            .map_err(|error| JetStreamError::Publish(error.to_string()))?
            .await
            .map_err(|error| JetStreamError::Publish(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl EventTransport for JetStreamEventTransport {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<(), TransportError> {
        self.publish_event(envelope)
            .await
            .map_err(|error| TransportError(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct DurableConsumerConfig {
    pub durable_name: String,
    pub filter_subject: Option<String>,
    pub dead_letter_subject: String,
    pub ack_wait: Duration,
    pub retry_delay: Duration,
    pub max_deliveries: u32,
}

impl DurableConsumerConfig {
    fn validate(&self) -> Result<(), JetStreamError> {
        if self.durable_name.trim().is_empty() || self.dead_letter_subject.trim().is_empty() {
            return Err(JetStreamError::Configuration(
                "durable consumer and dead-letter subject are required".into(),
            ));
        }
        if self.ack_wait.is_zero()
            || self.retry_delay.is_zero()
            || self.max_deliveries == 0
            || self.max_deliveries > 100
        {
            return Err(JetStreamError::Configuration(
                "consumer timing and delivery limits are invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxClaim {
    Acquired { attempts: u32 },
    Completed,
    Busy,
}

#[async_trait]
pub trait EventInbox: Send + Sync {
    async fn claim(
        &self,
        consumer: &str,
        event: &EventEnvelope,
    ) -> Result<InboxClaim, JetStreamError>;
    async fn complete(&self, consumer: &str, event_id: Uuid) -> Result<(), JetStreamError>;
    async fn fail(
        &self,
        consumer: &str,
        event_id: Uuid,
        sanitized_error: &str,
    ) -> Result<u32, JetStreamError>;
}

#[async_trait]
pub trait EventHandler: Send + Sync {
    async fn handle(&self, event: &EventEnvelope) -> Result<(), String>;
}

#[derive(Debug, Error)]
pub enum JetStreamError {
    #[error("JetStream configuration error: {0}")]
    Configuration(String),
    #[error("NATS connection error: {0}")]
    Connection(String),
    #[error("JetStream API error: {0}")]
    Api(String),
    #[error("JetStream publish error: {0}")]
    Publish(String),
    #[error("JetStream consumer error: {0}")]
    Consumer(String),
    #[error("unsupported event schema {actual} for event {event_id}; supported through {supported}")]
    UnsupportedSchema {
        event_id: Uuid,
        actual: u32,
        supported: u32,
    },
    #[error("JetStream serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("JetStream inbox error: {0}")]
    Inbox(String),
}

fn normalize_subject_token(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '.'
            }
        })
        .collect()
}
