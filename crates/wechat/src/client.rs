use serde_json::json;

/// 连接器发送客户端 — 把出站消息 POST 到连接器的 send_url。
#[derive(Clone)]
pub struct WeChatConnectorClient {
    send_url: String,
    send_token: Option<String>,
    http: reqwest::Client,
}

/// 出站目标: 私聊给 to_user, 群聊给 room_id。
pub struct SendTarget {
    pub to_user: Option<String>,
    pub room_id: Option<String>,
}

impl WeChatConnectorClient {
    pub fn new(send_url: String, send_token: Option<String>) -> Self {
        Self { send_url, send_token, http: reqwest::Client::new() }
    }

    async fn post(&self, body: serde_json::Value) -> anyhow::Result<()> {
        if self.send_url.is_empty() {
            return Err(anyhow::anyhow!("wechat connector_send_url 未配置"));
        }
        let mut req = self.http.post(&self.send_url).json(&body);
        if let Some(ref token) = self.send_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await
            .map_err(|e| anyhow::anyhow!("发送到连接器失败: {}", e))?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("连接器返回 {}", resp.status()));
        }
        Ok(())
    }

    fn target_fields(target: &SendTarget) -> serde_json::Value {
        let mut v = serde_json::Map::new();
        if let Some(ref u) = target.to_user {
            v.insert("to_user".to_string(), json!(u));
        }
        if let Some(ref r) = target.room_id {
            v.insert("room_id".to_string(), json!(r));
        }
        serde_json::Value::Object(v)
    }

    pub async fn send_text(&self, target: &SendTarget, text: &str) -> anyhow::Result<()> {
        let mut body = Self::target_fields(target);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("msg_type".to_string(), json!("text"));
            obj.insert("content".to_string(), json!(text));
        }
        self.post(body).await
    }

    pub async fn send_image(&self, target: &SendTarget, image_base64: &str, image_type: &str) -> anyhow::Result<()> {
        let mut body = Self::target_fields(target);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("msg_type".to_string(), json!("image"));
            obj.insert("image_base64".to_string(), json!(image_base64));
            obj.insert("image_type".to_string(), json!(image_type));
        }
        self.post(body).await
    }
}
