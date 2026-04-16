use anyhow::Result;
use open_lark::client::ws_client::LarkWsClient;
use open_lark::core::config::Config;
use open_lark::event::dispatcher::EventDispatcherHandler;
use open_lark::service::im::v1::p2_im_message_receive_v1::P2ImMessageReceiveV1;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;

use crate::channel::FeishuChannel;
use crate::config::FeishuConfig;
use crate::message::{
    FeishuEvent, FeishuEventHeader, FeishuEventMessage, FeishuEventPayload, FeishuEventSender,
    FeishuSenderId,
};

pub struct WebSocketManager {
    config: FeishuConfig,
    channel: Arc<FeishuChannel>,
    is_connected: Arc<RwLock<bool>>,
}

impl WebSocketManager {
    pub fn new(config: FeishuConfig, channel: Arc<FeishuChannel>) -> Self {
        Self {
            config,
            channel,
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

        // Note: Using std::thread with embedded tokio runtime because open_lark's
        // LarkWsClient contains non-Send types that cannot be used with tokio::spawn
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");

            rt.block_on(async {
                tracing::info!("Starting Feishu WebSocket (v0.14.0 logic) for app_id={}", app_id);

                let lark_config = Config::builder()
                    .app_id(app_id.clone())
                    .app_secret(app_secret)
                    .base_url("https://open.feishu.cn")
                    .req_timeout(Duration::from_secs(30))
                    .build();

                let ch_inner = channel.clone();

                let builder = EventDispatcherHandler::builder();
                let event_handler = match builder.register_p2_im_message_receive_v1(move |event: P2ImMessageReceiveV1| {
                    let event_id = event.header.event_id.clone().unwrap_or_default();
                    tracing::info!("!!! [WS EVENT] Received im.message.receive_v1 (event_id={}) !!!", event_id);

                    tracing::info!("Message content preview: {}", event.event.message.content.chars().take(100).collect::<String>());

                    let ch = ch_inner.clone();

                    let fe_event = FeishuEvent {
                        schema_version: event.schema.clone(),
                        header: FeishuEventHeader {
                            event_id: event_id.clone(),
                            event_type: event.header.event_type.clone().unwrap_or_default(),
                            create_time: event.header.create_time.clone().unwrap_or_default(),
                            token: event.header.token.clone().unwrap_or_default(),
                            app_id: event.header.app_id.clone().unwrap_or_default(),
                            tenant_key: event.header.tenant_key.clone().unwrap_or_default(),
                        },
                        event: FeishuEventPayload {
                            sender: FeishuEventSender {
                                sender_id: FeishuSenderId {
                                    open_id: Some(event.event.sender.sender_id.open_id.clone()),
                                    user_id: Some(event.event.sender.sender_id.user_id.clone()),
                                    union_id: Some(event.event.sender.sender_id.union_id.clone()),
                                },
                                sender_type: event.event.sender.sender_type.clone(),
                                tenant_key: event.event.sender.tenant_key.clone(),
                            },
                            message: FeishuEventMessage {
                                message_id: event.event.message.message_id.clone(),
                                root_id: event.event.message.root_id.clone(),
                                parent_id: event.event.message.parent_id.clone(),
                                create_time: event.event.message.create_time.clone(),
                                chat_id: event.event.message.chat_id.clone(),
                                chat_type: event.event.message.chat_type.clone(),
                                message_type: event.event.message.message_type.clone(),
                                content: event.event.message.content.clone(),
                                mentions: None,
                            },
                        },
                    };

                    tokio::spawn(async move {
                        if let Err(e) = ch.handle_event(&fe_event).await {
                            tracing::error!("Failed to process event {}: {}", event_id, e);
                        }
                    });
                }) {
                    Ok(b) => b.build(),
                    Err(e) => {
                        tracing::error!("Failed to register event handler: {}", e);
                        return;
                    }
                };

                match LarkWsClient::open(Arc::new(lark_config), event_handler).await {
                    Ok(_) => {
                        tracing::info!("Feishu WebSocket connection established");
                        *is_connected.write() = true;
                    }
                    Err(e) => {
                        tracing::error!("Feishu WebSocket connection failed: {:?}", e);
                        *is_connected.write() = false;
                    }
                }
            });
        });

        *self.is_connected.write() = true;
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        *self.is_connected.write() = false;
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        *self.is_connected.read()
    }
}
