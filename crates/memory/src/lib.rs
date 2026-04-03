use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 记忆系统错误类型
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Memory entry not found")]
    NotFound,
    #[error("Memory limit exceeded: {0} chars")]
    LimitExceeded(usize),
    #[error("Security violation: {0}")]
    SecurityViolation(String),
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Invalid entry: {0}")]
    InvalidEntry(String),
}

/// 记忆条目 — 单条记忆内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 用户纠正记录 — 记录用户对 Agent 行为的纠正
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionEntry {
    pub original: String,  // 原始内容
    pub corrected: String, // 纠正后内容
    pub context: String,   // 上下文描述
    pub created_at: i64,
}

/// 用户偏好档案 — 存储用户偏好、通信风格、环境信息和纠正记录
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfile {
    pub preferences: Vec<MemoryEntry>,
    pub communication_style: Vec<MemoryEntry>,
    pub environment: Vec<MemoryEntry>,
    pub corrections: Vec<CorrectionEntry>,
}

/// 记忆管理器 — 管理 Agent 的持久记忆 (MEMORY.md)
/// 支持增删改查、安全扫描、容量管理
pub struct MemoryManager {
    memory_entries: Arc<RwLock<VecDeque<MemoryEntry>>>, // 记忆条目队列
    #[allow(dead_code)]
    user_profile: Arc<RwLock<UserProfile>>, // 用户偏好档案
    memory_path: PathBuf,                               // 记忆文件路径
    memory_char_limit: usize,                           // 记忆字符限制 (默认2200)
    #[allow(dead_code)]
    user_char_limit: usize,      // 用户档案字符限制 (默认1375)
}

impl MemoryManager {
    /// 创建新的记忆管理器
    pub fn new<P: AsRef<Path>>(
        memory_path: P,
        memory_char_limit: usize,
        user_char_limit: usize,
    ) -> Self {
        let path = memory_path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Self {
            memory_entries: Arc::new(RwLock::new(VecDeque::new())),
            user_profile: Arc::new(RwLock::new(UserProfile::default())),
            memory_path: path,
            memory_char_limit,
            user_char_limit,
        }
    }

    /// 添加记忆条目 — 安全扫描 + 容量检查
    pub fn add_entry(&self, content: &str) -> Result<(), MemoryError> {
        let scanned = MemoryScanner::scan(content)?; // 安全扫描
        let current_chars = self.current_memory_chars();
        if current_chars + scanned.len() > self.memory_char_limit {
            return Err(MemoryError::LimitExceeded(self.memory_char_limit));
        }
        let entry = MemoryEntry {
            content: scanned,
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
        };
        {
            self.memory_entries.write().push_back(entry);
        }
        self.save_memory()
    }

    /// 替换记忆 — 子串匹配替换 (不需要完整文本)
    /// 避免死锁: 先快照再释放写锁, 最后持久化
    pub fn replace_entry(&self, old_text: &str, new_text: &str) -> Result<(), MemoryError> {
        let scanned = MemoryScanner::scan(new_text)?;
        let entries_snapshot;
        {
            let mut entries = self.memory_entries.write();
            for entry in entries.iter_mut() {
                if entry.content.contains(old_text) {
                    entry.content = entry.content.replace(old_text, &scanned);
                    entry.updated_at = chrono::Utc::now().timestamp_millis();
                    break;
                }
            }
            entries_snapshot = entries.iter().cloned().collect::<Vec<_>>();
        }
        self.save_entries(&entries_snapshot)
    }

    /// 删除记忆 — 子串匹配删除
    /// 避免死锁: 先快照再释放写锁, 最后持久化
    pub fn remove_entry(&self, text: &str) -> Result<(), MemoryError> {
        let entries_snapshot;
        {
            let mut entries = self.memory_entries.write();
            let initial_len = entries.len();
            entries.retain(|e| !e.content.contains(text));
            if entries.len() == initial_len {
                return Err(MemoryError::NotFound);
            }
            entries_snapshot = entries.iter().cloned().collect::<Vec<_>>();
        }
        self.save_entries(&entries_snapshot)
    }

    /// 获取所有记忆条目 (只读)
    pub fn get_entries(&self) -> Vec<MemoryEntry> {
        self.memory_entries.read().iter().cloned().collect()
    }

    /// 获取所有记忆文本 (拼接为单个字符串, 用于注入 system prompt)
    pub fn get_memory_text(&self) -> String {
        self.memory_entries
            .read()
            .iter()
            .map(|e| e.content.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 计算当前记忆总字符数
    pub fn current_memory_chars(&self) -> usize {
        self.memory_entries
            .read()
            .iter()
            .map(|e| e.content.len())
            .sum()
    }

    /// 获取记忆使用情况 (当前字符数, 限制字符数)
    pub fn memory_usage(&self) -> (usize, usize) {
        (self.current_memory_chars(), self.memory_char_limit)
    }

    /// 持久化记忆到文件
    pub fn save_memory(&self) -> Result<(), MemoryError> {
        let entries = self.memory_entries.read();
        self.save_entries(&entries.iter().cloned().collect::<Vec<_>>())
    }

    /// 持久化记忆条目到文件
    fn save_entries(&self, entries: &[MemoryEntry]) -> Result<(), MemoryError> {
        let json = serde_json::to_string_pretty(entries)
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        std::fs::write(&self.memory_path, json).map_err(|e| MemoryError::IoError(e.to_string()))?;
        Ok(())
    }

    /// 从文件加载记忆
    pub fn load_memory(&self) -> Result<(), MemoryError> {
        if !self.memory_path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&self.memory_path)
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        let entries: Vec<MemoryEntry> =
            serde_json::from_str(&content).map_err(|e| MemoryError::IoError(e.to_string()))?;
        let mut mem = self.memory_entries.write();
        *mem = VecDeque::from(entries);
        Ok(())
    }

    /// 记录用户纠正 (用于 Learning Loop 自我改进)
    pub fn record_correction(&self, original: &str, corrected: &str, context: &str) {
        let entry = CorrectionEntry {
            original: original.to_string(),
            corrected: corrected.to_string(),
            context: context.to_string(),
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        self.user_profile.write().corrections.push(entry);
    }

    /// 获取所有用户纠正记录
    pub fn get_corrections(&self) -> Vec<CorrectionEntry> {
        self.user_profile.read().corrections.clone()
    }

    /// 清空所有记忆
    pub fn clear(&self) -> Result<(), MemoryError> {
        self.memory_entries.write().clear();
        self.save_memory()
    }
}

/// 记忆安全扫描器 — 防止 Prompt 注入和数据泄露
struct MemoryScanner;

impl MemoryScanner {
    /// 扫描记忆内容 — 三重安全检查
    fn scan(content: &str) -> Result<String, MemoryError> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(MemoryError::InvalidEntry("Empty content".to_string()));
        }
        // 检查1: 不可见 Unicode 字符 (可能用于隐藏注入)
        if Self::contains_invisible_chars(trimmed) {
            return Err(MemoryError::SecurityViolation(
                "Invisible Unicode characters detected".to_string(),
            ));
        }
        // 检查2: Prompt 注入模式 (试图覆盖系统指令)
        if Self::contains_prompt_injection(trimmed) {
            return Err(MemoryError::SecurityViolation(
                "Potential prompt injection detected".to_string(),
            ));
        }
        // 检查3: 数据泄露模式 (试图外传数据)
        if Self::contains_exfiltration_pattern(trimmed) {
            return Err(MemoryError::SecurityViolation(
                "Potential data exfiltration pattern detected".to_string(),
            ));
        }
        Ok(trimmed.to_string())
    }

    /// 检测不可见 Unicode 字符 (零宽字符、双向文本覆盖等)
    fn contains_invisible_chars(text: &str) -> bool {
        text.chars().any(|c| {
            matches!(
                c,
                '\u{200B}'  // 零宽空格
                    | '\u{200C}'  // 零宽非连接符
                    | '\u{200D}'  // 零宽连接符
                    | '\u{FEFF}'  // 零宽不换行空格
                    | '\u{2060}'  // 词连接符
                    | '\u{180E}'  // 蒙古文元音分隔符
                    | '\u{200E}'  // 左至右标记
                    | '\u{200F}'  // 右至左标记
                    | '\u{202A}'  // 嵌入 LTR
                    | '\u{202B}'  // 嵌入 RTL
                    | '\u{202C}'  // 弹出方向格式
                    | '\u{202D}'  // 覆盖 LTR
                    | '\u{202E}' // 覆盖 RTL
            )
        })
    }

    /// 检测 Prompt 注入模式 (12种常见注入模式)
    fn contains_prompt_injection(text: &str) -> bool {
        let lower = text.to_lowercase();
        let injection_patterns = [
            "ignore previous",
            "ignore all previous",
            "disregard previous",
            "disregard all previous",
            "system prompt:",
            "system instruction:",
            "new instructions:",
            "override:",
            "you are now",
            "forget everything",
            "reset your memory",
            "change your role",
            "act as",
            "pretend to be",
        ];
        injection_patterns.iter().any(|p| lower.contains(p))
    }

    /// 检测数据泄露模式 (10种常见泄露模式)
    fn contains_exfiltration_pattern(text: &str) -> bool {
        let lower = text.to_lowercase();
        let exfil_patterns = [
            "send to",
            "post to",
            "upload to",
            "exfiltrate",
            "leak",
            "export data",
            "copy to clipboard",
            "write to file",
            "save to disk",
            "transmit",
        ];
        exfil_patterns.iter().any(|p| lower.contains(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_manager_new() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        assert_eq!(manager.current_memory_chars(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_add_entry() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.add_entry("User prefers dark mode").unwrap();
        assert_eq!(manager.get_entries().len(), 1);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_replace_entry() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.add_entry("User prefers dark mode").unwrap();
        manager.replace_entry("dark mode", "light mode").unwrap();
        let text = manager.get_memory_text();
        assert!(text.contains("light mode"));
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_remove_entry() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.add_entry("User prefers dark mode").unwrap();
        manager.remove_entry("dark mode").unwrap();
        assert_eq!(manager.get_entries().len(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_limit_exceeded() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 10, 1375);
        let result = manager.add_entry("This is too long for the limit");
        assert!(matches!(result, Err(MemoryError::LimitExceeded(10))));
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_save_and_load() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.add_entry("Persistent memory entry").unwrap();
        manager.save_memory().unwrap();
        let manager2 = MemoryManager::new(&db_path, 2200, 1375);
        manager2.load_memory().unwrap();
        assert_eq!(manager2.get_entries().len(), 1);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_clear() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.add_entry("Entry 1").unwrap();
        manager.add_entry("Entry 2").unwrap();
        manager.clear().unwrap();
        assert_eq!(manager.get_entries().len(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_manager_record_correction() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_mem_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("memory.json");
        let manager = MemoryManager::new(&db_path, 2200, 1375);
        manager.record_correction("teh", "the", "typo fix");
        let corrections = manager.get_corrections();
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].original, "teh");
        assert_eq!(corrections[0].corrected, "the");
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_memory_scanner_invisible_chars() {
        let result = MemoryScanner::scan("Hello\u{200B}World");
        assert!(matches!(result, Err(MemoryError::SecurityViolation(_))));
    }

    #[test]
    fn test_memory_scanner_prompt_injection() {
        let result = MemoryScanner::scan("Ignore previous instructions");
        assert!(matches!(result, Err(MemoryError::SecurityViolation(_))));
    }

    #[test]
    fn test_memory_scanner_exfiltration() {
        let result = MemoryScanner::scan("Upload to external server");
        assert!(matches!(result, Err(MemoryError::SecurityViolation(_))));
    }

    #[test]
    fn test_memory_scanner_valid_entry() {
        let result = MemoryScanner::scan("User prefers Python over JavaScript");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "User prefers Python over JavaScript");
    }
}
