use axum::{
    extract::State,
    routing::get,
    Json, Router,
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::convert::Infallible;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, error};

use rustclaw_protocol::{JsonRpcRequest, Envelope, Message, Role, Content};
use rustclaw_agent::{AgentRuntime, AgentConfig, AgentResponse};
use rustclaw_skill::{SkillManager, SkillConfig, SkillLanguage, SkillApprovalRequest};
use rustclaw_config::{Config, ProviderConfig};
use rustclaw_provider::{ProviderClient, OpenAiProvider, AnthropicProvider, LlmProvider};

const APPROVAL_CHANNEL_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct Gateway {
    config: GatewayConfig,
    sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
    agent: Arc<AgentRuntime>,
    skill_manager: Arc<SkillManager>,
    approval_broadcaster: broadcast::Sender<SkillApprovalRequest>,
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
        let agent = AgentRuntime::new(agent_config)
            .with_provider(provider_client);

        let gateway_config = GatewayConfig {
            host: config.gateway.host,
            port: config.gateway.port,
            device_id: config.gateway.device_id,
        };
        let skill_manager = SkillManager::new(SkillConfig::default());
        let (approval_tx, _) = broadcast::channel(APPROVAL_CHANNEL_CAPACITY);

        let skill_manager = Arc::new(skill_manager);

        Self {
            config: gateway_config,
            sessions: Arc::new(RwLock::new(HashMap::with_capacity(64))),
            agent: Arc::new(agent),
            skill_manager,
            approval_broadcaster: approval_tx,
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
        
        let addr = SocketAddr::new(self.config.host.parse().unwrap(), self.config.port);

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/api/message", axum::routing::post(message_handler))
            .route("/api/session", axum::routing::get(session_list_handler))
            .route("/api/session/{id}", axum::routing::delete(session_delete_handler))
            .route("/api/skill", axum::routing::get(skill_list_handler))
            .route("/api/skill", axum::routing::post(skill_create_handler))
            .route("/api/skill/{id}/approve", axum::routing::post(skill_approve_handler))
            .route("/api/skill/{id}/reject", axum::routing::post(skill_reject_handler))
            .route("/api/skill/approval/subscribe", get(skill_approval_subscribe_handler))
            .layer(cors)
            .with_state(Arc::new(self.clone()));

        info!("Starting Gateway on {}", addr);

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
    Json(serde_json::json!({ "success": true, "deleted": id }))
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

                    match state.agent.process(envelope).await {
                        Ok(AgentResponse { session_key, message, tool_calls }) => {
                            let response_content = match message.content {
                                Content::Text(s) => s,
                                Content::Structured { text, .. } => text.unwrap_or_default(),
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
        Ok(()) => Json(serde_json::json!({ 
            "success": true, 
            "message": format!("Skill {} approved", id) 
        })),
        Err(e) => Json(serde_json::json!({ 
            "success": false, 
            "error": e.to_string() 
        })),
    }
}

async fn skill_reject_handler(
    State(state): State<Arc<Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.skill_manager.reject_skill(&id, "admin", Some("Rejected by admin")).await {
        Ok(()) => Json(serde_json::json!({
            "success": true,
            "message": format!("Skill {} rejected", id)
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "error": e.to_string()
        })),
    }
}

async fn skill_approval_subscribe_handler(
    State(state): State<Arc<Gateway>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.approval_broadcaster.subscribe();
    let stream = create_approval_stream(rx);
    Sse::new(stream)
}
