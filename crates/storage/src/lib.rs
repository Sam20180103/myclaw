use parking_lot::{Mutex, RwLock};
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
    conn: Arc<Mutex<Connection>>,
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
             CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);
             CREATE TABLE IF NOT EXISTS processed_events (
                 event_id TEXT PRIMARY KEY,
                 seen_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_events_seen ON processed_events(seen_at);",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 获取会话 — 从 SQLite 查询并反序列化消息
    pub fn get(&self, key: &str) -> Option<Session> {
        let conn = self.conn.lock();
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
        let conn = self.conn.lock();
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
        let conn = self.conn.lock();
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
        let conn = self.conn.lock();
        conn.execute("DELETE FROM sessions WHERE key = ?", [key])?;
        Ok(())
    }

    pub fn list(&self) -> Vec<Session> {
        let conn = self.conn.lock();
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
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap_or(0)
    }

    pub fn cleanup_expired(&self, ttl_seconds: i64) -> Result<usize, rusqlite::Error> {
        let now = chrono::Utc::now().timestamp_millis();
        let cutoff = now - (ttl_seconds * 1000);
        let conn = self.conn.lock();
        let deleted = conn.execute(
            "DELETE FROM sessions WHERE updated_at < ?",
            [cutoff.to_string()],
        )?;
        Ok(deleted)
    }

    /// P0-4: 入站事件判重。返回 `true` 表示这是**首次**见到该 event_id (应当处理),
    /// `false` 表示重复投递 (飞书事件重推 / Webhook 重试),必须跳过。
    ///
    /// 依赖 SQLite 主键的原子性,并发调用下也只有一个调用方拿到 `true`。
    /// 写库失败时返回 `true` (放行) — 宁可重复一次,也不因存储故障丢消息。
    pub fn mark_event_seen(&self, event_id: &str) -> bool {
        if event_id.is_empty() {
            return true;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.conn.lock();
        match conn.execute(
            "INSERT OR IGNORE INTO processed_events (event_id, seen_at) VALUES (?, ?)",
            rusqlite::params![event_id, now],
        ) {
            Ok(rows) => rows > 0,
            Err(e) => {
                tracing::warn!("mark_event_seen failed for {}: {}", event_id, e);
                true
            }
        }
    }

    /// P0-4: 清理过期的判重记录 (默认保留 7 天足以覆盖任何平台的重推窗口)。
    pub fn cleanup_events(&self, ttl_seconds: i64) -> Result<usize, rusqlite::Error> {
        let cutoff = chrono::Utc::now().timestamp_millis() - (ttl_seconds * 1000);
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM processed_events WHERE seen_at < ?",
            [cutoff.to_string()],
        )
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Session> {
        let conn = self.conn.lock();
        let sql = "SELECT key, channel, parent_session_id, messages, created_at, updated_at
             FROM sessions
             WHERE messages LIKE ?
             ORDER BY updated_at DESC
             LIMIT ?";
        let pattern = format!("%{}%", query);
        let mut stmt = match conn.prepare(sql) {
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

/// G11: 把会话导出为 ShareGPT 训练样本。
///
/// 映射: System→system, User→human, Assistant→gpt (ToolUse 附带 tool_calls JSON),
/// Tool→tool (含 tool_call_id 配对), Image→占位文本。空会话返回 None。
pub fn session_to_sharegpt(session: &Session) -> Option<serde_json::Value> {
    use rustclaw_protocol::{Content, Role};
    if session.messages.is_empty() {
        return None;
    }
    let conversations: Vec<serde_json::Value> = session
        .messages
        .iter()
        .map(|m| {
            let from = match m.role {
                Role::System => "system",
                Role::User => "human",
                Role::Assistant => "gpt",
                Role::Tool => "tool",
            };
            let value = match &m.content {
                Content::Text(t) => t.clone(),
                Content::Structured { text, .. } => text.clone().unwrap_or_default(),
                Content::Image { description, .. } => format!(
                    "[Image: {}]",
                    description.as_deref().filter(|d| !d.is_empty()).unwrap_or("image")
                ),
                Content::ToolUse { tool_calls, text } => {
                    let calls = serde_json::to_string(tool_calls).unwrap_or_default();
                    match text.as_deref().filter(|t| !t.is_empty()) {
                        Some(t) => format!("{}\n<tool_calls>{}</tool_calls>", t, calls),
                        None => format!("<tool_calls>{}</tool_calls>", calls),
                    }
                }
                Content::ToolResult { tool_call_id, name, output } => {
                    format!("<tool_result id=\"{}\" name=\"{}\">{}</tool_result>", tool_call_id, name, output)
                }
            };
            serde_json::json!({ "from": from, "value": value })
        })
        .collect();
    Some(serde_json::json!({
        "id": session.key,
        "channel": session.channel,
        "parent_session_id": session.parent_session_id,
        "conversations": conversations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclaw_protocol::{Content, Role};

    // G11: ShareGPT 导出
    #[test]
    fn test_g11_session_to_sharegpt() {
        use rustclaw_protocol::ToolCallSpec;
        let session = Session {
            key: "s1".to_string(),
            channel: "http".to_string(),
            parent_session_id: None,
            messages: vec![
                Message { role: Role::User, content: Content::text("查热门") },
                Message {
                    role: Role::Assistant,
                    content: Content::tool_use(
                        vec![ToolCallSpec { id: "c1".into(), name: "xhs_reach".into(), arguments: serde_json::json!({"action":"hot"}) }],
                        Some("let me check".into()),
                    ),
                },
                Message { role: Role::Tool, content: Content::tool_result("c1", "xhs_reach", "{\"ok\":true}") },
                Message { role: Role::Assistant, content: Content::text("done") },
            ],
            created_at: 0,
            updated_at: 0,
        };
        let v = session_to_sharegpt(&session).unwrap();
        let conv = v["conversations"].as_array().unwrap();
        assert_eq!(conv.len(), 4);
        assert_eq!(conv[0]["from"], "human");
        assert_eq!(conv[1]["from"], "gpt");
        assert!(conv[1]["value"].as_str().unwrap().contains("<tool_calls>"));
        assert!(conv[1]["value"].as_str().unwrap().contains("xhs_reach"));
        assert_eq!(conv[2]["from"], "tool");
        assert!(conv[2]["value"].as_str().unwrap().contains("id=\"c1\""));
        assert_eq!(conv[3]["value"], "done");
        assert_eq!(v["id"], "s1");
    }

    #[test]
    fn test_g11_empty_session_skipped() {
        let session = Session {
            key: "empty".to_string(),
            channel: "http".to_string(),
            parent_session_id: None,
            messages: vec![],
            created_at: 0,
            updated_at: 0,
        };
        assert!(session_to_sharegpt(&session).is_none());
    }

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

    #[test]
    fn test_mark_event_seen_is_idempotent() {
        // P0-4: 同一 event_id 只允许处理一次 (飞书重推 / Webhook 重试)
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_events_{}", uuid::Uuid::new_v4()));
        let store = SQLiteSessionStore::new(temp_dir.join("state.db")).unwrap();

        assert!(store.mark_event_seen("om_msg_1"), "首次必须放行");
        assert!(!store.mark_event_seen("om_msg_1"), "重复投递必须拦截");
        assert!(store.mark_event_seen("om_msg_2"), "不同事件互不影响");
        assert!(store.mark_event_seen(""), "空 id 不判重, 直接放行");

        assert_eq!(store.cleanup_events(-1).unwrap(), 2, "清理应删掉两条记录");
        assert!(store.mark_event_seen("om_msg_1"), "清理后可再次处理");

        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
