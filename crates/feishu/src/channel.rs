use anyhow::Result;
use async_trait::async_trait;
use rustclaw_channel::{Channel, ChannelEvent, EventStream};
use rustclaw_protocol::{Content, Envelope, Message, Role};
use rustclaw_agent::AgentResponse;
use parking_lot::RwLock;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::config::FeishuConfig;
use crate::client::FeishuClient;
use crate::message::{FeishuEvent, FeishuEventMessage, FeishuMsgType, FeishuMention};

/// 飞书通道 — 实现 Channel trait
/// 通过飞书开放平台 API 实现与用户的消息交互
pub struct FeishuChannel {
    config: FeishuConfig,
    client: Arc<FeishuClient>,
    event_tx: Arc<RwLock<Option<mpsc::Sender<ChannelEvent>>>>,
    event_rx: Arc<RwLock<Option<mpsc::Receiver<ChannelEvent>>>>,
}

impl FeishuChannel {
    /// 创建新的飞书通道
    pub fn new(config: FeishuConfig) -> Self {
        let client = Arc::new(FeishuClient::new(config.clone()));
        let (tx, rx) = mpsc::channel(100);
        Self {
            config,
            client,
            event_tx: Arc::new(RwLock::new(Some(tx))),
            event_rx: Arc::new(RwLock::new(Some(rx))),
        }
    }

    /// 处理飞书事件
    pub async fn handle_event(&self, event: &FeishuEvent) -> Result<()> {
        // 解析消息
        let msg = self.parse_event_message(&event.event.message);

        // 检查权限
        if !self.check_permission(&msg.chat_type, &msg.chat_id, &msg.sender_id) {
            tracing::warn!(
                "Permission denied for user {} in chat {} ({})",
                msg.sender_id, msg.chat_id, msg.chat_type
            );
            return Ok(());
        }

        // 转换为 Envelope 并发送
        let envelope = self.message_to_envelope(&msg);

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

    /// 将飞书消息转换为 Envelope
    fn message_to_envelope(&self, msg: &crate::message::FeishuMessage) -> Envelope {
        // 提取文本内容
        let text = self.extract_text(msg);

        let message = Message {
            role: Role::User,
            content: Content::text(text),
        };

        Envelope::new("feishu", &format!("{}_{}", msg.chat_type, msg.chat_id), message)
    }

    /// 从飞书消息中提取文本内容
    fn extract_text(&self, msg: &crate::message::FeishuMessage) -> String {
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

    /// 检查权限
    fn check_permission(&self, chat_type: &str, chat_id: &str, sender_id: &str) -> bool {
        if chat_type == "p2p" {
            if !self.config.dm_enabled {
                return false;
            }
            if !self.config.allowed_users.is_empty()
                && !self.config.allowed_users.contains(&sender_id.to_string())
            {
                return false;
            }
        } else if chat_type == "group" {
            if !self.config.group_enabled {
                return false;
            }
            if !self.config.allowed_groups.is_empty()
                && !self.config.allowed_groups.contains(&chat_id.to_string())
            {
                return false;
            }
        }
        true
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
        let text = channel.extract_text(&msg);
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
        assert!(channel.check_permission("p2p", "chat_1", "user_1"));
    }

    #[test]
    fn test_check_permission_dm_disabled() {
        let config = FeishuConfig {
            dm_enabled: false,
            ..Default::default()
        };
        let channel = FeishuChannel::new(config);
        assert!(!channel.check_permission("p2p", "chat_1", "user_1"));
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
        assert!(channel.check_permission("group", "group_1", "user_1"));
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
        assert!(!channel.check_permission("group", "group_1", "user_1"));
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
