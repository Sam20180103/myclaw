use anyhow::Result;
use async_trait::async_trait;
use base64::Engine;
use governor::{Quota, RateLimiter};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use rustclaw_channel::{Channel, ChannelEvent, EventStream};
use rustclaw_protocol::{Content, Envelope, Message, Role};
use rustclaw_agent::AgentResponse;
use rustclaw_memory::MemoryScanner;
use parking_lot::RwLock;
use serde_json::json;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config::FeishuConfig;
use crate::client::FeishuClient;
use crate::message::{FeishuEvent, FeishuEventMessage, FeishuMsgType};
use crate::permission::FeishuPermission;

/// 通过文件头魔数探测图片格式 (默认 png)。
fn detect_image_format(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpeg"
    } else if bytes.starts_with(&[0x47, 0x49, 0x46]) {
        "gif"
    } else if bytes.starts_with(b"RIFF") && bytes.len() > 11 && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        "png"
    }
}

type DefaultRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// 飞书通道 — 实现 Channel trait
/// 通过飞书开放平台 API 实现与用户的消息交互
#[derive(Clone)]
pub struct FeishuChannel {
    #[allow(dead_code)]
    config: FeishuConfig,
    client: Arc<FeishuClient>,
    event_tx: Arc<RwLock<Option<mpsc::Sender<ChannelEvent>>>>,
    event_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<ChannelEvent>>>,
    #[allow(dead_code)]
    rate_limiter: Arc<DefaultRateLimiter>,
    permission: FeishuPermission,
}

impl FeishuChannel {
    /// 清洗飞书文本消息内容，避免空内容和非法控制字符触发 validate content fail
    fn sanitize_text_for_feishu(text: &str) -> String {
        const FEISHU_TEXT_SOFT_LIMIT: usize = 8000;

        let mut cleaned: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\r' || *c == '\t')
            .collect();

        if cleaned.chars().count() > FEISHU_TEXT_SOFT_LIMIT {
            cleaned = cleaned.chars().take(FEISHU_TEXT_SOFT_LIMIT).collect();
            cleaned.push_str("\n\n[内容过长，已截断]");
        }

        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            "[空响应]".to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// 创建新的飞书通道
    pub fn new(config: FeishuConfig) -> Self {
        let client = Arc::new(FeishuClient::new(config.clone()));
        let (tx, rx) = mpsc::channel(100);
        // 频率限制: 60次/分钟
        let quota = Quota::per_minute(NonZeroU32::new(60).unwrap());
        let rate_limiter = Arc::new(RateLimiter::direct(quota));
        let permission = FeishuPermission::new(config.clone());
        Self {
            config,
            client,
            event_tx: Arc::new(RwLock::new(Some(tx))),
            event_rx: Arc::new(tokio::sync::Mutex::new(rx)),
            rate_limiter,
            permission,
        }
    }

    /// 获取配置
    pub fn config(&self) -> &FeishuConfig {
        &self.config
    }

    /// 获取飞书 API 客户端
    pub fn client(&self) -> Arc<FeishuClient> {
        self.client.clone()
    }

    /// 处理飞书事件
    pub async fn handle_event(&self, event: &FeishuEvent) -> Result<()> {
        // 频率限制检查
        if self.rate_limiter.check().is_err() {
            tracing::warn!("Rate limit exceeded for feishu channel");
            return Ok(());
        }

        // 提取发送者 ID
        let sender_open_id = event.event.sender.sender_id.open_id.as_deref().unwrap_or("unknown");

        // 解析消息
        let mut msg = self.parse_event_message(&event.event.message);
        msg.sender_id = sender_open_id.to_string(); // 修正 sender_id

        tracing::info!("Received Feishu Message: chat_id={}, sender_id={}, type={:?}, content_len={}", 
            msg.chat_id, msg.sender_id, msg.msg_type, msg.content.len());
        
        // 调试打印原始内容
        tracing::debug!("Raw message content: {}", msg.content);

        // 检查权限
        if !self.permission.check_permission(&msg.chat_type, &msg.chat_id, &msg.sender_id) {
            tracing::warn!(
                "Permission denied for user {} in chat {} ({})",
                msg.sender_id, msg.chat_id, msg.chat_type
            );
            return Ok(());
        }

        // 图片消息走视觉路径: 下载并构建 Content::Image 传给视觉模型;
        // 下载失败则降级为文本提示。
        let content = if matches!(msg.msg_type, FeishuMsgType::Image) {
            match self.extract_image(&msg).await {
                Some(c) => c,
                None => Content::text("[图片下载失败]".to_string()),
            }
        } else {
            // 提取文本内容并进行安全扫描
            let text = self.extract_text(&msg).await;
            if text.is_empty() {
                tracing::warn!("Extracted text is empty, skipping");
                return Ok(());
            }

            if let Err(e) = MemoryScanner::scan(&text) {
                tracing::warn!("Security scan failed for feishu message: {}", e);
                return Ok(());
            }

            // 构建消息内容
            let final_content = if msg.chat_type == "group" {
                if let Some(group_config) = self.permission.get_group_config(&msg.chat_id) {
                    if let Some(ref prompt) = group_config.system_prompt {
                        format!("{}\n\n[群聊系统提示]\n{}", prompt, text)
                    } else {
                        text
                    }
                } else {
                    text
                }
            } else {
                text
            };
            Content::text(final_content)
        };

        // 使用 chat_id 和 sender_id 构建 session_key
        // 格式: {chat_type}:{chat_id}:{sender_id}
        let session_key = format!("{}:{}:{}", msg.chat_type, msg.chat_id, msg.sender_id);

        let message = Message {
            role: Role::User,
            content,
        };
        let envelope = Envelope::new("feishu", &session_key, message);

        let sender = {
            let tx = self.event_tx.read();
            tx.clone()
        };

        if let Some(sender) = sender {
            let env = envelope.clone();
            match sender.send(ChannelEvent::Message(env)).await {
                Ok(_) => tracing::info!("Message queued for agent: session_key={}", envelope.session_key),
                Err(e) => tracing::error!("Failed to queue message: {}", e),
            }
        }

        Ok(())
    }

    pub async fn send_response(&self, response: &AgentResponse, session_key: &str) -> Result<()> {
        match &response.message.content {
            Content::Image { image_data, image_type, description } => {
                self.send_image_response(session_key, image_data, image_type, description.as_deref()).await
            }
            Content::Text(t) => {
                self.send_text_response(session_key, t).await
            }
            Content::Structured { text, .. } => {
                self.send_text_response(session_key, text.as_deref().unwrap_or("")).await
            }
            // 工具消息不会作为出站内容,降级为文本兜底
            Content::ToolUse { text, .. } => {
                self.send_text_response(session_key, text.as_deref().unwrap_or("")).await
            }
            Content::ToolResult { output, .. } => {
                self.send_text_response(session_key, output).await
            }
        }
    }

    /// 发送文本响应 (内部方法)
    async fn send_text_response(&self, session_key: &str, text: &str) -> Result<()> {
        let safe_text = Self::sanitize_text_for_feishu(text);
        let content = json!({"text": safe_text}).to_string();

        let parts: Vec<&str> = session_key.split(':').collect();
        if parts.len() == 3 {
            let chat_type = parts[0];
            let chat_id = parts[1];
            let sender_id = parts[2];

            if chat_type == "p2p" {
                self.send_raw(sender_id, "text", &content).await
            } else {
                self.send_raw(chat_id, "text", &content).await
            }
        } else {
            self.send_raw(session_key, "text", &content).await
        }
    }

    /// 发送图片响应 (Phase 17.5: 扫码登录支持)
    async fn send_image_response(
        &self,
        session_key: &str,
        image_data: &str,
        file_format: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let image_bytes = base64::engine::general_purpose::STANDARD
            .decode(image_data)
            .map_err(|e| anyhow::anyhow!("Failed to decode base64 image: {}", e))?;

        let image_key = self.client.upload_image(&image_bytes, file_format).await
            .map_err(|e| anyhow::anyhow!("Failed to upload image: {}", e))?;

        tracing::info!("Image uploaded to Feishu, key: {}", image_key);

        // 发送图片消息
        let parts: Vec<&str> = session_key.split(':').collect();
        let receive_id = if parts.len() == 3 {
            if parts[0] == "p2p" {
                parts[2]
            } else {
                parts[1]
            }
        } else {
            session_key
        };

        let id_type = if receive_id.starts_with("ou_") { "open_id" } else { "chat_id" };

        self.client.send_image(receive_id, id_type, &image_key).await
            .map_err(|e| anyhow::anyhow!("Failed to send image: {}", e))?;

        // 如果有描述，发送额外的文本消息
        if let Some(desc) = description {
            let content = json!({"text": desc}).to_string();
            let _ = self.send_raw(receive_id, "text", &content).await;
        }

        Ok(())
    }

    /// 统一发送逻辑, 自动检测 ID 类型
    async fn send_raw(&self, id: &str, msg_type: &str, content: &str) -> Result<()> {
        let id_type = if id.starts_with("ou_") {
            "open_id"
        } else {
            "chat_id"
        };

        tracing::info!("Calling Feishu API to send: id={}, id_type={}", id, id_type);

        match self.client.send_message(id, id_type, msg_type, content).await {
            Ok(msg_id) => {
                tracing::info!("Feishu message SENT successfully. MessageID: {}", msg_id);
                Ok(())
            }
            Err(e) => {
                tracing::error!("Feishu API delivery FAILED: {}", e);
                Err(anyhow::anyhow!("Feishu delivery error: {}", e))
            }
        }
    }

    /// 发送流式响应
    pub async fn send_streaming_response(&self, text: &str, chat_id: &str) -> Result<()> {
        let safe_text = Self::sanitize_text_for_feishu(text);
        let content = json!({"text": safe_text}).to_string();
        self.send_raw(chat_id, "text", &content).await
    }

    /// 发送交互卡片 (用于状态更新)
    pub async fn send_interactive_card(&self, card: crate::card::FeishuCard, chat_id: &str) -> Result<()> {
        let card_json: serde_json::Value = serde_json::from_str(&card.to_json()
            .map_err(|e| anyhow::anyhow!("Failed to serialize card: {}", e))?)
            .map_err(|e| anyhow::anyhow!("Failed to parse card JSON: {}", e))?;

        let id_type = if chat_id.starts_with("ou_") {
            "open_id"
        } else {
            "chat_id"
        };

        self.client
            .send_card(chat_id, id_type, card_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send interactive card: {}", e))?;

        Ok(())
    }

    /// 解析飞书事件消息
    fn parse_event_message(&self, event_msg: &FeishuEventMessage) -> crate::message::FeishuMessage {
        let msg_type = match event_msg.message_type.as_str() {
            "text" => FeishuMsgType::Text,
            "post" => FeishuMsgType::Post,
            "image" => FeishuMsgType::Image,
            "file" => FeishuMsgType::File,
            "audio" => FeishuMsgType::Audio,
            "media" => FeishuMsgType::Media,
            "sticker" => FeishuMsgType::Sticker,
            "interactive" => FeishuMsgType::Interactive,
            other => FeishuMsgType::Unknown(other.to_string()),
        };

        let sender_id = event_msg.message_id.clone();
        let mentions = event_msg.mentions.clone().unwrap_or_default();

        crate::message::FeishuMessage {
            message_id: event_msg.message_id.clone(),
            chat_id: event_msg.chat_id.clone(),
            chat_type: event_msg.chat_type.clone(),
            sender_id,
            msg_type,
            content: event_msg.content.clone(),
            mentions,
            parent_id: event_msg.parent_id.clone(),
            create_time: event_msg.create_time.parse().unwrap_or(0),
        }
    }

    /// 将飞书消息转换为 Envelope (已废弃, 使用内联转换代替)
    #[allow(dead_code)]
    fn message_to_envelope(&self, msg: &crate::message::FeishuMessage) -> Envelope {
        // 注意: 此方法不支持异步文件下载, 仅用于测试
        let text = match &msg.msg_type {
            FeishuMsgType::Image => "[图片消息]".to_string(),
            FeishuMsgType::File => "[文件消息]".to_string(),
            _ => self.extract_text_sync(msg),
        };

        let message = Message {
            role: Role::User,
            content: Content::text(text),
        };

        Envelope::new("feishu", &format!("{}_{}", msg.chat_type, msg.chat_id), message)
    }

    /// 同步版本的文本提取 (用于测试和废弃方法)
    fn extract_text_sync(&self, msg: &crate::message::FeishuMessage) -> String {
        match &msg.msg_type {
            FeishuMsgType::Text => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    json.get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    msg.content.clone()
                }
            }
            FeishuMsgType::Post => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    self.extract_post_text(&json)
                } else {
                    msg.content.clone()
                }
            }
            FeishuMsgType::Image => "[图片消息]".to_string(),
            FeishuMsgType::File => "[文件消息]".to_string(),
            FeishuMsgType::Audio => "[语音消息]".to_string(),
            FeishuMsgType::Media => "[视频消息]".to_string(),
            FeishuMsgType::Sticker => "[表情消息]".to_string(),
            FeishuMsgType::Interactive => "[卡片消息]".to_string(),
            FeishuMsgType::Unknown(t) => format!("[未知消息类型: {}]", t),
            _ => msg.content.clone(),
        }
    }

    /// 从飞书消息中提取文本内容 (支持文件下载)
    async fn extract_text(&self, msg: &crate::message::FeishuMessage) -> String {
        match &msg.msg_type {
            FeishuMsgType::Text => {
                // 解析 JSON 文本: {"text": "content"}
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    json.get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    msg.content.clone()
                }
            }
            FeishuMsgType::Post => {
                // 富文本消息, 提取纯文本
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    self.extract_post_text(&json)
                } else {
                    msg.content.clone()
                }
            }
                FeishuMsgType::Image => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                        if let Some(file_key) = json.get("image_key").and_then(|v| v.as_str()) {
                            let download_dir = std::path::Path::new("/tmp/rustclaw/feishu_downloads");
                            match self.client.download_file_to_path(file_key, download_dir).await {
                                Ok(path) => format!("[图片已保存: {}]", path.display()),
                                Err(e) => format!("[图片下载失败: {}]", e),
                            }
                        } else {
                            "[图片消息]".to_string()
                        }
                    } else {
                        "[图片消息]".to_string()
                    }
                }
                FeishuMsgType::File => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                        if let Some(file_key) = json.get("file_key").and_then(|v| v.as_str()) {
                            let download_dir = std::path::Path::new("/tmp/rustclaw/feishu_downloads");
                            match self.client.download_file_to_path(file_key, download_dir).await {
                                Ok(path) => format!("[文件已保存: {}]", path.display()),
                                Err(e) => format!("[文件下载失败: {}]", e),
                            }
                        } else {
                            "[文件消息]".to_string()
                        }
                    } else {
                        "[文件消息]".to_string()
                    }
                }
            FeishuMsgType::Audio => "[语音消息]".to_string(),
            FeishuMsgType::Media => "[视频消息]".to_string(),
            FeishuMsgType::Sticker => "[表情消息]".to_string(),
            FeishuMsgType::Interactive => "[卡片消息]".to_string(),
            FeishuMsgType::Unknown(t) => format!("[未知消息类型: {}]", t),
            _ => msg.content.clone(),
        }
    }

    /// 提取图片消息为 Content::Image (下载 + base64),供视觉模型理解。
    /// 下载失败时返回 None,调用方应降级处理。
    async fn extract_image(&self, msg: &crate::message::FeishuMessage) -> Option<Content> {
        let json = serde_json::from_str::<serde_json::Value>(&msg.content).ok()?;
        let image_key = json.get("image_key").and_then(|v| v.as_str())?;

        match self
            .client
            .download_message_resource(&msg.message_id, image_key, "image")
            .await
        {
            Ok(bytes) => {
                let image_type = detect_image_format(&bytes);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Some(Content::image_with_description(b64, image_type, "用户发送的图片"))
            }
            Err(e) => {
                tracing::warn!("Failed to download feishu image {}: {}", image_key, e);
                None
            }
        }
    }

/// 从富文本消息中提取纯文本
    fn extract_post_text(&self, json: &serde_json::Value) -> String {
        let mut texts = Vec::new();
        if let Some(content) = json.get("content").and_then(|v| v.as_array()) {
            for row in content {
                if let Some(cells) = row.as_array() {
                    for cell in cells {
                        // 处理纯文本
                        if let Some(text) = cell.get("text").and_then(|v| v.as_str()) {
                            if !text.is_empty() {
                                texts.push(text.to_string());
                            }
                        }
                        // 处理链接 (Phase 17.5: 支持链接提取)
                        if let Some(link) = cell.get("link") {
                            if let Some(href) = link.get("href").and_then(|h| h.as_str()) {
                                let text = cell.get("text").and_then(|t| t.as_str()).unwrap_or(href);
                                texts.push(format!("[{}]({})", text, href));
                            }
                        }
                        // 处理@提及 (Phase 17.5: 支持@提及提取)
                        if let Some(mention) = cell.get("mention") {
                            if let Some(name) = mention.get("name").and_then(|n| n.as_str()) {
                                texts.push(format!("@{}", name));
                            }
                        }
                        // 处理图片 (Phase 17.5: 支持图片提取)
                        if let Some(img) = cell.get("image") {
                            if let Some(key) = img.get("image_key").and_then(|k| k.as_str()) {
                                texts.push(format!("[Image: {}]", key));
                            }
                        }
                    }
                }
            }
        }
        texts.join("\\n")
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name(&self) -> &str {
        "feishu"
    }

    async fn connect(&self) -> Result<()> {
        // 验证令牌
        self.client.get_tenant_token().await
            .map_err(|e| anyhow::anyhow!("Failed to get feishu token: {}", e))?;
        tracing::info!("Feishu channel connected");
        Ok(())
    }

    async fn disconnect(&self) -> Result<()> {
        self.event_tx.write().take();
        tracing::info!("Feishu channel disconnected");
        Ok(())
    }

    async fn send(&self, envelope: Envelope) -> Result<()> {
        let response = match envelope.message.content {
            Content::Text(t) => {
                AgentResponse {
                    session_key: envelope.session_key,
                    message: Message {
                        role: Role::Assistant,
                        content: Content::text(t),
                    },
                    tool_calls: vec![],
                }
            }
            Content::Structured { text, .. } => {
                AgentResponse {
                    session_key: envelope.session_key,
                    message: Message {
                        role: Role::Assistant,
                        content: Content::text(text.unwrap_or_default()),
                    },
                    tool_calls: vec![],
                }
            }
            Content::Image { image_data, image_type, description } => {
                // Phase 17.5: 图片消息直接发送
                AgentResponse {
                    session_key: envelope.session_key,
                    message: Message {
                        role: Role::Assistant,
                        content: Content::image_with_description(
                            image_data,
                            image_type,
                            description.unwrap_or_default(),
                        ),
                    },
                    tool_calls: vec![],
                }
            }
            // 工具消息不会作为出站内容,降级为文本兜底
            Content::ToolUse { text, .. } => AgentResponse {
                session_key: envelope.session_key,
                message: Message {
                    role: Role::Assistant,
                    content: Content::text(text.unwrap_or_default()),
                },
                tool_calls: vec![],
            },
            Content::ToolResult { output, .. } => AgentResponse {
                session_key: envelope.session_key,
                message: Message {
                    role: Role::Assistant,
                    content: Content::text(output),
                },
                tool_calls: vec![],
            },
        };

        self.send_response(&response, &response.session_key).await
    }

    fn events(&self) -> EventStream {
        let rx = self.event_rx.clone();
        Box::pin(async_stream::stream! {
            let mut rx = rx.lock().await;
            while let Some(event) = rx.recv().await {
                yield event;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_channel_new() {
        let config = FeishuConfig::default();
        let channel = FeishuChannel::new(config);
        assert_eq!(channel.name(), "feishu");
    }

    #[test]
    fn test_extract_text_from_text_message() {
        let config = FeishuConfig::default();
        let channel = FeishuChannel::new(config);
        let msg = crate::message::FeishuMessage {
            message_id: "msg_1".to_string(),
            chat_id: "chat_1".to_string(),
            chat_type: "p2p".to_string(),
            sender_id: "user_1".to_string(),
            msg_type: FeishuMsgType::Text,
            content: r#"{"text": "Hello World"}"#.to_string(),
            mentions: vec![],
            parent_id: None,
            create_time: 0,
        };
        let text = channel.extract_text_sync(&msg);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_check_permission_dm_enabled() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: false,
            allowed_users: vec![],
            allowed_groups: vec![],
            ..Default::default()
        };
        let channel = FeishuChannel::new(config);
        assert!(channel.permission.check_permission("p2p", "chat_1", "user_1"));
    }

    #[test]
    fn test_check_permission_dm_disabled() {
        let config = FeishuConfig {
            dm_enabled: false,
            ..Default::default()
        };
        let channel = FeishuChannel::new(config);
        assert!(!channel.permission.check_permission("p2p", "chat_1", "user_1"));
    }

    #[test]
    fn test_check_permission_group_enabled() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: true,
            allowed_users: vec![],
            allowed_groups: vec![],
            ..Default::default()
        };
        let channel = FeishuChannel::new(config);
        assert!(channel.permission.check_permission("group", "group_1", "user_1"));
    }

    #[test]
    fn test_check_permission_group_not_in_whitelist() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: true,
            allowed_groups: vec!["group_2".to_string()],
            ..Default::default()
        };
        let channel = FeishuChannel::new(config);
        assert!(!channel.permission.check_permission("group", "group_1", "user_1"));
    }

    #[test]
    fn test_message_to_envelope() {
        let config = FeishuConfig::default();
        let channel = FeishuChannel::new(config);
        let msg = crate::message::FeishuMessage {
            message_id: "msg_1".to_string(),
            chat_id: "chat_1".to_string(),
            chat_type: "p2p".to_string(),
            sender_id: "user_1".to_string(),
            msg_type: FeishuMsgType::Text,
            content: r#"{"text": "Hello"}"#.to_string(),
            mentions: vec![],
            parent_id: None,
            create_time: 0,
        };
        let envelope = channel.message_to_envelope(&msg);
        assert_eq!(envelope.channel, "feishu");
        assert_eq!(envelope.session_key, "p2p_chat_1");
    }
}
