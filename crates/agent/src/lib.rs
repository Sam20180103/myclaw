use rustclaw_protocol::{Message, Envelope, Role, Content};
use rustclaw_tool::{ToolRegistry, ToolCall, ToolSchema};
use rustclaw_provider::ProviderClient;
use rustclaw_memory::MemoryManager;
use rustclaw_compression::ContextCompressor;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use tokio::sync::mpsc;
use futures_util::stream::StreamExt;

/// Agent 事件枚举 — 用于回调系统，通知外部 Agent 的运行状态
pub enum AgentEvent {
    /// 工具开始执行
    ToolStart { tool_name: String, args: String },
    /// 工具执行结束
    ToolEnd { tool_name: String, success: bool },
    /// 迭代步骤
    Step { iteration: u32, message_count: usize },
    /// 状态变化 (started/completed/max_iterations/provider_error)
    Status { status: String },
    /// 思考内容
    Thinking { content: String },
    /// Skill 创建建议 (复杂任务完成后触发)
    SkillSuggestion { task_summary: String, tool_call_count: u32 },
    /// 用户纠正记录
    UserCorrection { original: String, corrected: String },
}

/// 事件回调类型 — 用于外部监听 Agent 运行事件
pub type EventCallback = Box<dyn Fn(AgentEvent) + Send + Sync>;

use dashmap::DashMap;

/// Agent 运行时 — 核心执行引擎
/// 负责管理会话、执行 Agent 推理循环、调用工具和 LLM
pub struct AgentRuntime {
    config: AgentConfig,                       // Agent 配置
    session_manager: SessionManager,           // 会话管理器
    lane_router: LaneRouter,                   // 车道路由器 (并发隔离)
    tool_registry: Arc<ToolRegistry>,          // 工具注册表
    provider_client: ProviderClient,           // LLM 提供商客户端
    memory_manager: Option<Arc<MemoryManager>>, // 可选: 记忆管理器
    context_compressor: Option<Arc<ContextCompressor>>, // 可选: 上下文压缩器
    event_callback: Option<Arc<EventCallback>>, // 可选: 事件回调
    iteration_budget: Option<u32>,             // 可选: 迭代预算 (父子Agent共享)
    session_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>, // 会话锁
}

/// Agent 配置
#[derive(Clone)]
pub struct AgentConfig {
    pub max_loop_iterations: u32,   // 最大循环迭代次数
    pub tool_timeout_ms: u64,       // 工具执行超时 (毫秒)
    pub session_ttl_seconds: u64,   // 会话过期时间 (秒)
    pub max_concurrent_per_lane: usize, // 每车道最大并发数
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_loop_iterations: 100,    // 默认最多100次迭代
            tool_timeout_ms: 60000,      // 默认工具超时60秒
            session_ttl_seconds: 3600,   // 默认会话TTL 1小时
            max_concurrent_per_lane: 10, // 默认每车道10并发
        }
    }
}

/// 会话 — 代表一次完整的 Agent 交互
#[derive(Clone)]
pub struct Session {
    pub key: String,                        // 会话唯一标识
    pub lane: String,                       // 所属车道
    pub parent_session_id: Option<String>,  // 父会话ID (谱系追踪)
    pub messages: Arc<Vec<Message>>,        // 消息历史 (Arc优化减少克隆开销)
    pub metadata: Arc<SessionMetadata>,     // 元数据 (Arc优化)
    pub last_accessed: Instant,             // 最后访问时间 (用于TTL清理)
}

/// 会话元数据
#[derive(Clone)]
pub struct SessionMetadata {
    pub channel: String,            // 通道类型 (http, websocket, etc.)
    pub created_at: i64,            // 创建时间戳 (毫秒)
    pub updated_at: i64,            // 更新时间戳 (毫秒)
    pub tool_calls_enabled: bool,   // 是否启用工具调用
}

/// 会话管理器 — 管理 Agent 会话的生命周期
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>, // 所有活跃会话
    #[allow(dead_code)]
    max_sessions: usize,  // 最大会话数
    ttl_seconds: u64,     // 会话过期时间 (秒)
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new(max_sessions: usize, ttl_seconds: u64) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::with_capacity(max_sessions))),
            max_sessions,
            ttl_seconds,
        }
    }

    /// 创建新会话 (生成 UUID 作为 key)
    pub fn create(&self, channel: &str, lane: &str) -> Session {
        let session = Session {
            key: Uuid::new_v4().to_string(),
            lane: lane.to_string(),
            parent_session_id: None,
            messages: Arc::new(Vec::with_capacity(16)),
            metadata: Arc::new(SessionMetadata {
                channel: channel.to_string(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                tool_calls_enabled: true,
            }),
            last_accessed: Instant::now(),
        };
        self.sessions.write().insert(session.key.clone(), session.clone());
        session
    }

    /// 获取会话 (只读)
    pub fn get(&self, key: &str) -> Option<Session> {
        self.sessions.read().get(key).cloned()
    }

    /// 获取或创建会话 (原子操作,避免竞态条件)
    pub fn get_or_create(&self, key: &str, channel: &str, lane: &str) -> Session {
        {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(key) {
                return session.clone();
            }
        }

        let mut sessions = self.sessions.write();
        // Double check after acquiring write lock
        if let Some(session) = sessions.get(key) {
            return session.clone();
        }

        let session = Session {
            key: key.to_string(),
            lane: lane.to_string(),
            parent_session_id: None,
            messages: Arc::new(Vec::with_capacity(16)),
            metadata: Arc::new(SessionMetadata {
                channel: channel.to_string(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                tool_calls_enabled: true,
            }),
            last_accessed: Instant::now(),
        };
        sessions.insert(key.to_string(), session.clone());
        session
    }

    /// 添加消息到会话 (使用 Arc 优化减少克隆开销)
    pub fn add_message(&self, key: &str, message: Message) {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(key) {
            // 克隆 messages Arc -> 新 Vec, 追加消息, 用新 Arc 替换
            let mut messages = (*session.messages).clone();
            messages.push(message);
            session.messages = Arc::new(messages);
            // 克隆 metadata Arc -> 新 Metadata, 更新时间, 用新 Arc 替换
            let mut metadata = (*session.metadata).clone();
            metadata.updated_at = chrono::Utc::now().timestamp_millis();
            session.metadata = Arc::new(metadata);
            session.last_accessed = Instant::now();
        }
    }

    /// 删除会话
    pub fn delete(&self, key: &str) {
        self.sessions.write().remove(key);
    }

    /// 清理过期会话 (两阶段: 先读锁收集, 再写锁删除)
    pub fn cleanup_expired(&self) -> usize {
        let ttl = Duration::from_secs(self.ttl_seconds);
        let now = Instant::now();
        let mut to_remove = vec![];
        
        // 阶段1: 读锁收集过期 key
        {
            let sessions = self.sessions.read();
            for (key, session) in sessions.iter() {
                if now.duration_since(session.last_accessed) > ttl {
                    to_remove.push(key.clone());
                }
            }
        }
        
        // 阶段2: 写锁逐个删除
        for key in &to_remove {
            self.sessions.write().remove(key);
        }
        to_remove.len()
    }

    /// 列出所有会话
    pub fn list(&self) -> Vec<Session> {
        self.sessions.read().values().cloned().collect()
    }

    /// 获取会话数量
    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }
}

pub struct LaneRouter {
    lanes: Arc<RwLock<HashMap<String, Lane>>>,
    #[allow(dead_code)]
    max_concurrent: usize,
}

pub struct Lane {
    pub name: String,
    pub queue_tx: mpsc::Sender<LaneTask>,
}

pub struct LaneTask {
    pub session_key: String,
    pub message: Message,
    pub response_tx: mpsc::Sender<AgentResponse>,
}

impl LaneRouter {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            lanes: Arc::new(RwLock::new(HashMap::with_capacity(10))),
            max_concurrent,
        }
    }

    pub fn route(&self, session_key: &str) -> String {
        let hash = session_key.bytes().fold(0u64, |acc, b| acc.wrapping_add(b as u64));
        format!("lane_{}", hash % 10)
    }

    pub fn get_lane(&self, name: &str) -> Option<Lane> {
        self.lanes.read().get(name).cloned()
    }

    pub fn register_lane(&self, name: String, tx: mpsc::Sender<LaneTask>) {
        self.lanes.write().insert(name.clone(), Lane { name, queue_tx: tx });
    }
}

impl Clone for Lane {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            queue_tx: self.queue_tx.clone(),
        }
    }
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
        let registry = Arc::new(ToolRegistry::new());
        registry.register_all_tools();
        Self {
            config: config.clone(),
            session_manager: SessionManager::new(100, config.session_ttl_seconds),
            lane_router: LaneRouter::new(config.max_concurrent_per_lane),
            tool_registry: registry,
            provider_client: ProviderClient::new(None),
            memory_manager: None,
            context_compressor: None,
            event_callback: None,
            iteration_budget: None,
            session_locks: DashMap::new(),
        }
    }

    pub fn with_provider(mut self, provider: ProviderClient) -> Self {
        self.provider_client = provider;
        self
    }

    pub fn with_tools(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = registry;
        self
    }

    pub fn with_memory(mut self, memory: Arc<MemoryManager>) -> Self {
        memory.load_memory().ok();
        self.memory_manager = Some(memory);
        self
    }

    pub fn with_compressor(mut self, compressor: Arc<ContextCompressor>) -> Self {
        self.context_compressor = Some(compressor);
        self
    }

    pub fn with_event_callback(mut self, callback: EventCallback) -> Self {
        self.event_callback = Some(Arc::new(callback));
        self
    }

    pub fn with_iteration_budget(mut self, budget: u32) -> Self {
        self.iteration_budget = Some(budget);
        self
    }

    pub fn memory_manager(&self) -> Option<&Arc<MemoryManager>> {
        self.memory_manager.as_ref()
    }

    fn emit_event(&self, event: AgentEvent) {
        if let Some(ref cb) = self.event_callback {
            cb(event);
        }
    }

    pub async fn process(&self, envelope: Envelope) -> Result<AgentResponse, AgentError> {
        tracing::debug!("Agent processing envelope: channel={}, session_key={}", 
            envelope.channel, envelope.session_key);
        
        let session_key = &envelope.session_key;
        let lane = self.lane_router.route(session_key);
        
        let _session = self.session_manager.get_or_create(
            session_key,
            &envelope.channel,
            &lane,
        );

        self.session_manager.add_message(session_key, envelope.message);

        let mut messages = self.session_manager.get(session_key)
            .map(|s| (*s.messages).clone())
            .unwrap_or_default();

        if let Some(ref memory) = self.memory_manager {
            let memory_text = memory.get_memory_text();
            if !memory_text.is_empty() {
                messages.insert(0, Message {
                    role: Role::System,
                    content: Content::text(format!("Memory:\n{}", memory_text)),
                });
            }
        }

        if let Some(ref compressor) = self.context_compressor {
            let result = compressor.compress(&messages, 100000);
            if result.compressed {
                messages = result.messages;
            }
        }

        tracing::debug!("Agent about to call LLM with {} messages", messages.len());

        let tool_schemas: Vec<serde_json::Value> = self.tool_registry.list()
            .into_iter()
            .map(|s| serde_json::json!({
                "type": "function",
                "function": {
                    "name": s.name,
                    "description": s.description,
                    "parameters": s.parameters,
                }
            }))
            .collect();

        let (response_text, tool_call_results) = match self.provider_client.complete_with_tools(messages, tool_schemas).await {
            Ok((text, calls)) => {
                tracing::debug!("LLM response received: {} chars", text.len());
                (text, calls)
            },
            Err(e) => {
                tracing::error!("LLM call failed: {}", e);
                return Err(AgentError::ProviderError(e.to_string()));
            },
        };

        let mut tool_calls: Vec<ToolCall> = tool_call_results.into_iter().map(|tc| {
            ToolCall {
                id: tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: tc.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                arguments: tc.get("arguments").cloned().unwrap_or(serde_json::Value::Object(Default::default())),
            }
        }).collect();

        if tool_calls.is_empty() {
            tool_calls = self.parse_tool_calls(&response_text);
        }

        tracing::debug!("Parsed {} tool calls from response", tool_calls.len());

        let assistant_message = Message {
            role: Role::Assistant,
            content: Content::text(response_text.clone()),
        };
        self.session_manager.add_message(session_key, assistant_message.clone());

        Ok(AgentResponse {
            session_key: session_key.to_string(),
            message: assistant_message,
            tool_calls,
        })
    }

    pub async fn process_with_loop(&self, envelope: Envelope) -> Result<AgentResponse, AgentError> {
        let session_key = envelope.session_key.clone();

        // 1. Acquire session lock
        let lock = self.session_locks.entry(session_key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .value()
            .clone();
        
        let _guard = lock.lock().await;

        let lane = self.lane_router.route(&session_key);
        
        self.session_manager.get_or_create(&session_key, &envelope.channel, &lane);
        self.session_manager.add_message(&session_key, envelope.message);
        self.emit_event(AgentEvent::Status { status: "started".to_string() });

        let max_iterations = self.iteration_budget.unwrap_or(self.config.max_loop_iterations);
        let mut iterations = 0;
        let mut total_tool_calls: u32 = 0;

        loop {
            if iterations >= max_iterations {
                if total_tool_calls >= 5 {
                    self.emit_event(AgentEvent::SkillSuggestion {
                        task_summary: "Complex task completed".to_string(),
                        tool_call_count: total_tool_calls,
                    });
                }
                self.emit_event(AgentEvent::Status { status: "max_iterations".to_string() });
                return Err(AgentError::MaxIterationsExceeded);
            }

            self.emit_event(AgentEvent::Step {
                iteration: iterations,
                message_count: self.session_manager.get(&session_key)
                    .map(|s| s.messages.len())
                    .unwrap_or(0),
            });

            let messages = self.session_manager.get(&session_key)
                .map(|s| (*s.messages).clone())
                .unwrap_or_default();

            let tool_schemas: Vec<serde_json::Value> = self.tool_registry.list()
                .into_iter()
                .map(|s| serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": s.name,
                        "description": s.description,
                        "parameters": s.parameters,
                    }
                }))
                .collect();

            let (response_text, tool_call_results) = match self.provider_client.complete_with_tools(messages, tool_schemas).await {
                Ok((text, tool_calls)) => (text, tool_calls),
                Err(e) => {
                    self.emit_event(AgentEvent::Status { status: "provider_error".to_string() });
                    return Err(AgentError::ProviderError(e.to_string()));
                }
            };

            let mut tool_calls: Vec<ToolCall> = tool_call_results.into_iter().map(|tc| {
                ToolCall {
                    id: tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    name: tc.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    arguments: tc.get("arguments").cloned().unwrap_or(serde_json::Value::Object(Default::default())),
                }
            }).collect();

            if tool_calls.is_empty() {
                tool_calls = self.parse_tool_calls(&response_text);
            }

            if tool_calls.is_empty() {
                if total_tool_calls >= 5 {
                    self.emit_event(AgentEvent::SkillSuggestion {
                        task_summary: "Complex task completed".to_string(),
                        tool_call_count: total_tool_calls,
                    });
                }
                self.emit_event(AgentEvent::Status { status: "completed".to_string() });
                
                let assistant_message = Message {
                    role: Role::Assistant,
                    content: Content::text(response_text.clone()),
                };
                self.session_manager.add_message(&session_key, assistant_message.clone());

                return Ok(AgentResponse {
                    session_key,
                    message: assistant_message,
                    tool_calls: vec![],
                });
            }

            for tool_call in &tool_calls {
                total_tool_calls += 1;
                self.emit_event(AgentEvent::ToolStart {
                    tool_name: tool_call.name.clone(),
                    args: tool_call.arguments.to_string(),
                });

                match self.execute_tool(tool_call).await {
                    Ok(result) => {
                        self.emit_event(AgentEvent::ToolEnd {
                            tool_name: tool_call.name.clone(),
                            success: true,
                        });
                        let result_message = Message {
                            role: Role::System,
                            content: Content::text(format!(
                                "Tool {} result: {}",
                                tool_call.name,
                                result
                            )),
                        };
                        self.session_manager.add_message(&session_key, result_message);
                    }
                    Err(e) => {
                        self.emit_event(AgentEvent::ToolEnd {
                            tool_name: tool_call.name.clone(),
                            success: false,
                        });
                        let error_message = Message {
                            role: Role::System,
                            content: Content::text(format!(
                                "Tool {} error: {}",
                                tool_call.name,
                                e
                            )),
                        };
                        self.session_manager.add_message(&session_key, error_message);
                    }
                }
            }

            iterations += 1;
        }
    }

    fn parse_tool_calls(&self, response: &str) -> Vec<ToolCall> {
        let mut tool_calls = Vec::new();

        // 1. Try to parse the entire response as JSON (native tool calls format)
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(response) {
            if let Some(calls) = json.get("tool_calls").and_then(|v| v.as_array()) {
                for c in calls {
                    if let Some(func) = c.get("function") {
                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                            tool_calls.push(ToolCall {
                                id: c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                name: name.to_string(),
                                arguments: func.get("arguments").cloned().unwrap_or(serde_json::Value::Null),
                            });
                        }
                    }
                }
                if !tool_calls.is_empty() {
                    return tool_calls;
                }
            }
        }

        // 2. Look for XML format <invoke name="tool_name">JSON_ARGS</invoke>
        // Regex supports optional closing tag and captures content in between
        let re = regex::Regex::new(r#"(?s)<invoke name="([^"]+)">\s*(.*?)\s*(?:</invoke>|$)"#).ok();
        if let Some(re) = re {
            for cap in re.captures_iter(response) {
                let name = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
                let args_text = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                
                let arguments = if args_text.is_empty() {
                    serde_json::Value::Object(Default::default())
                } else {
                    serde_json::from_str(args_text).unwrap_or_else(|_| {
                        // If it's not valid JSON, it might be inside a code block or just messy
                        if let Some(start) = args_text.find('{') {
                            if let Some(end) = args_text.rfind('}') {
                                serde_json::from_str(&args_text[start..=end]).unwrap_or(serde_json::Value::Null)
                            } else {
                                serde_json::Value::Null
                            }
                        } else {
                            serde_json::Value::Null
                        }
                    })
                };

                tool_calls.push(ToolCall {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    arguments,
                });
            }
        }

        // 3. Look for plain JSON tool_calls array anywhere in the text if no tools found yet
        if tool_calls.is_empty() {
            if let Some(start) = response.find(r#""tool_calls""#) {
                if let Some(json_start) = response[start..].find('[') {
                    let part = &response[start + json_start..];
                    // Try to find matching closing bracket
                    let mut balance = 0;
                    let mut end_pos = 0;
                    for (i, c) in part.char_indices() {
                        if c == '[' { balance += 1; }
                        else if c == ']' { 
                            balance -= 1; 
                            if balance == 0 {
                                end_pos = i + 1;
                                break;
                            }
                        }
                    }
                    if end_pos > 0 {
                        if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(&part[..end_pos]) {
                            for c in calls {
                                if let Some(func) = c.get("function") {
                                    if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                        tool_calls.push(ToolCall {
                                            id: c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                            name: name.to_string(),
                                            arguments: func.get("arguments").cloned().unwrap_or(serde_json::Value::Null),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 4. Look for simple {"name": "...", "arguments": "..."} or {"name": "...", "parameters": "..."} anywhere in the text
        if tool_calls.is_empty() {
            let formats = [
                (r#""name""#, r#""arguments""#),
                (r#""name""#, r#""parameters""#),
                (r#""tool_name""#, r#""arguments""#),
            ];
            for (name_key, arg_key) in formats {
                if let Some(start) = response.find(name_key) {
                    if response.find(arg_key).is_some() {
                        // Try to find the surrounding object safely by looking for '{' backwards
                        let mut object_start = start;
                        let bytes = response.as_bytes();
                        while object_start > 0 {
                            if bytes[object_start] == b'{' {
                                break;
                            }
                            object_start -= 1;
                        }
                        
                        // Ensure we are at a valid UTF-8 boundary before slicing
                        while object_start > 0 && !response.is_char_boundary(object_start) {
                            object_start -= 1;
                        }
                        
                        // We also need to skip leading text before '{'
                        while object_start < start && bytes[object_start] != b'{' {
                            object_start += 1;
                        }
                        
                        if bytes[object_start] != b'{' {
                            continue;
                        }
                        
                        let part = &response[object_start..];
                        let mut balance = 0;
                        let mut end_pos = 0;
                        for (i, c) in part.char_indices() {
                            if c == '{' { balance += 1; }
                            else if c == '}' { 
                                balance -= 1; 
                                if balance == 0 {
                                    end_pos = i + 1;
                                    break;
                                }
                            }
                        }
                        
                        if end_pos > 0 {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&part[..end_pos]) {
                                let name = json.get("name").or_else(|| json.get("tool_name"))
                                    .and_then(|v| v.as_str());
                                let args = json.get("arguments").or_else(|| json.get("parameters")).cloned();
                                
                                if let (Some(name), Some(args)) = (name, args) {
                                    let arguments = if let Some(s) = args.as_str() {
                                        serde_json::from_str(s).unwrap_or(args)
                                    } else {
                                        args
                                    };
                                    
                                    tool_calls.push(ToolCall {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        name: name.to_string(),
                                        arguments,
                                    });
                                    return tool_calls;
                                }
                            }
                        }
                    }
                }
            }
        }

        tool_calls
    }

    async fn execute_tool(&self, tool_call: &ToolCall) -> Result<String, AgentError> {
        let tool = self.tool_registry.get(&tool_call.name)
            .ok_or_else(|| AgentError::ToolError(format!("Tool not found: {}", tool_call.name)))?;

        let result = tool.execute(tool_call.arguments.clone()).await
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        Ok(result.to_string())
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    pub fn get_session(&self, key: &str) -> Option<Session> {
        self.session_manager.get(key)
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        self.session_manager.list()
    }

    pub fn delete_session(&self, key: &str) {
        self.session_manager.delete(key);
    }

    pub async fn process_stream(
        &self,
        envelope: Envelope,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<String, AgentError>>, AgentError> {
        let session_key = envelope.session_key.clone();
        let lane = self.lane_router.route(&session_key);
        
        self.session_manager.get_or_create(
            &session_key,
            &envelope.channel,
            &lane,
        );

        self.session_manager.add_message(&session_key, envelope.message);

        let messages = self.session_manager.get(&session_key)
            .map(|s| (*s.messages).clone())
            .unwrap_or_default();

        let (tx, rx) = tokio::sync::mpsc::channel(100);
        
        let provider = self.provider_client.clone();
        tokio::spawn(async move {
            let mut stream = provider.stream_complete(messages).await;
            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        if tx.send(Ok(token)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(AgentError::ProviderError(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub session_key: String,
    pub message: Message,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Tool error: {0}")]
    ToolError(String),
    #[error("Max iterations exceeded")]
    MaxIterationsExceeded,
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Lane full: {0}")]
    LaneFull(String),
    #[error("Timeout")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_manager_create_and_get() {
        let manager = SessionManager::new(100, 3600);
        let session = manager.create("http", "lane_0");
        assert_eq!(session.lane, "lane_0");
        assert_eq!(session.metadata.channel, "http");
        assert!(manager.get(&session.key).is_some());
    }

    #[test]
    fn test_session_manager_get_or_create_returns_existing() {
        let manager = SessionManager::new(100, 3600);
        let s1 = manager.get_or_create("test-key", "http", "lane_1");
        let s2 = manager.get_or_create("test-key", "http", "lane_1");
        assert_eq!(s1.key, s2.key);
    }

    #[test]
    fn test_session_manager_get_or_create_creates_new() {
        let manager = SessionManager::new(100, 3600);
        let session = manager.get_or_create("new-key", "ws", "lane_2");
        assert_eq!(session.key, "new-key");
        assert_eq!(session.metadata.channel, "ws");
        assert_eq!(session.lane, "lane_2");
    }

    #[test]
    fn test_session_manager_add_message() {
        let manager = SessionManager::new(100, 3600);
        let session = manager.create("http", "lane_0");
        let key = session.key.clone();
        manager.add_message(&key, Message {
            role: Role::User,
            content: Content::text("hello"),
        });
        let updated = manager.get(&key).unwrap();
        assert_eq!(updated.messages.len(), 1);
    }

    #[test]
    fn test_session_manager_delete() {
        let manager = SessionManager::new(100, 3600);
        let session = manager.create("http", "lane_0");
        let key = session.key.clone();
        assert!(manager.get(&key).is_some());
        manager.delete(&key);
        assert!(manager.get(&key).is_none());
    }

    #[test]
    fn test_session_manager_cleanup_expired() {
        let manager = SessionManager::new(100, 1);
        let session = manager.create("http", "lane_0");
        let key = session.key.clone();
        assert!(manager.get(&key).is_some());
        std::thread::sleep(std::time::Duration::from_secs(2));
        let removed = manager.cleanup_expired();
        assert_eq!(removed, 1);
        assert!(manager.get(&key).is_none());
    }

    #[test]
    fn test_session_manager_count() {
        let manager = SessionManager::new(100, 3600);
        assert_eq!(manager.count(), 0);
        manager.create("http", "lane_0");
        manager.create("http", "lane_1");
        assert_eq!(manager.count(), 2);
    }

    #[test]
    fn test_session_manager_list() {
        let manager = SessionManager::new(100, 3600);
        manager.create("http", "lane_0");
        manager.create("http", "lane_1");
        let sessions = manager.list();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_lane_router_route() {
        let router = LaneRouter::new(10);
        let lane1 = router.route("session_abc");
        let lane2 = router.route("session_abc");
        assert_eq!(lane1, lane2);
        assert!(lane1.starts_with("lane_"));
    }

    #[test]
    fn test_lane_router_register_and_get() {
        let router = LaneRouter::new(10);
        let (tx, _rx) = tokio::sync::mpsc::channel(10);
        router.register_lane("test_lane".to_string(), tx);
        let lane = router.get_lane("test_lane");
        assert!(lane.is_some());
        assert_eq!(lane.unwrap().name, "test_lane");
    }
}
