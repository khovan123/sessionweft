use std::sync::Arc;

use async_trait::async_trait;
use sessionweft_core::EventEnvelope;
use sessionweft_runtime::{EventTransport, TransportError};
use thiserror::Error;

use crate::{ClientEventRecord, EventBatch, EventCursor, MAX_EVENT_BATCH_LIMIT};

#[async_trait]
pub trait EventJournal: Send + Sync {
    async fn append(
        &self,
        envelope: &EventEnvelope,
    ) -> Result<ClientEventRecord, EventJournalError>;

    async fn list_after(
        &self,
        after: EventCursor,
        limit: u32,
    ) -> Result<EventBatch, EventJournalError>;

    async fn latest_cursor(&self) -> Result<EventCursor, EventJournalError>;
}

#[derive(Clone)]
pub struct JournalEventTransport<J, T>
where
    J: EventJournal,
    T: EventTransport,
{
    journal: Arc<J>,
    downstream: Arc<T>,
}

impl<J, T> JournalEventTransport<J, T>
where
    J: EventJournal,
    T: EventTransport,
{
    #[must_use]
    pub fn new(journal: Arc<J>, downstream: Arc<T>) -> Self {
        Self {
            journal,
            downstream,
        }
    }

    #[must_use]
    pub fn journal(&self) -> &Arc<J> {
        &self.journal
    }
}

#[async_trait]
impl<J, T> EventTransport for JournalEventTransport<J, T>
where
    J: EventJournal,
    T: EventTransport,
{
    async fn publish(&self, envelope: &EventEnvelope) -> Result<(), TransportError> {
        self.journal
            .append(envelope)
            .await
            .map_err(|error| TransportError(error.to_string()))?;
        self.downstream.publish(envelope).await
    }
}

pub fn validate_event_limit(limit: u32) -> Result<u32, EventJournalError> {
    if limit == 0 || limit > MAX_EVENT_BATCH_LIMIT {
        return Err(EventJournalError::Validation(format!(
            "event limit must be between 1 and {MAX_EVENT_BATCH_LIMIT}"
        )));
    }
    Ok(limit)
}

#[derive(Debug, Error)]
pub enum EventJournalError {
    #[error("event journal validation failed: {0}")]
    Validation(String),
    #[error("event journal backend error: {0}")]
    Backend(String),
    #[error("event journal serialization error: {0}")]
    Serialization(String),
}
