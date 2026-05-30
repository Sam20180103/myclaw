use anyhow::Result;
use parking_lot::RwLock;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::FeishuConfig;

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

#[derive(Debug, serde::Deserialize)]
struct TenantTokenResponse {
    code: i64,
    msg: Option<String>,
    tenant_access_token: Option<String>,
    expire: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct ApiResponse {
    code: i64,
    msg: Option<String>,
}

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

struct CachedToken {
    token: String,
    expires_at: Instant,
}

#[derive(Clone)]
pub struct FeishuClient {
    config: FeishuConfig,
    tenant_token: Arc<RwLock<Option<CachedToken>>>,
    client: reqwest::Client,
}

impl FeishuClient {
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            config,
            tenant_token: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
    }

    pub async fn get_tenant_token(&self) -> Result<String, FeishuError> {
        {
            let token = self.tenant_token.read();
            if let Some(cached) = token.as_ref() {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

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
        
        let expire_seconds = token_resp.expire.unwrap_or(7200);
        let buffer_seconds = 300i64;
        let effective_expire = Duration::from_secs((expire_seconds - buffer_seconds).max(60) as u64);

        let cached = CachedToken {
            token: token.clone(),
            expires_at: Instant::now() + effective_expire,
        };

        {
            let mut cached_token = self.tenant_token.write();
            *cached_token = Some(cached);
        }

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

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        tracing::info!("!!! [DEBUG] Raw WebSocket link response (Status {}): {} !!!", status, body);

        if !status.is_success() {
            return Err(FeishuError::ApiError(format!("HTTP {}: {}", status, body)));
        }

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

    /// 下载文件/图片到指定目录
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

    /// 下载消息中的资源 (图片/文件)。
    /// 收到的消息图片需通过 messages/{message_id}/resources/{file_key} 端点下载,
    /// 与通过 files API 上传的文件 (download_file) 不同。
    /// `res_type` 为 "image" 或 "file"。
    pub async fn download_message_resource(
        &self,
        message_id: &str,
        file_key: &str,
        res_type: &str,
    ) -> Result<Vec<u8>, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/messages/{}/resources/{}?type={}",
            message_id, file_key, res_type
        );

        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(FeishuError::ApiError(format!(
                "Resource download failed: {}", response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// 下载文件并保存到本地
    pub async fn download_file_to_path(
        &self,
        file_key: &str,
        save_dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, FeishuError> {
        let bytes = self.download_file(file_key).await?;

        std::fs::create_dir_all(save_dir)
            .map_err(|e| FeishuError::ApiError(format!("Failed to create directory: {}", e)))?;

        let file_path = save_dir.join(file_key);
        std::fs::write(&file_path, &bytes)
            .map_err(|e| FeishuError::ApiError(format!("Failed to write file: {}", e)))?;

        Ok(file_path)
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

    /// 上传图片 (Phase 17.5: 支持扫码登录图片发送)
    /// image_type 参数应为 "message" 或 "avatar"，此处固定使用 "message"
    pub async fn upload_image(
        &self,
        image_data: &[u8],
        file_format: &str,
    ) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = "https://open.feishu.cn/open-apis/im/v1/images";

        let file_name = format!("qrcode.{}", file_format);
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", reqwest::multipart::Part::bytes(image_data.to_vec()).file_name(file_name));

        let response = self.client
            .post(url)
            .header("Authorization", format!("Bearer {}", token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| FeishuError::HttpError(e.to_string()))?;

        #[derive(serde::Deserialize)]
        struct ImageUploadResponse {
            code: i64,
            msg: Option<String>,
            data: Option<ImageData>,
        }

        #[derive(serde::Deserialize)]
        struct ImageData {
            image_key: Option<String>,
        }

        let resp: ImageUploadResponse = response
            .json()
            .await
            .map_err(|e| FeishuError::ParseError(e.to_string()))?;

        if resp.code != 0 {
            return Err(FeishuError::ApiError(
                resp.msg.unwrap_or_else(|| "Unknown API error".to_string()),
            ));
        }

        let image_key = resp.data
            .and_then(|d| d.image_key)
            .ok_or_else(|| FeishuError::ApiError("No image_key in response".to_string()))?;

        Ok(image_key)
    }

    /// 发送图片消息 (Phase 17.5)
    pub async fn send_image(
        &self,
        receive_id: &str,
        receive_id_type: &str,
        image_key: &str,
    ) -> Result<String, FeishuError> {
        let token = self.get_tenant_token().await?;
        let url = format!(
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type={}",
            receive_id_type
        );

        let body = json!({
            "receive_id": receive_id,
            "msg_type": "image",
            "content": json!({
                "image_key": image_key
            }).to_string(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_config_default() {
        let config = FeishuConfig::default();
        assert!(config.dm_enabled);
        assert!(config.group_enabled);
        assert!(config.allowed_groups.is_empty());
        assert!(config.allowed_users.is_empty());
    }
}
