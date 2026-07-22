use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sessionweft_core::SessionId;
use sessionweft_execution::{
    AgentManifest, AgentRecord, AgentRole, ApprovalGrant, Capability, McpTransport, Permission,
    PolicyConfig, PolicyEngine, RiskLevel, ToolDescriptor, ToolError, ToolInvocation, ToolResult,
};
use sessionweft_mcp::{
    AuditedMcpGateway, IssueApprovalCommand, McpApprovalRepository,
};
use sessionweft_mcp_sqlite::SqliteMcpApprovalRepository;
use uuid::Uuid;

struct CountingTransport {
    calls: AtomicUsize,
    descriptor: ToolDescriptor,
}

#[async_trait]
impl McpTransport for CountingTransport {
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn call_tool(&self, _invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: serde_json::json!({"ok": true}),
            metadata: BTreeMap::new(),
        })
    }
}

#[tokio::test]
async fn consumes_approval_before_exactly_one_external_call() {
    let repository = Arc::new(
        SqliteMcpApprovalRepository::connect("sqlite::memory:")
            .await
            .expect("repository"),
    );
    let tool_name = "fixture.write".to_owned();
    let tool_permission = Permission::Tool(tool_name.clone());
    let transport = Arc::new(CountingTransport {
        calls: AtomicUsize::new(0),
        descriptor: ToolDescriptor {
            name: tool_name.clone(),
            version: "1.0.0".into(),
            permissions: BTreeSet::from([tool_permission.clone()]),
            risk: RiskLevel::High,
            input_schema: serde_json::json!({"type": "object"}),
        },
    });
    let gateway = AuditedMcpGateway::new(
        Arc::clone(&transport),
        PolicyEngine::new(PolicyConfig {
            allowed: BTreeSet::from([tool_permission]),
            approval_required: BTreeSet::new(),
            denied: BTreeSet::new(),
        }),
        Arc::clone(&repository),
    );
    let session_id = SessionId::new();
    let agent = AgentRecord::new(
        session_id,
        AgentManifest {
            name: "mcp-worker".into(),
            role: AgentRole::Worker,
            capabilities: BTreeSet::from([Capability::Tool(tool_name.clone())]),
            heartbeat_timeout_seconds: 30,
        },
    )
    .expect("agent");
    let now = Utc::now();
    let grant = ApprovalGrant {
        id: Uuid::new_v4(),
        session_id,
        agent_id: agent.id,
        tool_name: tool_name.clone(),
        expires_at: now + Duration::minutes(5),
    };
    repository
        .issue(&IssueApprovalCommand {
            grant: grant.clone(),
            issued_at: now,
            actor_id: Some("reviewer".into()),
            correlation_id: Uuid::new_v4(),
        })
        .await
        .expect("issue approval");
    let invocation = ToolInvocation {
        session_id,
        task_id: Some("task-1".into()),
        agent_id: agent.id,
        tool_name,
        arguments: serde_json::json!({}),
        correlation_id: Uuid::new_v4(),
    };

    gateway
        .invoke(&agent, &invocation, Some(&grant), Some("runtime"))
        .await
        .expect("first invocation");
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);

    let error = gateway
        .invoke(&agent, &invocation, Some(&grant), Some("runtime"))
        .await
        .expect_err("approval cannot be reused");
    assert!(matches!(error, ToolError::Denied(_)));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}
