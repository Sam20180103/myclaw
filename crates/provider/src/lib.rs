use async_trait::async_trait;
use futures_util::Stream;
use rustclaw_protocol::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError>;
    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream;
    async fn list_models(&self) -> Result<Vec<String>, ProviderError>;
}

pub struct ProviderManager {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    default_provider: Option<String>,
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_provider: None,
        }
    }

    pub fn register(&mut self, name: String, provider: Arc<dyn LlmProvider>) {
        if self.default_provider.is_none() {
            self.default_provider = Some(name.clone());
        }
        self.providers.insert(name, provider);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(name).cloned()
    }

    pub fn get_default(&self) -> Option<Arc<dyn LlmProvider>> {
        self.default_provider
            .as_ref()
            .and_then(|n| self.providers.get(n).cloned())
    }
}

impl Default for ProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API request failed: {0}")]
    ApiError(String),
    #[error("Authentication failed")]
    AuthError,
    #[error("Rate limited")]
    RateLimited,
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: &str) -> Self {
        Self {
            api_key,
            base_url: "https://api.openai.com".to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.to_string();
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageResponse,
}

#[derive(Deserialize)]
struct OpenAiMessageResponse {
    content: String,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    rustclaw_protocol::Role::User => "user",
                    rustclaw_protocol::Role::Assistant => "assistant",
                    rustclaw_protocol::Role::System => "system",
                };
                let content = match &m.content {
                    rustclaw_protocol::Content::Text(s) => s.clone(),
                    rustclaw_protocol::Content::Structured { text, .. } => {
                        text.clone().unwrap_or_default()
                    }
                };
                OpenAiMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
        };

        let url = format!("{}/v1/chat/completions", self.base_url);
        
        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!("Status {}: {}", status, body)));
        }

        let response: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))?;

        response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| ProviderError::ParseError("No choices in response".to_string()))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        use async_stream::stream;
        use futures_util::stream::StreamExt;
        
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    rustclaw_protocol::Role::User => "user",
                    rustclaw_protocol::Role::Assistant => "assistant",
                    rustclaw_protocol::Role::System => "system",
                };
                let content = match &m.content {
                    rustclaw_protocol::Content::Text(s) => s.clone(),
                    rustclaw_protocol::Content::Structured { text, .. } => {
                        text.clone().unwrap_or_default()
                    }
                };
                OpenAiMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
        };

        let url = format!("{}/v1/chat/completions", self.base_url);
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        
        Box::pin(stream! {
            let result = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await;

            match result {
                Ok(response) => {
                    if !response.status().is_success() {
                        yield Err(ProviderError::ApiError(format!(
                            "Status {}",
                            response.status()
                        )));
                        return;
                    }

                    let mut stream = response.bytes_stream();
                    let mut buffer = String::new();
                    
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(bytes) => {
                                if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                                    buffer.push_str(&text);
                                    
                                    let lines: Vec<&str> = buffer.split('\n').collect();
                                    for line in lines.iter().take(lines.len() - 1) {
                                        let line = line.trim();
                                        if line.starts_with("data: ") {
                                            let data = &line[6..];
                                            if data == "[DONE]" {
                                                return;
                                            }
                                            
                                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                                if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                                                    if let Some(choice) = choices.first() {
                                                        if let Some(delta) = choice.get("delta") {
                                                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                                                yield Ok(content.to_string());
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    
                                    if let Some(last) = lines.last() {
                                        buffer = last.to_string();
                                    }
                                }
                            }
                            Err(e) => {
                                yield Err(ProviderError::ApiError(e.to_string()));
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(ProviderError::ApiError(e.to_string()));
                }
            }
        })
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let url = format!("{}/v1/models", self.base_url);
        
        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(e.to_string()))?;

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelData>,
        }
        
        #[derive(Deserialize)]
        struct ModelData {
            id: String,
        }

        let response: ModelsResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))?;

        Ok(response.data.into_iter().map(|m| m.id).collect())
    }
}

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: &str) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.to_string();
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct AnthropicStreamRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    text: Option<String>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        let anthropic_messages: Vec<AnthropicMessage> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    rustclaw_protocol::Role::User => "user",
                    rustclaw_protocol::Role::Assistant => "assistant",
                    rustclaw_protocol::Role::System => "system",
                };
                let content = match &m.content {
                    rustclaw_protocol::Content::Text(s) => s.clone(),
                    rustclaw_protocol::Content::Structured { text, .. } => {
                        text.clone().unwrap_or_default()
                    }
                };
                AnthropicMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect();

        let request = AnthropicRequest {
            model: self.model.clone(),
            messages: anthropic_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let url = format!("{}/v1/messages", self.base_url);
        
        let response = self.client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!("Status {}: {}", status, body)));
        }

        let response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))?;

        response
            .content
            .first()
            .and_then(|c| c.text.clone())
            .ok_or_else(|| ProviderError::ParseError("No content in response".to_string()))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        use async_stream::stream;
        use futures_util::stream::StreamExt;

        let anthropic_messages: Vec<AnthropicMessage> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    rustclaw_protocol::Role::User => "user",
                    rustclaw_protocol::Role::Assistant => "assistant",
                    rustclaw_protocol::Role::System => "system",
                };
                let content = match &m.content {
                    rustclaw_protocol::Content::Text(s) => s.clone(),
                    rustclaw_protocol::Content::Structured { text, .. } => {
                        text.clone().unwrap_or_default()
                    }
                };
                AnthropicMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect();

        let request = AnthropicStreamRequest {
            model: self.model.clone(),
            messages: anthropic_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
        };

        let url = format!("{}/v1/messages", self.base_url);
        let client = self.client.clone();
        let api_key = self.api_key.clone();

        Box::pin(stream! {
            let result = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await;

            match result {
                Ok(response) => {
                    if !response.status().is_success() {
                        yield Err(ProviderError::ApiError(format!(
                            "Status {}",
                            response.status()
                        )));
                        return;
                    }

                    let mut stream = response.bytes_stream();
                    let mut buffer = String::new();

                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(bytes) => {
                                if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                                    buffer.push_str(&text);

                                    let lines: Vec<&str> = buffer.split('\n').collect();
                                    for line in lines.iter().take(lines.len() - 1) {
                                        let line = line.trim();
                                        if line.starts_with("data: ") {
                                            let data = &line[6..];

                                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                                if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
                                                    for block in content {
                                                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                                            yield Ok(text.to_string());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if let Some(last) = lines.last() {
                                        buffer = last.to_string();
                                    }
                                }
                            }
                            Err(e) => {
                                yield Err(ProviderError::ApiError(e.to_string()));
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(ProviderError::ApiError(e.to_string()));
                }
            }
        })
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec![
            "claude-opus-4-20250514".to_string(),
            "claude-3-5-sonnet-20241022".to_string(),
            "claude-3-5-sonnet-20241022".to_string(),
            "claude-3-opus-20240229".to_string(),
            "claude-3-sonnet-20240229".to_string(),
            "claude-3-haiku-20240307".to_string(),
        ])
    }
}

#[derive(Clone)]
pub struct ProviderClient {
    provider: Option<Arc<dyn LlmProvider>>,
}

impl ProviderClient {
    pub fn new(provider: Option<Arc<dyn LlmProvider>>) -> Self {
        Self { provider }
    }

    pub async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        if let Some(ref provider) = self.provider {
            provider.complete(messages).await
        } else {
            Ok("Provider not configured".to_string())
        }
    }

    pub async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        if let Some(ref provider) = self.provider {
            provider.stream_complete(messages).await
        } else {
            Box::pin(futures_util::stream::iter(vec![Ok(
                "Provider not configured".to_string()
            )]))
        }
    }
}
