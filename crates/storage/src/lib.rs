use parking_lot::RwLock;
use rusqlite::{Connection, OptionalExtension};
use rustclaw_protocol::Message;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// 内存会话存储 — 基于 HashMap 的临时会话存储 (重启丢失)
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

/// 会话数据结构 — 用于内存和 SQLite 存储
#[derive(Clone)]
pub struct Session {
    pub key: String,                       // 会话唯一标识
    pub channel: String,                   // 通道类型
    pub parent_session_id: Option<String>, // 父会话ID (谱系追踪)
    pub messages: Vec<Message>,            // 消息历史
    pub created_at: i64,                   // 创建时间戳 (毫秒)
    pub updated_at: i64,                   // 更新时间戳 (毫秒)
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

/// SQLite 会话存储 — 持久化会话到 SQLite 数据库
/// 使用 WAL 模式实现并发安全, 支持会话搜索和谱系追踪
pub struct SQLiteSessionStore {
    #[allow(clippy::arc_with_non_send_sync)]
    conn: Arc<RwLock<Connection>>,
}

impl SQLiteSessionStore {
    /// 创建新的 SQLite 会话存储
    /// 自动创建父目录, 初始化 WAL 模式和数据库表
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)?;
        // 初始化数据库: WAL模式 + 正常同步 + 创建表和索引
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 key TEXT PRIMARY KEY,
                 channel TEXT NOT NULL,
                 parent_session_id TEXT,
                 messages TEXT NOT NULL DEFAULT '[]',
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at);
             CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel);
             CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);",
        )?;
        Ok(Self {
            conn: Arc::new(RwLock::new(conn)),
        })
    }

    /// 获取会话 — 从 SQLite 查询并反序列化消息
    pub fn get(&self, key: &str) -> Option<Session> {
        let conn = self.conn.read();
        conn.query_row(
            "SELECT key, channel, parent_session_id, messages, created_at, updated_at FROM sessions WHERE key = ?",
            [key],
            |row| {
                let messages_str: String = row.get(3)?;
                let parent_id: Option<String> = row.get(2).ok();
                let messages: Vec<Message> =
                    serde_json::from_str(&messages_str).unwrap_or_default();
                Ok(Session {
                    key: row.get(0)?,
                    channel: row.get(1)?,
                    parent_session_id: parent_id,
                    messages,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn insert(&self, session: &Session) -> Result<(), rusqlite::Error> {
        let messages_json = serde_json::to_string(&session.messages).unwrap_or_default();
        let conn = self.conn.write();
        conn.execute(
            "INSERT OR REPLACE INTO sessions (key, channel, parent_session_id, messages, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                session.key,
                session.channel,
                session.parent_session_id.clone().unwrap_or_default(),
                messages_json,
                session.created_at,
                session.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn add_message(&self, key: &str, message: &Message) -> Result<(), rusqlite::Error> {
        let conn = self.conn.write();
        let messages_str: Option<String> = conn
            .query_row(
                "SELECT messages FROM sessions WHERE key = ?",
                [key],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let mut messages: Vec<Message> = messages_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        messages.push(message.clone());
        let messages_json = serde_json::to_string(&messages).unwrap_or_default();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE sessions SET messages = ?, updated_at = ? WHERE key = ?",
            rusqlite::params![messages_json, now, key],
        )?;
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.write();
        conn.execute("DELETE FROM sessions WHERE key = ?", [key])?;
        Ok(())
    }

    pub fn list(&self) -> Vec<Session> {
        let conn = self.conn.read();
        let mut stmt = match conn
            .prepare("SELECT key, channel, parent_session_id, messages, created_at, updated_at FROM sessions")
        {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let sessions = match stmt.query_map([], |row| {
            let messages_str: String = row.get(3)?;
            let parent_id: Option<String> = row.get(2).ok();
            let messages: Vec<Message> = serde_json::from_str(&messages_str).unwrap_or_default();
            Ok(Session {
                key: row.get(0)?,
                channel: row.get(1)?,
                parent_session_id: parent_id,
                messages,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        }) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        sessions.filter_map(|r| r.ok()).collect()
    }

    pub fn count(&self) -> usize {
        let conn = self.conn.read();
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap_or(0)
    }

    pub fn cleanup_expired(&self, ttl_seconds: i64) -> Result<usize, rusqlite::Error> {
        let now = chrono::Utc::now().timestamp_millis();
        let cutoff = now - (ttl_seconds * 1000);
        let conn = self.conn.write();
        let deleted = conn.execute(
            "DELETE FROM sessions WHERE updated_at < ?",
            [cutoff.to_string()],
        )?;
        Ok(deleted)
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Session> {
        let conn = self.conn.read();
        let sql = "SELECT key, channel, parent_session_id, messages, created_at, updated_at
             FROM sessions
             WHERE messages LIKE ?
             ORDER BY updated_at DESC
             LIMIT ?";
        let pattern = format!("%{}%", query);
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let sessions = match stmt.query_map([&pattern, &limit.to_string()], |row| {
            let messages_str: String = row.get(3)?;
            let parent_id: Option<String> = row.get(2).ok();
            let messages: Vec<Message> = serde_json::from_str(&messages_str).unwrap_or_default();
            Ok(Session {
                key: row.get(0)?,
                channel: row.get(1)?,
                parent_session_id: parent_id,
                messages,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        }) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        sessions.filter_map(|r| r.ok()).collect()
    }

    pub fn get_lineage(&self, session_key: &str) -> Vec<Session> {
        let mut lineage = Vec::new();
        let mut current_key = Some(session_key.to_string());
        while let Some(key) = current_key {
            if let Some(session) = self.get(&key) {
                lineage.push(session.clone());
                current_key = session.parent_session_id.clone();
            } else {
                break;
            }
        }
        lineage
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
            parent_session_id: None,
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
            parent_session_id: None,
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
                parent_session_id: None,
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
            parent_session_id: None,
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
            parent_session_id: None,
            messages: vec![msg],
            created_at: 1000,
            updated_at: 1000,
        });
        let session = store.get("msg-session").unwrap();
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn test_sqlite_session_store_create() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        assert_eq!(store.count(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_insert_and_get() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        let session = Session {
            key: "sqlite-1".to_string(),
            channel: "http".to_string(),
            parent_session_id: None,
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        };
        store.insert(&session).unwrap();
        let retrieved = store.get("sqlite-1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().channel, "http");
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_add_message() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        let session = Session {
            key: "sqlite-msg".to_string(),
            channel: "ws".to_string(),
            parent_session_id: None,
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        };
        store.insert(&session).unwrap();
        store
            .add_message(
                "sqlite-msg",
                &Message {
                    role: Role::User,
                    content: Content::text("hello sqlite"),
                },
            )
            .unwrap();
        let retrieved = store.get("sqlite-msg").unwrap();
        assert_eq!(retrieved.messages.len(), 1);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_remove() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        let session = Session {
            key: "sqlite-del".to_string(),
            channel: "http".to_string(),
            parent_session_id: None,
            messages: vec![],
            created_at: 1000,
            updated_at: 1000,
        };
        store.insert(&session).unwrap();
        assert_eq!(store.count(), 1);
        store.remove("sqlite-del").unwrap();
        assert_eq!(store.count(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_list() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        for i in 0..3 {
            store
                .insert(&Session {
                    key: format!("list-{}", i),
                    channel: "http".to_string(),
                    parent_session_id: None,
                    messages: vec![],
                    created_at: 1000,
                    updated_at: 1000,
                })
                .unwrap();
        }
        assert_eq!(store.list().len(), 3);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_search() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        let msg = Message {
            role: Role::User,
            content: Content::text("hello world"),
        };
        store
            .insert(&Session {
                key: "search-1".to_string(),
                channel: "http".to_string(),
                parent_session_id: None,
                messages: vec![msg],
                created_at: 1000,
                updated_at: 1000,
            })
            .unwrap();
        store
            .insert(&Session {
                key: "search-2".to_string(),
                channel: "http".to_string(),
                parent_session_id: None,
                messages: vec![],
                created_at: 1000,
                updated_at: 1000,
            })
            .unwrap();
        let results = store.search("hello", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "search-1");
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_sqlite_session_store_lineage() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_test_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("test.db");
        let store = SQLiteSessionStore::new(&db_path).unwrap();
        let parent_id = "parent-session".to_string();
        store
            .insert(&Session {
                key: parent_id.clone(),
                channel: "http".to_string(),
                parent_session_id: None,
                messages: vec![],
                created_at: 1000,
                updated_at: 1000,
            })
            .unwrap();
        store
            .insert(&Session {
                key: "child-session".to_string(),
                channel: "http".to_string(),
                parent_session_id: Some(parent_id.clone()),
                messages: vec![],
                created_at: 2000,
                updated_at: 2000,
            })
            .unwrap();
        let lineage = store.get_lineage("child-session");
        assert_eq!(lineage.len(), 2);
        assert_eq!(lineage[0].key, "child-session");
        assert_eq!(lineage[1].key, "parent-session");
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
