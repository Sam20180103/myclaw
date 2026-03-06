use parking_lot::RwLock;
use rustclaw_protocol::Message;
use std::collections::HashMap;
use std::sync::Arc;

pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

#[derive(Clone)]
pub struct Session {
    pub key: String,
    pub channel: String,
    pub messages: Vec<Message>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get(&self, key: &str) -> Option<Session> {
        self.sessions.read().get(key).cloned()
    }

    pub fn insert(&self, session: Session) {
        self.sessions.write().insert(session.key.clone(), session);
    }

    pub fn remove(&self, key: &str) {
        self.sessions.write().remove(key);
    }

    pub fn list(&self) -> Vec<Session> {
        self.sessions.read().values().cloned().collect()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
