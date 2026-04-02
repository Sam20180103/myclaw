use rustclaw_protocol::{Content, Message};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum CompressionError {
    #[error("Context window exceeded")]
    WindowExceeded,
    #[error("Compression failed: {0}")]
    CompressionFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionResult {
    pub messages: Vec<Message>,
    pub compressed: bool,
    pub original_count: usize,
    pub final_count: usize,
    pub removed_tool_results: usize,
}

pub struct SessionHygiene {
    threshold: f64,
    max_chars: usize,
}

impl SessionHygiene {
    pub fn new(threshold: f64, max_chars: usize) -> Self {
        Self {
            threshold,
            max_chars,
        }
    }

    pub fn should_compress(&self, current_chars: usize) -> bool {
        current_chars as f64 > self.max_chars as f64 * self.threshold
    }

    pub fn clean_old_tool_results(&self, messages: &[Message]) -> CompressionResult {
        let original_count = messages.len();
        let mut removed = 0;
        let mut cleaned = Vec::new();
        let mut keep_tool_results = true;

        for msg in messages.iter().rev() {
            if let Content::Text(text) = &msg.content {
                if text.starts_with("Tool ") && text.contains("result:") {
                    if keep_tool_results {
                        keep_tool_results = false;
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

    pub fn truncate_to_char_limit(&self, messages: &[Message], limit: usize) -> Vec<Message> {
        let mut result = Vec::new();
        let mut total_chars = 0;

        for msg in messages.iter().rev() {
            let msg_chars = match &msg.content {
                Content::Text(t) => t.len(),
                Content::Structured { text, .. } => text.as_ref().map(|t| t.len()).unwrap_or(0),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedSummary {
    pub goal: String,
    pub constraints: String,
    pub progress: String,
    pub key_decisions: Vec<String>,
    pub relevant_files: Vec<String>,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheBreakpoint {
    pub message_index: usize,
    pub cache_type: CacheType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheType {
    SystemPrompt,
    Rolling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStrategy {
    pub breakpoints: Vec<CacheBreakpoint>,
    pub ttl_seconds: u64,
}

impl CacheStrategy {
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

pub struct ContextCompressor {
    threshold: f64,
    #[allow(dead_code)]
    protect_last_n: usize,
}

impl ContextCompressor {
    pub fn new(threshold: f64, protect_last_n: usize) -> Self {
        Self {
            threshold,
            protect_last_n,
        }
    }

    pub fn estimate_chars(messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|m| match &m.content {
                Content::Text(t) => t.len(),
                Content::Structured { text, .. } => text.as_ref().map(|t| t.len()).unwrap_or(0),
            })
            .sum()
    }

    pub fn should_compress(&self, current_chars: usize, context_window: usize) -> bool {
        current_chars as f64 > context_window as f64 * self.threshold
    }

    pub fn compress(&self, messages: &[Message], context_window: usize) -> CompressionResult {
        let original_count = messages.len();
        let total_chars = Self::estimate_chars(messages);

        if !self.should_compress(total_chars, context_window) {
            return CompressionResult {
                messages: messages.to_vec(),
                compressed: false,
                original_count,
                final_count: original_count,
                removed_tool_results: 0,
            };
        }

        let hygiene = SessionHygiene::new(0.85, context_window);
        let cleaned = hygiene.clean_old_tool_results(messages);

        let msgs = cleaned.messages.clone();
        let count = msgs.len();
        let remaining_chars = Self::estimate_chars(&msgs);
        let is_compressed = cleaned.compressed;
        let removed_so_far = cleaned.removed_tool_results;

        if remaining_chars as f64 <= context_window as f64 * self.threshold {
            return CompressionResult {
                messages: msgs,
                compressed: is_compressed,
                original_count,
                final_count: count,
                removed_tool_results: removed_so_far,
            };
        }

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
