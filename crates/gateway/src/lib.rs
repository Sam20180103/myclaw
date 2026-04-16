use axum::{
    extract::State,
    routing::get,
    Json, Router,
    response::sse::{Event, Sse},
    http::{StatusCode, HeaderValue},
};
use futures::stream::Stream;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::convert::Infallible;
use tokio::sync::broadcast;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, error, warn};

use rustclaw_protocol::{JsonRpcRequest, Envelope, Message, Role, Content};
use rustclaw_agent::{AgentRuntime, AgentConfig, AgentResponse};
use rustclaw_tool;
use rustclaw_skill::{SkillManager, SkillConfig, SkillLanguage, SkillApprovalRequest};
use rustclaw_config::{Config, ProviderConfig};
use rustclaw_provider::{ProviderClient, OpenAiProvider, AnthropicProvider, LlmProvider};
use rustclaw_feishu::{FeishuChannel, FeishuConfig, WebSocketManager};
use rustclaw_channel::{Channel, ChannelEvent};
use rustclaw_compression::ContextCompressor;
use rustclaw_memory::MemoryManager;
use rustclaw_storage::SQLiteSessionStore;

const APPROVAL_CHANNEL_CAPACITY: usize = 64;

fn api_success(data: serde_json::Value) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "success": true, "data": data }))
}

#[allow(dead_code)]
fn api_error(msg: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "success": false, "error": msg }))
}

#[derive(Clone)]
pub struct Gateway {
    config: GatewayConfig,
    full_config: Config,
    config_path: std::path::PathBuf,
    sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
    agent: Arc<AgentRuntime>,
    skill_manager: Arc<SkillManager>,
    approval_broadcaster: broadcast::Sender<SkillApprovalRequest>,
    channels: Arc<RwLock<HashMap<String, ChannelInfo>>>,
    #[allow(dead_code)]
    feishu_channel: Option<Arc<FeishuChannel>>,
    #[allow(dead_code)]
    feishu_ws: Option<Arc<WebSocketManager>>,
}

pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub device_id: String,
}

impl Clone for GatewayConfig {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
    device_id: self.device_id.clone(),
    }
    }
}

pub struct GatewaySession {
    pub id: String,
    pub channel: String,
    pub authenticated: bool,
    pub agent_session_key: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelInfo {
    pub name: String,
    pub channel_type: String,
    pub enabled: bool,
    pub config: HashMap<String, serde_json::Value>,
}

/// Create an approval subscription stream that yields approval requests
fn create_approval_stream(
    mut rx: tokio::sync::broadcast::Receiver<SkillApprovalRequest>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        while let Ok(request) = rx.recv().await {
            let json = serde_json::to_string(&request).unwrap_or_default();
            yield Ok(Event::default().data(json));
        }
    }
}

impl Gateway {
    pub fn new(config: Config) -> Self {
        let provider_client = Self::create_provider_client(&config.providers);

        let agent_config = AgentConfig {
            max_loop_iterations: config.agent.max_loop_iterations,
            tool_timeout_ms: config.agent.tool_timeout_ms,
            session_ttl_seconds: 3600,
            max_concurrent_per_lane: config.agent.max_concurrent_sessions,
        };
        
let reach_config = rustclaw_tool::ReachConfig {
        web_endpoint: config.tools.reach.web_endpoint.clone(),
        web_timeout_ms: config.tools.reach.web_timeout_ms,
        video_timeout_ms: config.tools.reach.video_timeout_ms,
        github_timeout_ms: config.tools.reach.github_timeout_ms,
        social_timeout_ms: config.tools.reach.social_timeout_ms,
        reddit_user_agent: config.tools.reach.reddit_user_agent.clone(),
        weibo_api_base: config.tools.reach.weibo_api_base.clone(),
        mcp_endpoint: config.tools.reach.mcp_endpoint.clone(),
        linkedin_mcp_endpoint: config.tools.reach.linkedin_mcp_endpoint.clone(),
        wechat_mcp_endpoint: config.tools.reach.wechat_mcp_endpoint.clone(),
        boss_mcp_endpoint: config.tools.reach.boss_mcp_endpoint.clone(),
        podcast_mcp_endpoint: config.tools.reach.podcast_mcp_endpoint.clone(),
        boss_cookie: config.tools.reach.boss_cookie.clone(),
        exa_api_key: config.tools.reach.exa_api_key.clone(),
    };

        let mut agent_builder = AgentRuntime::new_with_reach_config(agent_config, Some(reach_config))
            .with_provider(provider_client);

        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".rustclaw");
        std::fs::create_dir_all(&data_dir).ok();

        // 1. ContextCompressor
        let compressor = Arc::new(ContextCompressor::new(0.50, 128000));
        agent_builder = agent_builder.with_compressor(compressor);

        // 2. MemoryManager
        let memory_path = data_dir.join("memories").join("MEMORY.md");
        let memory_manager = Arc::new(MemoryManager::new(&memory_path, 2200, 1375));
        agent_builder = agent_builder.with_memory(memory_manager);

        // 3. SQLiteSessionStore
        let db_path = data_dir.join("state.db");
        if let Ok(store) = SQLiteSessionStore::new(&db_path) {
            let store_arc = Arc::new(store);
            agent_builder = agent_builder.with_session_store(store_arc);
        }

        let agent = Arc::new(agent_builder);

        let gateway_config = GatewayConfig {
            host: config.gateway.host.clone(),
            port: config.gateway.port,
            device_id: config.gateway.device_id.clone(),
        };
        let skill_manager = SkillManager::new(SkillConfig::default());
        let (approval_tx, _) = broadcast::channel(APPROVAL_CHANNEL_CAPACITY);

        let skill_manager = Arc::new(skill_manager);

        let config_path = Config::config_path();
        
        let mut channels = HashMap::new();
        channels.insert("http".to_string(), ChannelInfo {
            name: "http".to_string(),
            channel_type: "http".to_string(),
            enabled: true,
            config: HashMap::new(),
        });
        channels.insert("websocket".to_string(), ChannelInfo {
            name: "websocket".to_string(),
            channel_type: "websocket".to_string(),
            enabled: true,
            config: HashMap::new(),
        });
        channels.insert("webhook".to_string(), ChannelInfo {
            name: "webhook".to_string(),
            channel_type: "webhook".to_string(),
            enabled: true,
            config: HashMap::new(),
        });

        // 初始化飞书通道 (如果配置了)
        let feishu_channel = config.channels.feishu.as_ref().map(|fc| {
            let feishu_config = FeishuConfig {
                app_id: fc.app_id.clone(),
                app_secret: fc.app_secret.clone(),
                verification_token: fc.verification_token.clone(),
                encrypt_key: fc.encrypt_key.clone(),
                default_model: fc.default_model.clone(),
                system_prompt: fc.system_prompt.clone(),
                dm_enabled: fc.dm_enabled,
                group_enabled: fc.group_enabled,
                allowed_groups: fc.allowed_groups.clone(),
                allowed_users: fc.allowed_users.clone(),
                group_configs: fc.group_configs.clone().unwrap_or_default().into_iter().map(|gc| {
                    rustclaw_feishu::config::GroupConfig {
                        group_id: gc.group_id,
                        allowed_users: gc.allowed_users,
                        bound_skills: gc.bound_skills,
                        system_prompt: gc.system_prompt,
                        rate_limit_per_minute: gc.rate_limit_per_minute.unwrap_or(60),
                    }
                }).collect(),
            };
            Arc::new(FeishuChannel::new(feishu_config))
        });

        let feishu_ws = feishu_channel.as_ref().map(|ch| {
            let ws = WebSocketManager::new(
                FeishuConfig {
                    app_id: config.channels.feishu.as_ref().map(|c| c.app_id.clone()).unwrap_or_default(),
                    app_secret: config.channels.feishu.as_ref().map(|c| c.app_secret.clone()).unwrap_or_default(),
                    verification_token: config.channels.feishu.as_ref().map(|c| c.verification_token.clone()).unwrap_or_default(),
                    encrypt_key: config.channels.feishu.as_ref().and_then(|c| c.encrypt_key.clone()),
                    default_model: config.channels.feishu.as_ref().and_then(|c| c.default_model.clone()),
                    system_prompt: config.channels.feishu.as_ref().and_then(|c| c.system_prompt.clone()),
                    dm_enabled: config.channels.feishu.as_ref().map(|c| c.dm_enabled).unwrap_or(true),
                    group_enabled: config.channels.feishu.as_ref().map(|c| c.group_enabled).unwrap_or(false),
                    allowed_groups: config.channels.feishu.as_ref().map(|c| c.allowed_groups.clone()).unwrap_or_default(),
                    allowed_users: config.channels.feishu.as_ref().map(|c| c.allowed_users.clone()).unwrap_or_default(),
                    group_configs: config.channels.feishu.as_ref().and_then(|c| c.group_configs.clone()).unwrap_or_default().into_iter().map(|gc| {
                        rustclaw_feishu::config::GroupConfig {
                            group_id: gc.group_id,
                            allowed_users: gc.allowed_users,
                            bound_skills: gc.bound_skills,
                            system_prompt: gc.system_prompt,
                            rate_limit_per_minute: gc.rate_limit_per_minute.unwrap_or(60),
                        }
                    }).collect(),
                },
                ch.clone(),
            );
            Arc::new(ws)
        });

        // Feishu channel is now handled via WebSocket, no need to set_agent

        Self {
            config: gateway_config,
            full_config: config,
            config_path,
            sessions: Arc::new(RwLock::new(HashMap::with_capacity(64))),
            agent,
            skill_manager,
            approval_broadcaster: approval_tx,
            channels: Arc::new(RwLock::new(channels)),
            feishu_channel,
            feishu_ws,
        }
    }

    fn create_provider_client(providers: &[ProviderConfig]) -> ProviderClient {
        for p in providers {
            let api_key = p.api_key.clone().or_else(|| {
                std::env::var("OPENAI_API_KEY").ok()
            });
            
            info!("Provider config: name={}, type={}, model={:?}, base_url={:?}", 
                p.name, p.provider_type, p.default_model, p.base_url);
            
            if let Some(key) = api_key {
                let provider: Arc<dyn LlmProvider> = match p.provider_type.as_str() {
                    "openai" | "nvidia" => {
                        let model = p.default_model.as_deref().unwrap_or("gpt-4o");
                        info!("Creating OpenAI provider: model={}, base_url={}", 
                            model, p.base_url.as_deref().unwrap_or("https://api.openai.com"));
                        Arc::new(
                            OpenAiProvider::new(key, model)
                                .with_base_url(p.base_url.as_deref().unwrap_or("https://api.openai.com"))
                        )
                    }
                    "anthropic" => {
                        let model = p.default_model.as_deref().unwrap_or("claude-3-5-sonnet-20241022");
                        Arc::new(AnthropicProvider::new(key, model))
                    }
                    _ => continue,
                };
                return ProviderClient::new(Some(provider));
            }
        }
        warn!("No provider configured - API key missing or no providers defined");
        ProviderClient::new(None)
    }

    pub fn with_agent(mut self, agent: Arc<AgentRuntime>) -> Self {
        self.agent = agent;
        self
    }

    pub fn with_skill_manager(mut self, skill_manager: Arc<SkillManager>) -> Self {
        self.skill_manager = skill_manager;
        self
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        self.skill_manager.init().await?;

        let host = self.config.host.parse()
            .map_err(|e| anyhow::anyhow!("Invalid host '{}': {}", self.config.host, e))?;
        let addr = SocketAddr::new(host, self.config.port);

        // CORS 配置收紧: 仅允许配置的来源
        let cors = if self.full_config.gateway.auth.allowed_origins.is_empty() {
            CorsLayer::new()
                .allow_origin(AllowOrigin::exact(HeaderValue::from_static("http://localhost:18789")))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::DELETE])
                .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE])
        } else {
            let origins: Vec<HeaderValue> = self.full_config.gateway.auth.allowed_origins
                .iter()
                .filter_map(|o| o.parse().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::DELETE])
                .allow_headers([axum::http::header::AUTHORIZATION, axum::http::header::CONTENT_TYPE])
        };

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/api/message", axum::routing::post(message_handler))
            .route("/api/session", axum::routing::get(session_list_handler))
            .route("/api/session/{id}", axum::routing::delete(session_delete_handler))
            .route("/api/session/{id}", axum::routing::get(session_get_handler))
            .route("/api/agent", axum::routing::get(agent_list_handler))
            .route("/api/agent/{id}", axum::routing::get(agent_get_handler))
            .route("/api/config", axum::routing::get(config_get_handler))
            .route("/api/config", axum::routing::post(config_update_handler))
            .route("/api/config/save", axum::routing::post(config_save_handler))
            .route("/api/config/model", axum::routing::post(config_model_handler))
            .route("/api/channel/feishu/status", get(feishu_status_handler))
            .route("/api/channel", axum::routing::get(channel_list_handler))
            .route("/api/channel/{name}", axum::routing::get(channel_get_handler))
            .route("/api/channel/{name}", axum::routing::post(channel_update_handler))
            .route("/api/security/audit", axum::routing::get(security_audit_handler))
            .route("/api/memory/status", axum::routing::get(memory_status_handler))
            .route("/api/memory/stats", axum::routing::get(memory_stats_handler))
            .route("/api/system/version", axum::routing::get(system_version_handler))
            .route("/api/system/update/check", axum::routing::get(system_update_check_handler))
            .route("/api/skill", axum::routing::get(skill_list_handler))
            .route("/api/skill", axum::routing::post(skill_create_handler))
            .route("/api/skill/{id}/approve", axum::routing::post(skill_approve_handler))
            .route("/api/skill/{id}/reject", axum::routing::post(skill_reject_handler))
            .route("/api/skill/approval/subscribe", get(skill_approval_subscribe_handler))
            .layer(cors)
            .layer(axum::middleware::from_fn_with_state(Arc::new(self.clone()), api_key_auth))
            .with_state(Arc::new(self.clone()));

        info!("Starting Gateway on {}", addr);

        // 启动飞书消息处理循环 (在启动连接之前, 确保消息能被处理)
        if let Some(feishu_ch) = self.feishu_channel.as_ref() {
            let feishu_ch = feishu_ch.clone();
            let agent = self.agent.clone();
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut events = feishu_ch.events();
                while let Some(event) = events.next().await {
                    if let ChannelEvent::Message(envelope) = event {
                        info!("Feishu message received: session_key={}", envelope.session_key);
                        let ch = feishu_ch.clone();
                        let ag = agent.clone();
                        tokio::spawn(async move {
                            let session_key = envelope.session_key.clone();
                            info!("Processing feishu message in loop: session_key={}", session_key);
                            match ag.process_with_loop(envelope).await {
                                Ok(response) => {
                                    info!("Feishu message processed, sending response: session_key={}", response.session_key);
                                    let response_env = Envelope::new(
                                        "feishu",
                                        &response.session_key,
                                        response.message,
                                    );
                                    if let Err(e) = ch.send(response_env).await {
                                        error!("Failed to send response to feishu: {}", e);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to process feishu message: {}", e);
                                    // 给用户一个错误反馈，防止“没反应”
                                    let error_msg = Message {
                                        role: Role::Assistant,
                                        content: Content::Text(format!("抱歉，处理您的请求时发生了错误：{}", e)),
                                    };
                                    let error_env = Envelope::new("feishu", &session_key, error_msg);
                                    let _ = ch.send(error_env).await;
                                }
                            }
                        });
                    }
                }
            });
        }

        // 启动飞书 WebSocket 连接 (如果配置了)
        if let Some(ref ws) = self.feishu_ws {
            info!("Starting Feishu WebSocket connection...");
            match ws.start().await {
                Ok(()) => info!("Feishu WebSocket started (connecting in background)"),
                Err(e) => warn!("Failed to start Feishu WebSocket: {}", e),
            }
        }

        // Note: Feishu event processor is now handled via WebSocket connection

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }

    pub fn sessions(&self) -> &Arc<RwLock<HashMap<String, GatewaySession>>> {
        &self.sessions
    }

    pub fn agent(&self) -> &Arc<AgentRuntime> {
        &self.agent
    }

    pub fn skill_manager(&self) -> &Arc<SkillManager> {
        &self.skill_manager
    }
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({ 
        "status": "ok", 
        "service": "rustclaw-gateway",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn session_list_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let agent_sessions = state.agent.list_sessions();
    Json(serde_json::json!({
        "success": true,
        "sessions": agent_sessions.iter().map(|s| serde_json::json!({
            "key": s.key,
            "lane": s.lane,
            "channel": s.metadata.channel,
            "message_count": s.messages.len(),
            "created_at": s.metadata.created_at,
            "updated_at": s.metadata.updated_at,
        })).collect::<Vec<_>>()
    }))
}

async fn session_delete_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    state.agent.delete_session(&id);
    api_success(serde_json::json!({ "deleted": id }))
}

async fn message_handler(
    State(state): State<Arc<Gateway>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let request: Result<JsonRpcRequest, _> = serde_json::from_value(payload);

    match request {
        Ok(JsonRpcRequest::Standard(req)) => {
            match req.method.as_str() {
                "message.send" => {
                    let channel = req.params.get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or("http");
                    
                    let session_key = req.params.get("sessionKey")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    
                    let content = req.params.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

let envelope = Envelope::new(
    channel,
    &session_key,
    Message {
        role: Role::User,
        content: Content::text(content),
    },
);

    match state.agent.process_with_loop(envelope).await {
        Ok(AgentResponse { session_key, message, tool_calls }) => {
            let response_content = match message.content {
                Content::Text(s) => s,
                Content::Structured { text, .. } => text.unwrap_or_default(),
                Content::Image { image_data, description, .. } => {
                    let desc = description.as_deref().filter(|d| !d.is_empty()).unwrap_or("图片消息");
                    format!("[Image: {}]", desc)
                }
            };
            Json(serde_json::json!({
                "success": true,
                "sessionKey": session_key,
                "response": response_content,
                "toolCalls": tool_calls.len(),
            }))
        }
        Err(e) => {
            error!("Agent error: {}", e);
            Json(serde_json::json!({
                "success": false,
                "error": e.to_string()
            }))
        }
    }
}
"message.send_with_loop" => {
    let channel = req.params.get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or("http");
                    
                    let session_key = req.params.get("sessionKey")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    
                    let content = req.params.get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

let envelope = Envelope::new(
    channel,
    &session_key,
    Message {
        role: Role::User,
        content: Content::text(content),
    },
);

    match state.agent.process_with_loop(envelope).await {
        Ok(AgentResponse { session_key, message, tool_calls }) => {
            let response_content = match message.content {
                Content::Text(s) => s,
                Content::Structured { text, .. } => text.unwrap_or_default(),
                Content::Image { image_data, description, .. } => {
                    let desc = description.as_deref().filter(|d| !d.is_empty()).unwrap_or("图片消息");
                    format!("[Image: {}]", desc)
                }
            };
            Json(serde_json::json!({
                "success": true,
                "sessionKey": session_key,
                "response": response_content,
                "toolCalls": tool_calls.len(),
            }))
        }
        Err(e) => {
            error!("Agent error: {}", e);
            Json(serde_json::json!({
                "success": false,
                "error": e.to_string()
            }))
        }
    }
}
"session.list" => {
                    let sessions: Vec<_> = state.sessions.read().values()
                        .map(|s| serde_json::json!({
                            "id": s.id,
                            "channel": s.channel,
                            "authenticated": s.authenticated,
                        }))
                        .collect();
                    Json(serde_json::json!({ "sessions": sessions }))
                }
                _ => Json(serde_json::json!({ "error": "method not found", "method": req.method }))
            }
        }
        _ => Json(serde_json::json!({ "error": "invalid request" }))
    }
}

async fn skill_list_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let pending = state.skill_manager.get_pending_skills();
    let active = state.skill_manager.get_active_skills();
    
    Json(serde_json::json!({
        "success": true,
        "pending": pending.iter().map(|s| serde_json::json!({
            "id": s.definition.id,
            "name": s.definition.name,
            "language": s.definition.language.to_string(),
            "status": "pending",
        })).collect::<Vec<_>>(),
        "active": active.iter().map(|s| serde_json::json!({
            "id": s.definition.id,
            "name": s.definition.name,
            "language": s.definition.language.to_string(),
            "status": "active",
        })).collect::<Vec<_>>()
    }))
}

async fn skill_create_handler(
    State(state): State<Arc<Gateway>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = payload.get("name")
    .and_then(|v| v.as_str())
    .unwrap_or("");

    let description = payload.get("description")
    .and_then(|v| v.as_str())
    .unwrap_or("");

    let language_str = payload.get("language")
    .and_then(|v| v.as_str())
    .unwrap_or("shell");

    let language = match language_str {
        "rust" => SkillLanguage::Rust,
        "python" => SkillLanguage::Python,
        "javascript" => SkillLanguage::JavaScript,
        _ => SkillLanguage::Shell,
    };

    let source_code = payload.get("source_code")
    .and_then(|v| v.as_str())
    .unwrap_or("");

    let parameters = payload.get("parameters").cloned().unwrap_or(serde_json::json!({}));

    match state.skill_manager.create_skill(name, description, language, source_code, parameters) {
        Ok(definition) => {
            let definition_clone = definition.clone();
            match state.skill_manager.submit_skill(definition).await {
                Ok(id) => {
                    let request = SkillApprovalRequest {
                        skill_id: id.clone(),
                        skill_name: definition_clone.name.clone(),
                        skill_definition: definition_clone,
                        requested_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                    };
                    let _ = state.approval_broadcaster.send(request);
                    
                    Json(serde_json::json!({
                        "success": true,
                        "skillId": id,
                        "message": "Skill submitted for approval"
                    }))
                }
                Err(e) => Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string()
                })),
            }
        }
        Err(e) => Json(serde_json::json!({
            "success": false,
            "error": e.to_string()
        })),
    }
}

async fn skill_approve_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.skill_manager.approve_skill(&id, "admin").await {
        Ok(()) => api_success(serde_json::json!({ 
            "message": format!("Skill {} approved", id) 
        })),
        Err(e) => api_error(&e.to_string()),
    }
}

async fn skill_reject_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.skill_manager.reject_skill(&id, "admin", Some("Rejected by admin")).await {
        Ok(()) => api_success(serde_json::json!({
            "message": format!("Skill {} rejected", id)
        })),
        Err(e) => api_error(&e.to_string()),
    }
}

async fn skill_approval_subscribe_handler(
    State(state): State<Arc<Gateway>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.approval_broadcaster.subscribe();
    let stream = create_approval_stream(rx);
    Sse::new(stream)
}

async fn session_get_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.agent.get_session(&id) {
        Some(session) => Json(serde_json::json!({
            "success": true,
            "session": {
                "key": session.key,
                "lane": session.lane,
                "channel": session.metadata.channel,
                "message_count": session.messages.len(),
                "created_at": session.metadata.created_at,
                "updated_at": session.metadata.updated_at,
                "tool_calls_enabled": session.metadata.tool_calls_enabled,
            }
        })),
        None => Json(serde_json::json!({
            "success": false,
            "error": format!("Session not found: {}", id)
        }))
    }
}

async fn agent_list_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let sessions = state.agent.list_sessions();
    Json(serde_json::json!({
        "success": true,
        "agents": sessions.iter().map(|s| serde_json::json!({
            "session_key": s.key,
            "lane": s.lane,
            "channel": s.metadata.channel,
            "status": "active",
            "message_count": s.messages.len(),
            "created_at": s.metadata.created_at,
            "updated_at": s.metadata.updated_at,
        })).collect::<Vec<_>>()
    }))
}

async fn agent_get_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.agent.get_session(&id) {
        Some(session) => Json(serde_json::json!({
            "success": true,
            "agent": {
                "session_key": session.key,
                "lane": session.lane,
                "channel": session.metadata.channel,
                "status": "active",
                "message_count": session.messages.len(),
                "created_at": session.metadata.created_at,
                "updated_at": session.metadata.updated_at,
                "tool_calls_enabled": session.metadata.tool_calls_enabled,
                "messages": session.messages.iter().map(|m| serde_json::json!({
                    "role": match m.role {
                        rustclaw_protocol::Role::User => "user",
                        rustclaw_protocol::Role::Assistant => "assistant",
                        rustclaw_protocol::Role::System => "system",
                    },
"content": match &m.content {
                    rustclaw_protocol::Content::Text(s) => s.to_string(),
                    rustclaw_protocol::Content::Structured { text, .. } => text.as_deref().unwrap_or("").to_string(),
                    rustclaw_protocol::Content::Image { image_data, description, .. } => {
                        let desc = description.as_deref().filter(|d| !d.is_empty()).unwrap_or("图片消息");
                        format!("[Image: {}]", desc)
                    }
                }
                })).collect::<Vec<_>>()
            }
        })),
        None => Json(serde_json::json!({
            "success": false,
            "error": format!("Agent session not found: {}", id)
        }))
    }
}

async fn config_get_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "success": true,
        "config": {
            "gateway": {
                "host": state.full_config.gateway.host,
                "port": state.full_config.gateway.port,
                "device_id": state.full_config.gateway.device_id,
                "auth": {
                    "require_pairing": state.full_config.gateway.auth.require_pairing,
                    "allowed_devices": state.full_config.gateway.auth.allowed_devices,
                }
            },
            "agent": {
                "max_concurrent_sessions": state.full_config.agent.max_concurrent_sessions,
                "default_model": state.full_config.agent.default_model,
                "max_loop_iterations": state.full_config.agent.max_loop_iterations,
                "tool_timeout_ms": state.full_config.agent.tool_timeout_ms,
            },
            "tools": {
                "enabled_tools": state.full_config.tools.enabled_tools,
                "disabled_tools": state.full_config.tools.disabled_tools,
                "require_approval": state.full_config.tools.require_approval,
                "sandbox_enabled": state.full_config.tools.sandbox_enabled,
                "max_memory_mb": state.full_config.tools.max_memory_mb,
                "max_timeout_ms": state.full_config.tools.max_timeout_ms,
            },
            "providers": state.full_config.providers.iter().map(|p| serde_json::json!({
                "name": p.name,
                "provider_type": p.provider_type,
                "default_model": p.default_model,
                "base_url": p.base_url,
            })).collect::<Vec<_>>()
        }
    }))
}

async fn config_update_handler(
    State(_state): State<Arc<Gateway>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut updated_fields = vec![];
    
    if payload.get("agent").is_some() {
        updated_fields.push("agent");
    }
    if payload.get("tools").is_some() {
        updated_fields.push("tools");
    }
    if payload.get("providers").is_some() {
        updated_fields.push("providers");
    }
    
    if updated_fields.is_empty() {
        return api_error("No valid configuration fields provided for update");
    }
    
    api_success(serde_json::json!({
        "message": format!("Configuration update received for: {}", updated_fields.join(", ")),
        "note": "Configuration changes require gateway restart to take effect."
    }))
}

async fn config_save_handler(
    State(state): State<Arc<Gateway>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut config = state.full_config.clone();
    
    if let Some(agent) = payload.get("agent") {
        if let Some(model) = agent.get("default_model").and_then(|m| m.as_str()) {
            config.agent.default_model = model.to_string();
        }
        if let Some(iterations) = agent.get("max_loop_iterations").and_then(|i| i.as_u64()) {
            config.agent.max_loop_iterations = iterations as u32;
        }
        if let Some(timeout) = agent.get("tool_timeout_ms").and_then(|t| t.as_u64()) {
            config.agent.tool_timeout_ms = timeout;
        }
    }
    
    if let Some(tools) = payload.get("tools") {
        if let Some(enabled) = tools.get("enabled_tools").and_then(|e| e.as_array()) {
            config.tools.enabled_tools = enabled.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(disabled) = tools.get("disabled_tools").and_then(|d| d.as_array()) {
            config.tools.disabled_tools = disabled.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect();
        }
    }
    
    match config.save(Some(state.config_path.clone())) {
        Ok(()) => Json(serde_json::json!({
            "success": true,
            "message": "Configuration saved successfully",
            "path": state.config_path.to_string_lossy()
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "error": format!("Failed to save config: {}", e)
        }))
    }
}

async fn config_model_handler(
    State(state): State<Arc<Gateway>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let model = payload.get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("gpt-4o");
    
    let provider_type = payload.get("provider_type")
        .and_then(|p| p.as_str())
        .unwrap_or("openai");
    
    let mut config = state.full_config.clone();
    config.agent.default_model = model.to_string();
    
    for provider in &mut config.providers {
        if provider.provider_type == provider_type {
            provider.default_model = Some(model.to_string());
        }
    }
    
    match config.save(Some(state.config_path.clone())) {
        Ok(()) => api_success(serde_json::json!({
            "message": format!("Model switched to {} for {}", model, provider_type),
            "note": "Gateway restart required for changes to take effect"
        })),
        Err(e) => api_error(&format!("Failed to save config: {}", e))
    }
}

async fn channel_list_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let channels: Vec<_> = state.channels.read().values().cloned().collect();
    api_success(serde_json::json!({
        "channels": channels.iter().map(|c| serde_json::json!({
            "name": c.name,
            "type": c.channel_type,
            "enabled": c.enabled,
            "config": c.config
        })).collect::<Vec<_>>()
    }))
}

async fn channel_get_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.channels.read().get(&name) {
        Some(channel) => Json(serde_json::json!({
            "success": true,
            "channel": {
                "name": channel.name,
                "type": channel.channel_type,
                "enabled": channel.enabled,
                "config": channel.config
            }
        })),
        None => Json(serde_json::json!({
            "success": false,
            "error": format!("Channel not found: {}", name)
        }))
    }
}

async fn channel_update_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut channels = state.channels.write();
    
    match channels.get_mut(&name) {
        Some(channel) => {
            if let Some(enabled) = payload.get("enabled").and_then(|e| e.as_bool()) {
                channel.enabled = enabled;
            }
            if let Some(config) = payload.get("config").and_then(|c| c.as_object()) {
                for (k, v) in config {
                    channel.config.insert(k.clone(), v.clone());
                }
            }
            Json(serde_json::json!({
                "success": true,
                "message": format!("Channel {} updated", name)
            }))
        }
        None => Json(serde_json::json!({
            "success": false,
            "error": format!("Channel not found: {}", name)
        }))
    }
}

async fn security_audit_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let mut findings: Vec<serde_json::Value> = vec![];
    let mut score = 100;
    
    if !state.full_config.tools.sandbox_enabled {
        findings.push(serde_json::json!({
            "severity": "warning",
            "message": "Sandbox execution is disabled",
            "recommendation": "Enable sandbox for safer tool execution"
        }));
        score -= 20;
    }
    
    if state.full_config.gateway.auth.allowed_devices.is_empty() {
        findings.push(serde_json::json!({
            "severity": "info",
            "message": "No devices are paired yet",
            "recommendation": "Pair devices for secure access"
        }));
    }
    
    for tool in &state.full_config.tools.require_approval {
        findings.push(serde_json::json!({
            "severity": "info",
            "message": format!("Tool '{}' requires approval", tool),
            "recommendation": "Consider removing approval for trusted tools"
        }));
    }
    
    for tool in &state.full_config.tools.disabled_tools {
        findings.push(serde_json::json!({
            "severity": "info",
            "message": format!("Tool '{}' is disabled", tool)
        }));
    }
    
    if state.full_config.tools.max_timeout_ms > 60000 {
        findings.push(serde_json::json!({
            "severity": "warning",
            "message": "Tool timeout is high",
            "recommendation": "Consider reducing timeout for better security"
        }));
        score -= 10;
    }
    
    let has_api_key = state.full_config.providers.iter().any(|p| p.api_key.is_some());
    if !has_api_key {
        findings.push(serde_json::json!({
            "severity": "error",
            "message": "No API key configured",
            "recommendation": "Configure API key for LLM provider"
        }));
        score -= 30;
    }
    
    Json(serde_json::json!({
        "success": true,
        "audit": {
            "score": score,
            "status": if score >= 80 { "good" } else if score >= 50 { "moderate" } else { "poor" },
            "findings": findings,
            "checks": {
                "sandbox_enabled": state.full_config.tools.sandbox_enabled,
                "auth_required": state.full_config.gateway.auth.require_pairing,
                "api_key_configured": has_api_key,
                "approval_tools": state.full_config.tools.require_approval.len(),
                "disabled_tools": state.full_config.tools.disabled_tools.len()
            }
        }
    }))
}

async fn memory_status_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let sessions = state.agent.list_sessions();
    let total_messages: usize = sessions.iter().map(|s| s.messages.len()).sum();
    let total_sessions = sessions.len();
    
    let lanes: std::collections::HashSet<String> = sessions.iter().map(|s| s.lane.clone()).collect();
    
    let sessions_per_lane: std::collections::HashMap<String, usize> = lanes
        .iter()
        .map(|lane| {
            let count = sessions.iter().filter(|s| s.lane == *lane).count();
            (lane.clone(), count)
        })
        .collect();
    
    Json(serde_json::json!({
        "success": true,
        "memory": {
            "total_sessions": total_sessions,
            "total_messages": total_messages,
            "active_lanes": lanes.len(),
            "sessions_per_lane": sessions_per_lane,
            "oldest_session": sessions.iter().map(|s| s.metadata.created_at).min(),
            "newest_session": sessions.iter().map(|s| s.metadata.created_at).max(),
        }
    }))
}

/// 飞书通道状态查询
async fn feishu_status_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let feishu_config = state.full_config.channels.feishu.as_ref();
    let is_configured = feishu_config.is_some() && feishu_config.map(|c| !c.app_id.is_empty()).unwrap_or(false);
    let is_connected = state.feishu_ws.as_ref().map(|ws| ws.is_connected()).unwrap_or(false);

    Json(serde_json::json!({
        "success": true,
        "feishu": {
            "configured": is_configured,
            "connected": is_connected,
            "dm_enabled": feishu_config.map(|c| c.dm_enabled).unwrap_or(false),
            "group_enabled": feishu_config.map(|c| c.group_enabled).unwrap_or(false),
            "allowed_groups_count": feishu_config.map(|c| c.allowed_groups.len()).unwrap_or(0),
            "allowed_users_count": feishu_config.map(|c| c.allowed_users.len()).unwrap_or(0),
            "group_configs_count": feishu_config.and_then(|c| c.group_configs.as_ref()).map(|c| c.len()).unwrap_or(0),
        }
    }))
}

async fn memory_stats_handler(
    State(state): State<Arc<Gateway>>,
) -> Json<serde_json::Value> {
    let sessions = state.agent.list_sessions();
    
    let mut stats = std::collections::HashMap::new();
    stats.insert("total_sessions", sessions.len());
    stats.insert("total_messages", sessions.iter().map(|s| s.messages.len()).sum());
    
    let by_channel: std::collections::HashMap<String, usize> = sessions
        .iter()
        .fold(std::collections::HashMap::new(), |mut acc, s| {
            *acc.entry(s.metadata.channel.clone()).or_insert(0) += 1;
            acc
        });
    
    let by_lane: std::collections::HashMap<String, usize> = sessions
        .iter()
        .fold(std::collections::HashMap::new(), |mut acc, s| {
            *acc.entry(s.lane.clone()).or_insert(0) += 1;
            acc
        });
    
    Json(serde_json::json!({
        "success": true,
        "stats": {
            "total_sessions": stats.get("total_sessions"),
            "total_messages": stats.get("total_messages"),
            "by_channel": by_channel,
            "by_lane": by_lane,
            "config": {
                "max_concurrent_sessions": state.full_config.agent.max_concurrent_sessions,
                "max_loop_iterations": state.full_config.agent.max_loop_iterations,
                "tool_timeout_ms": state.full_config.agent.tool_timeout_ms,
            }
        }
    }))
}

async fn system_version_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "success": true,
        "version": {
            "current": env!("CARGO_PKG_VERSION"),
            "name": env!("CARGO_PKG_NAME"),
            "authors": env!("CARGO_PKG_AUTHORS"),
            "license": env!("CARGO_PKG_LICENSE"),
        }
    }))
}

async fn system_update_check_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "success": true,
        "update": {
            "current_version": env!("CARGO_PKG_VERSION"),
            "latest_version": "check github releases",
            "update_available": false,
            "update_command": "cargo install --git https://github.com/Sam20180103/myclaw.git",
            "repository": "https://github.com/Sam20180103/myclaw"
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclaw_protocol::{Message, Role, Content};

    #[test]
    fn test_gateway_session_structure() {
        let session = GatewaySession {
            id: "session-1".to_string(),
            channel: "http".to_string(),
            authenticated: true,
            agent_session_key: Some("agent-key".to_string()),
        };
        assert_eq!(session.channel, "http");
        assert_eq!(session.id, "session-1");
        assert!(session.authenticated);
        assert!(session.agent_session_key.is_some());
    }

    #[test]
    fn test_gateway_config_from_full_config() {
        let config = Config::default();
        assert_eq!(config.gateway.host, "127.0.0.1");
        assert_eq!(config.gateway.port, 18789);
    }

    #[test]
    fn test_health_response_structure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let response = rt.block_on(health_handler());
        let body = serde_json::to_string(&response.0).unwrap();
        assert!(body.contains("ok"));
        assert!(body.contains("rustclaw-gateway"));
    }

    #[test]
    fn test_system_version_response() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let response = rt.block_on(system_version_handler());
        let body = serde_json::to_string(&response.0).unwrap();
        assert!(body.contains("version"));
        assert!(body.contains("rustclaw"));
    }

    #[test]
    fn test_system_update_check_response() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let response = rt.block_on(system_update_check_handler());
        let body = serde_json::to_string(&response.0).unwrap();
        assert!(body.contains("update"));
        assert!(body.contains("current_version"));
    }

#[test]
fn test_envelope_creation() {
let message = Message {
role: Role::User,
content: Content::text("hello"),
};
let envelope = Envelope::new("http", "session-123", message);
assert_eq!(envelope.channel, "http");
assert_eq!(envelope.session_key, "session-123");
assert!(!envelope.id.is_empty());
}
}

async fn api_key_auth(
    State(gateway): State<Arc<Gateway>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    // Skip auth for health endpoint
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    // If API key is configured, validate it
    if let Some(ref expected_key) = gateway.full_config.gateway.auth.api_key {
        let auth_header = request.headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());

        match auth_header {
            Some(header) if header.starts_with("Bearer ") => {
                let token = &header[7..];
                if token != expected_key {
                    warn!("Invalid API key attempt");
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            _ => {
                warn!("Missing or invalid Authorization header");
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    Ok(next.run(request).await)
}
