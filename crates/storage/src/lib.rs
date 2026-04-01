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

    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.sessions.read().contains_key(key)
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclaw_protocol::{Content, Role};

    #[test]
    fn test_session_store_new() {
        let store = SessionStore::new();
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_session_store_insert_and_get() {
        let store = SessionStore::new();
        let session = Session {
            key: "test-1".to_string(),
            channel: "http".to_string(),
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        };
        store.insert(session);
        let retrieved = store.get("test-1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().channel, "http");
    }

    #[test]
    fn test_session_store_remove() {
        let store = SessionStore::new();
        let session = Session {
            key: "test-2".to_string(),
            channel: "ws".to_string(),
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        };
        store.insert(session);
        assert!(store.contains("test-2"));
        store.remove("test-2");
        assert!(!store.contains("test-2"));
    }

    #[test]
    fn test_session_store_list() {
        let store = SessionStore::new();
        for i in 0..3 {
            store.insert(Session {
                key: format!("session-{}", i),
                channel: "http".to_string(),
                messages: vec![],
                created_at: 1000,
                updated_at: 1000,
            });
        }
        let sessions = store.list();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn test_session_store_count() {
        let store = SessionStore::new();
        assert_eq!(store.count(), 0);
        store.insert(Session {
            key: "s1".to_string(),
            channel: "http".to_string(),
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        });
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn test_session_with_messages() {
        let store = SessionStore::new();
        let msg = rustclaw_protocol::Message {
            role: Role::User,
            content: Content::text("hello"),
        };
        store.insert(Session {
            key: "msg-session".to_string(),
            channel: "http".to_string(),
            messages: vec![msg],
            created_at: 1000,
            updated_at: 1000,
        });
        let session = store.get("msg-session").unwrap();
        assert_eq!(session.messages.len(), 1);
    }
}
