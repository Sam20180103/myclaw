use rustclaw_protocol::{Content, Message, Role};
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
    pub removed_tool_results: usize, // 被省略正文的工具结果数量
    /// P0-2: 被整条丢弃的消息 (供上层生成摘要后回填,避免 Agent 失忆)
    #[serde(default)]
    pub dropped: Vec<Message>,
}

/// 单张图片的 token 估算值 (与分辨率相关,取一个保守常数)
const IMAGE_TOKENS: usize = 1200;

/// P0-3: 估算文本 token 数。ASCII ≈ 4 字符/token,CJK 等非 ASCII ≈ 1 字符/token。
/// 旧实现用 `String::len()` (UTF-8 字节),中文会被高估 3 倍。
pub fn estimate_text_tokens(s: &str) -> usize {
    let (ascii, wide) = s.chars().fold((0usize, 0usize), |(a, w), c| {
        if c.is_ascii() {
            (a + 1, w)
        } else {
            (a, w + 1)
        }
    });
    ascii / 4 + wide
}

/// P0-3: 估算单条消息的 token 数 (图片按常数计,不按 Base64 长度)
pub fn estimate_message_tokens(m: &Message) -> usize {
    match &m.content {
        Content::Text(t) => estimate_text_tokens(t),
        Content::Structured { text, .. } => {
            text.as_ref().map(|t| estimate_text_tokens(t)).unwrap_or(0)
        }
        Content::Image { description, .. } => {
            IMAGE_TOKENS + description.as_ref().map(|d| estimate_text_tokens(d)).unwrap_or(0)
        }
        Content::ToolUse { text, tool_calls } => {
            text.as_ref().map(|t| estimate_text_tokens(t)).unwrap_or(0)
                + tool_calls
                    .iter()
                    .map(|c| {
                        estimate_text_tokens(&c.name)
                            + estimate_text_tokens(&c.arguments.to_string())
                    })
                    .sum::<usize>()
        }
        Content::ToolResult { output, .. } => estimate_text_tokens(output),
    }
}

/// P0-1: 强制 `tool_use` / `tool_result` 配对不变量。
///
/// 压缩/截断后历史可能出现两类非法形态,两家 Provider 都会以 400 拒绝:
/// 1. 孤儿 `ToolResult` — 对应的 assistant `tool_use` 已被截掉 → 直接丢弃;
/// 2. 缺结果的 `tool_use` — 结果被截掉 → 合成一条占位 `ToolResult`。
///
/// 该函数是压缩落盘前的最后一道闸,任何新增的压缩策略都必须经过它。
pub fn sanitize_pairing(messages: Vec<Message>) -> Vec<Message> {
    // 第一遍: 收集所有仍存在的 tool_use ID
    let mut known_ids = std::collections::HashSet::new();
    for m in &messages {
        if let Content::ToolUse { tool_calls, .. } = &m.content {
            for c in tool_calls {
                known_ids.insert(c.id.clone());
            }
        }
    }

    // 第二遍: 丢弃孤儿 ToolResult, 并记录已满足的 ID
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut satisfied = std::collections::HashSet::new();
    for m in messages {
        if let Content::ToolResult { tool_call_id, .. } = &m.content {
            if !known_ids.contains(tool_call_id) {
                continue; // 孤儿结果
            }
            satisfied.insert(tool_call_id.clone());
        }
        out.push(m);
    }

    // 第三遍: 为缺失结果的 tool_use 就地补一条占位结果 (紧跟其后, 保持顺序语义)
    let mut patched: Vec<Message> = Vec::with_capacity(out.len());
    for m in out {
        let missing: Vec<(String, String)> = match &m.content {
            Content::ToolUse { tool_calls, .. } => tool_calls
                .iter()
                .filter(|c| !satisfied.contains(&c.id))
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect(),
            _ => Vec::new(),
        };
        patched.push(m);
        for (id, name) in missing {
            satisfied.insert(id.clone());
            patched.push(Message {
                role: Role::Tool,
                content: Content::tool_result(id, name, "[结果已被上下文压缩丢弃]"),
            });
        }
    }

    patched
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

    /// 省略旧的工具结果正文 — 只保留最近 `KEEP_FULL_TOOL_RESULTS` 条的完整输出。
    ///
    /// P0-1: 绝不整条删除 `ToolResult`。删除会让配对的 assistant `tool_use` 变成孤儿,
    /// Anthropic / OpenAI 都会返回 400。因此这里只把旧结果的正文替换为省略标记,
    /// 消息条数与 tool_call_id 配对关系保持不变。
    pub fn clean_old_tool_results(&self, messages: &[Message]) -> CompressionResult {
        const KEEP_FULL_TOOL_RESULTS: usize = 3;
        const PREVIEW_CHARS: usize = 120;

        let original_count = messages.len();
        let mut elided = 0;
        let mut kept_full = 0;
        let mut cleaned = Vec::with_capacity(original_count);

        for msg in messages.iter().rev() {
            match &msg.content {
                Content::ToolResult {
                    tool_call_id,
                    name,
                    output,
                } => {
                    if kept_full < KEEP_FULL_TOOL_RESULTS {
                        kept_full += 1;
                        cleaned.push(msg.clone());
                    } else {
                        let preview: String = output.chars().take(PREVIEW_CHARS).collect();
                        elided += 1;
                        cleaned.push(Message {
                            role: msg.role.clone(),
                            content: Content::tool_result(
                                tool_call_id,
                                name,
                                format!("[已省略的历史工具结果] {}", preview),
                            ),
                        });
                    }
                }
                // 兼容历史遗留的 "Tool X result:" 文本形态 (C1 之前写入的会话)
                Content::Text(text)
                    if text.starts_with("Tool ") && text.contains("result:") =>
                {
                    if kept_full < KEEP_FULL_TOOL_RESULTS {
                        kept_full += 1;
                        cleaned.push(msg.clone());
                    } else {
                        let preview: String = text.chars().take(PREVIEW_CHARS).collect();
                        elided += 1;
                        cleaned.push(Message {
                            role: msg.role.clone(),
                            content: Content::text(format!("[已省略的历史工具结果] {}", preview)),
                        });
                    }
                }
                _ => cleaned.push(msg.clone()),
            }
        }

        cleaned.reverse();

        let final_count = cleaned.len();
        CompressionResult {
            messages: cleaned,
            compressed: elided > 0,
            original_count,
            final_count,
            removed_tool_results: elided,
            dropped: Vec::new(),
        }
    }

    /// 按 token 预算截断消息 — 从后向前保留, 直到超过限制。
    /// 返回 (保留的消息, 被丢弃的消息)。调用方需对结果再跑 `sanitize_pairing`。
    pub fn truncate_to_token_limit(
        &self,
        messages: &[Message],
        limit: usize,
    ) -> (Vec<Message>, Vec<Message>) {
        let mut kept = Vec::new();
        let mut total = 0;
        let mut cut_at = 0; // 保留段的起始下标

        for (offset, msg) in messages.iter().rev().enumerate() {
            let msg_tokens = estimate_message_tokens(msg);
            if total + msg_tokens > limit {
                cut_at = messages.len() - offset;
                break;
            }
            total += msg_tokens;
            kept.push(msg.clone());
        }

        kept.reverse();
        let dropped = messages[..cut_at].to_vec();
        (kept, dropped)
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
    threshold: f64,        // 压缩阈值 (0.50 = 50%)
    context_window: usize, // 上下文窗口大小 (字符近似),作为压缩预算基准
}

impl ContextCompressor {
    /// 创建上下文压缩器。`context_window` 为预算基准 (字符近似),
    /// 压缩在 `chars > context_window * threshold` 时触发。
    pub fn new(threshold: f64, context_window: usize) -> Self {
        Self {
            threshold,
            context_window,
        }
    }

    /// 压缩预算基准 (C2: 供 Agent 复用,避免硬编码)
    pub fn context_window(&self) -> usize {
        self.context_window
    }

    /// 估算消息总 token 数 (P0-3: 图片按常数计,中文不再按 UTF-8 字节高估)
    pub fn estimate_tokens(messages: &[Message]) -> usize {
        messages.iter().map(estimate_message_tokens).sum()
    }

    /// 判断是否需要压缩 — 当前 token 数 > 上下文窗口 * 阈值
    pub fn should_compress(&self, current_tokens: usize, context_window: usize) -> bool {
        current_tokens as f64 > context_window as f64 * self.threshold
    }

    /// 执行压缩 — 两阶段压缩
    /// 阶段1: 省略旧工具结果正文 (保留最近 3 条完整)
    /// 阶段2: 如果仍超限, 截断到 50% 上下文窗口并回报被丢弃的消息
    ///
    /// P0-1: 两个阶段的输出都经 `sanitize_pairing` 强制 tool_use/tool_result 配对。
    pub fn compress(&self, messages: &[Message], context_window: usize) -> CompressionResult {
        let original_count = messages.len();
        let total_tokens = Self::estimate_tokens(messages);

        // 如果不需要压缩, 直接返回原消息
        if !self.should_compress(total_tokens, context_window) {
            return CompressionResult {
                messages: messages.to_vec(),
                compressed: false,
                original_count,
                final_count: original_count,
                removed_tool_results: 0,
                dropped: Vec::new(),
            };
        }

        // 阶段1: 省略旧工具结果正文 (不改变消息条数, 天然保持配对)
        let hygiene = SessionHygiene::new(0.85, context_window);
        let cleaned = hygiene.clean_old_tool_results(messages);

        let msgs = cleaned.messages;
        let remaining_tokens = Self::estimate_tokens(&msgs);
        let elided = cleaned.removed_tool_results;

        // 如果省略后已满足阈值, 返回
        if remaining_tokens as f64 <= context_window as f64 * self.threshold {
            let final_count = msgs.len();
            return CompressionResult {
                messages: msgs,
                compressed: elided > 0,
                original_count,
                final_count,
                removed_tool_results: elided,
                dropped: Vec::new(),
            };
        }

        // 阶段2: 截断到 50% 上下文窗口
        let target_tokens = (context_window as f64 * 0.5) as usize;
        let (truncated, dropped) = hygiene.truncate_to_token_limit(&msgs, target_tokens);
        let sanitized = sanitize_pairing(truncated);
        let final_count = sanitized.len();

        CompressionResult {
            messages: sanitized,
            compressed: true,
            original_count,
            final_count,
            removed_tool_results: elided,
            dropped,
        }
    }

    /// P0-2: 为"即将被丢弃的历史"生成摘要请求提示词。
    /// 上层 (Agent) 拿它去问 LLM,把返回文本作为一条 System 消息插回历史头部。
    pub fn summary_prompt(dropped: &[Message]) -> String {
        let mut transcript = String::new();
        for m in dropped {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => "tool",
            };
            let body = match &m.content {
                Content::Text(t) => t.clone(),
                Content::Structured { text, .. } => text.clone().unwrap_or_default(),
                Content::Image { description, .. } => {
                    format!("[image] {}", description.clone().unwrap_or_default())
                }
                Content::ToolUse { text, tool_calls } => format!(
                    "{} [调用工具: {}]",
                    text.clone().unwrap_or_default(),
                    tool_calls
                        .iter()
                        .map(|c| format!("{}({})", c.name, c.arguments))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Content::ToolResult { name, output, .. } => {
                    let preview: String = output.chars().take(400).collect();
                    format!("[工具 {} 结果] {}", name, preview)
                }
            };
            transcript.push_str(role);
            transcript.push_str(": ");
            transcript.push_str(body.trim());
            transcript.push('\n');
        }

        format!(
            "以下是一段即将被移出上下文的对话历史。请压缩为不超过 400 字的中文摘要,\
             必须保留: 用户目标与约束、已完成的动作(尤其是已产生外部副作用的动作,\
             如发布/投递/发送)、关键结论与待办。不要编造未出现的信息。\n\n---\n{}\n---\n\n摘要:",
            transcript
        )
    }

    /// P0-2: LLM 不可用时的本地降级摘要 — 至少保住"已执行过哪些工具",
    /// 避免 Agent 压缩后重复执行有副作用的动作。
    pub fn local_summary(dropped: &[Message]) -> String {
        let mut tools: Vec<String> = Vec::new();
        let mut first_user = String::new();
        for m in dropped {
            match &m.content {
                Content::ToolUse { tool_calls, .. } => {
                    for c in tool_calls {
                        let sig = format!("{}({})", c.name, c.arguments);
                        let sig: String = sig.chars().take(160).collect();
                        if !tools.contains(&sig) {
                            tools.push(sig);
                        }
                    }
                }
                Content::Text(t) if m.role == Role::User && first_user.is_empty() => {
                    first_user = t.chars().take(200).collect();
                }
                _ => {}
            }
        }

        let mut out = String::from("[历史摘要 · 本地降级版]");
        if !first_user.is_empty() {
            out.push_str(&format!("\n最初的用户请求: {}", first_user));
        }
        if tools.is_empty() {
            out.push_str("\n此前未执行任何工具调用。");
        } else {
            out.push_str(&format!(
                "\n此前已执行的工具调用 (勿重复执行):\n- {}",
                tools.join("\n- ")
            ));
        }
        out.push_str(&format!("\n(已从上下文中移出 {} 条消息)", dropped.len()));
        out
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

    fn tool_pair(id: &str, output: &str) -> Vec<Message> {
        vec![
            Message::tool_use(
                vec![rustclaw_protocol::ToolCallSpec {
                    id: id.to_string(),
                    name: "file_read".to_string(),
                    arguments: serde_json::json!({ "path": "/tmp/x" }),
                }],
                None,
            ),
            Message {
                role: Role::Tool,
                content: Content::tool_result(id, "file_read", output),
            },
        ]
    }

    #[test]
    fn test_session_hygiene_clean_tool_results() {
        // P0-1: 旧结果只省略正文, 消息条数与配对关系必须保持不变
        let hygiene = SessionHygiene::new(0.85, 10000);
        let mut messages = vec![make_text_msg(Role::User, "Hello")];
        for i in 0..5 {
            messages.extend(tool_pair(&format!("call_{}", i), &format!("payload {}", i)));
        }

        let result = hygiene.clean_old_tool_results(&messages);
        assert!(result.compressed);
        assert_eq!(result.removed_tool_results, 2); // 5 条结果保留最近 3 条完整
        assert_eq!(result.messages.len(), messages.len()); // 不删条数
        assert_eq!(sanitize_pairing(result.messages.clone()).len(), messages.len());
        // 最老的结果正文被省略, 最新的保持原样
        let outputs: Vec<String> = result
            .messages
            .iter()
            .filter_map(|m| match &m.content {
                Content::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            })
            .collect();
        assert!(outputs[0].starts_with("[已省略的历史工具结果]"));
        assert_eq!(outputs[4], "payload 4");
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

        let (kept, dropped) = hygiene.truncate_to_token_limit(&messages, 6);
        assert!(kept.len() < messages.len());
        assert_eq!(kept.len() + dropped.len(), messages.len());
    }

    #[test]
    fn test_estimate_tokens_cjk_vs_ascii() {
        // P0-3: 中文按字符计 token, 不再按 UTF-8 字节 (旧实现会是 3 倍)
        assert_eq!(estimate_text_tokens("中文测试"), 4);
        assert_eq!(estimate_text_tokens("abcdefgh"), 2);
        let messages = vec![
            make_text_msg(Role::User, "Hello world!"), // 12 ascii → 3
            make_text_msg(Role::Assistant, "你好"),    // 2 cjk → 2
        ];
        assert_eq!(ContextCompressor::estimate_tokens(&messages), 5);
    }

    #[test]
    fn test_sanitize_drops_orphan_tool_result() {
        // P0-1: 没有配对 tool_use 的结果必须丢弃, 否则 Provider 返回 400
        let msgs = vec![
            Message {
                role: Role::Tool,
                content: Content::tool_result("ghost", "file_read", "data"),
            },
            make_text_msg(Role::User, "继续"),
        ];
        let out = sanitize_pairing(msgs);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].content, Content::Text(_)));
    }

    #[test]
    fn test_sanitize_synthesizes_missing_tool_result() {
        // P0-1: 结果被截掉的 tool_use 必须补占位结果
        let msgs = vec![
            Message::tool_use(
                vec![rustclaw_protocol::ToolCallSpec {
                    id: "c1".to_string(),
                    name: "shell_exec".to_string(),
                    arguments: serde_json::json!({}),
                }],
                None,
            ),
            make_text_msg(Role::User, "然后呢"),
        ];
        let out = sanitize_pairing(msgs);
        assert_eq!(out.len(), 3);
        match &out[1].content {
            Content::ToolResult { tool_call_id, .. } => assert_eq!(tool_call_id, "c1"),
            other => panic!("expected synthesized tool result, got {:?}", other),
        }
    }

    #[test]
    fn test_compress_never_orphans_tool_use() {
        // P0-1: 压缩到极小预算后, 历史仍必须满足配对不变量
        let compressor = ContextCompressor::new(0.50, 200);
        let mut messages = vec![make_text_msg(Role::User, "开始一个很长的任务")];
        for i in 0..30 {
            messages.extend(tool_pair(
                &format!("call_{}", i),
                &"x".repeat(200).to_string(),
            ));
        }

        let result = compressor.compress(&messages, 200);
        assert!(result.compressed);
        assert_eq!(
            result.messages.len(),
            sanitize_pairing(result.messages.clone()).len(),
            "压缩输出必须已经是配对干净的"
        );

        let use_ids: std::collections::HashSet<String> = result
            .messages
            .iter()
            .flat_map(|m| match &m.content {
                Content::ToolUse { tool_calls, .. } => {
                    tool_calls.iter().map(|c| c.id.clone()).collect()
                }
                _ => Vec::new(),
            })
            .collect();
        let result_ids: std::collections::HashSet<String> = result
            .messages
            .iter()
            .filter_map(|m| match &m.content {
                Content::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(use_ids, result_ids, "tool_use 与 tool_result 必须一一配对");
    }

    #[test]
    fn test_compress_reports_dropped() {
        // P0-2: 被丢弃的历史要能拿回来做摘要
        let compressor = ContextCompressor::new(0.50, 60);
        let mut messages = Vec::new();
        for i in 0..40 {
            messages.push(make_text_msg(Role::User, &format!("message number {}", i)));
        }
        let result = compressor.compress(&messages, 60);
        assert!(result.compressed);
        assert!(!result.dropped.is_empty());
        assert_eq!(
            result.dropped.len() + result.messages.len(),
            messages.len(),
            "丢弃 + 保留 = 原始"
        );
    }

    #[test]
    fn test_local_summary_mentions_dropped_tools() {
        // P0-2: LLM 不可用时, 至少要记住已执行过的有副作用动作
        let mut dropped = vec![make_text_msg(Role::User, "帮我发一篇小红书")];
        dropped.extend(tool_pair("c1", "ok"));
        let summary = ContextCompressor::local_summary(&dropped);
        assert!(summary.contains("file_read"));
        assert!(summary.contains("勿重复执行"));
        assert!(summary.contains("帮我发一篇小红书"));
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
