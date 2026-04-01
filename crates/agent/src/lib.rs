use rustclaw_protocol::{Message, Envelope, Role, Content};
use rustclaw_tool::{ToolRegistry, ToolCall};
use rustclaw_provider::ProviderClient;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use tokio::sync::mpsc;
use futures_util::stream::StreamExt;

pub struct AgentRuntime {
    config: AgentConfig,
    session_manager: SessionManager,
    lane_router: LaneRouter,
    tool_registry: Arc<ToolRegistry>,
    provider_client: ProviderClient,
}

#[derive(Clone)]
pub struct AgentConfig {
    pub max_loop_iterations: u32,
    pub tool_timeout_ms: u64,
    pub session_ttl_seconds: u64,
    pub max_concurrent_per_lane: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_loop_iterations: 100,
            tool_timeout_ms: 60000,
            session_ttl_seconds: 3600,
            max_concurrent_per_lane: 10,
        }
    }
}

#[derive(Clone)]
pub struct Session {
    pub key: String,
    pub lane: String,
    pub messages: Vec<Message>,
    pub metadata: SessionMetadata,
    pub last_accessed: Instant,
}

#[derive(Clone)]
pub struct SessionMetadata {
    pub channel: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub tool_calls_enabled: bool,
}

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    #[allow(dead_code)]
    max_sessions: usize,
    ttl_seconds: u64,
}

impl SessionManager {
    pub fn new(max_sessions: usize, ttl_seconds: u64) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::with_capacity(max_sessions))),
            max_sessions,
            ttl_seconds,
        }
    }

    pub fn create(&self, channel: &str, lane: &str) -> Session {
        let session = Session {
            key: Uuid::new_v4().to_string(),
            lane: lane.to_string(),
            messages: Vec::with_capacity(16),
            metadata: SessionMetadata {
                channel: channel.to_string(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                tool_calls_enabled: true,
            },
            last_accessed: Instant::now(),
        };
        self.sessions.write().insert(session.key.clone(), session.clone());
        session
    }

    pub fn get(&self, key: &str) -> Option<Session> {
        self.sessions.read().get(key).cloned()
    }

    pub fn get_or_create(&self, key: &str, channel: &str, lane: &str) -> Session {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get(key) {
            let mut session = session.clone();
            session.last_accessed = Instant::now();
            sessions.insert(key.to_string(), session.clone());
            return session;
        }
        let session = Session {
            key: key.to_string(),
            lane: lane.to_string(),
            messages: Vec::with_capacity(16),
            metadata: SessionMetadata {
                channel: channel.to_string(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                tool_calls_enabled: true,
            },
            last_accessed: Instant::now(),
        };
        sessions.insert(key.to_string(), session.clone());
        session
    }

    pub fn add_message(&self, key: &str, message: Message) {
        if let Some(session) = self.sessions.write().get_mut(key) {
            session.messages.push(message);
            session.metadata.updated_at = chrono::Utc::now().timestamp_millis();
            session.last_accessed = Instant::now();
        }
    }

    pub fn delete(&self, key: &str) {
        self.sessions.write().remove(key);
    }

    pub fn cleanup_expired(&self) -> usize {
        let ttl = Duration::from_secs(self.ttl_seconds);
        let now = Instant::now();
        let mut to_remove = vec![];
        
        {
            let sessions = self.sessions.read();
            for (key, session) in sessions.iter() {
                if now.duration_since(session.last_accessed) > ttl {
                    to_remove.push(key.clone());
                }
            }
        }
        
        for key in &to_remove {
            self.sessions.write().remove(key);
        }
        to_remove.len()
    }

    pub fn list(&self) -> Vec<Session> {
        self.sessions.read().values().cloned().collect()
    }

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
        registry.register_builtin_tools();
        Self {
            config: config.clone(),
            session_manager: SessionManager::new(100, config.session_ttl_seconds),
            lane_router: LaneRouter::new(config.max_concurrent_per_lane),
            tool_registry: registry,
            provider_client: ProviderClient::new(None),
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

    pub async fn process(&self, envelope: Envelope) -> Result<AgentResponse, AgentError> {
        let session_key = &envelope.session_key;
        let lane = self.lane_router.route(session_key);
        
        let _session = self.session_manager.get_or_create(
            session_key,
            &envelope.channel,
            &lane,
        );

        self.session_manager.add_message(session_key, envelope.message);

        let messages = self.session_manager.get(session_key)
            .map(|s| s.messages.clone())
            .unwrap_or_default();

        let response_text = match self.provider_client.complete(messages).await {
            Ok(text) => text,
            Err(e) => return Err(AgentError::ProviderError(e.to_string())),
        };

        let tool_calls = self.parse_tool_calls(&response_text);

        Ok(AgentResponse {
            session_key: session_key.to_string(),
            message: Message {
                role: Role::Assistant,
                content: Content::text(response_text),
            },
            tool_calls,
        })
    }

    pub async fn process_with_loop(&self, envelope: Envelope) -> Result<AgentResponse, AgentError> {
        let session_key = envelope.session_key.clone();
        let lane = self.lane_router.route(&session_key);
        
        self.session_manager.get_or_create(&session_key, &envelope.channel, &lane);
        self.session_manager.add_message(&session_key, envelope.message);

        let mut iterations = 0;

        loop {
            if iterations >= self.config.max_loop_iterations {
                return Err(AgentError::MaxIterationsExceeded);
            }

            let messages = self.session_manager.get(&session_key)
                .map(|s| s.messages.clone())
                .unwrap_or_default();

            let response_text = match self.provider_client.complete(messages).await {
                Ok(text) => text,
                Err(e) => return Err(AgentError::ProviderError(e.to_string())),
            };

            let tool_calls = self.parse_tool_calls(&response_text);

            if tool_calls.is_empty() {
                return Ok(AgentResponse {
                    session_key,
                    message: Message {
                        role: Role::Assistant,
                        content: Content::text(response_text),
                    },
                    tool_calls: vec![],
                });
            }

            for tool_call in &tool_calls {
                match self.execute_tool(tool_call).await {
                    Ok(result) => {
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
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(response) {
            if let Some(calls) = json.get("tool_calls").and_then(|v| v.as_array()) {
                return calls.iter().filter_map(|c| {
                    Some(ToolCall {
                        id: c.get("id")?.as_str()?.to_string(),
                        name: c.get("function")?.get("name")?.as_str()?.to_string(),
                        arguments: c.get("function")?.get("arguments").cloned().unwrap_or(serde_json::Value::Null),
                    })
                }).collect();
            }
        }
        vec![]
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
            .map(|s| s.messages.clone())
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
