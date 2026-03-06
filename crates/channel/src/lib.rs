use async_trait::async_trait;
use futures_util::Stream;
use rustclaw_protocol::Envelope;
use std::pin::Pin;

pub type EventStream = Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>;

#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    async fn connect(&self) -> anyhow::Result<()>;
    async fn disconnect(&self) -> anyhow::Result<()>;
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()>;
    fn events(&self) -> EventStream;
}

#[derive(Debug, Clone)]
pub enum ChannelEvent {
    Message(Envelope),
    Connected { channel: String },
    Disconnected { channel: String },
    Error { channel: String, error: String },
}

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
