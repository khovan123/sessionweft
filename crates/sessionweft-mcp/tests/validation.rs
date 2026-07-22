use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use sessionweft_execution::RiskLevel;
use sessionweft_mcp::{
    CompatibilityPolicy, HttpTransportConfig, McpAdapterError, OfficialMcpConfig,
    OfficialTransportConfig, StdioTransportConfig,
};

fn stdio_config(root: &Path) -> OfficialMcpConfig {
    OfficialMcpConfig {
        server_id: "test_server".into(),
        transport: OfficialTransportConfig::Stdio(StdioTransportConfig {
            program: std::env::current_exe().expect("current executable"),
            args: Vec::new(),
            cwd: root.to_owned(),
            workspace_root: root.to_owned(),
            environment: BTreeMap::new(),
            allowed_environment: BTreeSet::new(),
        }),
        compatibility: CompatibilityPolicy::default(),
        operation_timeout: Duration::from_secs(1),
        max_result_bytes: 1024,
        calls_per_minute: 1,
        default_risk: RiskLevel::Medium,
        risk_overrides: BTreeMap::new(),
        declared_permissions: BTreeSet::new(),
    }
}

#[test]
fn rejects_workspace_escape_and_secret_environment() {
    let root = std::env::temp_dir();
    let child = root.join(format!("sessionweft-mcp-validation-{}", std::process::id()));
    fs::create_dir_all(&child).expect("test directory");

    let mut config = stdio_config(&child);
    {
        let OfficialTransportConfig::Stdio(stdio) = &mut config.transport else {
            panic!("stdio config")
        };
        stdio.cwd = root.clone();
    }
    assert!(matches!(
        config.validate(),
        Err(McpAdapterError::WorkspaceEscape(_))
    ));

    {
        let OfficialTransportConfig::Stdio(stdio) = &mut config.transport else {
            panic!("stdio config")
        };
        stdio.cwd = child.clone();
        stdio
            .environment
            .insert("SECRET_TOKEN".into(), "hidden".into());
    }
    assert!(matches!(
        config.validate(),
        Err(McpAdapterError::InvalidConfig(_))
    ));
    fs::remove_dir_all(child).expect("cleanup");
}

#[test]
fn validates_http_scheme_host_and_credentials() {
    let config = OfficialMcpConfig {
        server_id: "remote".into(),
        transport: OfficialTransportConfig::StreamableHttp(HttpTransportConfig {
            endpoint: "http://example.com/mcp".into(),
            allowed_hosts: BTreeSet::from(["example.com".into()]),
            allow_plaintext_loopback: false,
        }),
        compatibility: CompatibilityPolicy::default(),
        operation_timeout: Duration::from_secs(1),
        max_result_bytes: 1024,
        calls_per_minute: 1,
        default_risk: RiskLevel::Low,
        risk_overrides: BTreeMap::new(),
        declared_permissions: BTreeSet::new(),
    };
    assert!(matches!(
        config.validate(),
        Err(McpAdapterError::InvalidConfig(_))
    ));

    let mut config = config;
    config.transport = OfficialTransportConfig::StreamableHttp(HttpTransportConfig {
        endpoint: "https://user:secret@example.com/mcp".into(),
        allowed_hosts: BTreeSet::from(["example.com".into()]),
        allow_plaintext_loopback: false,
    });
    assert!(matches!(
        config.validate(),
        Err(McpAdapterError::InvalidConfig(_))
    ));
}

#[test]
fn validates_server_identifier_and_limits() {
    let root = std::env::temp_dir();
    let child = root.join(format!("sessionweft-mcp-limits-{}", std::process::id()));
    fs::create_dir_all(&child).expect("test directory");
    let mut config = stdio_config(&child);
    config.server_id = "invalid server".into();
    assert!(config.validate().is_err());
    config.server_id = "valid_server".into();
    config.calls_per_minute = 0;
    assert!(config.validate().is_err());
    config.calls_per_minute = 1;
    config.max_result_bytes = 0;
    assert!(config.validate().is_err());
    fs::remove_dir_all(child).expect("cleanup");
}

#[allow(dead_code)]
fn assert_path_is_owned(_: PathBuf) {}
