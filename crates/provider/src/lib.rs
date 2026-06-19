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
    async fn complete_with_tools(&self, _messages: Vec<Message>, _tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> { Err(ProviderError::ApiError("Not implemented".to_string())) }
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
    /// String for plain text, or an array of content parts (text + image_url) for vision.
    content: serde_json::Value,
    /// C1: assistant 请求的原生 tool_calls
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<serde_json::Value>>,
    /// C1: tool 角色消息对应的调用 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// C1: 将内部 Message 映射为 OpenAI 消息,正确处理 tool_calls / tool 角色。
fn openai_message(m: &Message) -> OpenAiMessage {
    use rustclaw_protocol::Content;
    match &m.content {
        Content::ToolUse { tool_calls, text } => OpenAiMessage {
            role: "assistant".to_string(),
            content: serde_json::Value::String(text.clone().unwrap_or_default()),
            tool_calls: Some(
                tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                // OpenAI 要求 arguments 为 JSON 字符串
                                "arguments": tc.arguments.to_string(),
                            }
                        })
                    })
                    .collect(),
            ),
            tool_call_id: None,
        },
        Content::ToolResult { tool_call_id, output, .. } => OpenAiMessage {
            role: "tool".to_string(),
            content: serde_json::Value::String(output.clone()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.clone()),
        },
        other => {
            let role = match m.role {
                rustclaw_protocol::Role::User => "user",
                rustclaw_protocol::Role::Assistant => "assistant",
                rustclaw_protocol::Role::System => "system",
                rustclaw_protocol::Role::Tool => "tool",
            };
            OpenAiMessage {
                role: role.to_string(),
                content: openai_content(other),
                tool_calls: None,
                tool_call_id: None,
            }
        }
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
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
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(serde::Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    call_type: String,
    function: OpenAiFunction,
}

#[derive(serde::Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

/// 归一化图片格式为标准 MIME 后缀 (jpg -> jpeg)。
fn normalize_image_format(fmt: &str) -> String {
    match fmt.to_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg".to_string(),
        "" => "png".to_string(),
        other => other.to_string(),
    }
}

/// 把 image_data 转为可用于 OpenAI image_url 的 URL。
/// 已是 http(s):// 或 data: 开头则原样使用,否则视为 base64 构造 data URL。
fn image_data_url(image_data: &str, image_type: &str) -> String {
    if image_data.starts_with("http://")
        || image_data.starts_with("https://")
        || image_data.starts_with("data:")
    {
        image_data.to_string()
    } else {
        format!("data:image/{};base64,{}", normalize_image_format(image_type), image_data)
    }
}

/// 将内部 Content 转换为 OpenAI 消息 content (字符串或多模态数组)。
fn openai_content(content: &rustclaw_protocol::Content) -> serde_json::Value {
    use rustclaw_protocol::Content;
    match content {
        Content::Text(s) => serde_json::Value::String(s.clone()),
        Content::Structured { text, .. } => {
            serde_json::Value::String(text.clone().unwrap_or_default())
        }
        Content::Image { image_data, image_type, description } => {
            let mut parts = Vec::new();
            if let Some(desc) = description {
                if !desc.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": desc }));
                }
            }
            parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": image_data_url(image_data, image_type) }
            }));
            serde_json::Value::Array(parts)
        }
        // 工具语义由 openai_message 处理;此处仅作降级文本兜底。
        Content::ToolUse { text, .. } => {
            serde_json::Value::String(text.clone().unwrap_or_default())
        }
        Content::ToolResult { output, .. } => serde_json::Value::String(output.clone()),
    }
}

/// 将内部 Content 转换为 Anthropic 消息 content (字符串或内容块数组)。
fn anthropic_content(content: &rustclaw_protocol::Content) -> serde_json::Value {
    use rustclaw_protocol::Content;
    match content {
        Content::Text(s) => serde_json::Value::String(s.clone()),
        Content::Structured { text, .. } => {
            serde_json::Value::String(text.clone().unwrap_or_default())
        }
        Content::Image { image_data, image_type, description } => {
            // Anthropic 暂不支持远程 URL 图片,仅对 base64 走视觉块,否则降级为文本。
            if image_data.starts_with("http://") || image_data.starts_with("https://") {
                return serde_json::Value::String(format!(
                    "[Image: {}]",
                    description.as_deref().unwrap_or(image_data)
                ));
            }
            // data:image/png;base64,XXXX -> 取逗号后的纯 base64
            let data = if image_data.starts_with("data:") {
                image_data.rsplit(',').next().unwrap_or(image_data)
            } else {
                image_data.as_str()
            };
            let mut parts = Vec::new();
            if let Some(desc) = description {
                if !desc.is_empty() {
                    parts.push(serde_json::json!({ "type": "text", "text": desc }));
                }
            }
            parts.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": format!("image/{}", normalize_image_format(image_type)),
                    "data": data
                }
            }));
            serde_json::Value::Array(parts)
        }
        // 工具语义由 anthropic_message 处理;此处仅作降级文本兜底。
        Content::ToolUse { text, .. } => {
            serde_json::Value::String(text.clone().unwrap_or_default())
        }
        Content::ToolResult { output, .. } => serde_json::Value::String(output.clone()),
    }
}

/// C1: 将内部 Message 映射为 Anthropic 消息,正确处理 tool_use / tool_result 块。
fn anthropic_message(m: &Message) -> AnthropicMessage {
    use rustclaw_protocol::Content;
    match &m.content {
        Content::ToolUse { tool_calls, text } => {
            let mut blocks = Vec::new();
            if let Some(t) = text {
                if !t.is_empty() {
                    blocks.push(serde_json::json!({ "type": "text", "text": t }));
                }
            }
            for tc in tool_calls {
                blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.arguments,
                }));
            }
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::Value::Array(blocks),
            }
        }
        Content::ToolResult { tool_call_id, output, .. } => AnthropicMessage {
            // Anthropic 把 tool_result 放在 user 回合
            role: "user".to_string(),
            content: serde_json::json!([{
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": output,
            }]),
        },
        other => {
            let role = match m.role {
                rustclaw_protocol::Role::Assistant => "assistant",
                // Anthropic messages 数组无 system/tool 角色,统一降级为 user
                _ => "user",
            };
            AnthropicMessage {
                role: role.to_string(),
                content: anthropic_content(other),
            }
        }
    }
}

fn extract_tool_calls(response: &OpenAiResponse) -> Vec<serde_json::Value> {
    let mut result = Vec::new();
    if let Some(choice) = response.choices.first() {
        if let Some(ref tcs) = choice.message.tool_calls {
            for tc in tcs {
                let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));
                result.push(serde_json::json!({
                    "id": tc.id,
                    "name": tc.function.name,
                    "arguments": args
                }));
            }
        }
    }
    result
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
            tools: None,
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);
        
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
            .and_then(|c| c.message.content.clone())
            .ok_or_else(|| ProviderError::ParseError("No choices in response".to_string()))
    }


    async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
            tools: Some(tools),
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);
        
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

        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let tool_calls = extract_tool_calls(&response);
        Ok((content, tool_calls))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        use async_stream::stream;
        use futures_util::stream::StreamExt;
        
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
            tools: None,
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);
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
                                        if let Some(data) = line.strip_prefix("data: ") {
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
    /// String for plain text, or an array of content blocks (text + image) for vision.
    content: serde_json::Value,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
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
            .map(anthropic_message)
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
            .header("anthropic-version", "2024-06-01")
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

    async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        let anthropic_messages: Vec<AnthropicMessage> = messages
            .iter()
            .map(anthropic_message)
            .collect();

        // Convert OpenAI-style tool schema to Anthropic-style tool schema
        let anthropic_tools: Vec<serde_json::Value> = tools.into_iter().map(|t| {
            if let Some(func) = t.get("function") {
                serde_json::json!({
                    "name": func.get("name"),
                    "description": func.get("description"),
                    "input_schema": func.get("parameters")
                })
            } else {
                t
            }
        }).collect();

        let request = serde_json::json!({
            "model": self.model.clone(),
            "messages": anthropic_messages,
            "max_tokens": 4096,
            "temperature": 0.7,
            "tools": anthropic_tools
        });

        let url = format!("{}/v1/messages", self.base_url);
        
        let response = self.client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2024-06-01")
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

        let response: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))?;

        let mut content_text = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content_blocks) = response.get("content").and_then(|v| v.as_array()) {
            for block in content_blocks {
                match block.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                            content_text.push_str(text);
                        }
                    }
                    Some("tool_use") => {
                        if let (Some(id), Some(name), Some(input)) = (
                            block.get("id").and_then(|v| v.as_str()),
                            block.get("name").and_then(|v| v.as_str()),
                            block.get("input")
                        ) {
                            tool_calls.push(serde_json::json!({
                                "id": id,
                                "name": name,
                                "arguments": input
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok((content_text, tool_calls))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        use async_stream::stream;
        use futures_util::stream::StreamExt;

        let anthropic_messages: Vec<AnthropicMessage> = messages
            .iter()
            .map(anthropic_message)
            .collect();

        let request = AnthropicStreamRequest {
            model: self.model.clone(),
            messages: anthropic_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
            tools: None,
        };

        let url = format!("{}/v1/messages", self.base_url);
        let client = self.client.clone();
        let api_key = self.api_key.clone();

        Box::pin(stream! {
            let result = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2024-06-01")
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
                                        if let Some(data) = line.strip_prefix("data: ") {
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

pub struct NvProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl NvProvider {
    pub fn new(api_key: String, model: &str) -> Self {
        Self {
            api_key,
            base_url: "https://integrate.api.nvidia.com".to_string(),
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

#[async_trait]
impl LlmProvider for NvProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
            tools: None,
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
        
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

        #[derive(Deserialize)]
        struct NvResponse {
            choices: Vec<NvChoice>,
        }
        
        #[derive(Deserialize)]
        struct NvChoice {
            message: OpenAiMessageResponse,
        }

        let response: NvResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))?;

        response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .ok_or_else(|| ProviderError::ParseError("No choices in response".to_string()))
    }


    async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: false,
            tools: Some(tools),
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
        
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

        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let tool_calls = extract_tool_calls(&response);
        Ok((content, tool_calls))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        use async_stream::stream;
        use futures_util::stream::StreamExt;
        
        let openai_messages: Vec<OpenAiMessage> = messages
            .iter()
            .map(openai_message)
            .collect();

        let request = OpenAiRequest {
            model: self.model.clone(),
            messages: openai_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
            tools: None,
        };

        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
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
                                        if let Some(data) = line.strip_prefix("data: ") {
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
        Ok(vec![
            "meta/llama-3.1-405b-instruct".to_string(),
            "meta/llama-3.1-70b-instruct".to_string(),
            "meta/llama-3.1-8b-instruct".to_string(),
            "mistralai/mixtral-8x22b-instruct-v0.1".to_string(),
            "mistralai/mistral-large".to_string(),
            "google/gemma-2b".to_string(),
            "google/gemma-7b".to_string(),
            "microsoft/phi-3-mini-128k-instruct".to_string(),
            "nvidia/nemotron-4-340b-instruct".to_string(),
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

    pub async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        if let Some(ref provider) = self.provider {
            provider.complete_with_tools(messages, tools).await
        } else {
            Ok(("Provider not configured".to_string(), vec![]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_manager_register_and_get() {
        let mut manager = ProviderManager::new();
        let provider = Arc::new(OpenAiProvider::new("test-key".to_string(), "gpt-4"));
        manager.register("openai".to_string(), provider.clone());
        assert!(manager.get("openai").is_some());
        assert!(manager.get("nonexistent").is_none());
    }

    #[test]
    fn test_provider_manager_default() {
        let mut manager = ProviderManager::new();
        assert!(manager.get_default().is_none());
        let provider = Arc::new(OpenAiProvider::new("test-key".to_string(), "gpt-4"));
        manager.register("openai".to_string(), provider);
        assert!(manager.get_default().is_some());
    }

    #[test]
    fn test_openai_provider_construction() {
        let provider = OpenAiProvider::new("key".to_string(), "gpt-4");
        assert_eq!(provider.model, "gpt-4");
    }

    #[test]
    fn test_openai_provider_with_base_url() {
        let provider = OpenAiProvider::new("key".to_string(), "gpt-4")
            .with_base_url("https://custom.api.com");
        assert_eq!(provider.base_url, "https://custom.api.com");
    }

    #[test]
    fn test_openai_provider_with_model() {
        let provider = OpenAiProvider::new("key".to_string(), "gpt-3.5")
            .with_model("gpt-4");
        assert_eq!(provider.model, "gpt-4");
    }

    #[test]
    fn test_anthropic_provider_construction() {
        let provider = AnthropicProvider::new("key".to_string(), "claude-3-5-sonnet-20241022");
        assert_eq!(provider.model, "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn test_anthropic_provider_list_models() {
        let provider = AnthropicProvider::new("key".to_string(), "claude-3-5-sonnet-20241022");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let models = rt.block_on(provider.list_models()).unwrap();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.contains("claude")));
    }

    #[test]
    fn test_provider_client_no_provider() {
        let client = ProviderClient::new(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(client.complete(vec![]));
        assert_eq!(result.unwrap(), "Provider not configured");
    }

    #[test]
    fn test_nv_provider_construction() {
        let provider = NvProvider::new("key".to_string(), "meta/llama-3.1-70b-instruct");
        assert_eq!(provider.model, "meta/llama-3.1-70b-instruct");
        assert_eq!(provider.base_url, "https://integrate.api.nvidia.com");
    }

    #[test]
    fn test_nv_provider_with_base_url() {
        let provider = NvProvider::new("key".to_string(), "meta/llama-3.1-70b-instruct")
            .with_base_url("https://custom.nvidia.com");
        assert_eq!(provider.base_url, "https://custom.nvidia.com");
    }

    #[test]
    fn test_nv_provider_with_model() {
        let provider = NvProvider::new("key".to_string(), "meta/llama-3.1-8b-instruct")
            .with_model("meta/llama-3.1-405b-instruct");
        assert_eq!(provider.model, "meta/llama-3.1-405b-instruct");
    }

    #[test]
    fn test_nv_provider_list_models() {
        let provider = NvProvider::new("key".to_string(), "meta/llama-3.1-70b-instruct");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let models = rt.block_on(provider.list_models()).unwrap();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.contains("llama")));
    }

    #[test]
    fn test_openai_content_text_is_string() {
        let v = openai_content(&rustclaw_protocol::Content::Text("hello".into()));
        assert_eq!(v, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn test_openai_content_image_is_multimodal_array() {
        let c = rustclaw_protocol::Content::image_with_description("BASE64DATA", "png", "a cat");
        let v = openai_content(&c);
        let arr = v.as_array().expect("image content should be an array");
        // 含一个文本块 + 一个 image_url 块
        assert!(arr.iter().any(|p| p["type"] == "text" && p["text"] == "a cat"));
        let img = arr.iter().find(|p| p["type"] == "image_url").expect("image_url part");
        assert_eq!(img["image_url"]["url"], "data:image/png;base64,BASE64DATA");
    }

    #[test]
    fn test_anthropic_content_image_uses_base64_source() {
        let c = rustclaw_protocol::Content::image("BASE64DATA", "jpg");
        let v = anthropic_content(&c);
        let arr = v.as_array().expect("image content should be an array");
        let img = arr.iter().find(|p| p["type"] == "image").expect("image block");
        assert_eq!(img["source"]["type"], "base64");
        assert_eq!(img["source"]["media_type"], "image/jpeg"); // jpg -> jpeg
        assert_eq!(img["source"]["data"], "BASE64DATA");
    }

    #[test]
    fn test_anthropic_content_remote_url_downgrades_to_text() {
        let c = rustclaw_protocol::Content::image("https://example.com/a.png", "png");
        let v = anthropic_content(&c);
        // Anthropic 不支持远程 URL,降级为文本
        assert!(v.is_string());
    }
}
