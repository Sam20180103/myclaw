use rustclaw_protocol::{Content, Message};
use serde::{Deserialize, Serialize};

/// 压缩错误类型
#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    #[error("Context window exceeded")]
    WindowExceeded,
    #[error("Compression failed: {0}")]
    CompressionFailed(String),
}

/// 压缩结果 — 包含压缩后的消息和统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionResult {
    pub messages: Vec<Message>,      // 压缩后的消息列表
    pub compressed: bool,            // 是否发生了压缩
    pub original_count: usize,       // 原始消息数量
    pub final_count: usize,          // 压缩后消息数量
    pub removed_tool_results: usize, // 移除的工具结果数量
}

/// 会话清理器 — Gateway 层的粗略字符级压缩 (85% 阈值安全网)
/// 负责清理旧的工具结果和字符级截断
pub struct SessionHygiene {
    threshold: f64,   // 压缩阈值 (0.85 = 85%)
    max_chars: usize, // 最大字符数 (上下文窗口大小)
}

impl SessionHygiene {
    /// 创建会话清理器
    pub fn new(threshold: f64, max_chars: usize) -> Self {
        Self {
            threshold,
            max_chars,
        }
    }

    /// 判断是否需要压缩 — 当前字符数 > 阈值 * 最大字符数
    pub fn should_compress(&self, current_chars: usize) -> bool {
        current_chars as f64 > self.max_chars as f64 * self.threshold
    }

    /// 清理旧的工具结果 — 只保留最后一个工具结果
    /// 从后向前遍历, 遇到第一个工具结果后跳过后续所有工具结果
    pub fn clean_old_tool_results(&self, messages: &[Message]) -> CompressionResult {
        let original_count = messages.len();
        let mut removed = 0;
        let mut cleaned = Vec::new();
        let mut keep_tool_results = true;

        for msg in messages.iter().rev() {
            if let Content::Text(text) = &msg.content {
                if text.starts_with("Tool ") && text.contains("result:") {
                    if keep_tool_results {
                        keep_tool_results = false; // 只保留最后一个工具结果
                        cleaned.push(msg.clone());
                    } else {
                        removed += 1;
                        continue;
                    }
                }
            }
            cleaned.push(msg.clone());
        }

        cleaned.reverse();

        let final_count = cleaned.len();
        CompressionResult {
            messages: cleaned,
            compressed: removed > 0,
            original_count,
            final_count,
            removed_tool_results: removed,
        }
    }

    /// 按字符限制截断消息 — 从后向前保留, 直到超过限制
    pub fn truncate_to_char_limit(&self, messages: &[Message], limit: usize) -> Vec<Message> {
        let mut result = Vec::new();
        let mut total_chars = 0;

        for msg in messages.iter().rev() {
            let msg_chars = match &msg.content {
                Content::Text(t) => t.len(),
                Content::Structured { text, .. } => text.as_ref().map(|t| t.len()).unwrap_or(0),
                Content::Image { image_data, .. } => image_data.len(),
            };
            if total_chars + msg_chars > limit {
                break;
            }
            total_chars += msg_chars;
            result.push(msg.clone());
        }

        result.reverse();
        result
    }
}

/// 压缩摘要模板 — 用于结构化摘要生成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedSummary {
    pub goal: String,                // 当前目标
    pub constraints: String,         // 约束条件
    pub progress: String,            // 进度
    pub key_decisions: Vec<String>,  // 关键决策
    pub relevant_files: Vec<String>, // 相关文件
    pub next_steps: Vec<String>,     // 下一步
}

/// 缓存断点 — 定义哪些消息应该被缓存
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    pub message_index: usize,  // 消息索引
    pub cache_type: CacheType, // 缓存类型
}

/// 缓存类型 — SystemPrompt (系统提示) 或 Rolling (滚动窗口)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheType {
    SystemPrompt, // 系统提示 — 跨所有轮次缓存
    Rolling,      // 滚动窗口 — 最近3条消息
}

/// 缓存策略 — 管理 Anthropic cache_control 断点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStrategy {
    pub breakpoints: Vec<CacheBreakpoint>, // 缓存断点列表
    pub ttl_seconds: u64,                  // 缓存过期时间 (秒)
}

impl CacheStrategy {
    /// 创建 system_and_3 策略 — 缓存系统提示 + 最后3条消息
    /// 这是 Anthropic 推荐的缓存策略, 可降低 75% 输入 token 成本
    pub fn system_and_3(ttl_seconds: u64) -> Self {
        Self {
            breakpoints: vec![
                CacheBreakpoint {
                    message_index: 0,
                    cache_type: CacheType::SystemPrompt,
                },
                CacheBreakpoint {
                    message_index: 1,
                    cache_type: CacheType::Rolling,
                },
                CacheBreakpoint {
                    message_index: 2,
                    cache_type: CacheType::Rolling,
                },
                CacheBreakpoint {
                    message_index: 3,
                    cache_type: CacheType::Rolling,
                },
            ],
            ttl_seconds,
        }
    }

    /// 动态计算缓存断点 — 系统提示 + 最后3条消息
    pub fn calculate_breakpoints(message_count: usize, ttl_seconds: u64) -> Self {
        if message_count <= 4 {
            return Self::system_and_3(ttl_seconds);
        }
        let mut breakpoints = vec![CacheBreakpoint {
            message_index: 0,
            cache_type: CacheType::SystemPrompt,
        }];
        let last_three_start = message_count.saturating_sub(3);
        for i in last_three_start..message_count {
            breakpoints.push(CacheBreakpoint {
                message_index: i,
                cache_type: CacheType::Rolling,
            });
        }
        Self {
            breakpoints,
            ttl_seconds,
        }
    }
}

/// 上下文压缩器 — Agent 层的精确 token 级压缩 (50% 阈值)
/// 结合 SessionHygiene 实现双级压缩
pub struct ContextCompressor {
    threshold: f64, // 压缩阈值 (0.50 = 50%)
    #[allow(dead_code)]
    protect_last_n: usize, // 最少保护的消息数
}

impl ContextCompressor {
    /// 创建上下文压缩器
    pub fn new(threshold: f64, protect_last_n: usize) -> Self {
        Self {
            threshold,
            protect_last_n,
        }
    }

    /// 估算消息总字符数
    pub fn estimate_chars(messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|m| match &m.content {
                Content::Text(t) => t.len(),
                Content::Structured { text, .. } => text.as_ref().map(|t| t.len()).unwrap_or(0),
                Content::Image { image_data, .. } => image_data.len(),
            })
            .sum()
    }

    /// 判断是否需要压缩 — 当前字符数 > 上下文窗口 * 阈值
    pub fn should_compress(&self, current_chars: usize, context_window: usize) -> bool {
        current_chars as f64 > context_window as f64 * self.threshold
    }

    /// 执行压缩 — 两阶段压缩
    /// 阶段1: 使用 SessionHygiene 清理旧工具结果 (85% 阈值)
    /// 阶段2: 如果仍超限, 截断到 50% 上下文窗口
    pub fn compress(&self, messages: &[Message], context_window: usize) -> CompressionResult {
        let original_count = messages.len();
        let total_chars = Self::estimate_chars(messages);

        // 如果不需要压缩, 直接返回原消息
        if !self.should_compress(total_chars, context_window) {
            return CompressionResult {
                messages: messages.to_vec(),
                compressed: false,
                original_count,
                final_count: original_count,
                removed_tool_results: 0,
            };
        }

        // 阶段1: 清理旧工具结果
        let hygiene = SessionHygiene::new(0.85, context_window);
        let cleaned = hygiene.clean_old_tool_results(messages);

        let msgs = cleaned.messages.clone();
        let count = msgs.len();
        let remaining_chars = Self::estimate_chars(&msgs);
        let is_compressed = cleaned.compressed;
        let removed_so_far = cleaned.removed_tool_results;

        // 如果清理后已满足阈值, 返回清理后的消息
        if remaining_chars as f64 <= context_window as f64 * self.threshold {
            return CompressionResult {
                messages: msgs,
                compressed: is_compressed,
                original_count,
                final_count: count,
                removed_tool_results: removed_so_far,
            };
        }

        // 阶段2: 截断到 50% 上下文窗口
        let target_chars = (context_window as f64 * 0.5) as usize;
        let truncated = hygiene.truncate_to_char_limit(&msgs, target_chars);
        let total_removed = removed_so_far + (count - truncated.len());
        let final_count = truncated.len();

        CompressionResult {
            messages: truncated,
            compressed: true,
            original_count,
            final_count,
            removed_tool_results: total_removed,
        }
    }

    /// 生成空的压缩摘要模板
    pub fn generate_summary_template() -> CompressedSummary {
        CompressedSummary {
            goal: String::new(),
            constraints: String::new(),
            progress: String::new(),
            key_decisions: Vec::new(),
            relevant_files: Vec::new(),
            next_steps: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclaw_protocol::Role;

    fn make_text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: Content::Text(text.to_string()),
        }
    }

    #[test]
    fn test_session_hygiene_should_compress() {
        let hygiene = SessionHygiene::new(0.85, 1000);
        assert!(!hygiene.should_compress(500));
        assert!(hygiene.should_compress(900));
    }

    #[test]
    fn test_session_hygiene_clean_tool_results() {
        let hygiene = SessionHygiene::new(0.85, 10000);
        let messages = vec![
            make_text_msg(Role::System, "You are a helpful assistant"),
            make_text_msg(Role::User, "Hello"),
            make_text_msg(Role::Assistant, "Hi there"),
            make_text_msg(Role::System, "Tool file_read result: content of file1"),
            make_text_msg(Role::User, "Read another file"),
            make_text_msg(Role::System, "Tool file_read result: content of file2"),
            make_text_msg(Role::Assistant, "Done reading"),
        ];

        let result = hygiene.clean_old_tool_results(&messages);
        assert!(result.compressed);
        assert_eq!(result.removed_tool_results, 1);
    }

    #[test]
    fn test_session_hygiene_truncate() {
        let hygiene = SessionHygiene::new(0.85, 100);
        let messages = vec![
            make_text_msg(Role::System, "System prompt"),
            make_text_msg(Role::User, "User message 1"),
            make_text_msg(Role::Assistant, "Assistant response 1"),
            make_text_msg(Role::User, "User message 2"),
            make_text_msg(Role::Assistant, "Assistant response 2"),
        ];

        let result = hygiene.truncate_to_char_limit(&messages, 50);
        assert!(result.len() < messages.len());
    }

    #[test]
    fn test_context_compressor_estimate_chars() {
        let messages = vec![
            make_text_msg(Role::User, "Hello"),
            make_text_msg(Role::Assistant, "Hi"),
        ];
        assert_eq!(ContextCompressor::estimate_chars(&messages), 7);
    }

    #[test]
    fn test_context_compressor_no_compress() {
        let compressor = ContextCompressor::new(0.50, 10000);
        let messages = vec![
            make_text_msg(Role::System, "System prompt"),
            make_text_msg(Role::User, "Hello"),
        ];
        let result = compressor.compress(&messages, 10000);
        assert!(!result.compressed);
        assert_eq!(result.messages.len(), 2);
    }

    #[test]
    fn test_context_compressor_compress() {
        let compressor = ContextCompressor::new(0.50, 100);
        let mut messages = vec![make_text_msg(Role::System, "System prompt")];
        for i in 0..20 {
            messages.push(make_text_msg(Role::User, &format!("User message {}", i)));
            messages.push(make_text_msg(
                Role::Assistant,
                &format!("Assistant response {}", i),
            ));
        }

        let result = compressor.compress(&messages, 100);
        assert!(result.compressed);
        assert!(result.final_count < result.original_count);
    }

    #[test]
    fn test_compressed_summary_template() {
        let summary = ContextCompressor::generate_summary_template();
        assert!(summary.goal.is_empty());
        assert!(summary.key_decisions.is_empty());
    }

    #[test]
    fn test_cache_strategy_system_and_3() {
        let strategy = CacheStrategy::system_and_3(300);
        assert_eq!(strategy.breakpoints.len(), 4);
        assert_eq!(strategy.ttl_seconds, 300);
    }

    #[test]
    fn test_cache_strategy_calculate_breakpoints_small() {
        let strategy = CacheStrategy::calculate_breakpoints(3, 300);
        assert_eq!(strategy.breakpoints.len(), 4);
    }

    #[test]
    fn test_cache_strategy_calculate_breakpoints_large() {
        let strategy = CacheStrategy::calculate_breakpoints(10, 600);
        assert_eq!(strategy.breakpoints.len(), 4);
        assert_eq!(strategy.breakpoints[0].message_index, 0);
        assert_eq!(strategy.breakpoints[1].message_index, 7);
        assert_eq!(strategy.breakpoints[2].message_index, 8);
        assert_eq!(strategy.breakpoints[3].message_index, 9);
    }
}
