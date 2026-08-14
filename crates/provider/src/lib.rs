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

/// P2-13: 进程级 token 用量统计。
///
/// `Usage` 此前只是个没人写入的结构体 —— 实战跑起来根本不知道烧了多少 token。
/// 这里用全局计数器在各 provider 解析响应时累加,避免改动 `LlmProvider` trait 签名
/// (那会波及 4 个 provider × 2 个方法和全部 mock)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// 按模型细分 (requests, prompt, completion)
    pub by_model: HashMap<String, ModelUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelUsage {
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

static USAGE: std::sync::OnceLock<std::sync::Mutex<UsageStats>> = std::sync::OnceLock::new();

fn usage_cell() -> &'static std::sync::Mutex<UsageStats> {
    USAGE.get_or_init(|| std::sync::Mutex::new(UsageStats::default()))
}

/// P2-13: 记录一次 LLM 调用的 token 用量。
pub fn record_usage(model: &str, prompt_tokens: u32, completion_tokens: u32) {
    if let Ok(mut u) = usage_cell().lock() {
        u.requests += 1;
        u.prompt_tokens += prompt_tokens as u64;
        u.completion_tokens += completion_tokens as u64;
        u.total_tokens += (prompt_tokens + completion_tokens) as u64;
        let e = u.by_model.entry(model.to_string()).or_default();
        e.requests += 1;
        e.prompt_tokens += prompt_tokens as u64;
        e.completion_tokens += completion_tokens as u64;
    }
}

/// P2-13: 取当前累计用量快照。
pub fn usage_snapshot() -> UsageStats {
    usage_cell().lock().map(|u| u.clone()).unwrap_or_default()
}

/// OpenAI 兼容响应里的 usage 段
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApiUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    /// Anthropic 用 input_tokens/output_tokens
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

impl ApiUsage {
    fn prompt(&self) -> u32 {
        if self.prompt_tokens > 0 { self.prompt_tokens } else { self.input_tokens }
    }
    fn completion(&self) -> u32 {
        if self.completion_tokens > 0 { self.completion_tokens } else { self.output_tokens }
    }
    /// 记录到全局统计 (缺 usage 段的响应不计)
    fn record(&self, model: &str) {
        if self.prompt() > 0 || self.completion() > 0 {
            record_usage(model, self.prompt(), self.completion());
        }
    }
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
    /// P2-13
    #[serde(default)]
    usage: Option<ApiUsage>,
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

/// G2: 将前导 System 消息提升为 Anthropic 顶层 `system` 块数组 (并在最后一块打
/// `cache_control` 缓存断点),其余消息走 `anthropic_message`。
///
/// 这同时修正了"System 角色被降级成 user 消息"的旧问题。返回 (system_blocks, messages)。
/// 注意:缓存前缀 = system + tools;只要单次请求内 system (含 memory) 不变,
/// 工具循环的多次迭代即可命中缓存 (成本主要来源)。memory 被工具改写时该轮失效。
fn anthropic_system_blocks(messages: &[Message]) -> (Vec<serde_json::Value>, Vec<AnthropicMessage>) {
    use rustclaw_protocol::{Content, Role};
    let mut system_blocks: Vec<serde_json::Value> = Vec::new();
    let mut rest: Vec<AnthropicMessage> = Vec::new();
    let mut in_prefix = true;
    for m in messages {
        let system_text = if in_prefix && m.role == Role::System {
            match &m.content {
                Content::Text(s) => Some(s.clone()),
                Content::Structured { text, .. } => Some(text.clone().unwrap_or_default()),
                _ => None,
            }
        } else {
            None
        };
        match system_text {
            Some(text) => system_blocks.push(serde_json::json!({ "type": "text", "text": text })),
            None => {
                in_prefix = false;
                rest.push(anthropic_message(m));
            }
        }
    }
    if let Some(last) = system_blocks.last_mut() {
        last["cache_control"] = serde_json::json!({ "type": "ephemeral" });
    }
    (system_blocks, rest)
}

/// G2: 在工具数组最后一项打 `cache_control`,缓存 (system + tools) 这一大块稳定前缀。
fn with_tool_cache(mut tools: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = serde_json::json!({ "type": "ephemeral" });
    }
    tools
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

        // P2-13: 回填 token 用量到进程级统计
        if let Some(ref u) = response.usage {
            u.record(&self.model);
        }

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

        // P2-13: 回填 token 用量到进程级统计
        if let Some(ref u) = response.usage {
            u.record(&self.model);
        }

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
    /// G2: 顶层 system 块 (含 cache_control)
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<serde_json::Value>>,
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
    /// P2-13
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    text: Option<String>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        // G2: 提升 system 前缀 + 缓存断点
        let (system_blocks, anthropic_messages) = anthropic_system_blocks(&messages);

        let request = AnthropicRequest {
            model: self.model.clone(),
            messages: anthropic_messages,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            system: if system_blocks.is_empty() { None } else { Some(system_blocks) },
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

        // P2-13: 回填 token 用量到进程级统计
        if let Some(ref u) = response.usage {
            u.record(&self.model);
        }

        response
            .content
            .first()
            .and_then(|c| c.text.clone())
            .ok_or_else(|| ProviderError::ParseError("No content in response".to_string()))
    }

    async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        // G2: 提升 system 前缀 + 打缓存断点
        let (system_blocks, anthropic_messages) = anthropic_system_blocks(&messages);

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

        let anthropic_tools = with_tool_cache(anthropic_tools);

        let mut request = serde_json::json!({
            "model": self.model.clone(),
            "messages": anthropic_messages,
            "max_tokens": 4096,
            "temperature": 0.7,
            "tools": anthropic_tools
        });
        if !system_blocks.is_empty() {
            request["system"] = serde_json::Value::Array(system_blocks);
        }

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

        // P2-13: 回填 token 用量到进程级统计
        if let Some(ref u) = response.usage {
            u.record(&self.model);
        }

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

// ---------------------------------------------------------------------------
// G12: OpenAI Responses API 形态 (/v1/responses)
// ---------------------------------------------------------------------------

/// G12: OpenAI Responses API provider — 与 Chat Completions 并列的第二种 OpenAI 形态。
/// 请求为 `input` 条目数组 (工具调用/结果是顶层条目而非消息字段),工具 schema 为扁平结构。
pub struct OpenAiResponsesProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiResponsesProvider {
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

    async fn call(&self, body: serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        let url = format!("{}/v1/responses", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!("Status {}: {}", status, text)));
        }
        response
            .json()
            .await
            .map_err(|e| ProviderError::ParseError(e.to_string()))
    }
}

/// G12: 内部 Message → Responses `input` 条目数组。
/// ToolUse/ToolResult 映射为顶层 `function_call` / `function_call_output` 条目。
fn responses_input(messages: &[Message]) -> Vec<serde_json::Value> {
    use rustclaw_protocol::{Content, Role};
    let mut items = Vec::new();
    for m in messages {
        match &m.content {
            Content::ToolUse { tool_calls, text } => {
                if let Some(t) = text.as_deref().filter(|t| !t.is_empty()) {
                    items.push(serde_json::json!({ "role": "assistant", "content": t }));
                }
                for tc in tool_calls {
                    items.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": tc.id,
                        "name": tc.name,
                        // Responses 与 Chat Completions 一致: arguments 为 JSON 字符串
                        "arguments": tc.arguments.to_string(),
                    }));
                }
            }
            Content::ToolResult { tool_call_id, output, .. } => {
                items.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": output,
                }));
            }
            Content::Image { image_data, image_type, description } => {
                let mut parts = Vec::new();
                if let Some(d) = description.as_deref().filter(|d| !d.is_empty()) {
                    parts.push(serde_json::json!({ "type": "input_text", "text": d }));
                }
                parts.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": image_data_url(image_data, image_type),
                }));
                items.push(serde_json::json!({ "role": "user", "content": parts }));
            }
            other => {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "user", // 兜底 (正常路径走 ToolResult 分支)
                };
                let text = match other {
                    Content::Text(t) => t.clone(),
                    Content::Structured { text, .. } => text.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                items.push(serde_json::json!({ "role": role, "content": text }));
            }
        }
    }
    items
}

/// G12: OpenAI 风格 (嵌套 function) 工具 schema → Responses 扁平工具 schema。
fn responses_tools(tools: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    tools
        .into_iter()
        .map(|t| {
            if let Some(func) = t.get("function") {
                serde_json::json!({
                    "type": "function",
                    "name": func.get("name"),
                    "description": func.get("description"),
                    "parameters": func.get("parameters"),
                })
            } else {
                t
            }
        })
        .collect()
}

/// G12: 解析 Responses `output` 数组 → (拼接文本, tool_calls[{id,name,arguments}])。
fn parse_responses_output(response: &serde_json::Value) -> (String, Vec<serde_json::Value>) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    if let Some(output) = response.get("output").and_then(|v| v.as_array()) {
        for item in output {
            match item.get("type").and_then(|v| v.as_str()) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                        for p in parts {
                            if p.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                    text.push_str(t);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let args_str = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                    let args: serde_json::Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    tool_calls.push(serde_json::json!({
                        "id": item.get("call_id").and_then(|v| v.as_str()).unwrap_or(""),
                        "name": item.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "arguments": args,
                    }));
                }
                _ => {}
            }
        }
    }
    (text, tool_calls)
}

#[async_trait]
impl LlmProvider for OpenAiResponsesProvider {
    async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        let body = serde_json::json!({
            "model": self.model,
            "input": responses_input(&messages),
            "max_output_tokens": 4096,
        });
        let response = self.call(body).await?;
        let (text, _) = parse_responses_output(&response);
        Ok(text)
    }

    async fn complete_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Vec<serde_json::Value>,
    ) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        let body = serde_json::json!({
            "model": self.model,
            "input": responses_input(&messages),
            "max_output_tokens": 4096,
            "tools": responses_tools(tools),
        });
        let response = self.call(body).await?;
        Ok(parse_responses_output(&response))
    }

    async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        // 最小实现: 非流式取全文后单次吐出 (真 SSE 流式为后续项)
        match self.complete(messages).await {
            Ok(text) => Box::pin(futures_util::stream::iter(vec![Ok(text)])),
            Err(e) => Box::pin(futures_util::stream::iter(vec![Err(e)])),
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec![self.model.clone()])
    }
}

/// G3: 某个错误是否值得重试 (限流 / 5xx / 网络瞬断),区别于鉴权/参数/解析等确定性错误。
fn is_retryable(e: &ProviderError) -> bool {
    match e {
        ProviderError::RateLimited => true,
        // ApiError 既含网络错误也含 "Status 5xx: ..." 文本;5xx 与网络错误可重试,4xx 不重试。
        ProviderError::ApiError(msg) => {
            !(msg.contains("Status 4") || msg.contains("status 4"))
        }
        ProviderError::AuthError | ProviderError::InvalidRequest(_) | ProviderError::ParseError(_) => false,
    }
}

/// 多 Provider 客户端 (G3): 持有有序 provider 列表 (主在前,其余为降级备选);
/// 对可重试错误指数退避重试,耗尽后降级到下一个 provider。
#[derive(Clone)]
pub struct ProviderClient {
    providers: Vec<Arc<dyn LlmProvider>>, // 主 provider 在 index 0,后续为 fallback
    max_retries: u32,                     // 单个 provider 的额外重试次数 (退避)
}

impl ProviderClient {
    /// 向后兼容: 单 provider (或无)。
    pub fn new(provider: Option<Arc<dyn LlmProvider>>) -> Self {
        Self {
            providers: provider.into_iter().collect(),
            max_retries: 2,
        }
    }

    /// G3: 以有序列表构造 (主 + 降级备选)。
    pub fn with_providers(providers: Vec<Arc<dyn LlmProvider>>) -> Self {
        Self {
            providers,
            max_retries: 2,
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 是否已配置至少一个 provider。
    pub fn is_configured(&self) -> bool {
        !self.providers.is_empty()
    }

    /// 退避时长 (指数: 200ms, 400ms, 800ms, 上限 5s)。
    fn backoff(attempt: u32) -> std::time::Duration {
        let ms = (200u64 << attempt.min(5)).min(5000);
        std::time::Duration::from_millis(ms)
    }

    /// 通用执行器: 遍历 provider (降级),每个 provider 内对可重试错误退避重试。
    async fn run<T, F, Fut>(&self, label: &str, mut call: F) -> Result<T, ProviderError>
    where
        F: FnMut(Arc<dyn LlmProvider>) -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        if self.providers.is_empty() {
            return Err(ProviderError::ApiError("No provider configured".to_string()));
        }
        let last_idx = self.providers.len() - 1;
        let mut last_err: Option<ProviderError> = None;
        for (idx, provider) in self.providers.iter().enumerate() {
            for attempt in 0..=self.max_retries {
                match call(provider.clone()).await {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        let retryable = is_retryable(&e);
                        if retryable && attempt < self.max_retries {
                            tracing::warn!(
                                "{}: provider[{}] retryable error (attempt {}): {} — backing off",
                                label, idx, attempt + 1, e
                            );
                            tokio::time::sleep(Self::backoff(attempt)).await;
                            continue;
                        }
                        // 不可重试,或重试已耗尽 → 尝试降级到下一个 provider
                        if idx < last_idx {
                            tracing::warn!(
                                "{}: provider[{}] failed ({}), failing over to next provider",
                                label, idx, e
                            );
                        }
                        last_err = Some(e);
                        break;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::ApiError("All providers failed".to_string())))
    }

    pub async fn complete(&self, messages: Vec<Message>) -> Result<String, ProviderError> {
        if self.providers.is_empty() {
            return Ok("Provider not configured".to_string());
        }
        self.run("complete", |p| {
            let messages = messages.clone();
            async move { p.complete(messages).await }
        })
        .await
    }

    pub async fn complete_with_tools(&self, messages: Vec<Message>, tools: Vec<serde_json::Value>) -> Result<(String, Vec<serde_json::Value>), ProviderError> {
        if self.providers.is_empty() {
            return Ok(("Provider not configured".to_string(), vec![]));
        }
        self.run("complete_with_tools", |p| {
            let messages = messages.clone();
            let tools = tools.clone();
            async move { p.complete_with_tools(messages, tools).await }
        })
        .await
    }

    pub async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream {
        // 流式不做降级/重试 (响应已开始难以回滚);用主 provider。
        if let Some(provider) = self.providers.first() {
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

    // --- G3: 多 Provider 重试/降级 ---

    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FlakyProvider {
        calls: Arc<AtomicUsize>,
        // 返回 Err 的次数 (之后成功);Err 类型由 retryable 决定
        fail_times: usize,
        retryable: bool,
        tag: String,
    }

    #[async_trait]
    impl LlmProvider for FlakyProvider {
        async fn complete(&self, _m: Vec<Message>) -> Result<String, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_times {
                if self.retryable {
                    Err(ProviderError::RateLimited)
                } else {
                    Err(ProviderError::AuthError)
                }
            } else {
                Ok(self.tag.clone())
            }
        }
        async fn stream_complete(&self, _m: Vec<Message>) -> TokenStream {
            Box::pin(futures_util::stream::iter(vec![Ok("x".to_string())]))
        }
        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_provider_client_retries_then_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let p = Arc::new(FlakyProvider {
            calls: calls.clone(),
            fail_times: 2, // 前两次限流,第三次成功
            retryable: true,
            tag: "ok".to_string(),
        });
        let client = ProviderClient::with_providers(vec![p]).with_max_retries(2);
        let r = client.complete(vec![]).await.unwrap();
        assert_eq!(r, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3); // 1 + 2 retries
    }

    #[tokio::test]
    async fn test_provider_client_fails_over_to_fallback() {
        // 主 provider 永远不可重试地失败 → 降级到备选
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let primary = Arc::new(FlakyProvider {
            calls: primary_calls.clone(),
            fail_times: 999,
            retryable: false, // AuthError 不重试,直接降级
            tag: "primary".to_string(),
        });
        let fallback = Arc::new(FlakyProvider {
            calls: fallback_calls.clone(),
            fail_times: 0,
            retryable: true,
            tag: "fallback".to_string(),
        });
        let client = ProviderClient::with_providers(vec![primary, fallback]).with_max_retries(2);
        let r = client.complete(vec![]).await.unwrap();
        assert_eq!(r, "fallback");
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1); // 不重试
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_provider_client_non_retryable_no_retry_single() {
        let calls = Arc::new(AtomicUsize::new(0));
        let p = Arc::new(FlakyProvider {
            calls: calls.clone(),
            fail_times: 1,
            retryable: false,
            tag: "ok".to_string(),
        });
        let client = ProviderClient::with_providers(vec![p]).with_max_retries(3);
        let err = client.complete(vec![]).await;
        assert!(err.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1); // AuthError 不重试
    }

    // --- G12: OpenAI Responses API ---

    #[test]
    fn test_g12_responses_input_mapping() {
        use rustclaw_protocol::{Content, Message, Role, ToolCallSpec};
        let msgs = vec![
            Message { role: Role::System, content: Content::text("sys") },
            Message { role: Role::User, content: Content::text("hi") },
            Message {
                role: Role::Assistant,
                content: Content::tool_use(
                    vec![ToolCallSpec { id: "c1".into(), name: "echo".into(), arguments: serde_json::json!({"x":1}) }],
                    Some("checking".into()),
                ),
            },
            Message { role: Role::Tool, content: Content::tool_result("c1", "echo", "ok") },
        ];
        let items = responses_input(&msgs);
        assert_eq!(items.len(), 5); // sys, user, assistant text, function_call, function_call_output
        assert_eq!(items[0]["role"], "system");
        assert_eq!(items[2]["role"], "assistant");
        assert_eq!(items[3]["type"], "function_call");
        assert_eq!(items[3]["call_id"], "c1");
        assert_eq!(items[3]["arguments"], "{\"x\":1}"); // JSON 字符串
        assert_eq!(items[4]["type"], "function_call_output");
        assert_eq!(items[4]["output"], "ok");
    }

    #[test]
    fn test_g12_responses_tools_flatten() {
        let nested = vec![serde_json::json!({
            "type": "function",
            "function": { "name": "echo", "description": "d", "parameters": {"type":"object"} }
        })];
        let flat = responses_tools(nested);
        assert_eq!(flat[0]["name"], "echo");
        assert_eq!(flat[0]["type"], "function");
        assert!(flat[0].get("function").is_none());
    }

    #[test]
    fn test_g12_parse_responses_output() {
        let response = serde_json::json!({
            "output": [
                { "type": "message", "content": [ { "type": "output_text", "text": "hello " }, { "type": "output_text", "text": "world" } ] },
                { "type": "function_call", "call_id": "c9", "name": "echo", "arguments": "{\"a\":2}" }
            ]
        });
        let (text, calls) = parse_responses_output(&response);
        assert_eq!(text, "hello world");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "c9");
        assert_eq!(calls[0]["arguments"]["a"], 2);
    }

    // --- G2: Anthropic prompt 缓存 ---

    #[test]
    fn test_anthropic_system_hoisting_and_cache_control() {
        use rustclaw_protocol::{Content, Message, Role};
        let msgs = vec![
            Message { role: Role::System, content: Content::text("base prompt") },
            Message { role: Role::System, content: Content::text("Memory:\nfoo") },
            Message { role: Role::User, content: Content::text("hi") },
        ];
        let (system_blocks, rest) = anthropic_system_blocks(&msgs);
        assert_eq!(system_blocks.len(), 2, "two leading system messages hoisted");
        assert_eq!(system_blocks[0]["text"], "base prompt");
        // 仅最后一个 system 块带缓存断点
        assert!(system_blocks[0].get("cache_control").is_none());
        assert_eq!(system_blocks[1]["cache_control"]["type"], "ephemeral");
        // 其余消息保留,user 不进 system
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].role, "user");
    }

    #[test]
    fn test_anthropic_no_system_blocks_when_none() {
        use rustclaw_protocol::{Content, Message, Role};
        let msgs = vec![Message { role: Role::User, content: Content::text("hi") }];
        let (system_blocks, rest) = anthropic_system_blocks(&msgs);
        assert!(system_blocks.is_empty());
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn test_with_tool_cache_marks_last_tool() {
        let tools = vec![
            serde_json::json!({"name": "a"}),
            serde_json::json!({"name": "b"}),
        ];
        let cached = with_tool_cache(tools);
        assert!(cached[0].get("cache_control").is_none());
        assert_eq!(cached[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable(&ProviderError::RateLimited));
        assert!(is_retryable(&ProviderError::ApiError("Status 503: x".into())));
        assert!(!is_retryable(&ProviderError::ApiError("Status 400: x".into())));
        assert!(!is_retryable(&ProviderError::AuthError));
        assert!(!is_retryable(&ProviderError::ParseError("x".into())));
    }

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

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn test_usage_accumulates() {
        // P2-13: 用量必须真的被累加 (此前 Usage 结构体全仓无人写入)
        let before = usage_snapshot();
        record_usage("test-model", 100, 20);
        record_usage("test-model", 50, 10);
        let after = usage_snapshot();

        assert_eq!(after.requests, before.requests + 2);
        assert_eq!(after.prompt_tokens, before.prompt_tokens + 150);
        assert_eq!(after.completion_tokens, before.completion_tokens + 30);
        assert_eq!(after.total_tokens, before.total_tokens + 180);
        let m = after.by_model.get("test-model").expect("per-model breakdown");
        assert!(m.requests >= 2 && m.prompt_tokens >= 150);
    }

    #[test]
    fn test_api_usage_handles_both_shapes() {
        // OpenAI 用 prompt/completion_tokens, Anthropic 用 input/output_tokens
        let openai: ApiUsage =
            serde_json::from_value(serde_json::json!({"prompt_tokens": 7, "completion_tokens": 3}))
                .unwrap();
        assert_eq!((openai.prompt(), openai.completion()), (7, 3));

        let anthropic: ApiUsage =
            serde_json::from_value(serde_json::json!({"input_tokens": 9, "output_tokens": 4}))
                .unwrap();
        assert_eq!((anthropic.prompt(), anthropic.completion()), (9, 4));
    }
}
