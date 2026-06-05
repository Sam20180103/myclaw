use serde::Deserialize;

/// 连接器推送给网关的入站消息 (POST /channel/wechat/inbound 的 body)
#[derive(Debug, Clone, Deserialize)]
pub struct WeChatInbound {
    #[serde(default)]
    pub msg_id: Option<String>,
    /// "text" | "image"
    #[serde(default = "default_msg_type")]
    pub msg_type: String,
    /// 发送者 wxid
    pub from_user: String,
    #[serde(default)]
    pub from_user_name: Option<String>,
    /// 群聊时为群 id (xxx@chatroom); 私聊缺省
    #[serde(default)]
    pub room_id: Option<String>,
    /// 群聊: 是否 @ 了机器人
    #[serde(default)]
    pub is_at_bot: bool,
    /// 文本内容
    #[serde(default)]
    pub content: Option<String>,
    /// 图片: base64 数据
    #[serde(default)]
    pub image_base64: Option<String>,
    /// 图片格式 (png/jpeg/...)
    #[serde(default)]
    pub image_type: Option<String>,
}

fn default_msg_type() -> String {
    "text".to_string()
}

impl WeChatInbound {
    /// 是否群聊
    pub fn is_group(&self) -> bool {
        self.room_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// 会话类型: "group" | "p2p"
    pub fn chat_type(&self) -> &'static str {
        if self.is_group() { "group" } else { "p2p" }
    }

    /// 会话 id: 群聊用 room_id, 私聊用 from_user
    pub fn chat_id(&self) -> &str {
        if self.is_group() {
            self.room_id.as_deref().unwrap_or("")
        } else {
            &self.from_user
        }
    }

    /// session_key 沿用飞书方案: "{chat_type}:{chat_id}:{sender}"
    pub fn session_key(&self) -> String {
        format!("{}:{}:{}", self.chat_type(), self.chat_id(), self.from_user)
    }
}

/// 从 session_key 解析出 (chat_type, chat_id, sender)
pub fn parse_session_key(key: &str) -> (String, String, String) {
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    match parts.as_slice() {
        [ct, cid, sender] => (ct.to_string(), cid.to_string(), sender.to_string()),
        _ => ("p2p".to_string(), key.to_string(), key.to_string()),
    }
}
