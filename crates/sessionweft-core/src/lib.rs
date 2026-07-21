use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

pub const SESSION_SCHEMA_VERSION: u32 = 1;
pub const EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for MessageRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSelection {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub id: SessionId,
    pub version: u64,
    pub status: SessionStatus,
    pub title: String,
    pub messages: Vec<Message>,
    pub provider: Option<ProviderSelection>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Session {
    pub fn new(title: impl Into<String>) -> Result<Self, DomainError> {
        let title = title.into().trim().to_owned();
        if title.is_empty() {
            return Err(DomainError::Validation("title cannot be empty".into()));
        }
        if title.len() > 200 {
            return Err(DomainError::Validation(
                "title cannot exceed 200 bytes".into(),
            ));
        }

        let now = Utc::now();
        Ok(Self {
            schema_version: SESSION_SCHEMA_VERSION,
            id: SessionId::new(),
            version: 0,
            status: SessionStatus::Active,
            title,
            messages: Vec::new(),
            provider: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn append_message(
        &mut self,
        expected_version: u64,
        role: MessageRole,
        content: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, DomainError> {
        self.ensure_mutable(expected_version)?;
        let content = content.into();
        if content.trim().is_empty() {
            return Err(DomainError::Validation(
                "message content cannot be empty".into(),
            ));
        }
        if content.len() > 1_000_000 {
            return Err(DomainError::Validation(
                "message content exceeds the configured hard limit".into(),
            ));
        }

        let message = Message {
            id: Uuid::new_v4(),
            role,
            content,
            created_at: Utc::now(),
        };
        self.messages.push(message.clone());
        self.advance_version();

        Ok(EventEnvelope::new(
            "session.message_appended",
            Some(self.id),
            correlation_id,
            actor_id,
            json!({
                "session_version": self.version,
                "message_id": message.id,
                "role": message.role,
            }),
        ))
    }

    pub fn select_provider(
        &mut self,
        expected_version: u64,
        provider: impl Into<String>,
        model: impl Into<String>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, DomainError> {
        self.ensure_mutable(expected_version)?;
        let provider = provider.into().trim().to_owned();
        let model = model.into().trim().to_owned();
        if provider.is_empty() || model.is_empty() {
            return Err(DomainError::Validation(
                "provider and model are required".into(),
            ));
        }

        let previous = self.provider.clone();
        self.provider = Some(ProviderSelection {
            provider: provider.clone(),
            model: model.clone(),
        });
        self.advance_version();

        Ok(EventEnvelope::new(
            "provider.switched",
            Some(self.id),
            correlation_id,
            actor_id,
            json!({
                "session_version": self.version,
                "previous": previous,
                "current": {"provider": provider, "model": model},
            }),
        ))
    }

    pub fn archive(
        &mut self,
        expected_version: u64,
        correlation_id: Uuid,
        actor_id: Option<&str>,
    ) -> Result<EventEnvelope, DomainError> {
        self.ensure_version(expected_version)?;
        if self.status == SessionStatus::Archived {
            return Err(DomainError::Archived);
        }
        self.status = SessionStatus::Archived;
        self.advance_version();

        Ok(EventEnvelope::new(
            "session.archived",
            Some(self.id),
            correlation_id,
            actor_id,
            json!({"session_version": self.version}),
        ))
    }

    pub fn ensure_version(&self, expected_version: u64) -> Result<(), DomainError> {
        if self.version != expected_version {
            return Err(DomainError::Conflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        Ok(())
    }

    fn ensure_mutable(&self, expected_version: u64) -> Result<(), DomainError> {
        self.ensure_version(expected_version)?;
        if self.status == SessionStatus::Archived {
            return Err(DomainError::Archived);
        }
        Ok(())
    }

    fn advance_version(&mut self) {
        self.version = self.version.saturating_add(1);
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: Uuid,
    pub event_type: String,
    pub schema_version: u32,
    pub session_id: Option<SessionId>,
    pub actor_id: Option<String>,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

impl EventEnvelope {
    #[must_use]
    pub fn new(
        event_type: impl Into<String>,
        session_id: Option<SessionId>,
        correlation_id: Uuid,
        actor_id: Option<&str>,
        payload: Value,
    ) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            event_type: event_type.into(),
            schema_version: EVENT_SCHEMA_VERSION,
            session_id,
            actor_id: actor_id.map(ToOwned::to_owned),
            correlation_id,
            causation_id: None,
            occurred_at: Utc::now(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub session_id: SessionId,
    pub model: String,
    pub messages: Vec<ProviderMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: MessageRole,
    pub content: String,
}

impl From<&Message> for ProviderMessage {
    fn from(message: &Message) -> Self {
        Self {
            role: message.role,
            content: message.content.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub text: String,
    pub provider_request_id: Option<String>,
    pub usage: ProviderUsage,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("session version conflict: expected {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("session is archived")]
    Archived,
}

#[derive(Debug, Clone, Default)]
pub struct SecretRedactor {
    secrets: Vec<String>,
}

impl SecretRedactor {
    #[must_use]
    pub fn new<I, S>(secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            secrets: secrets
                .into_iter()
                .map(Into::into)
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    #[must_use]
    pub fn redact(&self, value: &str) -> String {
        self.secrets
            .iter()
            .fold(value.to_owned(), |output, secret| {
                output.replace(secret, "[REDACTED]")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_version_is_rejected() {
        let mut session = Session::new("test").expect("session");
        session
            .append_message(0, MessageRole::User, "hello", Uuid::new_v4(), None)
            .expect("append");

        let error = session
            .append_message(0, MessageRole::User, "stale", Uuid::new_v4(), None)
            .expect_err("conflict");
        assert_eq!(
            error,
            DomainError::Conflict {
                expected: 0,
                actual: 1,
            }
        );
    }

    #[test]
    fn provider_switch_keeps_session_identity() {
        let mut session = Session::new("test").expect("session");
        let id = session.id;
        session
            .select_provider(0, "echo", "v1", Uuid::new_v4(), Some("tester"))
            .expect("switch");
        assert_eq!(session.id, id);
        assert_eq!(session.version, 1);
    }

    #[test]
    fn redactor_removes_all_configured_secrets() {
        let redactor = SecretRedactor::new(["token-123", "password"]);
        assert_eq!(
            redactor.redact("token-123 and password"),
            "[REDACTED] and [REDACTED]"
        );
    }
}
