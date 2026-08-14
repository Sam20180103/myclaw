pub mod builtin_tools;
pub mod metrics;

use rustclaw_protocol::{Message, Envelope, Role, Content, ToolCallSpec};
use rustclaw_tool::{ToolRegistry, ToolCall, ToolPolicy};
use std::future::Future;
use std::pin::Pin;
use builtin_tools::{MemoryTool, SkillManageTool};
use rustclaw_provider::ProviderClient;
use rustclaw_memory::MemoryProvider;
use rustclaw_compression::ContextCompressor;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Agent 事件枚举 — 用于回调系统，通知外部 Agent 的运行状态。
///
/// P1-8: 事件带 `session` 字段,否则订阅方无法把进度推回正确的聊天会话。
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// 工具开始执行
    ToolStart { session: String, tool_name: String, args: String },
    /// 工具执行结束
    ToolEnd { session: String, tool_name: String, success: bool },
    /// 迭代步骤
    Step { session: String, iteration: u32, message_count: usize },
    /// 状态变化 (started/completed/max_iterations/deadline/cancelled/provider_error)
    Status { session: String, status: String },
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
/// P1-9: 回调签名带 `session_key`,审批请求才能推送回发起它的聊天会话。
pub type ApprovalHook = Arc<
    dyn Fn(String, serde_json::Value, String) -> Pin<Box<dyn Future<Output = bool> + Send>>
        + Send
        + Sync,
>;

use dashmap::DashMap;

/// Agent 运行时 — 核心执行引擎
/// 负责管理会话、执行 Agent 推理循环、调用工具和 LLM
pub struct AgentRuntime {
    config: AgentConfig,                       // Agent 配置
    session_manager: SessionManager,           // 会话管理器
    tool_registry: Arc<ToolRegistry>,          // 工具注册表
    provider_client: ProviderClient,           // LLM 提供商客户端
    memory_manager: Option<Arc<dyn MemoryProvider>>, // 可选: 记忆后端 (G6: trait 抽象)
    context_compressor: Option<Arc<ContextCompressor>>, // 可选: 上下文压缩器
    event_callback: Option<Arc<EventCallback>>, // 可选: 事件回调
    iteration_budget: Option<u32>,             // 可选: 迭代预算 (父子Agent共享)
    session_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>, // 会话锁
    cancel_tokens: DashMap<String, tokio::sync::watch::Sender<bool>>, // P1-5: 会话取消句柄
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
    /// P1-6: 单次循环的墙钟预算 (毫秒),超时返回部分结果而非报错
    pub max_loop_duration_ms: u64,
    /// P1-11: 单条工具结果写入历史的最大字符数
    pub max_tool_result_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_loop_iterations: 100,    // 默认最多100次迭代
            tool_timeout_ms: 60000,      // 默认工具超时60秒
            session_ttl_seconds: 3600,   // 默认会话TTL 1小时
            max_concurrent_per_lane: 10, // 默认每车道10并发
            max_loop_duration_ms: 600_000, // 默认整轮 10 分钟
            max_tool_result_chars: 16_000, // 默认单条工具结果 16k 字符
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

    /// G10: 设置父会话 (谱系),内存与 SQLite 同步。
    pub fn set_parent(&self, key: &str, parent: &str) {
        let mut snapshot = None;
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(key) {
                session.parent_session_id = Some(parent.to_string());
                snapshot = Some(session.clone());
            }
        }
        if let (Some(ref store), Some(session)) = (&self.store, snapshot) {
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
            cancel_tokens: DashMap::new(),
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

    /// G6: 接受任意 MemoryProvider 实现 (默认文件型 MemoryManager 可直接传入)。
    pub fn with_memory(mut self, memory: Arc<dyn MemoryProvider>) -> Self {
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

    pub fn memory_manager(&self) -> Option<&Arc<dyn MemoryProvider>> {
        self.memory_manager.as_ref()
    }

    fn emit_event(&self, event: AgentEvent) {
        if let Some(ref cb) = self.event_callback {
            cb(event);
        }
    }

    pub async fn process_with_loop(&self, envelope: Envelope) -> Result<AgentResponse, AgentError> {
        self.process_with_loop_budget(envelope, None).await
    }

    /// G10: 带迭代预算覆盖的核心循环 (子 Agent 用较小预算跑同一循环)。
    ///
    /// P1-5/P1-6/P1-7: 循环受三重约束 —— 迭代预算、墙钟 deadline、用户取消。
    /// 任何一条触发都返回**部分结果**而非错误,避免整轮工作白做。
    pub async fn process_with_loop_budget(
        &self,
        envelope: Envelope,
        budget_override: Option<u32>,
    ) -> Result<AgentResponse, AgentError> {
        let session_key = envelope.session_key.clone();
        // P2-17: 按调用者取有效工具策略 (未配置覆盖时即全局策略)
        let policy = self.tool_policy.for_principal(envelope.principal.as_deref());

        // P1-5: 注册取消句柄 (在拿会话锁之前注册,排队中的请求也能被取消)
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        self.cancel_tokens.insert(session_key.clone(), cancel_tx);
        let _cancel_guard = CancelGuard {
            tokens: &self.cancel_tokens,
            key: session_key.clone(),
        };

        // 1. Acquire session lock
        let lock = self.session_locks.entry(session_key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .value()
            .clone();

        let _guard = lock.lock().await;

        let lane = route_lane(&session_key);

        self.session_manager.get_or_create(&session_key, &envelope.channel, &lane);
        self.session_manager.add_message(&session_key, envelope.message);
        self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "started".to_string() });
        metrics::inc(&metrics::LOOPS_STARTED);

        let max_iterations = budget_override
            .unwrap_or_else(|| self.iteration_budget.unwrap_or(self.config.max_loop_iterations));
        // P1-6: 整轮墙钟预算
        let deadline = Instant::now() + Duration::from_millis(self.config.max_loop_duration_ms);
        let mut iterations = 0;
        let mut total_tool_calls: u32 = 0;

        loop {
            // P1-5: 用户取消 → 返回已完成的部分结果
            if *cancel_rx.borrow() {
                self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "cancelled".to_string() });
                metrics::inc(&metrics::LOOPS_CANCELLED);
                return Ok(self.partial_response(&session_key, "[已取消] 任务被用户中止。"));
            }

            // P1-6: 墙钟超时 → 返回部分结果 (此前只数迭代次数,单轮最长可跑上百分钟)
            if Instant::now() >= deadline {
                self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "deadline".to_string() });
                metrics::inc(&metrics::LOOPS_DEADLINE);
                return Ok(self.partial_response(
                    &session_key,
                    "[未完成 · 超时] 已达到单轮时间预算,以下是当前进展。",
                ));
            }

            // P1-7: 迭代耗尽 → 返回部分结果而非 Err (此前所有中间成果被丢弃)
            if iterations >= max_iterations {
                if total_tool_calls >= 5 {
                    self.emit_event(AgentEvent::SkillSuggestion {
                        task_summary: "Complex task completed".to_string(),
                        tool_call_count: total_tool_calls,
                    });
                }
                self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "max_iterations".to_string() });
                metrics::inc(&metrics::LOOPS_MAX_ITERATIONS);
                return Ok(self.partial_response(
                    &session_key,
                    "[未完成 · 达到迭代上限] 以下是当前进展。",
                ));
            }

            self.emit_event(AgentEvent::Step {
                session: session_key.clone(),
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
                    metrics::inc(&metrics::COMPRESSIONS);
                    history = result.messages;
                    // P0-2: 被整条丢弃的历史先摘要,再作为 System 消息插回队首,
                    // 否则 Agent 会忘记自己已经做过哪些有副作用的动作 (重复发帖/投递)。
                    if !result.dropped.is_empty() {
                        let summary = self.summarize_dropped(&result.dropped).await;
                        history.insert(
                            0,
                            Message {
                                role: Role::System,
                                content: Content::text(summary),
                            },
                        );
                    }
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

            let tool_schemas: Vec<serde_json::Value> = self.tool_registry.available_schemas()
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

            // P1-5: LLM 调用与取消竞速 (长响应期间用户可中止)
            let llm_call = self.provider_client.complete_with_tools(messages, tool_schemas);
            let llm_result = tokio::select! {
                biased;
                _ = cancel_rx.changed() => {
                    self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "cancelled".to_string() });
                    metrics::inc(&metrics::LOOPS_CANCELLED);
                metrics::inc(&metrics::LOOPS_CANCELLED);
                    return Ok(self.partial_response(&session_key, "[已取消] 任务被用户中止。"));
                }
                r = llm_call => r,
            };

            let (response_text, tool_call_results) = match llm_result {
                Ok((text, tool_calls)) => (text, tool_calls),
                Err(e) => {
                    self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "provider_error".to_string() });
                    metrics::inc(&metrics::PROVIDER_ERRORS);
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
                self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "completed".to_string() });
                metrics::inc(&metrics::LOOPS_COMPLETED);

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
                metrics::inc(&metrics::TOOL_CALLS);
                self.emit_event(AgentEvent::ToolStart {
                    session: session_key.clone(),
                    tool_name: tool_call.name.clone(),
                    args: tool_call.arguments.to_string(),
                });
                tracing::info!(
                    tool = %tool_call.name,
                    args = %tool_call.arguments,
                    "Executing tool call"
                );
            }

            // P2-15: 同一轮的多个工具调用并发执行 (此前串行,只读工具也要排队);
            // 结果按原下标回灌,顺序语义不变。
            // P1-5: 整批执行与取消竞速。
            let batch = futures_util::future::join_all(
                tool_calls
                    .iter()
                    .map(|tc| self.execute_tool(tc, &session_key, &policy)),
            );
            let results = tokio::select! {
                biased;
                _ = cancel_rx.changed() => {
                    // 取消时先补齐 tool_result 占位,保住 tool_use/tool_result 配对不变量
                    for tool_call in &tool_calls {
                        self.session_manager.add_message(
                            &session_key,
                            Message::tool_result(&tool_call.id, &tool_call.name, "[已取消]"),
                        );
                    }
                    self.emit_event(AgentEvent::Status { session: session_key.clone(), status: "cancelled".to_string() });
                    metrics::inc(&metrics::LOOPS_CANCELLED);
                metrics::inc(&metrics::LOOPS_CANCELLED);
                    return Ok(self.partial_response(&session_key, "[已取消] 任务被用户中止。"));
                }
                r = batch => r,
            };

            let mut pending_image: Option<Message> = None;
            for (tool_call, result) in tool_calls.iter().zip(results.into_iter()) {
                match result {
                    Ok(result) => {
                        tracing::info!(
                            tool = %tool_call.name,
                            result_len = result.len(),
                            "Tool execution succeeded"
                        );
                        self.emit_event(AgentEvent::ToolEnd {
                            session: session_key.clone(),
                            tool_name: tool_call.name.clone(),
                            success: true,
                        });
                        // D1: 通用"工具结果含可渲染图片 → 直接回给用户"适配,
                        // 平台特例 (如 BOSS 扫码二维码) 不再硬编码在内核里。
                        if pending_image.is_none() {
                            pending_image = extract_renderable_image(&result);
                        }
                        // C1: 工具结果以 tool 角色 + tool_call_id 回灌,而非 System 文本
                        // P1-11: 超长结果截断后再入历史,防单条结果顶爆上下文
                        self.session_manager.add_message(
                            &session_key,
                            Message::tool_result(
                                &tool_call.id,
                                &tool_call.name,
                                truncate_tool_result(&result, self.config.max_tool_result_chars),
                            ),
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
                            session: session_key.clone(),
                            tool_name: tool_call.name.clone(),
                            success: false,
                        });
                        metrics::inc(&metrics::TOOL_FAILURES);
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

            // 有可渲染图片时直接回给用户 (所有 tool_result 已回灌,配对完整)
            if let Some(image_message) = pending_image {
                self.session_manager
                    .add_message(&session_key, image_message.clone());
                return Ok(AgentResponse {
                    session_key,
                    message: image_message,
                    tool_calls: vec![],
                });
            }

            iterations += 1;
            metrics::inc(&metrics::ITERATIONS);
        }
    }

    /// P1-5: 取消指定会话正在进行的 Agent 循环。返回是否命中一个在跑的会话。
    /// 取消是协作式的: 循环会在下一个检查点 (LLM 调用 / 工具批次 / 轮次开始) 退出,
    /// 并把已完成的部分结果回给用户。
    pub fn cancel_session(&self, session_key: &str) -> bool {
        match self.cancel_tokens.get(session_key) {
            Some(tx) => tx.send(true).is_ok(),
            None => false,
        }
    }

    /// P1-6/P1-7: 构造"未完成但有进展"的响应。
    /// 取最后一条 assistant 文本作为进展,加上原因前缀,并写回历史保持对话连贯。
    fn partial_response(&self, session_key: &str, note: &str) -> AgentResponse {
        let progress = self
            .session_manager
            .get(session_key)
            .and_then(|s| {
                s.messages.iter().rev().find_map(|m| match (&m.role, &m.content) {
                    (Role::Assistant, Content::Text(t)) if !t.trim().is_empty() => {
                        Some(t.clone())
                    }
                    (Role::Assistant, Content::ToolUse { text: Some(t), .. })
                        if !t.trim().is_empty() =>
                    {
                        Some(t.clone())
                    }
                    _ => None,
                })
            })
            .unwrap_or_default();

        let text = if progress.is_empty() {
            note.to_string()
        } else {
            format!("{}\n\n{}", note, progress)
        };
        let message = Message {
            role: Role::Assistant,
            content: Content::text(text),
        };
        self.session_manager.add_message(session_key, message.clone());
        AgentResponse {
            session_key: session_key.to_string(),
            message,
            tool_calls: vec![],
        }
    }

    /// P0-2: 把即将被丢弃的历史压成摘要。优先让 LLM 写,失败时降级为本地摘要
    /// (本地版至少保住"已执行过哪些工具",防止压缩后重复执行副作用动作)。
    async fn summarize_dropped(&self, dropped: &[Message]) -> String {
        let fallback = ContextCompressor::local_summary(dropped);
        if !self.provider_client.is_configured() {
            return fallback;
        }
        let prompt = ContextCompressor::summary_prompt(dropped);
        let req = vec![Message {
            role: Role::User,
            content: Content::text(prompt),
        }];
        match self.provider_client.complete(req).await {
            Ok(text) if !text.trim().is_empty() => {
                format!("[历史摘要]\n{}\n\n{}", text.trim(), fallback)
            }
            Ok(_) => fallback,
            Err(e) => {
                tracing::warn!("summarize_dropped failed, using local summary: {}", e);
                fallback
            }
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

    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        session_key: &str,
        policy: &ToolPolicy,
    ) -> Result<String, AgentError> {
        // G1: 策略门控 —— 黑名单拒绝 (P2-17: policy 已按调用者解析)
        if !policy.is_allowed(&tool_call.name) {
            let reason = "denied by tool policy".to_string();
            self.emit_event(AgentEvent::ToolDenied {
                tool_name: tool_call.name.clone(),
                reason: reason.clone(),
            });
            return Err(AgentError::ToolError(format!("tool '{}' {}", tool_call.name, reason)));
        }
        // G1: 需审批的工具,执行前征求批准 (无 hook → fail-closed 拒绝)
        if policy.requires_approval(&tool_call.name) {
            let approved = match &self.approval_hook {
                Some(hook) => {
                    hook(
                        tool_call.name.clone(),
                        tool_call.arguments.clone(),
                        session_key.to_string(),
                    )
                    .await
                }
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

        // P1-12: 先按 schema 校验参数。校验失败直接回错误,由模型自行修正重试,
        // 避免把脏参数丢进工具后拿到五花八门的底层报错。
        if let Err(msg) = validate_tool_args(&tool.schema().parameters, &tool_call.arguments) {
            return Err(AgentError::ToolError(format!(
                "invalid arguments for tool '{}': {}",
                tool_call.name, msg
            )));
        }

        // G10: spawn_subagent 需要知道父会话以记谱系 — 注入保留参数 (仅此内置工具)。
        let mut arguments = tool_call.arguments.clone();
        if tool_call.name == builtin_tools::SUBAGENT_TOOL_NAME {
            if let Some(obj) = arguments.as_object_mut() {
                obj.insert(
                    "_parent_session_key".to_string(),
                    serde_json::Value::String(session_key.to_string()),
                );
            }
        }

        // A3: 强制工具超时,避免挂死的 MCP/HTTP 工具无限期占住会话锁。
        let timeout = Duration::from_millis(self.config.tool_timeout_ms);
        let result = match tokio::time::timeout(timeout, tool.execute(arguments)).await {
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
        self.cancel_tokens.remove(key);
    }

    /// G10: 以受限预算派生一个子 Agent 执行独立子任务。
    ///
    /// 子会话 key `sub:<uuid>`,channel `subagent`,谱系经 `parent_session_id` 指回父会话;
    /// 子 Agent 复用同一工具注册表/策略/记忆,但迭代预算受 `budget` 上限约束。
    /// 返回 (子会话 key, 最终文本)。
    pub async fn process_subtask(
        &self,
        task: &str,
        parent_session_key: &str,
        budget: u32,
    ) -> Result<(String, String), AgentError> {
        let child_key = format!("sub:{}", Uuid::new_v4());
        let lane = route_lane(&child_key);
        self.session_manager.get_or_create(&child_key, "subagent", &lane);
        self.session_manager.set_parent(&child_key, parent_session_key);

        let envelope = Envelope::new(
            "subagent",
            &child_key,
            Message { role: Role::User, content: Content::text(task) },
        );
        let resp = self.process_with_loop_budget(envelope, Some(budget)).await?;
        let text = match resp.message.content {
            Content::Text(t) => t,
            Content::Structured { text, .. } => text.unwrap_or_default(),
            Content::Image { description, .. } => {
                format!("[Image: {}]", description.unwrap_or_default())
            }
            Content::ToolUse { text, .. } => text.unwrap_or_default(),
            Content::ToolResult { output, .. } => output,
        };
        Ok((child_key, text))
    }

    /// G10: 注册 `spawn_subagent` 工具 (需在 Arc 包装后调用,工具持 Weak 防循环引用)。
    pub fn enable_subagent(self: &Arc<Self>) {
        self.tool_registry
            .register(Arc::new(builtin_tools::SubAgentTool::new(Arc::downgrade(self))));
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

}

/// P1-5: 循环结束时(含 early return / panic 展开)自动摘掉取消句柄,防止 DashMap 无界增长。
struct CancelGuard<'a> {
    tokens: &'a DashMap<String, tokio::sync::watch::Sender<bool>>,
    key: String,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        self.tokens.remove(&self.key);
    }
}

/// P1-11: 截断超长工具结果 —— 保头保尾并标注,避免一次大抓取顶爆上下文。
/// 保头是为了留住结构/状态码,保尾是为了留住结论/错误信息。
fn truncate_tool_result(result: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return result.to_string();
    }
    let total = result.chars().count();
    if total <= max_chars {
        return result.to_string();
    }
    let head_len = max_chars * 2 / 3;
    let tail_len = max_chars - head_len;
    let head: String = result.chars().take(head_len).collect();
    let tail: String = result
        .chars()
        .skip(total.saturating_sub(tail_len))
        .collect();
    format!(
        "{}\n\n... [工具结果过长,已截断 {} 个字符;如需完整内容请缩小查询范围或分页] ...\n\n{}",
        head,
        total - max_chars,
        tail
    )
}

/// P1-12: 按工具 JSON Schema 校验模型给的参数。返回 Err(说明) 时,
/// 说明会作为 tool_result 回灌给模型,让它自己改参数重试,而不是把脏参数丢进工具。
fn validate_tool_args(schema: &serde_json::Value, args: &serde_json::Value) -> Result<(), String> {
    let props = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()), // 无 schema 约束 (自由参数工具) → 放行
    };

    let obj = match args.as_object() {
        Some(o) => o,
        None => return Err("arguments must be a JSON object".to_string()),
    };

    // 必填字段
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        let missing: Vec<String> = required
            .iter()
            .filter_map(|r| r.as_str())
            .filter(|name| !obj.contains_key(*name) || obj[*name].is_null())
            .map(|s| s.to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "missing required parameter(s): {}. expected schema: {}",
                missing.join(", "),
                schema
            ));
        }
    }

    // 类型检查 (只查显式声明了 type 的字段)
    for (name, value) in obj {
        if value.is_null() {
            continue;
        }
        let expected = match props
            .get(name)
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
        {
            Some(t) => t,
            None => continue,
        };
        let ok = match expected {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !ok {
            return Err(format!(
                "parameter '{}' must be of type {}, got: {}",
                name, expected, value
            ));
        }
    }

    Ok(())
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

    // --- G10: spawn_subagent ---

    /// 第一次调用请求 spawn_subagent,其余直接终答 "done"。
    struct SpawnMockProvider {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl rustclaw_provider::LlmProvider for SpawnMockProvider {
        async fn complete(&self, _m: Vec<Message>) -> Result<String, rustclaw_provider::ProviderError> {
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
                    String::new(),
                    vec![serde_json::json!({
                        "id": "spawn_1",
                        "name": "spawn_subagent",
                        "arguments": { "task": "sub task", "max_iterations": 3 }
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
    async fn test_g10_spawn_subagent_roundtrip() {
        let registry = Arc::new(ToolRegistry::new());
        let provider = ProviderClient::new(Some(Arc::new(SpawnMockProvider {
            calls: AtomicUsize::new(0),
        })));
        let rt = Arc::new(
            AgentRuntime::new(AgentConfig::default())
                .with_tools(registry)
                .with_provider(provider),
        );
        rt.enable_subagent();

        let env = Envelope::new("test", "sess-parent", Message {
            role: Role::User,
            content: Content::text("do it"),
        });
        let resp = rt.process_with_loop(env.clone()).await.expect("parent loop completes");
        assert!(matches!(&resp.message.content, Content::Text(t) if t == "done"));

        // 父会话里应有 spawn_subagent 的 tool_result,内含子会话最终文本与 key
        let parent = rt.get_session("sess-parent").unwrap();
        let (child_key, tool_output) = parent
            .messages
            .iter()
            .find_map(|m| match &m.content {
                Content::ToolResult { name, output, .. } if name == "spawn_subagent" => {
                    let v: serde_json::Value = serde_json::from_str(output).ok()?;
                    Some((v["session_key"].as_str()?.to_string(), output.clone()))
                }
                _ => None,
            })
            .expect("parent history should contain spawn_subagent tool_result");
        assert!(tool_output.contains("\"success\":true"));
        assert!(tool_output.contains("done"), "child final text should flow back: {}", tool_output);

        // 子会话存在,channel=subagent,谱系指回父会话
        assert!(child_key.starts_with("sub:"));
        let child = rt.get_session(&child_key).expect("child session exists");
        assert_eq!(child.metadata.channel, "subagent");
        assert_eq!(child.parent_session_id.as_deref(), Some("sess-parent"));
    }

    #[tokio::test]
    async fn test_g10_subagent_tool_requires_task() {
        let registry = Arc::new(ToolRegistry::new());
        let rt = Arc::new(
            AgentRuntime::new(AgentConfig::default())
                .with_tools(registry)
                .with_provider(ProviderClient::new(None)),
        );
        rt.enable_subagent();
        let tool = rt.tool_registry().get("spawn_subagent").expect("registered");
        let err = tool.execute(serde_json::json!({})).await;
        assert!(err.is_err(), "missing task should error");
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
        let rt = rt_with_policy(policy).with_approval_hook(Arc::new(|_name, _args, _session| {
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

    // --- P1: 取消 / 墙钟预算 / 部分结果 / 结果截断 / 参数校验 ---

    /// 永远请求工具的 provider — 用于把循环逼到预算边界
    struct LoopingProvider;

    #[async_trait::async_trait]
    impl rustclaw_provider::LlmProvider for LoopingProvider {
        async fn complete(&self, _m: Vec<Message>) -> Result<String, rustclaw_provider::ProviderError> {
            Ok(String::new())
        }
        async fn complete_with_tools(
            &self,
            _m: Vec<Message>,
            _t: Vec<serde_json::Value>,
        ) -> Result<(String, Vec<serde_json::Value>), rustclaw_provider::ProviderError> {
            Ok((
                "还在干活".to_string(),
                vec![serde_json::json!({ "id": "c", "name": "echo", "arguments": { "x": 1 } })],
            ))
        }
        async fn stream_complete(&self, _m: Vec<Message>) -> rustclaw_provider::TokenStream {
            Box::pin(futures_util::stream::iter(vec![Ok("x".to_string())]))
        }
        async fn list_models(&self) -> Result<Vec<String>, rustclaw_provider::ProviderError> {
            Ok(vec![])
        }
    }

    /// 慢工具 — 用于测试取消不必等工具跑完
    struct SlowTool;

    #[async_trait::async_trait]
    impl rustclaw_tool::Tool for SlowTool {
        fn name(&self) -> &str { "slow" }
        fn description(&self) -> &str { "sleeps" }
        fn schema(&self) -> rustclaw_tool::ToolSchema {
            rustclaw_tool::ToolSchema {
                name: "slow".to_string(),
                description: "sleeps".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(
            &self,
            _p: serde_json::Value,
        ) -> Result<serde_json::Value, rustclaw_tool::ToolError> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    fn looping_rt(config: AgentConfig) -> Arc<AgentRuntime> {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(SlowTool));
        Arc::new(
            AgentRuntime::new(config)
                .with_tools(registry)
                .with_provider(ProviderClient::new(Some(Arc::new(LoopingProvider)))),
        )
    }

    #[tokio::test]
    async fn test_max_iterations_returns_partial_response() {
        // P1-7: 迭代耗尽必须返回进展,而不是丢弃全部工作只报错
        let config = AgentConfig { max_loop_iterations: 2, ..AgentConfig::default() };
        let rt = looping_rt(config);
        let env = Envelope::new("test", "sess-maxiter", Message {
            role: Role::User,
            content: Content::text("干活"),
        });
        let resp = rt.process_with_loop(env).await.expect("应返回部分结果而非 Err");
        match &resp.message.content {
            Content::Text(t) => {
                assert!(t.contains("达到迭代上限"), "got: {}", t);
                assert!(t.contains("还在干活"), "部分进展必须带上, got: {}", t);
            }
            other => panic!("unexpected content: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_deadline_returns_partial() {
        // P1-6: 墙钟预算到点必须收工返回进展
        let config = AgentConfig {
            max_loop_iterations: 1000,
            max_loop_duration_ms: 150,
            ..AgentConfig::default()
        };
        let rt = looping_rt(config);
        let env = Envelope::new("test", "sess-deadline", Message {
            role: Role::User,
            content: Content::text("干活"),
        });
        let started = Instant::now();
        let resp = rt.process_with_loop(env).await.expect("应返回部分结果");
        assert!(started.elapsed() < Duration::from_secs(10), "必须在预算附近收工");
        assert!(matches!(&resp.message.content, Content::Text(t) if t.contains("超时")));
    }

    #[tokio::test]
    async fn test_cancel_session_stops_loop() {
        // P1-5: 取消不必等慢工具跑完 (SlowTool 睡 30s, 工具超时 60s)
        let config = AgentConfig { max_loop_iterations: 1000, ..AgentConfig::default() };
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(SlowTool));
        struct SlowCaller;
        #[async_trait::async_trait]
        impl rustclaw_provider::LlmProvider for SlowCaller {
            async fn complete(&self, _m: Vec<Message>) -> Result<String, rustclaw_provider::ProviderError> {
                Ok(String::new())
            }
            async fn complete_with_tools(
                &self,
                _m: Vec<Message>,
                _t: Vec<serde_json::Value>,
            ) -> Result<(String, Vec<serde_json::Value>), rustclaw_provider::ProviderError> {
                Ok((
                    "调用慢工具".to_string(),
                    vec![serde_json::json!({ "id": "c", "name": "slow", "arguments": {} })],
                ))
            }
            async fn stream_complete(&self, _m: Vec<Message>) -> rustclaw_provider::TokenStream {
                Box::pin(futures_util::stream::iter(vec![Ok("x".to_string())]))
            }
            async fn list_models(&self) -> Result<Vec<String>, rustclaw_provider::ProviderError> {
                Ok(vec![])
            }
        }
        let rt = Arc::new(
            AgentRuntime::new(config)
                .with_tools(registry)
                .with_provider(ProviderClient::new(Some(Arc::new(SlowCaller)))),
        );

        let rt2 = rt.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(rt2.cancel_session("sess-cancel"), "取消句柄应命中在跑的会话");
        });

        let env = Envelope::new("test", "sess-cancel", Message {
            role: Role::User,
            content: Content::text("干活"),
        });
        let started = Instant::now();
        let resp = rt.process_with_loop(env).await.expect("取消应返回部分结果");
        assert!(started.elapsed() < Duration::from_secs(10), "不应等满慢工具");
        assert!(matches!(&resp.message.content, Content::Text(t) if t.contains("已取消")));

        // 取消路径也必须保持 tool_use / tool_result 配对
        let session = rt.get_session("sess-cancel").unwrap();
        let uses = session.messages.iter().filter(|m| matches!(m.content, Content::ToolUse { .. })).count();
        let results = session.messages.iter().filter(|m| matches!(m.content, Content::ToolResult { .. })).count();
        assert_eq!(uses, results, "取消后 tool_use 与 tool_result 必须配平");
    }

    #[test]
    fn test_tool_result_truncated() {
        // P1-11: 超长结果保头保尾并标注
        let long = "a".repeat(5000);
        let out = truncate_tool_result(&long, 100);
        assert!(out.chars().count() < 400);
        assert!(out.contains("已截断"));
        assert!(out.starts_with('a') && out.ends_with('a'));
        // 未超限的原样返回
        assert_eq!(truncate_tool_result("short", 100), "short");
    }

    #[test]
    fn test_validate_args_missing_required() {
        // P1-12: 缺必填 / 类型不符必须在进入工具前拦下
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" }, "limit": { "type": "integer" } },
            "required": ["path"]
        });
        assert!(validate_tool_args(&schema, &serde_json::json!({ "path": "/tmp/a" })).is_ok());

        let err = validate_tool_args(&schema, &serde_json::json!({ "limit": 3 })).unwrap_err();
        assert!(err.contains("path"), "got: {}", err);

        let err = validate_tool_args(&schema, &serde_json::json!({ "path": 42 })).unwrap_err();
        assert!(err.contains("string"), "got: {}", err);

        // 无 properties 的自由参数工具放行
        assert!(validate_tool_args(&serde_json::json!({"type": "object"}), &serde_json::json!({"x": 1})).is_ok());
    }

    #[tokio::test]
    async fn test_parallel_tool_calls_preserve_order() {
        // P2-15: 同轮多工具并发执行, 但 tool_result 顺序必须与请求顺序一致
        struct MultiCall(AtomicUsize);
        #[async_trait::async_trait]
        impl rustclaw_provider::LlmProvider for MultiCall {
            async fn complete(&self, _m: Vec<Message>) -> Result<String, rustclaw_provider::ProviderError> {
                Ok(String::new())
            }
            async fn complete_with_tools(
                &self,
                _m: Vec<Message>,
                _t: Vec<serde_json::Value>,
            ) -> Result<(String, Vec<serde_json::Value>), rustclaw_provider::ProviderError> {
                if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok((
                        String::new(),
                        vec![
                            serde_json::json!({ "id": "a", "name": "echo", "arguments": { "n": 1 } }),
                            serde_json::json!({ "id": "b", "name": "echo", "arguments": { "n": 2 } }),
                            serde_json::json!({ "id": "c", "name": "echo", "arguments": { "n": 3 } }),
                        ],
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

        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let rt = AgentRuntime::new(AgentConfig::default())
            .with_tools(registry)
            .with_provider(ProviderClient::new(Some(Arc::new(MultiCall(AtomicUsize::new(0))))));

        let env = Envelope::new("test", "sess-par", Message {
            role: Role::User,
            content: Content::text("跑三个工具"),
        });
        rt.process_with_loop(env).await.expect("loop completes");

        let session = rt.get_session("sess-par").unwrap();
        let ids: Vec<String> = session.messages.iter().filter_map(|m| match &m.content {
            Content::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        }).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[tokio::test]
    async fn test_event_callback_receives_session_tagged_events() {
        // P1-8: 事件必须带 session,否则通道侧无法把进度推回正确的聊天
        use std::sync::Mutex;
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();

        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));
        let rt = AgentRuntime::new(AgentConfig::default())
            .with_tools(registry)
            .with_provider(ProviderClient::new(Some(Arc::new(MockProvider {
                calls: AtomicUsize::new(0),
            }))))
            .with_event_callback(Box::new(move |e| {
                if let AgentEvent::ToolStart { session, tool_name, .. } = e {
                    sink.lock().unwrap().push((session, tool_name));
                }
            }));

        let env = Envelope::new("test", "sess-events", Message {
            role: Role::User,
            content: Content::text("hi"),
        });
        rt.process_with_loop(env).await.expect("loop completes");

        let events = seen.lock().unwrap().clone();
        assert_eq!(events, vec![("sess-events".to_string(), "echo".to_string())]);
    }

    #[tokio::test]
    async fn test_approval_notifier_invoked() {
        // P1-9: 审批回调必须拿到 session_key,否则审批请求推不回发起它的聊天
        use std::sync::Mutex;
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let sink = captured.clone();

        let policy = ToolPolicy::permissive().with_require_approval(vec!["echo".to_string()]);
        let rt = rt_with_policy(policy).with_approval_hook(Arc::new(move |_tool, _args, session| {
            *sink.lock().unwrap() = Some(session);
            Box::pin(async { true })
        }));

        let env = Envelope::new("test", "sess-approval-session", Message {
            role: Role::User,
            content: Content::text("hi"),
        });
        rt.process_with_loop(env).await.expect("loop completes");

        assert_eq!(
            captured.lock().unwrap().as_deref(),
            Some("sess-approval-session")
        );
    }

    #[tokio::test]
    async fn test_principal_policy_applies_in_loop() {
        // P2-17: 受限 principal 的工具调用应被拒绝, 无限制 principal 正常执行
        let policy = ToolPolicy::permissive().with_principal(
            "guest",
            rustclaw_tool::PrincipalPolicy {
                allowed_tools: Some(vec!["nothing".to_string()]),
                denied_tools: None,
                require_approval: None,
            },
        );
        let rt = rt_with_policy(policy);
        let env = Envelope::new("test", "sess-principal", Message {
            role: Role::User,
            content: Content::text("hi"),
        })
        .with_principal("guest");
        rt.process_with_loop(env).await.expect("loop completes");

        let session = rt.get_session("sess-principal").unwrap();
        let denied = session.messages.iter().any(|m| matches!(&m.content,
            Content::ToolResult { output, .. } if output.contains("denied")));
        assert!(denied, "受限 principal 的工具调用应被策略拒绝");
    }
}
