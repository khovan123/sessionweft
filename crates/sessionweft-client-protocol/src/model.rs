use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sessionweft_core::{EventEnvelope, SessionId};
use uuid::Uuid;

pub const CLIENT_PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_EVENT_BATCH_LIMIT: u32 = 100;
pub const MAX_EVENT_BATCH_LIMIT: u32 = 1_000;
pub const DEFAULT_PTY_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
pub const MAX_PTY_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiEnvelope<T> {
    pub protocol_version: u32,
    pub correlation_id: Uuid,
    pub data: T,
}

impl<T> ApiEnvelope<T> {
    #[must_use]
    pub fn new(correlation_id: Uuid, data: T) -> Self {
        Self {
            protocol_version: CLIENT_PROTOCOL_VERSION,
            correlation_id,
            data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub protocol_version: u32,
    pub correlation_id: Uuid,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub committed_version: Option<u64>,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(
        correlation_id: Uuid,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        committed_version: Option<u64>,
    ) -> Self {
        Self {
            protocol_version: CLIENT_PROTOCOL_VERSION,
            correlation_id,
            code: code.into(),
            message: message.into(),
            retryable,
            committed_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCapabilities {
    pub protocol_version: u32,
    pub minimum_client_version: u32,
    pub authenticated: bool,
    pub event_cursor_resume: bool,
    pub pty_streaming: bool,
    pub approvals: bool,
    pub resource_views: bool,
    pub max_event_batch: u32,
    pub max_pty_output_bytes: usize,
}

impl Default for ProtocolCapabilities {
    fn default() -> Self {
        Self {
            protocol_version: CLIENT_PROTOCOL_VERSION,
            minimum_client_version: CLIENT_PROTOCOL_VERSION,
            authenticated: true,
            event_cursor_resume: true,
            pty_streaming: true,
            approvals: true,
            resource_views: true,
            max_event_batch: MAX_EVENT_BATCH_LIMIT,
            max_pty_output_bytes: MAX_PTY_OUTPUT_LIMIT,
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct EventCursor(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientEventRecord {
    pub cursor: EventCursor,
    pub envelope: EventEnvelope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    pub protocol_version: u32,
    pub after: EventCursor,
    pub next: EventCursor,
    pub latest: EventCursor,
    pub events: Vec<ClientEventRecord>,
    pub has_more: bool,
}

impl EventBatch {
    #[must_use]
    pub fn empty(after: EventCursor, latest: EventCursor) -> Self {
        Self {
            protocol_version: CLIENT_PROTOCOL_VERSION,
            after,
            next: after,
            latest,
            events: Vec::new(),
            has_more: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientResourceView {
    pub protocol_version: u32,
    pub session_id: SessionId,
    pub session: Value,
    pub agent: Option<Value>,
    pub workflow: Option<Value>,
    pub locks: Vec<Value>,
    pub pending_approvals: Vec<PendingApproval>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub workflow_id: Uuid,
    pub node_id: String,
    pub expected_version: u64,
    pub title: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPtyRequest {
    pub session_id: SessionId,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    pub rows: u16,
    pub cols: u16,
    #[serde(default = "default_pty_output_limit")]
    pub max_output_bytes: usize,
}

const fn default_pty_output_limit() -> usize {
    DEFAULT_PTY_OUTPUT_LIMIT
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySessionDescriptor {
    pub protocol_version: u32,
    pub pty_id: Uuid,
    pub session_id: SessionId,
    pub status: PtyStatus,
    pub rows: u16,
    pub cols: u16,
    pub output_cursor: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyStatus {
    Running,
    Exited,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyInputRequest {
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyResizeRequest {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOutputChunk {
    pub cursor: u64,
    pub data: String,
    pub truncated: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOutputBatch {
    pub protocol_version: u32,
    pub pty_id: Uuid,
    pub after: u64,
    pub next: u64,
    pub status: PtyStatus,
    pub chunks: Vec<PtyOutputChunk>,
    pub total_retained_bytes: usize,
}
