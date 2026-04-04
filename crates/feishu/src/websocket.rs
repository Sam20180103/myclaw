use anyhow::Result;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing;

use crate::config::FeishuConfig;
use crate::client::FeishuClient;
use crate::channel::FeishuChannel;
use crate::message::FeishuChallenge;

/// WebSocket 连接管理器 — 管理飞书 WebSocket 长连接的生命周期
pub struct WebSocketManager {
    config: FeishuConfig,
    channel: Arc<FeishuChannel>,
    client: Arc<FeishuClient>,
    ws_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    is_connected: Arc<RwLock<bool>>,
}

impl WebSocketManager {
    /// 创建新的 WebSocket 管理器
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

    /// 启动 WebSocket 连接
    pub async fn start(&self) -> Result<()> {
        if *self.is_connected.read() {
            tracing::warn!("WebSocket already connected");
            return Ok(());
        }

        // 获取租户令牌
        self.client.get_tenant_token().await
            .map_err(|e| anyhow::anyhow!("Failed to get feishu token: {}", e))?;

        // 建立 WebSocket 连接
        let ws_url = "wss://open.feishu.cn/websocket";
        tracing::info!("Connecting to Feishu WebSocket: {}", ws_url);

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

    /// 停止 WebSocket 连接
    pub async fn stop(&self) -> Result<()> {
        if let Some(handle) = self.ws_handle.write().take() {
            handle.abort();
            *self.is_connected.write() = false;
            tracing::info!("Feishu WebSocket connection stopped");
        }
        Ok(())
    }

    /// 检查连接状态
    pub fn is_connected(&self) -> bool {
        *self.is_connected.read()
    }

    /// 运行连接循环 (自动重连)
    async fn run_connection_loop(
        config: FeishuConfig,
        channel: Arc<FeishuChannel>,
        client: Arc<FeishuClient>,
        is_connected: Arc<RwLock<bool>>,
    ) {
        let mut retry_count = 0;
        let max_retries = 10;

        loop {
            match Self::connect_and_listen(&config, &channel, &client).await {
                Ok(()) => {
                    retry_count = 0;
                    tracing::info!("Feishu WebSocket connection closed normally");
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        tracing::error!("Max retries reached, stopping WebSocket connection: {}", e);
                        *is_connected.write() = false;
                        break;
                    }

                    // 指数退避重连
                    let delay = std::cmp::min(2u64.pow(retry_count) * 1000, 30000);
                    tracing::warn!(
                        "WebSocket connection failed (retry {}/{}): {}. Reconnecting in {}ms...",
                        retry_count, max_retries, e, delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
            }
        }
    }

    /// 建立连接并监听消息
    async fn connect_and_listen(
        _config: &FeishuConfig,
        _channel: &Arc<FeishuChannel>,
        _client: &Arc<FeishuClient>,
    ) -> Result<()> {
        // 这里使用 HTTP 长轮询作为 WebSocket 的替代方案
        // 因为飞书的 WebSocket 连接需要特殊的握手流程
        // 实际生产环境中应该使用 tokio-tungstenite 建立真正的 WebSocket 连接
        tracing::info!("Starting Feishu long-polling connection");

        // 模拟长轮询循环
        loop {
            // 在实际实现中, 这里应该:
            // 1. 建立 WebSocket 连接到 wss://open.feishu.cn/websocket
            // 2. 接收消息
            // 3. 解析为 FeishuEvent
            // 4. 调用 channel.handle_event(&event)
            
            // 当前为占位实现, 等待实际的消息推送
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    }

    /// 处理 URL 验证挑战
    pub fn handle_challenge(challenge: &str) -> Result<serde_json::Value> {
        let challenge_resp: FeishuChallenge = serde_json::from_str(challenge)
            .map_err(|e| anyhow::anyhow!("Failed to parse challenge: {}", e))?;

        Ok(serde_json::json!({
            "challenge": challenge_resp.challenge,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_challenge() {
        let challenge_json = r#"{
            "schema": "2.0",
            "header": {
                "event_id": "test",
                "event_type": "url_verification",
                "create_time": "0",
                "token": "test",
                "app_id": "test",
                "tenant_key": "test"
            },
            "challenge": "test_challenge_123"
        }"#;

        let result = WebSocketManager::handle_challenge(challenge_json).unwrap();
        assert_eq!(result["challenge"], "test_challenge_123");
    }

    #[test]
    fn test_websocket_manager_new() {
        let config = FeishuConfig::default();
        let channel = Arc::new(FeishuChannel::new(config.clone()));
        let manager = WebSocketManager::new(config, channel);
        assert!(!manager.is_connected());
    }
}
