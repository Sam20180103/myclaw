use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use parking_lot::RwLock;
use tokio::task::JoinHandle;
use tokio::sync::mpsc;
use open_lark::Config;
use open_lark::ws_client::{EventDispatcherHandler, LarkWsClient};
use tracing;

use crate::channel::FeishuChannel;
use crate::config::FeishuConfig;
use crate::message::FeishuEvent;

pub struct WebSocketManager {
    config: FeishuConfig,
    channel: Arc<FeishuChannel>,
    ws_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    is_connected: Arc<RwLock<bool>>,
}

impl WebSocketManager {
    pub fn new(config: FeishuConfig, channel: Arc<FeishuChannel>) -> Self {
        Self {
            config,
            channel,
            ws_handle: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn start(&self) -> Result<()> {
        if *self.is_connected.read() {
            tracing::warn!("WebSocket already connected");
            return Ok(());
        }

        let app_id = self.config.app_id.clone();
        let app_secret = self.config.app_secret.clone();
        let is_connected = self.is_connected.clone();
        let channel = self.channel.clone();

        let handle = tokio::spawn(async move {
            let lark_config = match Config::builder()
                .app_id(app_id)
                .app_secret(app_secret)
                .base_url("https://open.feishu.cn")
                .timeout(Duration::from_secs(30))
                .build() {
                Ok(config) => config,
                Err(e) => {
                    tracing::error!("Failed to build config: {:?}", e);
                    return;
                }
            };

            let (payload_tx, mut payload_rx) = mpsc::unbounded_channel::<Vec<u8>>();

            let event_handler = EventDispatcherHandler::builder()
                .payload_sender(payload_tx)
                .build();

            tracing::info!("Feishu WebSocket: connecting to Feishu...");

            match LarkWsClient::open(Arc::new(lark_config), event_handler).await {
                Ok(_) => {
                    tracing::info!("Feishu WebSocket connection established");
                    *is_connected.write() = true;

                    tokio::spawn(async move {
                        while let Some(payload) = payload_rx.recv().await {
                            let hex_str = payload.iter().take(100).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
                            let ascii_str: String = payload.iter().take(100).map(|&b| {
                                if b >= 32 && b < 127 { b as char } else { '.' }
                            }).collect();
                            tracing::info!("=== Raw WebSocket payload ===");
                            tracing::info!("Length: {} bytes", payload.len());
                            tracing::info!("Hex: {}", hex_str);
                            tracing::info!("ASCII: {}", ascii_str);
                            
                            let payload_str = String::from_utf8_lossy(&payload);
                            tracing::debug!("Raw payload string: {}", payload_str.chars().take(500).collect::<String>());
                            
                            match serde_json::from_slice::<FeishuEvent>(&payload) {
                                Ok(event) => {
                                    tracing::info!("✓ Parsed as FeishuEvent: type={}, event_id={}", 
                                        event.header.event_type, event.header.event_id);
                                    
                                    if event.header.event_type == "url_verification" {
                                        tracing::info!("URL verification challenge, skipping");
                                        continue;
                                    }
                                    
                                    let ch = channel.clone();
                                    let evt = event.clone();
                                    let event_id = evt.header.event_id.clone();

                                    tracing::debug!("Calling handle_event for event {}", event_id);

                                    tokio::spawn(async move {
                                        if let Err(e) = ch.handle_event(&evt).await {
                                            tracing::error!("Failed to handle feishu event {}: {}", event_id, e);
                                        }
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!("✗ JSON parse failed: {}", e);
                                    tracing::info!("Trying binary protobuf decode...");
                                }
                            }
                        }
                    });

                    loop {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                }
                Err(e) => {
                    tracing::error!("Feishu WebSocket connection failed: {:?}", e);
                    *is_connected.write() = false;
                }
            }
        });

        *self.ws_handle.write() = Some(handle);
        *self.is_connected.write() = true;

        tokio::time::sleep(Duration::from_millis(100)).await;

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
}

#[cfg(test)]
mod tests {
}
