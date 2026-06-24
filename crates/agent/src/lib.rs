pub mod builtin_tools;

use rustclaw_protocol::{Message, Envelope, Role, Content, ToolCallSpec};
use rustclaw_tool::{ToolRegistry, ToolCall, ToolPolicy};
use std::future::Future;
use std::pin::Pin;
use builtin_tools::{MemoryTool, SkillManageTool};
use rustclaw_provider::ProviderClient;
use rustclaw_memory::MemoryManager;
use rustclaw_compression::ContextCompressor;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
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
    /// G1: 工具被策略拒绝或未获审批
    ToolDenied { tool_name: String, reason: String },
}

/// 事件回调类型 — 用于外部监听 Agent 运行事件
pub type EventCallback = Box<dyn Fn(AgentEvent) + Send + Sync>;

/// G1: 工具审批回调 — 对 `require_approval` 名单内的工具,在执行前异步征求批准。
/// 返回 `true` 放行、`false` 拒绝。未设置时,需审批的工具一律拒绝 (fail-closed)。
pub type ApprovalHook =
    Arc<dyn Fn(String, serde_json::Value) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

use dashmap::DashMap;

/// Agent 运行时 — 核心执行引擎
/// 负责管理会话、执行 Agent 推理循环、调用工具和 LLM
pub struct AgentRuntime {
    config: AgentConfig,                       // Agent 配置
    session_manager: SessionManager,           // 会话管理器
    tool_registry: Arc<ToolRegistry>,          // 工具注册表
    provider_client: ProviderClient,           // LLM 提供商客户端
    memory_manager: Option<Arc<MemoryManager>>, // 可选: 记忆管理器
    context_compressor: Option<Arc<ContextCompressor>>, // 可选: 上下文压缩器
    event_callback: Option<Arc<EventCallback>>, // 可选: 事件回调
    iteration_budget: Option<u32>,             // 可选: 迭代预算 (父子Agent共享)
    session_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>, // 会话锁
    base_system_prompt: String,                 // 基础系统提示
    tool_policy: ToolPolicy,                    // G1: 工具策略 (允许/拒绝/需审批)
    approval_hook: Option<ApprovalHook>,        // G1: 工具审批回调 (fail-closed)
}

const DEFAULT_BASE_SYSTEM_PROMPT: &str = r#"You are RustClaw, an autonomous AI agent.
You have access to a set of tools to help you complete tasks.
When you need to use a tool, you should use the native tool calling mechanism if available.
If you cannot use the native mechanism, you MUST use one of the following formats:

1. JSON format:
```json
{
  "name": "tool_name",
  "arguments": {
    "param1": "value1"
  }
}
```

2. XML format:
<invoke name="tool_name">
{
  "param1": "value1"
}
</invoke>

Always prefer the native tool calling mechanism or the JSON format.
Only use tools when necessary. If you have enough information, answer the user directly.
Stay professional and concise."#;

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
use rustclaw_storage::SQLiteSessionStore;

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>, // 所有活跃会话 (内存缓存)
    store: Option<Arc<SQLiteSessionStore>>,           // SQLite 持久化存储
    max_sessions: usize,  // 内存中最大活跃会话数 (R3: 超出按 LRU 淘汰)
    ttl_seconds: u64,     // 会话过期时间 (秒)
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new(max_sessions: usize, ttl_seconds: u64) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::with_capacity(max_sessions))),
            store: None,
            max_sessions,
            ttl_seconds,
        }
    }

    pub fn with_store(mut self, store: Arc<SQLiteSessionStore>) -> Self {
        self.store = Some(store);
        self
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
        if let Some(ref store) = self.store {
            let db_session = rustclaw_storage::Session {
                key: session.key.clone(),
                channel: channel.to_string(),
                parent_session_id: None,
                messages: vec![],
                created_at: session.metadata.created_at,
                updated_at: session.metadata.updated_at,
            };
            let _ = store.insert(&db_session);
        }
        session
    }

    /// 获取会话 (只读)
    pub fn get(&self, key: &str) -> Option<Session> {
        if let Some(session) = self.sessions.read().get(key) {
            return Some(session.clone());
        }
        if let Some(ref store) = self.store {
            if let Some(db_session) = store.get(key) {
                let agent_session = Session {
                    key: db_session.key.clone(),
                    lane: "lane_0".to_string(), // fallback
                    parent_session_id: db_session.parent_session_id.clone(),
                    messages: Arc::new(db_session.messages),
                    metadata: Arc::new(SessionMetadata {
                        channel: db_session.channel.clone(),
                        created_at: db_session.created_at,
                        updated_at: chrono::Utc::now().timestamp_millis(),
                        tool_calls_enabled: true,
                    }),
                    last_accessed: Instant::now(),
                };
                self.sessions.write().insert(key.to_string(), agent_session.clone());
                return Some(agent_session);
            }
        }
        None
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

        if let Some(ref store) = self.store {
            if let Some(db_session) = store.get(key) {
                let agent_session = Session {
                    key: db_session.key.clone(),
                    lane: lane.to_string(),
                    parent_session_id: db_session.parent_session_id.clone(),
                    messages: Arc::new(db_session.messages),
                    metadata: Arc::new(SessionMetadata {
                        channel: db_session.channel.clone(),
                        created_at: db_session.created_at,
                        updated_at: chrono::Utc::now().timestamp_millis(),
                        tool_calls_enabled: true,
                    }),
                    last_accessed: Instant::now(),
                };
                sessions.insert(key.to_string(), agent_session.clone());
                return agent_session;
            }
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
        if let Some(ref store) = self.store {
            let db_session = rustclaw_storage::Session {
                key: session.key.clone(),
                channel: channel.to_string(),
                parent_session_id: None,
                messages: vec![],
                created_at: session.metadata.created_at,
                updated_at: session.metadata.updated_at,
            };
            let _ = store.insert(&db_session);
        }
        session
    }

    /// 添加消息到会话 (使用 Arc 优化减少克隆开销)
    pub fn add_message(&self, key: &str, message: Message) {
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(key) {
                // 克隆 messages Arc -> 新 Vec, 追加消息, 用新 Arc 替换
                let mut messages = (*session.messages).clone();
                messages.push(message.clone());
                session.messages = Arc::new(messages);
                // 克隆 metadata Arc -> 新 Metadata, 更新时间, 用新 Arc 替换
                let mut metadata = (*session.metadata).clone();
                metadata.updated_at = chrono::Utc::now().timestamp_millis();
                session.metadata = Arc::new(metadata);
                session.last_accessed = Instant::now();
            }
        }
        if let Some(ref store) = self.store {
            let _ = store.add_message(key, &message);
        }
    }

    /// 替换整个会话历史 (主要用于上下文压缩)
    pub fn replace_messages(&self, key: &str, messages: Vec<Message>, parent_session_id: Option<String>) {
        let mut session_clone = None;
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(key) {
                session.messages = Arc::new(messages.clone());
                if parent_session_id.is_some() {
                    session.parent_session_id = parent_session_id.clone();
                }
                session.last_accessed = Instant::now();
                session_clone = Some(session.clone());
            }
        }
        
        if let (Some(ref store), Some(session)) = (&self.store, session_clone) {
            let db_session = rustclaw_storage::Session {
                key: session.key.clone(),
                channel: session.metadata.channel.clone(),
                parent_session_id: session.parent_session_id.clone(),
                messages: (*session.messages).clone(),
                created_at: session.metadata.created_at,
                updated_at: chrono::Utc::now().timestamp_millis(),
            };
            let _ = store.insert(&db_session);
        }
    }

    /// 删除会话
    pub fn delete(&self, key: &str) {
        self.sessions.write().remove(key);
        if let Some(ref store) = self.store {
            let _ = store.remove(key);
        }
    }

    /// 清理过期会话并强制内存会话上限 (R1/R3)。
    /// 返回被移出内存的 session key,供上层 (AgentRuntime) 同步清理会话锁。
    /// 注意:仅移出内存缓存,SQLite 中的会话保留 (下次访问可回填)。
    pub fn cleanup_expired(&self) -> Vec<String> {
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

        // 阶段2: 写锁逐个删除过期会话,并按 LRU 强制上限
        {
            let mut sessions = self.sessions.write();
            for key in &to_remove {
                sessions.remove(key);
            }

            // R3: 超过上限时按 last_accessed 从旧到新淘汰
            if sessions.len() > self.max_sessions {
                let mut by_age: Vec<(String, Instant)> = sessions
                    .iter()
                    .map(|(k, s)| (k.clone(), s.last_accessed))
                    .collect();
                by_age.sort_by_key(|(_, t)| *t);
                let evict_count = sessions.len() - self.max_sessions;
                for (key, _) in by_age.into_iter().take(evict_count) {
                    sessions.remove(&key);
                    to_remove.push(key);
                }
            }
        }

        // 不从持久化存储中删除过期会话, 但清理过老的
        if let Some(ref store) = self.store {
            // 过期时间例如90天，而不是内存的几小时
            let _ = store.cleanup_expired(90 * 24 * 3600);
        }

        to_remove
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

/// 将 session_key 哈希到固定的 lane 标签 (lane_0..lane_9)。
///
/// A1: 历史上这是一个"车道队列并发"子系统的入口,但队列从未接线 —
/// 真实并发模型是 `process_with_loop` 里的 per-session `Mutex` (串行同一会话),
/// 叠加每请求一个 tokio task (跨会话并行)。此处仅保留 lane 标签,
/// 用于会话分组/可观测性 (写入 `Session.lane`)。
pub fn route_lane(session_key: &str) -> String {
    let hash = session_key.bytes().fold(0u64, |acc, b| acc.wrapping_add(b as u64));
    format!("lane_{}", hash % 10)
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
        Self::new_with_reach_config(config, None)
    }

    pub fn new_with_reach_config(config: AgentConfig, reach_config: Option<rustclaw_tool::ReachConfig>) -> Self {
        let registry = Arc::new(ToolRegistry::new());
        registry.register_all_tools_with_config(reach_config);
        Self {
            config: config.clone(),
            session_manager: SessionManager::new(100, config.session_ttl_seconds),
            tool_registry: registry,
            provider_client: ProviderClient::new(None),
            memory_manager: None,
            context_compressor: None,
            event_callback: None,
            iteration_budget: None,
            session_locks: DashMap::new(),
            base_system_prompt: DEFAULT_BASE_SYSTEM_PROMPT.to_string(),
            tool_policy: ToolPolicy::permissive(),
            approval_hook: None,
        }
    }

    pub fn with_base_system_prompt(mut self, prompt: String) -> Self {
        self.base_system_prompt = prompt;
        self
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
        // 把记忆管理暴露为 Agent 可调用工具,实现自主记忆管理
        self.tool_registry.register(Arc::new(MemoryTool::new(memory.clone())));
        self.memory_manager = Some(memory);
        self
    }

    /// 注入 SkillManager 并把 Skill 管理暴露为 Agent 可调用工具,实现自我进化闭环。
    pub fn with_skill_manager(self, skills: Arc<rustclaw_skill::SkillManager>) -> Self {
        self.tool_registry.register(Arc::new(SkillManageTool::new(skills)));
        self
    }

    pub fn with_compressor(mut self, compressor: Arc<ContextCompressor>) -> Self {
        self.context_compressor = Some(compressor);
        self
    }

    pub fn with_session_store(mut self, store: Arc<rustclaw_storage::SQLiteSessionStore>) -> Self {
        self.session_manager = self.session_manager.with_store(store);
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

    /// G1: 设置工具策略 (允许/拒绝/需审批)。
    pub fn with_tool_policy(mut self, policy: ToolPolicy) -> Self {
        self.tool_policy = policy;
        self
    }

    /// G1: 设置工具审批回调。未设置时,`require_approval` 名单内的工具一律拒绝。
    pub fn with_approval_hook(mut self, hook: ApprovalHook) -> Self {
        self.approval_hook = Some(hook);
        self
    }

    pub fn tool_policy(&self) -> &ToolPolicy {
        &self.tool_policy
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
        let lane = route_lane(session_key);
        
        let _session = self.session_manager.get_or_create(
            session_key,
            &envelope.channel,
            &lane,
        );

        self.session_manager.add_message(session_key, envelope.message);

        let mut messages = self.session_manager.get(session_key)
            .map(|s| (*s.messages).clone())
            .unwrap_or_default();

        // Inject base system prompt
        messages.insert(0, Message {
            role: Role::System,
            content: Content::text(self.base_system_prompt.clone()),
        });

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

        let lane = route_lane(&session_key);
        
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

            // 取原始会话历史
            let mut history = self.session_manager.get(&session_key)
                .map(|s| (*s.messages).clone())
                .unwrap_or_default();

            // C2: 压缩原始历史并落盘 (按配置预算触发),避免历史无界增长。
            // 注意:压缩对象是纯历史,不含下面临时注入的 system/memory,
            // 否则会把系统提示固化进历史导致重复注入。
            if let Some(ref compressor) = self.context_compressor {
                let result = compressor.compress(&history, compressor.context_window());
                if result.compressed {
                    history = result.messages;
                    self.session_manager
                        .replace_messages(&session_key, history.clone(), None);
                }
            }

            // 组装本轮发送给 LLM 的消息 (临时,不落盘): [base_system, memory?, ...history]
            // G2: 稳定的 base system prompt 置于最前,保证可缓存前缀稳定 (Anthropic
            // 将这些前导 System 消息提升为带 cache_control 的 system 块)。
            let mut messages = history;
            if let Some(ref memory) = self.memory_manager {
                let memory_text = memory.get_memory_text();
                if !memory_text.is_empty() {
                    messages.insert(0, Message {
                        role: Role::System,
                        content: Content::text(format!("Memory:\n{}", memory_text)),
                    });
                }
            }
            messages.insert(0, Message {
                role: Role::System,
                content: Content::text(self.base_system_prompt.clone()),
            });

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

            // C1: 确保每个 tool_call 有非空 ID,用于与 tool_result 配对
            for tc in tool_calls.iter_mut() {
                if tc.id.is_empty() {
                    tc.id = uuid::Uuid::new_v4().to_string();
                }
            }

            // C1: 把承载 tool_calls 的 assistant 消息回灌进历史,
            // 否则模型在下一轮看不到自己刚才请求了哪个工具。
            let call_specs: Vec<ToolCallSpec> = tool_calls
                .iter()
                .map(|tc| ToolCallSpec {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                })
                .collect();
            let assistant_text = if response_text.trim().is_empty() {
                None
            } else {
                Some(response_text.clone())
            };
            self.session_manager
                .add_message(&session_key, Message::tool_use(call_specs, assistant_text));

            for tool_call in &tool_calls {
                total_tool_calls += 1;
                self.emit_event(AgentEvent::ToolStart {
                    tool_name: tool_call.name.clone(),
                    args: tool_call.arguments.to_string(),
                });

                tracing::info!(
                    tool = %tool_call.name,
                    args = %tool_call.arguments,
                    "Executing tool call"
                );

                match self.execute_tool(tool_call).await {
                    Ok(result) => {
                        // D1: 通用"工具结果含可渲染图片 → 直接回给用户"适配,
                        // 平台特例 (如 BOSS 扫码二维码) 不再硬编码在内核里。
                        if let Some(image_message) = extract_renderable_image(&result) {
                            // C1: 先回灌 tool_result 保持 tool_calls 配对,再返回图片消息
                            self.session_manager.add_message(
                                &session_key,
                                Message::tool_result(&tool_call.id, &tool_call.name, &result),
                            );
                            self.session_manager
                                .add_message(&session_key, image_message.clone());
                            self.emit_event(AgentEvent::ToolEnd {
                                tool_name: tool_call.name.clone(),
                                success: true,
                            });
                            return Ok(AgentResponse {
                                session_key,
                                message: image_message,
                                tool_calls: vec![],
                            });
                        }

                        tracing::info!(
                            tool = %tool_call.name,
                            result_len = result.len(),
                            "Tool execution succeeded"
                        );
                        self.emit_event(AgentEvent::ToolEnd {
                            tool_name: tool_call.name.clone(),
                            success: true,
                        });
                        // C1: 工具结果以 tool 角色 + tool_call_id 回灌,而非 System 文本
                        self.session_manager.add_message(
                            &session_key,
                            Message::tool_result(&tool_call.id, &tool_call.name, &result),
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            tool = %tool_call.name,
                            args = %tool_call.arguments,
                            error = %e,
                            "Tool execution failed"
                        );
                        self.emit_event(AgentEvent::ToolEnd {
                            tool_name: tool_call.name.clone(),
                            success: false,
                        });
                        // C1: 工具错误同样以 tool 角色回灌,保持 tool_calls/tool_result 配对
                        self.session_manager.add_message(
                            &session_key,
                            Message::tool_result(
                                &tool_call.id,
                                &tool_call.name,
                                format!("error: {}", e),
                            ),
                        );
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

        // 2. Look for JSON in Markdown code blocks: ```json ... ```
        let block_re = regex::Regex::new(r"(?s)```json\s*(.*?)\s*```").ok();
        if let Some(re) = block_re {
            for cap in re.captures_iter(response) {
                let content = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
                    // Could be a single tool call object or an array of tool calls
                    if let Some(name) = json.get("name").or_else(|| json.get("tool_name")).and_then(|v| v.as_str()) {
                        let args = json.get("arguments").or_else(|| json.get("parameters")).cloned().unwrap_or(serde_json::Value::Null);
                        tool_calls.push(ToolCall {
                            id: uuid::Uuid::new_v4().to_string(),
                            name: name.to_string(),
                            arguments: args,
                        });
                    } else if let Some(arr) = json.as_array() {
                        for item in arr {
                            if let Some(name) = item.get("name").or_else(|| item.get("tool_name")).and_then(|v| v.as_str()) {
                                let args = item.get("arguments").or_else(|| item.get("parameters")).cloned().unwrap_or(serde_json::Value::Null);
                                tool_calls.push(ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name: name.to_string(),
                                    arguments: args,
                                });
                            }
                        }
                    } else if let Some(calls) = json.get("tool_calls").and_then(|v| v.as_array()) {
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
            if !tool_calls.is_empty() {
                return tool_calls;
            }
        }

        // 3. Look for XML format <invoke name="tool_name">JSON_ARGS</invoke>
        let xml_re = regex::Regex::new(r#"(?s)<invoke\s+name=["']([^"']+)["']\s*>(.*?)(?:</invoke>|$)"#).ok();
        if let Some(re) = xml_re {
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
            if !tool_calls.is_empty() {
                return tool_calls;
            }
        }

        // 4. Look for plain JSON tool_calls array anywhere in the text
        if let Some(start) = response.find(r#""tool_calls""#) {
            if let Some(json_start) = response[start..].find('[') {
                let part = &response[start + json_start..];
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

        // 5. Look for simple {"name": "...", "arguments": "..."} or {"name": "...", "parameters": "..."} anywhere in the text
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
                        
                        while object_start > 0 && !response.is_char_boundary(object_start) {
                            object_start -= 1;
                        }
                        
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
        // G1: 策略门控 —— 黑名单拒绝
        if !self.tool_policy.is_allowed(&tool_call.name) {
            let reason = "denied by tool policy".to_string();
            self.emit_event(AgentEvent::ToolDenied {
                tool_name: tool_call.name.clone(),
                reason: reason.clone(),
            });
            return Err(AgentError::ToolError(format!("tool '{}' {}", tool_call.name, reason)));
        }
        // G1: 需审批的工具,执行前征求批准 (无 hook → fail-closed 拒绝)
        if self.tool_policy.requires_approval(&tool_call.name) {
            let approved = match &self.approval_hook {
                Some(hook) => hook(tool_call.name.clone(), tool_call.arguments.clone()).await,
                None => false,
            };
            if !approved {
                let reason = "requires approval, not granted".to_string();
                self.emit_event(AgentEvent::ToolDenied {
                    tool_name: tool_call.name.clone(),
                    reason: reason.clone(),
                });
                return Err(AgentError::ToolError(format!("tool '{}' {}", tool_call.name, reason)));
            }
        }

        let tool = self.tool_registry.get(&tool_call.name)
            .ok_or_else(|| AgentError::ToolError(format!("Tool not found: {}", tool_call.name)))?;

        // A3: 强制工具超时,避免挂死的 MCP/HTTP 工具无限期占住会话锁。
        let timeout = Duration::from_millis(self.config.tool_timeout_ms);
        let result = match tokio::time::timeout(timeout, tool.execute(tool_call.arguments.clone())).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(AgentError::ToolError(e.to_string())),
            Err(_) => {
                return Err(AgentError::ToolError(format!(
                    "tool '{}' timed out after {}ms",
                    tool_call.name, self.config.tool_timeout_ms
                )))
            }
        };

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
        // R1: 同步移除会话锁,避免 DashMap 无界增长
        self.session_locks.remove(key);
    }

    /// 周期性维护 (R1/R3): 清理过期/超限会话,并回收对应的会话锁。
    /// 还会顺带回收当前无人持有的孤儿锁 (strong_count == 1)。
    pub fn maintenance(&self) -> usize {
        let removed = self.session_manager.cleanup_expired();
        for key in &removed {
            self.session_locks.remove(key);
        }
        // 回收不再对应活跃会话、且无在途持有者的锁条目
        self.session_locks
            .retain(|_, lock| Arc::strong_count(lock) > 1);
        removed.len()
    }

    pub async fn process_stream(
        &self,
        envelope: Envelope,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<String, AgentError>>, AgentError> {
        let session_key = envelope.session_key.clone();
        let lane = route_lane(&session_key);
        
        self.session_manager.get_or_create(
            &session_key,
            &envelope.channel,
            &lane,
        );

        self.session_manager.add_message(&session_key, envelope.message);

        let mut messages = self.session_manager.get(&session_key)
            .map(|s| (*s.messages).clone())
            .unwrap_or_default();

        // Inject base system prompt
        messages.insert(0, Message {
            role: Role::System,
            content: Content::text(self.base_system_prompt.clone()),
        });

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

/// D1: 通用地从工具结果 JSON 里识别"可直接渲染给用户的图片"。
///
/// 约定:工具结果若含 `qrcode_image`/`image`(或嵌套在 `data` 下),且其中带非空
/// base64 `image_data`,则转为一条图片 assistant 消息。把平台特例 (BOSS 扫码登录、
/// 未来其它需要展示图片的工具) 收敛到这一处约定,而非散落在 Agent 内核。
fn extract_renderable_image(result: &str) -> Option<Message> {
    let v: serde_json::Value = serde_json::from_str(result).ok()?;
    let img = v
        .get("qrcode_image")
        .or_else(|| v.get("image"))
        .or_else(|| v.get("data").and_then(|d| d.get("qrcode_image")))
        .or_else(|| v.get("data").and_then(|d| d.get("image")))?;

    let image_data = img.get("image_data").and_then(|x| x.as_str()).unwrap_or("");
    if image_data.is_empty() {
        return None;
    }
    let image_type = img.get("image_type").and_then(|x| x.as_str()).unwrap_or("png");

    // 描述行 = 图片自带描述 + 顶层 message + next_step (任意可缺省)
    let mut description = img
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    for key in ["message", "next_step"] {
        let extra = v
            .get(key)
            .and_then(|x| x.as_str())
            .or_else(|| v.get("data").and_then(|d| d.get(key)).and_then(|x| x.as_str()));
        if let Some(extra) = extra {
            if !extra.is_empty() {
                if description.is_empty() {
                    description = extra.to_string();
                } else {
                    description = format!("{}\n{}", description, extra);
                }
            }
        }
    }

    Some(Message {
        role: Role::Assistant,
        content: Content::image_with_description(image_data, image_type, description),
    })
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
    fn test_memory_tool_add_and_list() {
        use rustclaw_memory::MemoryManager;
        use rustclaw_tool::Tool;
        let dir = std::env::temp_dir().join(format!("rustclaw_test_{}", Uuid::new_v4()));
        let mem = Arc::new(MemoryManager::new(dir.join("MEMORY.md"), 2200, 1375));
        let tool = MemoryTool::new(mem);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let add = rt
            .block_on(tool.execute(serde_json::json!({ "action": "add", "content": "用户偏好深色主题" })))
            .unwrap();
        assert_eq!(add["success"], true);

        let list = rt
            .block_on(tool.execute(serde_json::json!({ "action": "list" })))
            .unwrap();
        let entries = list["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e.as_str().unwrap().contains("深色主题")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_memory_tool_rejects_unknown_action() {
        use rustclaw_memory::MemoryManager;
        use rustclaw_tool::Tool;
        let dir = std::env::temp_dir().join(format!("rustclaw_test_{}", Uuid::new_v4()));
        let mem = Arc::new(MemoryManager::new(dir.join("MEMORY.md"), 2200, 1375));
        let tool = MemoryTool::new(mem);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(tool.execute(serde_json::json!({ "action": "frobnicate" })));
        assert!(res.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

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
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], key);
        assert!(manager.get(&key).is_none());
    }

    #[test]
    fn test_session_manager_enforces_max_sessions_lru() {
        // R3: 上限为 2,插入 4 个,应淘汰最旧的 2 个,保留最近访问的 2 个
        let manager = SessionManager::new(2, 3600);
        let keep_recent = manager.get_or_create("s3", "http", "lane_0");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let _ = manager.get_or_create("s4", "http", "lane_0");
        // s1/s2 更旧
        let _old1 = manager.get_or_create("s1", "http", "lane_0");
        let _old2 = manager.get_or_create("s2", "http", "lane_0");
        // 触碰 s3/s4 使其成为最近访问
        std::thread::sleep(std::time::Duration::from_millis(5));
        manager.add_message(&keep_recent.key, Message { role: Role::User, content: Content::text("x") });
        manager.add_message("s4", Message { role: Role::User, content: Content::text("y") });
        let removed = manager.cleanup_expired();
        assert_eq!(manager.count(), 2);
        // 被淘汰的应是更旧的 s1/s2
        assert!(removed.contains(&"s1".to_string()));
        assert!(removed.contains(&"s2".to_string()));
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
    fn test_route_lane_is_deterministic() {
        let lane1 = route_lane("session_abc");
        let lane2 = route_lane("session_abc");
        assert_eq!(lane1, lane2);
        assert!(lane1.starts_with("lane_"));
    }

    // --- T1: 端到端工具循环测试 (锁定 C1 回归) ---

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock 工具:回显入参
    struct EchoTool;

    #[async_trait::async_trait]
    impl rustclaw_tool::Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo back the input"
        }
        fn schema(&self) -> rustclaw_tool::ToolSchema {
            rustclaw_tool::ToolSchema {
                name: "echo".to_string(),
                description: "echo back the input".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(
            &self,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, rustclaw_tool::ToolError> {
            Ok(serde_json::json!({ "ok": true, "echoed": params }))
        }
    }

    /// Mock provider:第一次调用请求一个工具,第二次返回终答。
    struct MockProvider {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl rustclaw_provider::LlmProvider for MockProvider {
        async fn complete(
            &self,
            _m: Vec<Message>,
        ) -> Result<String, rustclaw_provider::ProviderError> {
            Ok(String::new())
        }
        async fn complete_with_tools(
            &self,
            _m: Vec<Message>,
            _t: Vec<serde_json::Value>,
        ) -> Result<(String, Vec<serde_json::Value>), rustclaw_provider::ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok((
                    "let me check".to_string(),
                    vec![serde_json::json!({
                        "id": "call_1",
                        "name": "echo",
                        "arguments": { "x": 1 }
                    })],
                ))
            } else {
                Ok(("done".to_string(), vec![]))
            }
        }
        async fn stream_complete(&self, _m: Vec<Message>) -> rustclaw_provider::TokenStream {
            Box::pin(futures_util::stream::iter(vec![Ok("x".to_string())]))
        }
        async fn list_models(&self) -> Result<Vec<String>, rustclaw_provider::ProviderError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_process_with_loop_persists_tool_calls_and_results() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let provider = ProviderClient::new(Some(Arc::new(MockProvider {
            calls: AtomicUsize::new(0),
        })));
        let rt = AgentRuntime::new(AgentConfig::default())
            .with_tools(registry)
            .with_provider(provider);

        let env = Envelope::new(
            "test",
            "sess-roundtrip",
            Message {
                role: Role::User,
                content: Content::text("hi"),
            },
        );
        let resp = rt.process_with_loop(env).await.expect("loop should succeed");

        // 终答应为第二次响应文本
        match &resp.message.content {
            Content::Text(t) => assert_eq!(t, "done"),
            other => panic!("expected text final answer, got {:?}", other),
        }

        // C1 关键断言:历史按协议成对回灌
        // [user, assistant(ToolUse), tool(ToolResult), assistant(done)]
        let session = rt.get_session("sess-roundtrip").unwrap();
        let msgs = &*session.messages;
        assert_eq!(msgs.len(), 4, "history should have 4 messages, got {}", msgs.len());

        assert_eq!(msgs[0].role, Role::User);

        assert_eq!(msgs[1].role, Role::Assistant);
        match &msgs[1].content {
            Content::ToolUse { tool_calls, text } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "echo");
                assert_eq!(tool_calls[0].id, "call_1");
                assert_eq!(text.as_deref(), Some("let me check"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }

        assert_eq!(msgs[2].role, Role::Tool);
        match &msgs[2].content {
            Content::ToolResult { tool_call_id, name, output } => {
                // tool_result 必须与 tool_call 的 id 配对
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(name, "echo");
                assert!(output.contains("echoed"));
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }

        assert_eq!(msgs[3].role, Role::Assistant);
        assert!(matches!(&msgs[3].content, Content::Text(t) if t == "done"));
    }

    // --- G1: 工具策略与人审 ---

    fn rt_with_policy(policy: ToolPolicy) -> AgentRuntime {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let provider = ProviderClient::new(Some(Arc::new(MockProvider {
            calls: AtomicUsize::new(0),
        })));
        AgentRuntime::new(AgentConfig::default())
            .with_tools(registry)
            .with_provider(provider)
            .with_tool_policy(policy)
    }

    /// 被拒绝的工具不应执行,且历史里 tool_result 记录的是拒绝错误。
    #[tokio::test]
    async fn test_denied_tool_is_blocked() {
        let policy = ToolPolicy::permissive().with_denied_tools(vec!["echo".to_string()]);
        let rt = rt_with_policy(policy);
        let env = Envelope::new("test", "sess-deny", Message {
            role: Role::User,
            content: Content::text("hi"),
        });
        // 循环不应 panic;echo 被拒后以 tool_result(error) 回灌,模型第二轮给终答
        let resp = rt.process_with_loop(env).await.expect("loop should still complete");
        assert!(matches!(&resp.message.content, Content::Text(t) if t == "done"));
        let session = rt.get_session("sess-deny").unwrap();
        let tool_result = session.messages.iter().find_map(|m| match &m.content {
            Content::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        }).expect("a tool_result should exist");
        assert!(tool_result.contains("denied"), "got: {}", tool_result);
    }

    /// require_approval 工具:无 hook → fail-closed 拒绝。
    #[tokio::test]
    async fn test_require_approval_without_hook_is_denied() {
        let policy = ToolPolicy::permissive().with_require_approval(vec!["echo".to_string()]);
        let rt = rt_with_policy(policy);
        let env = Envelope::new("test", "sess-appr-none", Message {
            role: Role::User,
            content: Content::text("hi"),
        });
        let _ = rt.process_with_loop(env).await.expect("loop completes");
        let session = rt.get_session("sess-appr-none").unwrap();
        let denied = session.messages.iter().any(|m| matches!(&m.content,
            Content::ToolResult { output, .. } if output.contains("approval")));
        assert!(denied, "tool requiring approval should be denied without a hook");
    }

    /// require_approval 工具:hook 批准 → 执行成功。
    #[tokio::test]
    async fn test_require_approval_with_granting_hook_runs() {
        let policy = ToolPolicy::permissive().with_require_approval(vec!["echo".to_string()]);
        let rt = rt_with_policy(policy).with_approval_hook(Arc::new(|_name, _args| {
            Box::pin(async { true })
        }));
        let env = Envelope::new("test", "sess-appr-ok", Message {
            role: Role::User,
            content: Content::text("hi"),
        });
        let _ = rt.process_with_loop(env).await.expect("loop completes");
        let session = rt.get_session("sess-appr-ok").unwrap();
        let ran = session.messages.iter().any(|m| matches!(&m.content,
            Content::ToolResult { output, .. } if output.contains("echoed")));
        assert!(ran, "approved tool should execute and produce its result");
    }
}
