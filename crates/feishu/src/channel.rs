use anyhow::Result;
use async_trait::async_trait;
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

type DefaultRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// 飞书通道 — 实现 Channel trait
/// 通过飞书开放平台 API 实现与用户的消息交互
pub struct FeishuChannel {
    #[allow(dead_code)]
    config: FeishuConfig,
    client: Arc<FeishuClient>,
    event_tx: Arc<RwLock<Option<mpsc::Sender<ChannelEvent>>>>,
    event_rx: Arc<RwLock<Option<mpsc::Receiver<ChannelEvent>>>>,
    #[allow(dead_code)]
    rate_limiter: Arc<DefaultRateLimiter>,
    permission: FeishuPermission,
}

impl FeishuChannel {
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
            event_rx: Arc::new(RwLock::new(Some(rx))),
            rate_limiter,
            permission,
        }
    }

    /// 处理飞书事件
    pub async fn handle_event(&self, event: &FeishuEvent) -> Result<()> {
        // 频率限制检查
        if self.rate_limiter.check().is_err() {
            tracing::warn!("Rate limit exceeded for feishu channel");
            return Ok(());
        }

        // 解析消息
        let msg = self.parse_event_message(&event.event.message);

        // 检查权限
        if !self.permission.check_permission(&msg.chat_type, &msg.chat_id, &msg.sender_id) {
            tracing::warn!(
                "Permission denied for user {} in chat {} ({})",
                msg.sender_id, msg.chat_id, msg.chat_type
            );
            return Ok(());
        }

        // 提取文本内容并进行安全扫描
        let text = self.extract_text(&msg).await;
        if let Err(e) = MemoryScanner::scan(&text) {
            tracing::warn!("Security scan failed for feishu message: {}", e);
            return Ok(());
        }

        // 构建消息内容 (注入群聊自定义 System Prompt)
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

        // 转换为 Envelope 并发送
        let message = Message {
            role: Role::User,
            content: Content::text(final_content),
        };
        let envelope = Envelope::new("feishu", &format!("{}_{}", msg.chat_type, msg.chat_id), message);

        if let Some(tx) = self.event_tx.read().as_ref() {
            tx.send(ChannelEvent::Message(envelope)).await?;
        }

        Ok(())
    }

    /// 发送 Agent 响应到飞书
    pub async fn send_response(&self, response: &AgentResponse, chat_id: &str) -> Result<()> {
        let text = match &response.message.content {
            Content::Text(t) => t.clone(),
            Content::Structured { text, .. } => text.clone().unwrap_or_default(),
        };

        // 飞书文本消息内容格式: {"text": "content"}
        let content = json!({"text": text}).to_string();

        self.client
            .send_message(chat_id, "chat_id", "text", &content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send feishu message: {}", e))?;

        Ok(())
    }

    /// 发送流式响应 (先发送思考中卡片, 然后发送文本消息)
    pub async fn send_streaming_response(&self, text: &str, chat_id: &str) -> Result<()> {
        // 先发送思考中卡片
        let thinking_card = crate::card::FeishuCard::thinking_card();
        let card_json: serde_json::Value = serde_json::from_str(&thinking_card.to_json()
            .map_err(|e| anyhow::anyhow!("Failed to serialize thinking card: {}", e))?)
            .map_err(|e| anyhow::anyhow!("Failed to parse card JSON: {}", e))?;

        self.client
            .send_card(chat_id, "chat_id", card_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send thinking card: {}", e))?;

        // 发送文本响应
        let content = json!({"text": text}).to_string();
        self.client
            .send_message(chat_id, "chat_id", "text", &content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send feishu message: {}", e))?;

        Ok(())
    }

    /// 发送交互卡片 (用于状态更新)
    pub async fn send_interactive_card(&self, card: crate::card::FeishuCard, chat_id: &str) -> Result<()> {
        let card_json: serde_json::Value = serde_json::from_str(&card.to_json()
            .map_err(|e| anyhow::anyhow!("Failed to serialize card: {}", e))?)
            .map_err(|e| anyhow::anyhow!("Failed to parse card JSON: {}", e))?;

        self.client
            .send_card(chat_id, "chat_id", card_json)
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
                // 图片消息, 尝试下载并生成描述
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    if let Some(file_key) = json.get("image_key").and_then(|v| v.as_str()) {
                        match self.client.download_file(file_key).await {
                            Ok(_) => format!("[图片已下载: {}]", file_key),
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
                // 文件消息, 尝试下载
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                    if let Some(file_key) = json.get("file_key").and_then(|v| v.as_str()) {
                        match self.client.download_file(file_key).await {
                            Ok(_) => format!("[文件已下载: {}]", file_key),
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

    /// 从富文本消息中提取纯文本
    fn extract_post_text(&self, json: &serde_json::Value) -> String {
        let mut texts = Vec::new();
        if let Some(content) = json.get("content").and_then(|v| v.as_array()) {
            for row in content {
                if let Some(cells) = row.as_array() {
                    for cell in cells {
                        if let Some(text) = cell.get("text").and_then(|v| v.as_str()) {
                            texts.push(text);
                        }
                    }
                }
            }
        }
        texts.join("\n")
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
        // 清空事件队列
        *self.event_tx.write() = None;
        *self.event_rx.write() = None;
        tracing::info!("Feishu channel disconnected");
        Ok(())
    }

    async fn send(&self, envelope: Envelope) -> Result<()> {
        // 从 session_key 提取 chat_id (格式: "{chat_type}_{chat_id}")
        let parts: Vec<&str> = envelope.session_key.splitn(2, '_').collect();
        let chat_id = if parts.len() == 2 { parts[1] } else { &envelope.session_key };

        let text = match envelope.message.content {
            Content::Text(t) => t,
            Content::Structured { text, .. } => text.unwrap_or_default(),
        };

        let content = json!({"text": text}).to_string();
        self.client
            .send_message(chat_id, "chat_id", "text", &content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send feishu message: {}", e))?;

        Ok(())
    }

    fn events(&self) -> EventStream {
        // 创建一个空的事件流 (实际事件通过 handle_event 发送)
        Box::pin(futures_util::stream::iter(vec![]))
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
