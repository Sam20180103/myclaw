use async_trait::async_trait;
use futures_util::Stream;
use rustclaw_protocol::Envelope;
use std::pin::Pin;

/// 通道事件流类型 — 用于订阅通道事件
pub type EventStream = Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>;

/// 通道抽象接口 — 所有消息通道必须实现此 trait
/// 支持 WebSocket、HTTP、Webhook 等消息入口
#[async_trait]
pub trait Channel: Send + Sync {
    /// 通道名称
    fn name(&self) -> &str;
    /// 连接通道
    async fn connect(&self) -> anyhow::Result<()>;
    /// 断开通道
    async fn disconnect(&self) -> anyhow::Result<()>;
    /// 发送消息 (Envelope)
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()>;
    /// 订阅通道事件流
    fn events(&self) -> EventStream;
}

/// 通道事件枚举
#[derive(Debug, Clone)]
pub enum ChannelEvent {
    /// 收到消息
    Message(Envelope),
    /// 通道已连接
    Connected { channel: String },
    /// 通道已断开
    Disconnected { channel: String },
    /// 通道错误
    Error { channel: String, error: String },
}

/// WebSocket 通道实现 (当前为占位实现)
pub struct WebSocketChannel {
    name: String,
}

impl WebSocketChannel {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Channel for WebSocketChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn connect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, _envelope: Envelope) -> anyhow::Result<()> {
        Ok(())
    }

    fn events(&self) -> EventStream {
        Box::pin(futures_util::stream::iter(vec![]))
    }
}

/// HTTP 通道实现 (当前为占位实现)
pub struct HttpChannel {
    name: String,
}

impl HttpChannel {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Channel for HttpChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn connect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, _envelope: Envelope) -> anyhow::Result<()> {
        Ok(())
    }

    fn events(&self) -> EventStream {
        Box::pin(futures_util::stream::iter(vec![]))
    }
}

/// Webhook 通道实现 (当前为占位实现)
pub struct WebhookChannel {
    name: String,
}

impl WebhookChannel {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Channel for WebhookChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn connect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, _envelope: Envelope) -> anyhow::Result<()> {
        Ok(())
    }

    fn events(&self) -> EventStream {
        Box::pin(futures_util::stream::iter(vec![]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclaw_protocol::{Content, Envelope, Message, Role};

    #[test]
    fn test_websocket_channel_new() {
        let channel = WebSocketChannel::new("ws-main");
        assert_eq!(channel.name(), "ws-main");
    }

    #[test]
    fn test_http_channel_new() {
        let channel = HttpChannel::new("http-api");
        assert_eq!(channel.name(), "http-api");
    }

    #[test]
    fn test_webhook_channel_new() {
        let channel = WebhookChannel::new("webhook-notify");
        assert_eq!(channel.name(), "webhook-notify");
    }

    #[tokio::test]
    async fn test_websocket_channel_connect() {
        let channel = WebSocketChannel::new("ws-test");
        assert!(channel.connect().await.is_ok());
    }

    #[tokio::test]
    async fn test_http_channel_connect() {
        let channel = HttpChannel::new("http-test");
        assert!(channel.connect().await.is_ok());
    }

    #[tokio::test]
    async fn test_webhook_channel_connect() {
        let channel = WebhookChannel::new("webhook-test");
        assert!(channel.connect().await.is_ok());
    }

    #[tokio::test]
    async fn test_websocket_channel_send() {
        let channel = WebSocketChannel::new("ws-send");
        let envelope = Envelope::new("ws", "session-1", Message {
            role: Role::User,
            content: Content::text("test"),
        });
        assert!(channel.send(envelope).await.is_ok());
    }

    #[test]
    fn test_channel_event_message() {
        let envelope = Envelope::new("http", "session-1", Message {
            role: Role::User,
            content: Content::text("hello"),
        });
        let event = ChannelEvent::Message(envelope);
        match event {
            ChannelEvent::Message(e) => assert_eq!(e.session_key, "session-1"),
            _ => panic!("Expected Message event"),
        }
    }

    #[test]
    fn test_channel_event_connected() {
        let event = ChannelEvent::Connected { channel: "ws".to_string() };
        match event {
            ChannelEvent::Connected { channel } => assert_eq!(channel, "ws"),
            _ => panic!("Expected Connected event"),
        }
    }

    #[test]
    fn test_channel_event_disconnected() {
        let event = ChannelEvent::Disconnected { channel: "http".to_string() };
        match event {
            ChannelEvent::Disconnected { channel } => assert_eq!(channel, "http"),
            _ => panic!("Expected Disconnected event"),
        }
    }

    #[test]
    fn test_channel_event_error() {
        let event = ChannelEvent::Error {
            channel: "ws".to_string(),
            error: "connection lost".to_string(),
        };
        match event {
            ChannelEvent::Error { channel, error } => {
                assert_eq!(channel, "ws");
                assert_eq!(error, "connection lost");
            }
            _ => panic!("Expected Error event"),
        }
    }
}
