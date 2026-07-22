use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult, InitializeResult, Tool},
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde_json::{Map, Value};
use sessionweft_execution::{
    McpTransport, Permission, RiskLevel, ToolDescriptor, ToolError, ToolInvocation, ToolResult,
};
use thiserror::Error;
use tokio::{process::Command, sync::Mutex};
use tokio_util::sync::CancellationToken;

pub const MCP_ADAPTER_SCHEMA_VERSION: u32 = 1;
const MAX_SERVER_ID_BYTES: usize = 128;
const MAX_TOOL_COUNT: usize = 10_000;
const MAX_RATE_LIMIT: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPolicy {
    pub expected_server_name: Option<String>,
    pub expected_server_version: Option<String>,
    pub allowed_protocol_versions: BTreeSet<String>,
}

impl Default for CompatibilityPolicy {
    fn default() -> Self {
        Self {
            expected_server_name: None,
            expected_server_version: None,
            allowed_protocol_versions: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioTransportConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub allowed_environment: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransportConfig {
    pub endpoint: String,
    pub allowed_hosts: BTreeSet<String>,
    pub allow_plaintext_loopback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficialTransportConfig {
    Stdio(StdioTransportConfig),
    StreamableHttp(HttpTransportConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialMcpConfig {
    pub server_id: String,
    pub transport: OfficialTransportConfig,
    pub compatibility: CompatibilityPolicy,
    pub operation_timeout: Duration,
    pub max_result_bytes: usize,
    pub calls_per_minute: usize,
    pub default_risk: RiskLevel,
    pub risk_overrides: BTreeMap<String, RiskLevel>,
    pub declared_permissions: BTreeSet<Permission>,
}

impl OfficialMcpConfig {
    pub fn validate(&self) -> Result<(), McpAdapterError> {
        validate_server_id(&self.server_id)?;
        if self.operation_timeout.is_zero() || self.operation_timeout > Duration::from_secs(3_600) {
            return Err(McpAdapterError::InvalidConfig(
                "operation timeout must be between 1 ms and 1 hour".into(),
            ));
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > 16 * 1024 * 1024 {
            return Err(McpAdapterError::InvalidConfig(
                "result limit must be between 1 byte and 16 MiB".into(),
            ));
        }
        if self.calls_per_minute == 0 || self.calls_per_minute > MAX_RATE_LIMIT {
            return Err(McpAdapterError::InvalidConfig(format!(
                "calls per minute must be between 1 and {MAX_RATE_LIMIT}"
            )));
        }
        validate_compatibility(&self.compatibility)?;
        match &self.transport {
            OfficialTransportConfig::Stdio(config) => validate_stdio(config),
            OfficialTransportConfig::StreamableHttp(config) => validate_http(config),
        }
    }
}

#[derive(Clone)]
pub struct OfficialMcpTransport {
    config: Arc<OfficialMcpConfig>,
    limiter: Arc<FixedWindowLimiter>,
    cancellation: CancellationToken,
}

impl OfficialMcpTransport {
    pub fn new(config: OfficialMcpConfig) -> Result<Self, McpAdapterError> {
        config.validate()?;
        Ok(Self {
            limiter: Arc::new(FixedWindowLimiter::new(config.calls_per_minute)),
            config: Arc::new(config),
            cancellation: CancellationToken::new(),
        })
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    async fn discover(&self) -> Result<Vec<ToolDescriptor>, McpAdapterError> {
        self.ensure_active()?;
        match &self.config.transport {
            OfficialTransportConfig::Stdio(config) => self.discover_stdio(config).await,
            OfficialTransportConfig::StreamableHttp(config) => self.discover_http(config).await,
        }
    }

    async fn invoke_remote(
        &self,
        invocation: &ToolInvocation,
    ) -> Result<ToolResult, McpAdapterError> {
        self.ensure_active()?;
        self.limiter.acquire().await?;
        let remote_name = self.remote_tool_name(&invocation.tool_name)?;
        let arguments = invocation
            .arguments
            .as_object()
            .cloned()
            .ok_or_else(|| McpAdapterError::InvalidInvocation("arguments must be an object".into()))?;
        match &self.config.transport {
            OfficialTransportConfig::Stdio(config) => {
                self.call_stdio(config, remote_name, arguments).await
            }
            OfficialTransportConfig::StreamableHttp(config) => {
                self.call_http(config, remote_name, arguments).await
            }
        }
    }

    fn ensure_active(&self) -> Result<(), McpAdapterError> {
        if self.cancellation.is_cancelled() {
            Err(McpAdapterError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn remote_tool_name(&self, normalized_name: &str) -> Result<String, McpAdapterError> {
        let prefix = format!("{}.", self.config.server_id);
        normalized_name
            .strip_prefix(&prefix)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                McpAdapterError::InvalidInvocation(format!(
                    "tool '{normalized_name}' is outside MCP server namespace '{}'",
                    self.config.server_id
                ))
            })
    }

    async fn discover_stdio(
        &self,
        config: &StdioTransportConfig,
    ) -> Result<Vec<ToolDescriptor>, McpAdapterError> {
        let command = build_stdio_command(config)?;
        let transport = TokioChildProcess::new(command).map_err(McpAdapterError::SdkTransport)?;
        let service = ().serve(transport).await.map_err(sdk_error)?;
        let info = service
            .peer_info()
            .ok_or_else(|| McpAdapterError::Protocol("server omitted initialization info".into()))?;
        self.validate_server(&info)?;
        let result = tokio::select! {
            () = self.cancellation.cancelled() => Err(McpAdapterError::Cancelled),
            result = tokio::time::timeout(self.config.operation_timeout, service.list_all_tools()) => {
                result.map_err(|_| McpAdapterError::Timeout(self.config.operation_timeout))?
                    .map_err(sdk_error)
                    .and_then(|tools| self.normalize_tools(tools, &info))
            }
        };
        service.cancellation_token().cancel();
        result
    }

    async fn discover_http(
        &self,
        config: &HttpTransportConfig,
    ) -> Result<Vec<ToolDescriptor>, McpAdapterError> {
        let transport = StreamableHttpClientTransport::from_uri(config.endpoint.as_str());
        let service = ().serve(transport).await.map_err(sdk_error)?;
        let info = service
            .peer_info()
            .ok_or_else(|| McpAdapterError::Protocol("server omitted initialization info".into()))?;
        self.validate_server(&info)?;
        let result = tokio::select! {
            () = self.cancellation.cancelled() => Err(McpAdapterError::Cancelled),
            result = tokio::time::timeout(self.config.operation_timeout, service.list_all_tools()) => {
                result.map_err(|_| McpAdapterError::Timeout(self.config.operation_timeout))?
                    .map_err(sdk_error)
                    .and_then(|tools| self.normalize_tools(tools, &info))
            }
        };
        service.cancellation_token().cancel();
        result
    }

    async fn call_stdio(
        &self,
        config: &StdioTransportConfig,
        remote_name: String,
        arguments: Map<String, Value>,
    ) -> Result<ToolResult, McpAdapterError> {
        let command = build_stdio_command(config)?;
        let transport = TokioChildProcess::new(command).map_err(McpAdapterError::SdkTransport)?;
        let service = ().serve(transport).await.map_err(sdk_error)?;
        let info = service
            .peer_info()
            .ok_or_else(|| McpAdapterError::Protocol("server omitted initialization info".into()))?;
        self.validate_server(&info)?;
        let request = CallToolRequestParams::new(remote_name).with_arguments(arguments);
        let result = tokio::select! {
            () = self.cancellation.cancelled() => Err(McpAdapterError::Cancelled),
            result = tokio::time::timeout(self.config.operation_timeout, service.call_tool(request)) => {
                result.map_err(|_| McpAdapterError::Timeout(self.config.operation_timeout))?
                    .map_err(sdk_error)
                    .and_then(|result| self.normalize_result(result, &info))
            }
        };
        service.cancellation_token().cancel();
        result
    }

    async fn call_http(
        &self,
        config: &HttpTransportConfig,
        remote_name: String,
        arguments: Map<String, Value>,
    ) -> Result<ToolResult, McpAdapterError> {
        let transport = StreamableHttpClientTransport::from_uri(config.endpoint.as_str());
        let service = ().serve(transport).await.map_err(sdk_error)?;
        let info = service
            .peer_info()
            .ok_or_else(|| McpAdapterError::Protocol("server omitted initialization info".into()))?;
        self.validate_server(&info)?;
        let request = CallToolRequestParams::new(remote_name).with_arguments(arguments);
        let result = tokio::select! {
            () = self.cancellation.cancelled() => Err(McpAdapterError::Cancelled),
            result = tokio::time::timeout(self.config.operation_timeout, service.call_tool(request)) => {
                result.map_err(|_| McpAdapterError::Timeout(self.config.operation_timeout))?
                    .map_err(sdk_error)
                    .and_then(|result| self.normalize_result(result, &info))
            }
        };
        service.cancellation_token().cancel();
        result
    }

    fn validate_server(&self, info: &InitializeResult) -> Result<(), McpAdapterError> {
        let policy = &self.config.compatibility;
        if info.capabilities.tools.is_none() {
            return Err(McpAdapterError::Compatibility(
                "MCP server did not negotiate tool capability".into(),
            ));
        }
        if let Some(expected) = &policy.expected_server_name
            && info.server_info.name != *expected
        {
            return Err(McpAdapterError::Compatibility(format!(
                "server name '{}' does not match expected '{expected}'",
                info.server_info.name
            )));
        }
        if let Some(expected) = &policy.expected_server_version
            && info.server_info.version != *expected
        {
            return Err(McpAdapterError::Compatibility(format!(
                "server version '{}' does not match expected '{expected}'",
                info.server_info.version
            )));
        }
        let protocol = info.protocol_version.as_str();
        if !policy.allowed_protocol_versions.is_empty()
            && !policy.allowed_protocol_versions.contains(protocol)
        {
            return Err(McpAdapterError::Compatibility(format!(
                "protocol version '{protocol}' is not allowlisted"
            )));
        }
        Ok(())
    }

    fn normalize_tools(
        &self,
        tools: Vec<Tool>,
        info: &InitializeResult,
    ) -> Result<Vec<ToolDescriptor>, McpAdapterError> {
        if tools.len() > MAX_TOOL_COUNT {
            return Err(McpAdapterError::Protocol(format!(
                "server exposed {} tools, limit is {MAX_TOOL_COUNT}",
                tools.len()
            )));
        }
        let mut names = HashSet::with_capacity(tools.len());
        let mut normalized = Vec::with_capacity(tools.len());
        for tool in tools {
            let remote_name = tool.name.trim();
            if remote_name.is_empty() {
                return Err(McpAdapterError::SchemaSpoofing(
                    "server returned an empty tool name".into(),
                ));
            }
            let name = format!("{}.{}", self.config.server_id, remote_name);
            if !names.insert(name.clone()) {
                return Err(McpAdapterError::ToolCollision(name));
            }
            let input_schema = Value::Object((*tool.input_schema).clone());
            validate_input_schema(&name, &input_schema)?;
            let mut permissions = self.config.declared_permissions.clone();
            permissions.insert(Permission::Tool(name.clone()));
            let risk = self
                .config
                .risk_overrides
                .get(remote_name)
                .copied()
                .unwrap_or(self.config.default_risk);
            let descriptor = ToolDescriptor {
                name,
                version: info.server_info.version.clone(),
                permissions,
                risk,
                input_schema,
            };
            descriptor
                .validate()
                .map_err(|error| McpAdapterError::SchemaSpoofing(error.to_string()))?;
            normalized.push(descriptor);
        }
        Ok(normalized)
    }

    fn normalize_result(
        &self,
        result: CallToolResult,
        info: &InitializeResult,
    ) -> Result<ToolResult, McpAdapterError> {
        let content = result.structured_content.clone().unwrap_or_else(|| {
            serde_json::to_value(&result.content).unwrap_or(Value::Null)
        });
        let size = serde_json::to_vec(&content)
            .map_err(|error| McpAdapterError::Protocol(error.to_string()))?
            .len();
        if size > self.config.max_result_bytes {
            return Err(McpAdapterError::OutputLimitExceeded {
                actual: size,
                limit: self.config.max_result_bytes,
            });
        }
        if result.is_error.unwrap_or(false) {
            return Err(McpAdapterError::ToolReportedError(content));
        }
        Ok(ToolResult {
            content,
            metadata: BTreeMap::from([
                ("mcp.server".into(), info.server_info.name.clone()),
                ("mcp.server_version".into(), info.server_info.version.clone()),
                (
                    "mcp.protocol_version".into(),
                    info.protocol_version.as_str().to_owned(),
                ),
            ]),
        })
    }
}

#[async_trait]
impl McpTransport for OfficialMcpTransport {
    async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, ToolError> {
        self.discover().await.map_err(ToolError::from)
    }

    async fn call_tool(&self, invocation: &ToolInvocation) -> Result<ToolResult, ToolError> {
        self.invoke_remote(invocation).await.map_err(ToolError::from)
    }
}

struct FixedWindowLimiter {
    limit: usize,
    calls: Mutex<VecDeque<Instant>>,
}

impl FixedWindowLimiter {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            calls: Mutex::new(VecDeque::with_capacity(limit)),
        }
    }

    async fn acquire(&self) -> Result<(), McpAdapterError> {
        let now = Instant::now();
        let cutoff = now - Duration::from_secs(60);
        let mut calls = self.calls.lock().await;
        while calls.front().is_some_and(|instant| *instant <= cutoff) {
            calls.pop_front();
        }
        if calls.len() >= self.limit {
            return Err(McpAdapterError::RateLimited);
        }
        calls.push_back(now);
        Ok(())
    }
}

fn validate_server_id(value: &str) -> Result<(), McpAdapterError> {
    if value.is_empty()
        || value.len() > MAX_SERVER_ID_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(McpAdapterError::InvalidConfig(
            "server ID must contain 1-128 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(())
}

fn validate_compatibility(policy: &CompatibilityPolicy) -> Result<(), McpAdapterError> {
    for value in [
        policy.expected_server_name.as_deref(),
        policy.expected_server_version.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(McpAdapterError::InvalidConfig(
                "compatibility identifiers must contain 1-256 bytes".into(),
            ));
        }
    }
    if policy
        .allowed_protocol_versions
        .iter()
        .any(|version| version.trim().is_empty() || version.len() > 64)
    {
        return Err(McpAdapterError::InvalidConfig(
            "protocol allowlist contains an invalid version".into(),
        ));
    }
    Ok(())
}

fn validate_stdio(config: &StdioTransportConfig) -> Result<(), McpAdapterError> {
    let workspace_root = canonical_directory(&config.workspace_root, "workspace root")?;
    let cwd = canonical_directory(&config.cwd, "plugin working directory")?;
    if !cwd.starts_with(&workspace_root) {
        return Err(McpAdapterError::WorkspaceEscape(cwd));
    }
    let program = fs::canonicalize(&config.program).map_err(McpAdapterError::Io)?;
    if !program.is_file() {
        return Err(McpAdapterError::InvalidConfig(
            "stdio plugin program must be a file".into(),
        ));
    }
    if config
        .environment
        .keys()
        .any(|key| !config.allowed_environment.contains(key))
    {
        return Err(McpAdapterError::InvalidConfig(
            "stdio plugin environment contains a non-allowlisted key".into(),
        ));
    }
    Ok(())
}

fn validate_http(config: &HttpTransportConfig) -> Result<(), McpAdapterError> {
    let url = reqwest::Url::parse(&config.endpoint)
        .map_err(|error| McpAdapterError::InvalidConfig(error.to_string()))?;
    let host = url
        .host_str()
        .ok_or_else(|| McpAdapterError::InvalidConfig("HTTP endpoint has no host".into()))?;
    let is_loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if url.scheme() != "https" && !(config.allow_plaintext_loopback && is_loopback) {
        return Err(McpAdapterError::InvalidConfig(
            "Streamable HTTP requires HTTPS except explicitly allowed loopback".into(),
        ));
    }
    if !config.allowed_hosts.contains(host) {
        return Err(McpAdapterError::InvalidConfig(format!(
            "HTTP host '{host}' is not allowlisted"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(McpAdapterError::InvalidConfig(
            "credentials must not be embedded in the MCP endpoint".into(),
        ));
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, McpAdapterError> {
    let path = fs::canonicalize(path).map_err(McpAdapterError::Io)?;
    if !path.is_dir() {
        return Err(McpAdapterError::InvalidConfig(format!(
            "{label} must be a directory"
        )));
    }
    Ok(path)
}

fn build_stdio_command(config: &StdioTransportConfig) -> Result<Command, McpAdapterError> {
    validate_stdio(config)?;
    let program = fs::canonicalize(&config.program).map_err(McpAdapterError::Io)?;
    let cwd = fs::canonicalize(&config.cwd).map_err(McpAdapterError::Io)?;
    let mut command = Command::new(program);
    command
        .args(&config.args)
        .current_dir(cwd)
        .env_clear()
        .envs(&config.environment)
        .kill_on_drop(true);
    Ok(command)
}

fn validate_input_schema(name: &str, schema: &Value) -> Result<(), McpAdapterError> {
    let object = schema
        .as_object()
        .ok_or_else(|| McpAdapterError::SchemaSpoofing(format!("tool '{name}' schema is not an object")))?;
    if let Some(schema_type) = object.get("type")
        && schema_type != "object"
    {
        return Err(McpAdapterError::SchemaSpoofing(format!(
            "tool '{name}' input schema type must be object"
        )));
    }
    Ok(())
}

fn sdk_error(error: impl std::fmt::Display) -> McpAdapterError {
    McpAdapterError::Sdk(error.to_string())
}

impl From<McpAdapterError> for ToolError {
    fn from(error: McpAdapterError) -> Self {
        match error {
            McpAdapterError::InvalidInvocation(message)
            | McpAdapterError::Compatibility(message)
            | McpAdapterError::SchemaSpoofing(message)
            | McpAdapterError::Protocol(message)
            | McpAdapterError::Sdk(message) => ToolError::Execution(message),
            McpAdapterError::ToolCollision(name) => {
                ToolError::InvalidDescriptor(format!("MCP tool collision: {name}"))
            }
            McpAdapterError::Cancelled => ToolError::Execution("MCP operation cancelled".into()),
            McpAdapterError::Timeout(duration) => {
                ToolError::Execution(format!("MCP operation timed out after {duration:?}"))
            }
            McpAdapterError::RateLimited => ToolError::Denied("MCP rate limit exceeded".into()),
            McpAdapterError::OutputLimitExceeded { actual, limit } => ToolError::Execution(format!(
                "MCP result {actual} bytes exceeds limit {limit}"
            )),
            McpAdapterError::ToolReportedError(content) => {
                ToolError::Execution(format!("MCP tool reported error: {content}"))
            }
            McpAdapterError::InvalidConfig(message) => ToolError::Execution(message),
            McpAdapterError::WorkspaceEscape(path) => {
                ToolError::Denied(format!("MCP workspace escape: {}", path.display()))
            }
            McpAdapterError::Io(error) => ToolError::Execution(error.to_string()),
            McpAdapterError::SdkTransport(error) => ToolError::Execution(error.to_string()),
        }
    }
}

#[derive(Debug, Error)]
pub enum McpAdapterError {
    #[error("invalid MCP adapter configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid MCP invocation: {0}")]
    InvalidInvocation(String),
    #[error("MCP server compatibility rejected: {0}")]
    Compatibility(String),
    #[error("MCP schema spoofing rejected: {0}")]
    SchemaSpoofing(String),
    #[error("MCP tool collision: {0}")]
    ToolCollision(String),
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP SDK error: {0}")]
    Sdk(String),
    #[error("MCP transport error: {0}")]
    SdkTransport(std::io::Error),
    #[error("MCP operation cancelled")]
    Cancelled,
    #[error("MCP operation timed out after {0:?}")]
    Timeout(Duration),
    #[error("MCP rate limit exceeded")]
    RateLimited,
    #[error("MCP output {actual} bytes exceeds limit {limit}")]
    OutputLimitExceeded { actual: usize, limit: usize },
    #[error("MCP tool reported an error: {0}")]
    ToolReportedError(Value),
    #[error("MCP plugin working directory escapes workspace: {0}")]
    WorkspaceEscape(PathBuf),
    #[error("MCP adapter I/O error: {0}")]
    Io(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config(root: &Path) -> OfficialMcpConfig {
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
        let child = root.join(format!("sessionweft-mcp-{}", std::process::id()));
        fs::create_dir_all(&child).expect("test directory");
        let mut config = base_config(&child);
        let OfficialTransportConfig::Stdio(stdio) = &mut config.transport else {
            panic!("stdio config")
        };
        stdio.cwd = root.clone();
        assert!(matches!(
            config.validate(),
            Err(McpAdapterError::WorkspaceEscape(_))
        ));
        stdio.cwd = child.clone();
        stdio.environment.insert("SECRET_TOKEN".into(), "hidden".into());
        assert!(matches!(
            config.validate(),
            Err(McpAdapterError::InvalidConfig(_))
        ));
        fs::remove_dir_all(child).expect("cleanup");
    }

    #[test]
    fn rejects_unallowlisted_or_insecure_http_endpoint() {
        let config = HttpTransportConfig {
            endpoint: "http://example.com/mcp".into(),
            allowed_hosts: BTreeSet::from(["example.com".into()]),
            allow_plaintext_loopback: false,
        };
        assert!(validate_http(&config).is_err());
        let config = HttpTransportConfig {
            endpoint: "https://example.com/mcp".into(),
            allowed_hosts: BTreeSet::from(["different.example".into()]),
            allow_plaintext_loopback: false,
        };
        assert!(validate_http(&config).is_err());
    }

    #[tokio::test]
    async fn limiter_rejects_calls_beyond_window() {
        let limiter = FixedWindowLimiter::new(1);
        limiter.acquire().await.expect("first call");
        assert!(matches!(
            limiter.acquire().await,
            Err(McpAdapterError::RateLimited)
        ));
    }
}
