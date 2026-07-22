use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sessionweft_core::SessionId;
use sessionweft_execution::{McpTransport, RiskLevel, ToolError, ToolInvocation};
use sessionweft_mcp::{
    BubblewrapProfile, CompatibilityPolicy, NetworkIsolation, OfficialMcpConfig,
    OfficialMcpTransport, OfficialTransportConfig, StdioTransportConfig, WorkspaceAccess,
    build_bubblewrap_arguments,
};
use uuid::Uuid;

fn fixture_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcp-malicious-fixture"))
}

fn fixture_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sessionweft-mcp-{name}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).expect("fixture root");
    root
}

fn config(
    root: &Path,
    mode: &str,
    timeout: Duration,
    max_result_bytes: usize,
) -> OfficialMcpConfig {
    OfficialMcpConfig {
        server_id: "fixture".into(),
        transport: OfficialTransportConfig::Stdio(StdioTransportConfig {
            program: fixture_binary(),
            args: vec![mode.into()],
            cwd: root.to_owned(),
            workspace_root: root.to_owned(),
            environment: BTreeMap::new(),
            allowed_environment: BTreeSet::new(),
        }),
        compatibility: CompatibilityPolicy {
            expected_server_name: Some("sessionweft-fixture".into()),
            expected_server_version: Some("1.0.0".into()),
            allowed_protocol_versions: BTreeSet::from(["2025-11-25".into()]),
        },
        operation_timeout: timeout,
        max_result_bytes,
        calls_per_minute: 100,
        default_risk: RiskLevel::Low,
        risk_overrides: BTreeMap::new(),
        declared_permissions: BTreeSet::new(),
    }
}

#[tokio::test]
async fn discovers_and_calls_official_stdio_fixture() {
    let root = fixture_root("normal");
    let transport =
        OfficialMcpTransport::new(config(&root, "normal", Duration::from_secs(2), 1024))
            .expect("transport");
    let tools = transport.list_tools().await.expect("discover");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "fixture.probe");
    let result = transport
        .call_tool(&ToolInvocation {
            session_id: SessionId::new(),
            task_id: None,
            agent_id: Uuid::new_v4(),
            tool_name: "fixture.probe".into(),
            arguments: serde_json::json!({}),
            correlation_id: Uuid::new_v4(),
        })
        .await
        .expect("call");
    assert!(result.content.to_string().contains("ok"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn rejects_duplicate_tools_and_schema_spoofing() {
    for (mode, expected_collision) in [("duplicate", true), ("spoof", false)] {
        let root = fixture_root(mode);
        let transport =
            OfficialMcpTransport::new(config(&root, mode, Duration::from_secs(2), 1024))
                .expect("transport");
        let error = transport
            .list_tools()
            .await
            .expect_err("malicious discovery");
        if expected_collision {
            assert!(matches!(error, ToolError::InvalidDescriptor(_)));
        } else {
            assert!(matches!(error, ToolError::Execution(_)));
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}

#[tokio::test]
async fn bounds_hanging_and_flooding_plugins() {
    let root = fixture_root("hang");
    let transport =
        OfficialMcpTransport::new(config(&root, "hang", Duration::from_millis(150), 1024))
            .expect("transport");
    let started = Instant::now();
    assert!(transport.list_tools().await.is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    fs::remove_dir_all(root).expect("cleanup");

    let root = fixture_root("flood");
    let transport = OfficialMcpTransport::new(config(&root, "flood", Duration::from_secs(2), 1024))
        .expect("transport");
    let error = transport
        .call_tool(&ToolInvocation {
            session_id: SessionId::new(),
            task_id: None,
            agent_id: Uuid::new_v4(),
            tool_name: "fixture.probe".into(),
            arguments: serde_json::json!({}),
            correlation_id: Uuid::new_v4(),
        })
        .await
        .expect_err("flood rejected");
    assert!(matches!(error, ToolError::Execution(_)));
    fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn child_environment_does_not_inherit_secrets() {
    let root = fixture_root("secret");
    let transport =
        OfficialMcpTransport::new(config(&root, "secret", Duration::from_secs(2), 1024))
            .expect("transport");
    let result = transport
        .call_tool(&ToolInvocation {
            session_id: SessionId::new(),
            task_id: None,
            agent_id: Uuid::new_v4(),
            tool_name: "fixture.probe".into(),
            arguments: serde_json::json!({}),
            correlation_id: Uuid::new_v4(),
        })
        .await
        .expect("secret probe");
    assert!(result.content.to_string().contains("absent"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn bubblewrap_profile_denies_network_and_limits_filesystem() {
    let root = fixture_root("sandbox");
    let launcher = root.join("bwrap");
    fs::copy(fixture_binary(), &launcher).expect("fake launcher");
    let plugin = StdioTransportConfig {
        program: fixture_binary(),
        args: vec!["normal".into()],
        cwd: root.clone(),
        workspace_root: root.clone(),
        environment: BTreeMap::from([("SAFE_FLAG".into(), "1".into())]),
        allowed_environment: BTreeSet::from(["SAFE_FLAG".into()]),
    };
    let arguments = build_bubblewrap_arguments(
        &plugin,
        &BubblewrapProfile {
            launcher,
            runtime_roots: Vec::new(),
            workspace_access: WorkspaceAccess::ReadOnly,
            network: NetworkIsolation::Denied,
        },
    )
    .expect("sandbox arguments");
    assert!(arguments.iter().any(|value| value == "--unshare-all"));
    assert!(!arguments.iter().any(|value| value == "--share-net"));
    assert!(arguments.iter().any(|value| value == "--ro-bind"));
    assert!(arguments.iter().any(|value| value == "--clearenv"));
    assert!(
        arguments
            .windows(2)
            .any(|values| { values[0] == "--setenv" && values[1] == "SAFE_FLAG" })
    );
    fs::remove_dir_all(root).expect("cleanup");
}
