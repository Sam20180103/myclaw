use async_trait::async_trait;
use parking_lot::RwLock;
use rustclaw_channel::{Channel, ChannelEvent, EventStream};
use rustclaw_protocol::{Content, Envelope, Message, Role};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::client::{SendTarget, WeChatConnectorClient};
use crate::config::WeChatConfig;
use crate::message::{parse_session_key, WeChatInbound};
use crate::permission::WeChatPermission;

/// 个人微信通道 — 实现 Channel trait。
/// 入站: 连接器 POST 到网关 → gateway 调 `ingest()`。
/// 出站: `send()` → 连接器 send_url。
#[derive(Clone)]
pub struct WeChatChannel {
    config: WeChatConfig,
    client: WeChatConnectorClient,
    permission: WeChatPermission,
    event_tx: Arc<RwLock<Option<mpsc::Sender<ChannelEvent>>>>,
    event_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<ChannelEvent>>>,
}

impl WeChatChannel {
    pub fn new(config: WeChatConfig) -> Self {
        let client = WeChatConnectorClient::new(
            config.connector_send_url.clone(),
            config.send_token.clone(),
        );
        let permission = WeChatPermission::new(config.clone());
        let (tx, rx) = mpsc::channel(100);
        Self {
            config,
            client,
            permission,
            event_tx: Arc::new(RwLock::new(Some(tx))),
            event_rx: Arc::new(tokio::sync::Mutex::new(rx)),
        }
    }

    pub fn config(&self) -> &WeChatConfig {
        &self.config
    }

    /// 处理连接器推送的入站消息。返回 true 表示已入队交给 Agent, false 表示被忽略/拒绝。
    pub async fn ingest(&self, body: serde_json::Value) -> anyhow::Result<bool> {
        let msg: WeChatInbound = serde_json::from_value(body)
            .map_err(|e| anyhow::anyhow!("invalid wechat inbound payload: {}", e))?;

        // 群聊仅在 @机器人 时响应 (防刷屏)
        if msg.is_group() && self.config.require_at_in_group && !msg.is_at_bot {
            tracing::debug!("wechat group msg without @bot, ignored: room={:?}", msg.room_id);
            return Ok(false);
        }

        // 权限检查
        if !self.permission.check_permission(msg.chat_type(), msg.chat_id(), &msg.from_user) {
            tracing::warn!(
                "wechat permission denied: chat_type={}, chat_id={}, sender={}",
                msg.chat_type(), msg.chat_id(), msg.from_user
            );
            return Ok(false);
        }

        // 构建消息内容
        let content = if msg.msg_type == "image" {
            match msg.image_base64.as_deref() {
                Some(b64) if !b64.is_empty() => {
                    let image_type = msg.image_type.clone().unwrap_or_else(|| "png".to_string());
                    Content::image_with_description(b64, image_type, "用户发送的图片")
                }
                _ => {
                    tracing::warn!("wechat image msg without image_base64, ignored");
                    return Ok(false);
                }
            }
        } else {
            let text = msg.content.clone().unwrap_or_default();
            if text.trim().is_empty() {
                tracing::debug!("wechat empty text, ignored");
                return Ok(false);
            }
            // 群聊系统提示前缀 (如配置)
            let prefixed = match self.permission.get_group_config(msg.chat_id()) {
                Some(gc) => match &gc.system_prompt {
                    Some(p) => format!("{}\n\n[群聊系统提示]\n{}", p, text),
                    None => text,
                },
                None => text,
            };
            Content::text(prefixed)
        };

        let session_key = msg.session_key();
        let envelope = Envelope::new(
            "wechat",
            &session_key,
            Message { role: Role::User, content },
        );

        let sender = { self.event_tx.read().clone() };
        if let Some(sender) = sender {
            sender
                .send(ChannelEvent::Message(envelope))
                .await
                .map_err(|e| anyhow::anyhow!("failed to queue wechat message: {}", e))?;
            tracing::info!("wechat message queued: session_key={}", session_key);
            Ok(true)
        } else {
            Err(anyhow::anyhow!("wechat channel disconnected"))
        }
    }

    /// 根据 session_key 计算出站目标。
    fn target_from_session(session_key: &str) -> SendTarget {
        let (chat_type, chat_id, _sender) = parse_session_key(session_key);
        if chat_type == "group" {
            SendTarget { to_user: None, room_id: Some(chat_id) }
        } else {
            SendTarget { to_user: Some(chat_id), room_id: None }
        }
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn name(&self) -> &str {
        "wechat"
    }

    async fn connect(&self) -> anyhow::Result<()> {
        tracing::info!("WeChat channel ready (connector send_url={})", self.config.connector_send_url);
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        self.event_tx.write().take();
        Ok(())
    }

    async fn send(&self, envelope: Envelope) -> anyhow::Result<()> {
        let target = Self::target_from_session(&envelope.session_key);
        match envelope.message.content {
            Content::Text(t) => self.client.send_text(&target, &t).await,
            Content::Structured { text, .. } => {
                self.client.send_text(&target, &text.unwrap_or_default()).await
            }
            Content::Image { image_data, image_type, .. } => {
                self.client.send_image(&target, &image_data, &image_type).await
            }
        }
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

    fn base_config() -> WeChatConfig {
        WeChatConfig { dm_enabled: true, group_enabled: true, require_at_in_group: true, ..Default::default() }
    }

    #[tokio::test]
    async fn test_ingest_dm_text_queues() {
        let ch = WeChatChannel::new(base_config());
        let body = serde_json::json!({ "msg_type": "text", "from_user": "wxid_a", "content": "你好" });
        assert!(ch.ingest(body).await.unwrap());
    }

    #[tokio::test]
    async fn test_ingest_group_without_at_ignored() {
        let ch = WeChatChannel::new(base_config());
        let body = serde_json::json!({
            "msg_type": "text", "from_user": "wxid_a",
            "room_id": "g1@chatroom", "is_at_bot": false, "content": "hi"
        });
        assert!(!ch.ingest(body).await.unwrap());
    }

    #[tokio::test]
    async fn test_ingest_group_with_at_queues() {
        let ch = WeChatChannel::new(base_config());
        let body = serde_json::json!({
            "msg_type": "text", "from_user": "wxid_a",
            "room_id": "g1@chatroom", "is_at_bot": true, "content": "hi"
        });
        assert!(ch.ingest(body).await.unwrap());
    }

    #[tokio::test]
    async fn test_ingest_dm_disabled_rejected() {
        let cfg = WeChatConfig { dm_enabled: false, ..base_config() };
        let ch = WeChatChannel::new(cfg);
        let body = serde_json::json!({ "msg_type": "text", "from_user": "wxid_a", "content": "hi" });
        assert!(!ch.ingest(body).await.unwrap());
    }

    #[test]
    fn test_target_from_session() {
        let dm = WeChatChannel::target_from_session("p2p:wxid_a:wxid_a");
        assert_eq!(dm.to_user.as_deref(), Some("wxid_a"));
        assert!(dm.room_id.is_none());
        let g = WeChatChannel::target_from_session("group:g1@chatroom:wxid_a");
        assert_eq!(g.room_id.as_deref(), Some("g1@chatroom"));
        assert!(g.to_user.is_none());
    }
}
