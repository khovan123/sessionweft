use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    AgentExecutionState, AgentExecutionView, PtyError, PtyResizeRequest, PtyStatus, PtySupervisor,
    StartAgentExecutionRequest, StartAgentExecutionResponse, StartPtyRequest,
    StopAgentExecutionRequest, TerminalFrame, TerminalFrameBatch, TerminalInputRequest,
    TerminalResizeRequest,
};

#[derive(Clone)]
pub struct AgentExecutionSupervisor {
    workspace_root: Arc<PathBuf>,
    pty: PtySupervisor,
    executions: Arc<RwLock<HashMap<Uuid, ExecutionRecord>>>,
}

#[derive(Clone)]
struct ExecutionRecord {
    view: AgentExecutionView,
    pty_id: Uuid,
    context_dir: PathBuf,
}

impl AgentExecutionSupervisor {
    pub fn new(
        workspace_root: impl AsRef<Path>,
        pty: PtySupervisor,
    ) -> Result<Self, ExecutionError> {
        let workspace_root = fs::canonicalize(workspace_root)?;
        Ok(Self {
            workspace_root: Arc::new(workspace_root),
            pty,
            executions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn start(
        &self,
        session_id: Uuid,
        workflow_id: Uuid,
        node_id: String,
        request: StartAgentExecutionRequest,
    ) -> Result<StartAgentExecutionResponse, ExecutionError> {
        validate_start(&request)?;
        let execution_id = Uuid::new_v4();
        let fencing_token = request.expected_version.saturating_add(1);
        let context_dir = self
            .workspace_root
            .join(".sessionweft")
            .join("executions")
            .join(execution_id.to_string());
        materialize_context(
            &context_dir,
            execution_id,
            session_id,
            workflow_id,
            &node_id,
            fencing_token,
            &request,
        )?;

        let environment = BTreeMap::from([
            ("SESSIONWEFT_EXECUTION_ID".into(), execution_id.to_string()),
            ("SESSIONWEFT_SESSION_ID".into(), session_id.to_string()),
            ("SESSIONWEFT_WORKFLOW_ID".into(), workflow_id.to_string()),
            ("SESSIONWEFT_WORKFLOW_NODE_ID".into(), node_id.clone()),
            (
                "SESSIONWEFT_CONTEXT_FILE".into(),
                context_dir.join("context.md").display().to_string(),
            ),
            (
                "SESSIONWEFT_SKILLS_DIR".into(),
                context_dir.join("skills").display().to_string(),
            ),
            (
                "SESSIONWEFT_PLUGINS_DIR".into(),
                context_dir.join("plugins").display().to_string(),
            ),
            (
                "SESSIONWEFT_FENCING_TOKEN".into(),
                fencing_token.to_string(),
            ),
        ]);
        let pty = self.pty.start(StartPtyRequest {
            session_id,
            program: request.agent.clone(),
            args: Vec::new(),
            cwd: ".".into(),
            environment,
            rows: request.terminal.rows,
            cols: request.terminal.cols,
            max_output_bytes: crate::MAX_PTY_OUTPUT_LIMIT,
        })?;
        let view = AgentExecutionView {
            execution_id,
            session_id,
            workflow_id,
            node_id,
            agent: request.agent,
            workspace_id: request.workspace_id,
            owner_id: request.owner_id,
            state: AgentExecutionState::Running,
            fencing_token,
            skills: request.skills,
            plugins: request.plugins,
            terminal_cursor: 0,
            exit_code: None,
            error: None,
        };
        self.executions
            .write()
            .map_err(|_| ExecutionError::Poisoned)?
            .insert(
                execution_id,
                ExecutionRecord {
                    view: view.clone(),
                    pty_id: pty.pty_id,
                    context_dir,
                },
            );
        Ok(StartAgentExecutionResponse {
            execution: view,
            attach_path: format!("/v1/executions/{execution_id}/terminal"),
        })
    }

    pub fn view(&self, execution_id: Uuid) -> Result<AgentExecutionView, ExecutionError> {
        let mut record = self.record(execution_id)?;
        let descriptor = self.pty.descriptor(record.pty_id)?;
        record.view.state = map_status(descriptor.status);
        record.view.terminal_cursor = descriptor.output_cursor;
        Ok(record.view)
    }

    pub fn input(
        &self,
        execution_id: Uuid,
        request: TerminalInputRequest,
    ) -> Result<(), ExecutionError> {
        let record = self.authorized(execution_id, request.fencing_token)?;
        self.pty.input(record.pty_id, &request.data)?;
        Ok(())
    }

    pub fn resize(
        &self,
        execution_id: Uuid,
        request: TerminalResizeRequest,
    ) -> Result<(), ExecutionError> {
        let record = self.authorized(execution_id, request.fencing_token)?;
        self.pty.resize(
            record.pty_id,
            PtyResizeRequest {
                rows: request.rows,
                cols: request.cols,
            },
        )?;
        Ok(())
    }

    pub fn stop(
        &self,
        execution_id: Uuid,
        request: StopAgentExecutionRequest,
    ) -> Result<AgentExecutionView, ExecutionError> {
        let record = self.authorized(execution_id, request.fencing_token)?;
        self.pty.cancel(record.pty_id)?;
        self.view(execution_id)
    }

    pub fn terminal_after(
        &self,
        execution_id: Uuid,
        after: u64,
    ) -> Result<TerminalFrameBatch, ExecutionError> {
        let record = self.record(execution_id)?;
        let batch = self.pty.output_after(record.pty_id, after)?;
        Ok(TerminalFrameBatch {
            execution_id,
            next_cursor: batch.next,
            frames: batch
                .chunks
                .into_iter()
                .map(|chunk| TerminalFrame {
                    cursor: chunk.cursor,
                    stream: "pty".into(),
                    data: chunk.data,
                })
                .collect(),
        })
    }

    pub fn context_dir(&self, execution_id: Uuid) -> Result<PathBuf, ExecutionError> {
        Ok(self.record(execution_id)?.context_dir)
    }

    fn authorized(
        &self,
        execution_id: Uuid,
        fencing_token: u64,
    ) -> Result<ExecutionRecord, ExecutionError> {
        let record = self.record(execution_id)?;
        if record.view.fencing_token != fencing_token {
            return Err(ExecutionError::FencingTokenMismatch);
        }
        Ok(record)
    }

    fn record(&self, execution_id: Uuid) -> Result<ExecutionRecord, ExecutionError> {
        self.executions
            .read()
            .map_err(|_| ExecutionError::Poisoned)?
            .get(&execution_id)
            .cloned()
            .ok_or(ExecutionError::NotFound(execution_id))
    }
}

fn validate_start(request: &StartAgentExecutionRequest) -> Result<(), ExecutionError> {
    if request.agent.trim().is_empty() {
        return Err(ExecutionError::Validation("agent is required".into()));
    }
    if request.workspace_id.trim().is_empty() || request.owner_id.trim().is_empty() {
        return Err(ExecutionError::Validation(
            "workspace_id and owner_id are required".into(),
        ));
    }
    if request.task.trim().is_empty() {
        return Err(ExecutionError::Validation("task is required".into()));
    }
    Ok(())
}

fn materialize_context(
    context_dir: &Path,
    execution_id: Uuid,
    session_id: Uuid,
    workflow_id: Uuid,
    node_id: &str,
    fencing_token: u64,
    request: &StartAgentExecutionRequest,
) -> Result<(), ExecutionError> {
    fs::create_dir_all(context_dir.join("skills"))?;
    fs::create_dir_all(context_dir.join("plugins"))?;
    fs::write(
        context_dir.join("context.md"),
        format!(
            "# SessionWeft workflow execution\n\n- Execution: `{execution_id}`\n- Session: `{session_id}`\n- Workflow: `{workflow_id}`\n- Node: `{node_id}`\n- Agent: `{}`\n- Workspace: `{}`\n- Owner: `{}`\n\n## Task\n\n{}\n\n## Skills\n\n{}\n\n## Plugins\n\n{}\n",
            request.agent,
            request.workspace_id,
            request.owner_id,
            request.task,
            request.skills.join("\n"),
            request.plugins.join("\n")
        ),
    )?;
    fs::write(
        context_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "execution_id": execution_id,
            "session_id": session_id,
            "workflow_id": workflow_id,
            "node_id": node_id,
            "fencing_token": fencing_token,
            "agent": request.agent,
            "skills": request.skills,
            "plugins": request.plugins,
        }))?,
    )?;
    Ok(())
}

fn map_status(status: PtyStatus) -> AgentExecutionState {
    match status {
        PtyStatus::Running => AgentExecutionState::Running,
        PtyStatus::Exited => AgentExecutionState::Completed,
        PtyStatus::Cancelled => AgentExecutionState::Stopping,
        PtyStatus::Failed => AgentExecutionState::Failed,
    }
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("execution {0} was not found")]
    NotFound(Uuid),
    #[error("execution fencing token does not match")]
    FencingTokenMismatch,
    #[error("invalid execution request: {0}")]
    Validation(String),
    #[error("execution registry lock is poisoned")]
    Poisoned,
    #[error(transparent)]
    Pty(#[from] PtyError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_tasks() {
        let request = StartAgentExecutionRequest {
            expected_version: 1,
            agent: "claude".into(),
            workspace_id: "workspace".into(),
            owner_id: "owner".into(),
            task: "".into(),
            skills: Vec::new(),
            plugins: Vec::new(),
            terminal: Default::default(),
        };
        assert!(matches!(
            validate_start(&request),
            Err(ExecutionError::Validation(_))
        ));
    }
}
