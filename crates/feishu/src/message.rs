use serde::{Deserialize, Serialize};

/// 飞书消息类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeishuMsgType {
    Text,
    Post,
    Image,
    File,
    Audio,
    Media,
    Sticker,
    Interactive,
    ShareChat,
    ShareUser,
    Unknown(String),
}

/// 飞书 @提及
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuMention {
    pub key: String,
    pub id: String,
    pub name: String,
}

/// 飞书消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuMessage {
    pub message_id: String,
    pub chat_id: String,
    pub chat_type: String,
    pub sender_id: String,
    pub msg_type: FeishuMsgType,
    pub content: String,
    pub mentions: Vec<FeishuMention>,
    pub parent_id: Option<String>,
    pub create_time: i64,
}

/// 飞书事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuEvent {
    #[serde(rename = "schema")]
    pub schema_version: String,
    pub header: FeishuEventHeader,
    pub event: FeishuEventPayload,
}

/// 飞书事件头
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuEventHeader {
    pub event_id: String,
    pub event_type: String,
    pub create_time: String,
    pub token: String,
    pub app_id: String,
    pub tenant_key: String,
}

/// 飞书事件负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuEventPayload {
    pub sender: FeishuEventSender,
    pub message: FeishuEventMessage,
}

/// 飞书事件发送者
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuEventSender {
    pub sender_id: FeishuSenderId,
    pub sender_type: String,
    pub tenant_key: String,
}

/// 飞书发送者 ID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuSenderId {
    pub open_id: Option<String>,
    pub user_id: Option<String>,
    pub union_id: Option<String>,
}

/// 飞书事件消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuEventMessage {
    pub message_id: String,
    pub root_id: Option<String>,
    pub parent_id: Option<String>,
    pub create_time: String,
    pub chat_id: String,
    pub chat_type: String,
    pub message_type: String,
    pub content: String,
    pub mentions: Option<Vec<FeishuMention>>,
}

/// URL 验证挑战 (飞书 WebSocket 握手)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuChallenge {
    #[serde(rename = "schema")]
    pub schema_version: String,
    pub header: FeishuEventHeader,
    pub challenge: String,
}
