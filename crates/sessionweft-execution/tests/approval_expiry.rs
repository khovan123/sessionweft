use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::json;
use sessionweft_core::SessionId;
use sessionweft_execution::{
    AgentManifest, AgentRecord, AgentRole, ApprovalGrant, Capability, Permission, PolicyConfig,
    PolicyEngine, RiskLevel, ToolDescriptor, ToolError, ToolGateway, ToolHandler, ToolInvocation,
    ToolRegistry, ToolResult,
};
use uuid::Uuid;

struct CountingTool {
    descriptor: ToolDescriptor,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for CountingTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: json!({"ok": true}),
            metadata: BTreeMap::new(),
        })
    }
}

#[tokio::test]
async fn expired_approval_does_not_authorize_high_risk_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let descriptor = ToolDescriptor {
        name: "danger".into(),
        version: "1".into(),
        permissions: BTreeSet::from([Permission::Tool("danger".into())]),
        risk: RiskLevel::High,
        input_schema: json!({"type": "object"}),
    };
    let mut registry = ToolRegistry::default();
    registry
        .register(CountingTool {
            descriptor,
            calls: Arc::clone(&calls),
        })
        .expect("register");
    let gateway = ToolGateway::new(
        Arc::new(registry),
        PolicyEngine::new(PolicyConfig {
            allowed: BTreeSet::from([Permission::Tool("danger".into())]),
            approval_required: BTreeSet::new(),
            denied: BTreeSet::new(),
        }),
    );
    let agent = AgentRecord::new(
        SessionId::new(),
        AgentManifest {
            name: "approval-test".into(),
            role: AgentRole::Worker,
            capabilities: BTreeSet::from([Capability::Tool("danger".into())]),
            heartbeat_timeout_seconds: 30,
        },
    )
    .expect("agent");
    let invocation = ToolInvocation {
        session_id: agent.session_id,
        task_id: Some("task-approval".into()),
        agent_id: agent.id,
        tool_name: "danger".into(),
        arguments: json!({}),
        correlation_id: Uuid::new_v4(),
    };
    let expired = ApprovalGrant {
        id: Uuid::new_v4(),
        session_id: agent.session_id,
        agent_id: agent.id,
        tool_name: "danger".into(),
        expires_at: Utc::now() - Duration::seconds(1),
    };

    let error = gateway
        .invoke(&agent, &invocation, Some(&expired))
        .await
        .expect_err("expired approval must be rejected");
    assert!(matches!(error, ToolError::ApprovalRequired(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
