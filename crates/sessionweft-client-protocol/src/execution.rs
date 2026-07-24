use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 140,
            rows: 40,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartAgentExecutionRequest {
    pub expected_version: u64,
    pub agent: String,
    pub workspace_id: String,
    pub owner_id: String,
    pub task: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub terminal: TerminalSize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionState {
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentExecutionView {
    pub execution_id: Uuid,
    pub session_id: Uuid,
    pub workflow_id: Uuid,
    pub node_id: String,
    pub agent: String,
    pub workspace_id: String,
    pub owner_id: String,
    pub state: AgentExecutionState,
    pub fencing_token: u64,
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
    pub terminal_cursor: u64,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartAgentExecutionResponse {
    pub execution: AgentExecutionView,
    pub attach_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalInputRequest {
    pub fencing_token: u64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalResizeRequest {
    pub fencing_token: u64,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StopAgentExecutionRequest {
    pub fencing_token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalFrame {
    pub cursor: u64,
    pub stream: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalFrameBatch {
    pub execution_id: Uuid,
    pub next_cursor: u64,
    pub frames: Vec<TerminalFrame>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_terminal_size_is_human_friendly() {
        assert_eq!(
            TerminalSize::default(),
            TerminalSize {
                cols: 140,
                rows: 40
            }
        );
    }

    #[test]
    fn execution_state_uses_stable_wire_names() {
        let json = serde_json::to_string(&AgentExecutionState::Running).unwrap();
        assert_eq!(json, "\"running\"");
    }
}
