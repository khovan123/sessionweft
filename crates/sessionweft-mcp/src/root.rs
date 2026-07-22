#[cfg(not(test))]
include!("lib.rs");

#[cfg(not(test))]
mod approval;
#[cfg(not(test))]
mod isolation;

#[cfg(not(test))]
pub use approval::{
    AuditedMcpGateway, ConsumeApprovalCommand, IssueApprovalCommand, MCP_APPROVAL_SCHEMA_VERSION,
    McpApprovalError, McpApprovalRecord, McpApprovalRepository, McpApprovalRepositoryError,
};
#[cfg(not(test))]
pub use isolation::{
    BubblewrapProfile, McpIsolationError, NetworkIsolation, SandboxedStdioTransport,
    WorkspaceAccess, build_bubblewrap_arguments,
};
