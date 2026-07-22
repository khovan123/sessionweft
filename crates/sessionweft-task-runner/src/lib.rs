use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use sessionweft_core::ProviderRequest;
use sessionweft_provider::ProviderRegistry;
use sessionweft_scheduler::{
    TaskAction, TaskActionRunError, TaskActionRunner, TaskExecutionRecord,
};

#[async_trait]
pub trait ToolAction: Send + Sync {
    fn name(&self) -> &str;

    async fn invoke(&self, input: Value) -> Result<Value, ToolRunError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolAction>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: ToolAction + 'static,
    {
        self.tools.insert(tool.name().to_owned(), Arc::new(tool));
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolAction>> {
        self.tools.get(name).cloned()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names = self.tools.keys().cloned().collect::<Vec<_>>();
        names.sort_unstable();
        names
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EchoTool;

#[async_trait]
impl ToolAction for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    async fn invoke(&self, input: Value) -> Result<Value, ToolRunError> {
        Ok(serde_json::json!({"tool": "echo", "input": input}))
    }
}

#[derive(Clone)]
pub struct ProviderToolRunner {
    providers: Arc<ProviderRegistry>,
    tools: Arc<ToolRegistry>,
}

impl ProviderToolRunner {
    #[must_use]
    pub fn new(providers: Arc<ProviderRegistry>, tools: Arc<ToolRegistry>) -> Self {
        Self { providers, tools }
    }
}

#[async_trait]
impl TaskActionRunner for ProviderToolRunner {
    async fn run(&self, execution: &TaskExecutionRecord) -> Result<Value, TaskActionRunError> {
        match &execution.action {
            TaskAction::Provider {
                provider,
                model,
                messages,
            } => {
                let adapter = self.providers.get(provider).ok_or_else(|| {
                    TaskActionRunError::Unsupported(format!(
                        "provider '{provider}' is not registered"
                    ))
                })?;
                let response = adapter
                    .complete(ProviderRequest {
                        session_id: execution.session_id,
                        model: model.clone(),
                        messages: messages.clone(),
                    })
                    .await
                    .map_err(|error| TaskActionRunError::Failed(error.to_string()))?;
                serde_json::to_value(response)
                    .map_err(|error| TaskActionRunError::Failed(error.to_string()))
            }
            TaskAction::Tool { descriptor, input } => {
                let tool = self.tools.get(&descriptor.name).ok_or_else(|| {
                    TaskActionRunError::Unsupported(format!(
                        "tool '{}' is not registered",
                        descriptor.name
                    ))
                })?;
                tool.invoke(input.clone())
                    .await
                    .map_err(|error| TaskActionRunError::Failed(error.to_string()))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolRunError {
    #[error("tool invocation failed: {0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sessionweft_core::{MessageRole, ProviderMessage, SessionId};
    use sessionweft_execution::{Permission, RiskLevel, ToolDescriptor};
    use sessionweft_provider::EchoProvider;
    use sessionweft_scheduler::{TASK_EXECUTION_SCHEMA_VERSION, TaskExecutionStatus};
    use uuid::Uuid;

    use super::*;

    fn execution(action: TaskAction) -> TaskExecutionRecord {
        let now = Utc::now();
        TaskExecutionRecord {
            schema_version: TASK_EXECUTION_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            claim_id: Uuid::new_v4(),
            session_id: SessionId::new(),
            workflow_id: Uuid::new_v4(),
            node_id: "worker".into(),
            agent_id: Uuid::new_v4(),
            idempotency_key: "key".into(),
            action,
            status: TaskExecutionStatus::Running,
            output: None,
            sanitized_error: None,
            prepared_at: now,
            started_at: Some(now),
            completed_at: None,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn provider_action_uses_registered_provider() {
        let mut providers = ProviderRegistry::new();
        providers.register(EchoProvider);
        let runner = ProviderToolRunner::new(Arc::new(providers), Arc::new(ToolRegistry::new()));
        let output = runner
            .run(&execution(TaskAction::Provider {
                provider: "echo".into(),
                model: "test".into(),
                messages: vec![ProviderMessage {
                    role: MessageRole::User,
                    content: "hello".into(),
                }],
            }))
            .await
            .expect("provider output");
        assert_eq!(output["text"], "[echo:test] hello");
    }

    #[tokio::test]
    async fn tool_action_uses_registered_tool() {
        let mut tools = ToolRegistry::new();
        tools.register(EchoTool);
        let runner = ProviderToolRunner::new(Arc::new(ProviderRegistry::new()), Arc::new(tools));
        let descriptor = ToolDescriptor {
            name: "echo".into(),
            version: "1".into(),
            permissions: [Permission::Tool("echo".into())].into_iter().collect(),
            risk: RiskLevel::Low,
            input_schema: serde_json::json!({"type": "object"}),
        };
        let output = runner
            .run(&execution(TaskAction::Tool {
                descriptor,
                input: serde_json::json!({"value": 7}),
            }))
            .await
            .expect("tool output");
        assert_eq!(output["input"]["value"], 7);
    }
}
