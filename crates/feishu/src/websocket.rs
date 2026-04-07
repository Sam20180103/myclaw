use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing;

use crate::channel::FeishuChannel;
use crate::client::FeishuClient;
use crate::config::FeishuConfig;
use crate::message::FeishuEvent;

#[derive(Debug, Serialize)]
struct AuthPayload {
    app_id: String,
    app_secret: String,
}

pub struct WebSocketManager {
    config: FeishuConfig,
    channel: Arc<FeishuChannel>,
    client: Arc<FeishuClient>,
    ws_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    is_connected: Arc<RwLock<bool>>,
}

impl WebSocketManager {
    pub fn new(config: FeishuConfig, channel: Arc<FeishuChannel>) -> Self {
        let client = Arc::new(FeishuClient::new(config.clone()));
        Self {
            config,
            channel,
            client,
            ws_handle: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        if *self.is_connected.read() {
            tracing::warn!("WebSocket already connected");
            return Ok(());
        }

        self.client
            .get_tenant_token()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get feishu token: {}", e))?;

        let config = self.config.clone();
        let channel = self.channel.clone();
        let client = self.client.clone();
        let is_connected = self.is_connected.clone();

        let handle = tokio::spawn(async move {
            Self::run_connection_loop(config, channel, client, is_connected).await;
        });

        *self.ws_handle.write() = Some(handle);
        *self.is_connected.write() = true;

        tracing::info!("Feishu WebSocket connection started");
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        if let Some(handle) = self.ws_handle.write().take() {
            handle.abort();
            *self.is_connected.write() = false;
            tracing::info!("Feishu WebSocket connection stopped");
        }
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        *self.is_connected.read()
    }

    async fn run_connection_loop(
        config: FeishuConfig,
        channel: Arc<FeishuChannel>,
        client: Arc<FeishuClient>,
        is_connected: Arc<RwLock<bool>>,
    ) {
        let mut retry_count = 0u32;
        let max_retries = 50;

        loop {
            match Self::connect_and_listen(&config, &channel, &client).await {
                Ok(()) => {
                    tracing::info!("Feishu WebSocket connection closed normally");
                    retry_count = 0;
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        tracing::error!(
                            "Max retries reached, stopping WebSocket connection: {}",
                            e
                        );
                        *is_connected.write() = false;
                        break;
                    }

                    let delay = std::cmp::min(2u64.pow(retry_count.min(5)) * 1000, 30000);
                    tracing::warn!(
                        "WebSocket connection failed (retry {}/{}): {}. Reconnecting in {}ms...",
                        retry_count,
                        max_retries,
                        e,
                        delay
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    async fn connect_and_listen(
        config: &FeishuConfig,
        channel: &Arc<FeishuChannel>,
        client: &Arc<FeishuClient>,
    ) -> Result<()> {
        let ws_url = client.get_websocket_link().await?;
        tracing::info!("WebSocket URL: {}", ws_url);

        let (ws_stream, response) = connect_async(ws_url.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("WebSocket connect failed: {}", e))?;

        tracing::info!("WebSocket response status: {}", response.status());
        tracing::info!("Feishu WebSocket connection established");

        let (mut write, mut read) = ws_stream.split();

        let auth_payload = AuthPayload {
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
        };
        let auth_json = serde_json::to_string(&auth_payload)?;
        write
            .send(WsMessage::Text(auth_json))
            .await?;
        tracing::info!("Auth message sent");

        loop {
            let msg = read.next().await;
            match msg {
                Some(Ok(WsMessage::Text(text))) => {
                    tracing::info!("Received: {}", text);
                    if text.contains("\"code\":0") || text.contains("\"msg\":\"success\"") {
                        tracing::info!("Auth acknowledged");
                        break;
                    }
                }
                Some(Ok(WsMessage::Close(frame))) => {
                    return Err(anyhow::anyhow!("WebSocket closed: {:?}", frame));
                }
                Some(Err(e)) => {
                    return Err(anyhow::anyhow!("WebSocket error: {}", e));
                }
                None => {
                    return Err(anyhow::anyhow!("WebSocket stream ended"));
                }
                Some(Ok(other)) => {
                    tracing::debug!("Non-text message during auth: {:?}", other);
                }
            }
        }

        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    let hb = serde_json::json!({
                        "type": "ping",
                        "timestamp": chrono::Utc::now().timestamp_millis()
                    });
                    if let Err(e) = write.send(WsMessage::Text(hb.to_string())).await {
                        tracing::warn!("Failed to send heartbeat: {}", e);
                        return Err(anyhow::anyhow!("Heartbeat failed: {}", e));
                    }
                    tracing::debug!("Heartbeat sent");
                }

                msg = read.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            tracing::debug!("Event received: {}", text);
                            if let Ok(event) = serde_json::from_str::<FeishuEvent>(&text) {
                                if event.header.event_type != "url_verification" {
                                    let ch = channel.clone();
                                    let evt = event.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = ch.handle_event(&evt).await {
                                            tracing::error!("Failed to handle feishu event: {}", e);
                                        }
                                    });
                                }
                            }
                        }
                        Some(Ok(WsMessage::Ping(data))) => {
                            let _ = write.send(WsMessage::Pong(data)).await;
                        }
                        Some(Ok(WsMessage::Close(frame))) => {
                            tracing::info!("WebSocket closed: {:?}", frame);
                            return Err(anyhow::anyhow!("WebSocket closed: {:?}", frame));
                        }
                        Some(Err(e)) => {
                            return Err(anyhow::anyhow!("WebSocket error: {}", e));
                        }
                        None => {
                            return Err(anyhow::anyhow!("WebSocket stream ended"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_payload_serialization() {
        let payload = AuthPayload {
            app_id: "test_id".to_string(),
            app_secret: "test_secret".to_string(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("test_id"));
        assert!(json.contains("test_secret"));
    }
}
