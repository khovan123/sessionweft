use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use sessionweft_execution::{McpTransport, ToolDescriptor, ToolError, ToolInvocation, ToolResult};
use thiserror::Error;

use crate::{
    McpAdapterError, OfficialMcpConfig, OfficialMcpTransport, OfficialTransportConfig,
    StdioTransportConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkIsolation {
    Denied,
    Allowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BubblewrapProfile {
    pub launcher: PathBuf,
    pub runtime_roots: Vec<PathBuf>,
    pub workspace_access: WorkspaceAccess,
    pub network: NetworkIsolation,
}

pub struct SandboxedStdioTransport {
    inner: Arc<OfficialMcpTransport>,
    sandbox_arguments: Vec<String>,
}

impl SandboxedStdioTransport {
    pub fn new(
        mut config: OfficialMcpConfig,
        profile: BubblewrapProfile,
    ) -> Result<Self, McpIsolationError> {
        config.validate().map_err(McpIsolationError::Adapter)?;
        let OfficialTransportConfig::Stdio(plugin) = config.transport.clone() else {
            return Err(McpIsolationError::InvalidProfile(
                "bubblewrap isolation requires an stdio MCP transport".into(),
            ));
        };
        let launcher = canonical_file(&profile.launcher, "bubblewrap launcher")?;
        let sandbox_arguments = build_bubblewrap_arguments(&plugin, &profile)?;
        let workspace_root = canonical_directory(&plugin.workspace_root, "workspace root")?;
        config.transport = OfficialTransportConfig::Stdio(StdioTransportConfig {
            program: launcher,
            args: sandbox_arguments.clone(),
            cwd: workspace_root.clone(),
            workspace_root,
            environment: BTreeMap::new(),
            allowed_environment: BTreeSet::new(),
        });
        let inner = OfficialMcpTransport::new(config).map_err(McpIsolationError::Adapter)?;
        Ok(Self {
            inner: Arc::new(inner),
            sandbox_arguments,
        })
    }

    #[must_use]
    pub fn sandbox_arguments(&self) -> &[String] {
        &self.sandbox_arguments
    }

    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

#[async_trait]
impl McpTransport for SandboxedStdioTransport {
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        self.inner.list_tools().await
    }

    async fn call_tool(&self, invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
        self.inner.call_tool(invocation).await
    }
}

pub fn build_bubblewrap_arguments(
    plugin: &StdioTransportConfig,
    profile: &BubblewrapProfile,
) -> Result<Vec<String>, McpIsolationError> {
    let workspace_root = canonical_directory(&plugin.workspace_root, "workspace root")?;
    let cwd = canonical_directory(&plugin.cwd, "plugin working directory")?;
    if !cwd.starts_with(&workspace_root) {
        return Err(McpIsolationError::WorkspaceEscape(cwd));
    }
    let program = canonical_file(&plugin.program, "plugin program")?;
    let launcher = canonical_file(&profile.launcher, "bubblewrap launcher")?;
    if launcher == program {
        return Err(McpIsolationError::InvalidProfile(
            "bubblewrap launcher and plugin program must differ".into(),
        ));
    }
    let mut runtime_roots = Vec::with_capacity(profile.runtime_roots.len());
    for root in &profile.runtime_roots {
        runtime_roots.push(canonical_directory(root, "runtime root")?);
    }
    runtime_roots.sort();
    runtime_roots.dedup();

    let mut arguments = vec![
        "--die-with-parent".into(),
        "--new-session".into(),
        "--unshare-all".into(),
    ];
    if profile.network == NetworkIsolation::Allowed {
        arguments.push("--share-net".into());
    }
    arguments.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ]);
    for root in runtime_roots {
        push_bind(&mut arguments, "--ro-bind", &root, &root);
    }
    if !program.starts_with(&workspace_root)
        && !profile
            .runtime_roots
            .iter()
            .filter_map(|root| fs::canonicalize(root).ok())
            .any(|root| program.starts_with(root))
    {
        push_bind(&mut arguments, "--ro-bind", &program, &program);
    }
    push_bind(
        &mut arguments,
        match profile.workspace_access {
            WorkspaceAccess::ReadOnly => "--ro-bind",
            WorkspaceAccess::ReadWrite => "--bind",
        },
        &workspace_root,
        &workspace_root,
    );
    arguments.push("--chdir".into());
    arguments.push(path_string(&cwd)?);
    arguments.push("--clearenv".into());
    for (key, value) in &plugin.environment {
        if !plugin.allowed_environment.contains(key) {
            return Err(McpIsolationError::EnvironmentDenied(key.clone()));
        }
        arguments.push("--setenv".into());
        arguments.push(key.clone());
        arguments.push(value.clone());
    }
    arguments.push("--".into());
    arguments.push(path_string(&program)?);
    arguments.extend(plugin.args.clone());
    Ok(arguments)
}

fn push_bind(arguments: &mut Vec<String>, flag: &str, source: &Path, target: &Path) {
    arguments.push(flag.into());
    arguments.push(source.to_string_lossy().into_owned());
    arguments.push(target.to_string_lossy().into_owned());
}

fn canonical_file(path: &Path, label: &str) -> Result<PathBuf, McpIsolationError> {
    let path = fs::canonicalize(path).map_err(McpIsolationError::Io)?;
    if !path.is_file() {
        return Err(McpIsolationError::InvalidProfile(format!(
            "{label} must be a file"
        )));
    }
    Ok(path)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, McpIsolationError> {
    let path = fs::canonicalize(path).map_err(McpIsolationError::Io)?;
    if !path.is_dir() {
        return Err(McpIsolationError::InvalidProfile(format!(
            "{label} must be a directory"
        )));
    }
    Ok(path)
}

fn path_string(path: &Path) -> Result<String, McpIsolationError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| McpIsolationError::InvalidProfile("path is not valid UTF-8".into()))
}

#[derive(Debug, Error)]
pub enum McpIsolationError {
    #[error("invalid MCP sandbox profile: {0}")]
    InvalidProfile(String),
    #[error("MCP sandbox workspace escape: {0}")]
    WorkspaceEscape(PathBuf),
    #[error("MCP sandbox environment key denied: {0}")]
    EnvironmentDenied(String),
    #[error("MCP adapter error: {0}")]
    Adapter(McpAdapterError),
    #[error("MCP sandbox I/O error: {0}")]
    Io(std::io::Error),
}
