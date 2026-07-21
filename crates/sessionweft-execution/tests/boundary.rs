use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use sessionweft_core::SessionId;
use sessionweft_execution::{
    AgentManifest, AgentRecord, AgentRole, ApprovalGrant, Capability, FenceValidator, GitCli,
    GitError, GitFence, McpGateway, McpTransport, Permission, PolicyConfig, PolicyEngine,
    ProcessError, ProcessSpec, RestrictedProcessRunner, RiskLevel, ToolDescriptor, ToolError,
    ToolGateway, ToolHandler, ToolInvocation, ToolRegistry, ToolResult, find_executable,
};
use sessionweft_orchestration::LockResource;
use uuid::Uuid;

fn agent(capabilities: BTreeSet<Capability>) -> AgentRecord {
    AgentRecord::new(
        SessionId::new(),
        AgentManifest {
            name: "boundary-test".into(),
            role: AgentRole::Worker,
            capabilities,
            heartbeat_timeout_seconds: 30,
        },
    )
    .expect("agent")
}

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

#[test]
fn descriptor_requires_self_named_tool_permission() {
    let descriptor = ToolDescriptor {
        name: "danger".into(),
        version: "1".into(),
        permissions: BTreeSet::new(),
        risk: RiskLevel::Low,
        input_schema: json!({"type": "object"}),
    };
    assert!(descriptor.validate().is_err());
}

#[tokio::test]
async fn high_risk_tool_requires_scoped_unexpired_approval() {
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
    let agent = agent(BTreeSet::from([Capability::Tool("danger".into())]));
    let invocation = ToolInvocation {
        session_id: agent.session_id,
        task_id: Some("task-1".into()),
        agent_id: agent.id,
        tool_name: "danger".into(),
        arguments: json!({}),
        correlation_id: Uuid::new_v4(),
    };

    let denied = gateway
        .invoke(&agent, &invocation, None)
        .await
        .expect_err("approval required");
    assert!(matches!(denied, ToolError::ApprovalRequired(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let approval = ApprovalGrant {
        id: Uuid::new_v4(),
        session_id: agent.session_id,
        agent_id: agent.id,
        tool_name: "danger".into(),
        expires_at: Utc::now() + ChronoDuration::minutes(1),
    };
    gateway
        .invoke(&agent, &invocation, Some(&approval))
        .await
        .expect("approved invocation");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct CountingMcp {
    descriptor: ToolDescriptor,
    calls: AtomicUsize,
}

#[async_trait]
impl McpTransport for CountingMcp {
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn call_tool(&self, _invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: json!({"ok": true}),
            metadata: BTreeMap::new(),
        })
    }
}

#[tokio::test]
async fn mcp_scope_mismatch_is_denied_before_transport_call() {
    let transport = Arc::new(CountingMcp {
        descriptor: ToolDescriptor {
            name: "remote.echo".into(),
            version: "1".into(),
            permissions: BTreeSet::from([Permission::Tool("remote.echo".into())]),
            risk: RiskLevel::Low,
            input_schema: json!({"type": "object"}),
        },
        calls: AtomicUsize::new(0),
    });
    let gateway = McpGateway::new(
        Arc::clone(&transport),
        PolicyEngine::new(PolicyConfig {
            allowed: BTreeSet::from([Permission::Tool("remote.echo".into())]),
            approval_required: BTreeSet::new(),
            denied: BTreeSet::new(),
        }),
    );
    let agent = agent(BTreeSet::from([Capability::Tool("remote.echo".into())]));
    let error = gateway
        .invoke(
            &agent,
            &ToolInvocation {
                session_id: SessionId::new(),
                task_id: None,
                agent_id: agent.id,
                tool_name: "remote.echo".into(),
                arguments: json!({}),
                correlation_id: Uuid::new_v4(),
            },
            None,
        )
        .await
        .expect_err("scope mismatch");
    assert!(matches!(error, ToolError::Denied(_)));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

fn process_runner(root: &PathBuf) -> Option<RestrictedProcessRunner> {
    let executable = find_executable("git")?;
    RestrictedProcessRunner::new(
        root,
        BTreeMap::from([("git".into(), executable)]),
        BTreeSet::new(),
    )
    .ok()
}

#[tokio::test]
async fn process_runner_rejects_workspace_escape_and_environment_leak() {
    let root = env::temp_dir().join(format!("sessionweft-runner-root-{}", Uuid::new_v4()));
    let outside = env::temp_dir().join(format!("sessionweft-runner-outside-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&outside).expect("outside");
    let Some(runner) = process_runner(&root) else {
        fs::remove_dir_all(root).expect("cleanup root");
        fs::remove_dir_all(outside).expect("cleanup outside");
        return;
    };

    let escaped = runner
        .run(&ProcessSpec {
            program: "git".into(),
            args: vec!["--version".into()],
            cwd: outside.clone(),
            env: BTreeMap::new(),
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        })
        .await
        .expect_err("workspace escape");
    assert!(matches!(escaped, ProcessError::WorkspaceEscape(_)));

    let environment = runner
        .run(&ProcessSpec {
            program: "git".into(),
            args: vec!["--version".into()],
            cwd: root.clone(),
            env: BTreeMap::from([("SECRET".into(), "should-not-leak".into())]),
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        })
        .await
        .expect_err("environment denied");
    assert!(matches!(environment, ProcessError::EnvironmentDenied(_)));

    fs::remove_dir_all(root).expect("cleanup root");
    fs::remove_dir_all(outside).expect("cleanup outside");
}

struct RejectFence;

#[async_trait]
impl FenceValidator for RejectFence {
    async fn validate(&self, _fence: &GitFence) -> Result<(), GitError> {
        Err(GitError::FenceRejected("stale fence".into()))
    }
}

#[tokio::test]
async fn git_mutation_is_blocked_before_command_when_fence_is_stale() {
    let Some(git) = find_executable("git") else {
        return;
    };
    let root = env::temp_dir().join(format!("sessionweft-fence-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).expect("root");
    let status = std::process::Command::new(&git)
        .args(["init", "--quiet"])
        .current_dir(&root)
        .status()
        .expect("git init");
    assert!(status.success());
    let runner = RestrictedProcessRunner::new(
        &root,
        BTreeMap::from([("git".into(), git)]),
        BTreeSet::new(),
    )
    .expect("runner");
    let client = GitCli::new(runner, Arc::new(RejectFence));
    let error = client
        .commit_staged(
            "blocked",
            &GitFence {
                owner_id: "worker".into(),
                fencing_token: 1,
                resource: LockResource::Workspace {
                    workspace_id: "workspace".into(),
                },
            },
        )
        .await
        .expect_err("fence rejected");
    assert!(matches!(error, GitError::FenceRejected(_)));
    fs::remove_dir_all(root).expect("cleanup");
}
