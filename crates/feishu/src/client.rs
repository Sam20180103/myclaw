use anyhow::Result;
use parking_lot::RwLock;
use serde_json::json;
use std::sync::Arc;

use crate::config::FeishuConfig;

/// 飞书 API 错误
#[derive(Debug, thiserror::Error)]
pub enum FeishuError {
    #[error("Token error: {0}")]
    TokenError(String),
    #[error("API error: {0}")]
    ApiError(String),
    #[error("HTTP error: {0}")]
    HttpError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

/// 租户令牌响应
#[derive(Debug, serde::Deserialize)]
struct TenantTokenResponse {
    code: i64,
    msg: Option<String>,
    tenant_access_token: Option<String>,
    #[allow(dead_code)]
    expire: Option<i64>,
}

/// 飞书 API 响应
#[derive(Debug, serde::Deserialize)]
struct ApiResponse {
    code: i64,
    msg: Option<String>,
}

/// 飞书发送消息响应
#[derive(Debug, serde::Deserialize)]
struct SendMessageResponse {
    code: i64,
    msg: Option<String>,
    data: Option<SendMessageData>,
}

#[derive(Debug, serde::Deserialize)]
struct SendMessageData {
    message_id: Option<String>,
}

/// 飞书 API 客户端 — 封装飞书开放平台 API 调用
#[derive(Clone)]
pub struct FeishuClient {
    config: FeishuConfig,
    /// 租户访问令牌 (自动刷新)
    tenant_token: Arc<RwLock<Option<String>>>,
    /// HTTP 客户端
    client: reqwest::Client,
}

impl FeishuClient {
    /// 创建新的飞书 API 客户端
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            config,
            tenant_token: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
    }

    /// 获取租户访问令牌
    /// 如果缓存中没有或已过期, 自动获取新令牌
    pub async fn get_tenant_token(&self) -> Result<String, FeishuError> {
        // 先检查缓存
        {
            let token = self.tenant_token.read();
            if let Some(t) = token.as_ref() {
                return Ok(t.clone());
            }
        }

        // 获取新令牌
        let url = "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal";
        let body = json!({
            "app_id": &self.config.app_id,
            "app_secret": &self.config.app_secret,
        });

        let response = self.client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        let token_resp: TenantTokenResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if token_resp.code != 0 {
            return Err(FeishuError::TokenError(
                token_resp.msg.unwrap_or_else(|| "Unknown token error".to_string()),
            ));
        }

        let token = token_resp.tenant_access_token
            .ok_or_else(|| FeishuError::TokenError("No token in response".to_string()))?;

        // 缓存令牌
        let mut cached = self.tenant_token.write();
        *cached = Some(token.clone());

        Ok(token)
    }

    pub async fn get_websocket_link(&self) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = "https://open.feishu.cn/open-apis/ws/v2/link";

        let response = self.client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        let body = response
            .text()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        tracing::debug!("WebSocket link response: {}", body);

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1) != 0 {
            let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Err(FeishuError::ApiError(msg.to_string()));
        }

        let ws_url = json.get("data")
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| FeishuError::ApiError("No WebSocket URL in response".to_string()))?
            .to_string();

        Ok(ws_url)
    }

    /// 发送消息到飞书
    pub async fn send_message(
        &self,
        receive_id: &str,
        receive_id_type: &str,
        msg_type: &str,
        content: &str,
    ) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type={}",
            receive_id_type
        );

        let body = json!({
            "receive_id": receive_id,
            "msg_type": msg_type,
            "content": content,
        });

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        let resp: SendMessageResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if resp.code != 0 {
            return Err(FeishuError::ApiError(
                resp.msg.unwrap_or_else(|| "Unknown API error".to_string()),
            ));
        }

        let message_id = resp.data
            .and_then(|d| d.message_id)
            .ok_or_else(|| FeishuError::ApiError("No message_id in response".to_string()))?;

        Ok(message_id)
    }

    /// 回复消息
    pub async fn reply_message(
        &self,
        message_id: &str,
        msg_type: &str,
        content: &str,
    ) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/messages/{}/reply",
            message_id
        );

        let body = json!({
            "msg_type": msg_type,
            "content": content,
        });

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        let resp: SendMessageResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if resp.code != 0 {
            return Err(FeishuError::ApiError(
                resp.msg.unwrap_or_else(|| "Unknown API error".to_string()),
            ));
        }

        let reply_id = resp.data
            .and_then(|d| d.message_id)
            .ok_or_else(|| FeishuError::ApiError("No message_id in response".to_string()))?;

        Ok(reply_id)
    }

    /// 更新消息内容 (用于流式响应)
    pub async fn update_message(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<(), FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/messages/{}",
            message_id
        );

        let body = json!({
            "content": content,
        });

        let response = self.client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        let resp: ApiResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if resp.code != 0 {
            return Err(FeishuError::ApiError(
                resp.msg.unwrap_or_else(|| "Unknown API error".to_string()),
            ));
        }

        Ok(())
    }

    /// 发送交互卡片消息
    pub async fn send_card(
        &self,
        receive_id: &str,
        receive_id_type: &str,
        card: serde_json::Value,
    ) -> Result<String, FeishuError> {
        let content = json!({
            "config": { "wide_screen_mode": true },
            "elements": card["elements"],
        });

        let content_str = serde_json::to_string(&content)
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        self.send_message(receive_id, receive_id_type, "interactive", &content_str).await
    }

    /// 下载文件/图片
    pub async fn download_file(&self, file_key: &str) -> Result<Vec<u8>, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/files/{}",
            file_key
        );

        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(FeishuError::ApiError(format!(
                "Download failed: {}", response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// 上传文件
    pub async fn upload_file(
        &self,
        file_type: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = "https://open.feishu.cn/open-apis/im/v1/files";

        let form = reqwest::multipart::Form::new()
            .text("file_type", file_type.to_string())
            .text("file_name", file_name.to_string())
            .part("file", reqwest::multipart::Part::bytes(data.to_vec()).file_name(file_name.to_string()));

        let response = self.client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        #[derive(serde::Deserialize)]
        struct UploadResponse {
            code: i64,
            msg: Option<String>,
            data: Option<UploadData>,
        }

        #[derive(serde::Deserialize)]
        struct UploadData {
            key: Option<String>,
        }

        let resp: UploadResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if resp.code != 0 {
            return Err(FeishuError::ApiError(
                resp.msg.unwrap_or_else(|| "Unknown API error".to_string()),
            ));
        }

        let file_key = resp.data
            .and_then(|d| d.key)
            .ok_or_else(|| FeishuError::ApiError("No file key in response".to_string()))?;

        Ok(file_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_config_default() {
        let config = FeishuConfig::default();
        assert!(config.dm_enabled);
        assert!(!config.group_enabled);
        assert!(config.allowed_groups.is_empty());
        assert!(config.allowed_users.is_empty());
    }
}
