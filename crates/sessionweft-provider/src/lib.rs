use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sessionweft_adapter_certification::{AdapterKind, require_compiled_adapter};
use sessionweft_core::{MessageRole, ProviderRequest, ProviderResponse, ProviderUsage};
use thiserror::Error;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    fn adapter_id(&self) -> &'static str {
        self.name()
    }

    fn adapter_version(&self) -> &'static str {
        "1.0.0"
    }

    async fn complete(&self, request: ProviderRequest) -> Result<ProviderResponse, ProviderError>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn try_register<P>(&mut self, provider: P) -> Result<(), ProviderError>
    where
        P: Provider + 'static,
    {
        require_compiled_adapter(
            provider.adapter_id(),
            provider.adapter_version(),
            AdapterKind::Provider,
        )
        .map_err(|error| ProviderError::Certification(error.to_string()))?;
        self.providers
            .insert(provider.name().to_owned(), Arc::new(provider));
        Ok(())
    }

    pub fn register<P>(&mut self, provider: P)
    where
        P: Provider + 'static,
    {
        self.try_register(provider)
            .expect("provider adapter must be certified for this Runtime build");
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names = self.providers.keys().cloned().collect::<Vec<_>>();
        names.sort_unstable();
        names
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EchoProvider;

#[async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn adapter_id(&self) -> &'static str {
        "echo-provider"
    }

    async fn complete(&self, request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let input = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or_default();

        Ok(ProviderResponse {
            text: format!("[echo:{}] {input}", request.model),
            provider_request_id: None,
            usage: ProviderUsage::default(),
        })
    }
}

#[derive(Clone)]
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>, timeout: Duration) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(ProviderError::Transport)?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        })
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn adapter_id(&self) -> &'static str {
        "ollama-provider"
    }

    async fn complete(&self, request: ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        let body = OllamaChatRequest {
            model: request.model,
            messages: request
                .messages
                .into_iter()
                .map(|message| OllamaMessage {
                    role: message.role.to_string(),
                    content: message.content,
                })
                .collect(),
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Transport)?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited);
        }
        if !status.is_success() {
            return Err(ProviderError::HttpStatus(status.as_u16()));
        }

        let response = response
            .json::<OllamaChatResponse>()
            .await
            .map_err(ProviderError::Transport)?;
        if response.message.content.trim().is_empty() {
            return Err(ProviderError::InvalidResponse(
                "provider returned empty assistant content".into(),
            ));
        }

        Ok(ProviderResponse {
            text: response.message.content,
            provider_request_id: None,
            usage: ProviderUsage {
                input_tokens: response.prompt_eval_count,
                output_tokens: response.eval_count,
            },
        })
    }
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaAssistantMessage,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OllamaAssistantMessage {
    content: String,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider transport error: {0}")]
    Transport(reqwest::Error),
    #[error("provider rate limited the request")]
    RateLimited,
    #[error("provider returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("provider returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("provider is not registered: {0}")]
    NotRegistered(String),
    #[error("provider adapter certification failed: {0}")]
    Certification(String),
}

#[cfg(test)]
mod tests {
    use sessionweft_core::{ProviderMessage, SessionId};

    use super::*;

    #[tokio::test]
    async fn echo_provider_is_deterministic() {
        let response = EchoProvider
            .complete(ProviderRequest {
                session_id: SessionId::new(),
                model: "test-model".into(),
                messages: vec![ProviderMessage {
                    role: MessageRole::User,
                    content: "hello".into(),
                }],
            })
            .await
            .expect("response");
        assert_eq!(response.text, "[echo:test-model] hello");
    }

    #[test]
    fn registry_lists_registered_providers() {
        let mut registry = ProviderRegistry::new();
        registry.register(EchoProvider);
        assert_eq!(registry.names(), vec!["echo"]);
    }

    #[test]
    fn production_provider_ids_match_release_manifests() {
        assert_eq!(EchoProvider.adapter_id(), "echo-provider");
        let ollama = OllamaProvider::new("http://127.0.0.1:11434", Duration::from_secs(1))
            .expect("provider");
        assert_eq!(ollama.adapter_id(), "ollama-provider");
        assert_eq!(ollama.adapter_version(), "1.0.0");
    }
}
