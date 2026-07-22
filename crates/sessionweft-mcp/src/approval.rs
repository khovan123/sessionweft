use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sessionweft_core::SessionId;
use sessionweft_execution::{
    AgentRecord, ApprovalGrant, McpTransport, PolicyEffect, PolicyEngine, ToolDescriptor, ToolError,
    ToolInvocation, ToolResult,
};
use thiserror::Error;
use uuid::Uuid;

pub const MCP_APPROVAL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpApprovalRecord {
    pub schema_version: u32,
    pub grant: ApprovalGrant,
    pub issued_at: DateTime<Utc>,
    pub issued_by: Option<String>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub consumed_by_invocation: Option<Uuid>,
}

impl McpApprovalRecord {
    pub fn new(
        grant: ApprovalGrant,
        issued_at: DateTime<Utc>,
        issued_by: Option<String>,
    ) -> Result<Self, McpApprovalError> {
        if grant.tool_name.trim().is_empty() {
            return Err(McpApprovalError::Validation(
                "approval tool name cannot be empty".into(),
            ));
        }
        if grant.expires_at <= issued_at {
            return Err(McpApprovalError::Validation(
                "approval must expire after it is issued".into(),
            ));
        }
        Ok(Self {
            schema_version: MCP_APPROVAL_SCHEMA_VERSION,
            grant,
            issued_at,
            issued_by,
            consumed_at: None,
            consumed_by_invocation: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueApprovalCommand {
    pub grant: ApprovalGrant,
    pub issued_at: DateTime<Utc>,
    pub actor_id: Option<String>,
    pub correlation_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeApprovalCommand {
    pub grant_id: Uuid,
    pub session_id: SessionId,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub invocation_correlation_id: Uuid,
    pub consumed_at: DateTime<Utc>,
    pub actor_id: Option<String>,
    pub correlation_id: Uuid,
}

#[async_trait]
pub trait McpApprovalRepository: Send + Sync {
    async fn issue(
        &self,
        command: &IssueApprovalCommand,
    ) -> Result<McpApprovalRecord, McpApprovalRepositoryError>;

    async fn consume(
        &self,
        command: &ConsumeApprovalCommand,
    ) -> Result<McpApprovalRecord, McpApprovalRepositoryError>;

    async fn get(
        &self,
        grant_id: Uuid,
    ) -> Result<Option<McpApprovalRecord>, McpApprovalRepositoryError>;
}

pub struct AuditedMcpGateway<T, R>
where
    T: McpTransport,
    R: McpApprovalRepository,
{
    transport: Arc<T>,
    policy: PolicyEngine,
    approvals: Arc<R>,
}

impl<T, R> AuditedMcpGateway<T, R>
where
    T: McpTransport,
    R: McpApprovalRepository,
{
    #[must_use]
    pub fn new(transport: Arc<T>, policy: PolicyEngine, approvals: Arc<R>) -> Self {
        Self {
            transport,
            policy,
            approvals,
        }
    }

    pub async fn discover(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        let tools = self.transport.list_tools().await?;
        for tool in &tools {
            tool.validate()
                .map_err(|error| ToolError::InvalidDescriptor(error.to_string()))?;
        }
        Ok(tools)
    }

    pub async fn invoke(
        &self,
        agent: &AgentRecord,
        invocation: &ToolInvocation,
        approval: Option<&ApprovalGrant>,
        actor_id: Option<&str>,
    ) -> Result<ToolResult, ToolError> {
        if invocation.session_id != agent.session_id || invocation.agent_id != agent.id {
            return Err(ToolError::Denied("agent/session scope mismatch".into()));
        }
        let descriptor = self
            .discover()
            .await?
            .into_iter()
            .find(|tool| tool.name == invocation.tool_name)
            .ok_or_else(|| ToolError::NotFound(invocation.tool_name.clone()))?;
        let decision = self.policy.evaluate(agent, &descriptor);
        match decision.effect {
            PolicyEffect::Allow => self.transport.call_tool(invocation).await,
            PolicyEffect::Deny => Err(ToolError::Denied(decision.reason)),
            PolicyEffect::RequireApproval => {
                let grant = approval
                    .filter(|grant| {
                        grant.authorizes(
                            invocation.session_id,
                            invocation.agent_id,
                            &invocation.tool_name,
                            Utc::now(),
                        )
                    })
                    .ok_or_else(|| ToolError::ApprovalRequired(decision.reason.clone()))?;
                let command = ConsumeApprovalCommand {
                    grant_id: grant.id,
                    session_id: invocation.session_id,
                    agent_id: invocation.agent_id,
                    tool_name: invocation.tool_name.clone(),
                    invocation_correlation_id: invocation.correlation_id,
                    consumed_at: Utc::now(),
                    actor_id: actor_id.map(ToOwned::to_owned),
                    correlation_id: invocation.correlation_id,
                };
                self.approvals
                    .consume(&command)
                    .await
                    .map_err(map_repository_error)?;
                self.transport.call_tool(invocation).await
            }
        }
    }
}

fn map_repository_error(error: McpApprovalRepositoryError) -> ToolError {
    match error {
        McpApprovalRepositoryError::NotFound(_) => {
            ToolError::ApprovalRequired("approval grant was not persisted".into())
        }
        McpApprovalRepositoryError::Expired(_) => {
            ToolError::ApprovalRequired("approval grant expired".into())
        }
        McpApprovalRepositoryError::AlreadyConsumed(_) => {
            ToolError::Denied("approval grant was already consumed".into())
        }
        McpApprovalRepositoryError::ScopeMismatch(_) => {
            ToolError::Denied("approval grant scope mismatch".into())
        }
        McpApprovalRepositoryError::Conflict(message)
        | McpApprovalRepositoryError::Backend(message) => ToolError::Execution(message),
    }
}

#[derive(Debug, Error)]
pub enum McpApprovalError {
    #[error("invalid MCP approval: {0}")]
    Validation(String),
}

#[derive(Debug, Error)]
pub enum McpApprovalRepositoryError {
    #[error("MCP approval {0} was not found")]
    NotFound(Uuid),
    #[error("MCP approval {0} expired")]
    Expired(Uuid),
    #[error("MCP approval {0} was already consumed")]
    AlreadyConsumed(Uuid),
    #[error("MCP approval {0} scope mismatch")]
    ScopeMismatch(Uuid),
    #[error("MCP approval repository conflict: {0}")]
    Conflict(String),
    #[error("MCP approval repository backend error: {0}")]
    Backend(String),
}
