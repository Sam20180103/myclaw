use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JSON-RPC 请求枚举 — 支持标准请求和通知
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcRequest {
    Standard(Request),          // 带 ID 的请求 (需要响应)
    Notification(Notification), // 不带 ID 的通知 (不需要响应)
}

/// JSON-RPC 标准请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 通知 (不需要响应)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 响应枚举 — 成功或错误
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Success(SuccessResponse),
    Error(ErrorResponse),
}

/// JSON-RPC 成功响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub result: serde_json::Value,
}

/// JSON-RPC 错误响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub error: RpcError,
}

/// JSON-RPC 错误结构 — 符合 JSON-RPC 2.0 规范
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// -32700: 解析错误
    pub fn parse_error() -> Self {
        Self {
            code: -32700,
            message: "Parse error".to_string(),
            data: None,
        }
    }

    /// -32600: 无效请求
    pub fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "Invalid Request".to_string(),
            data: None,
        }
    }

    /// -32601: 方法未找到
    pub fn method_not_found() -> Self {
        Self {
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
    }

    /// -32602: 无效参数
    pub fn invalid_params(msg: &str) -> Self {
        Self {
            code: -32602,
            message: format!("Invalid params: {}", msg),
            data: None,
        }
    }

    /// -32603: 内部错误
    pub fn internal_error(msg: &str) -> Self {
        Self {
            code: -32603,
            message: format!("Internal error: {}", msg),
            data: None,
        }
    }
}

impl JsonRpcRequest {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        JsonRpcRequest::Standard(Request {
            jsonrpc: "2.0".to_string(),
            id: Some(Uuid::new_v4().to_string()),
            method: method.to_string(),
            params,
        })
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        JsonRpcRequest::Notification(Notification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        })
    }
}

impl JsonRpcResponse {
    pub fn success(id: Option<String>, result: serde_json::Value) -> Self {
        JsonRpcResponse::Success(SuccessResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        })
    }

    pub fn error(id: Option<String>, error: RpcError) -> Self {
        JsonRpcResponse::Error(ErrorResponse {
            jsonrpc: "2.0".to_string(),
            id,
            error,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub channel: String,
    pub session_key: String,
    pub message: Message,
    pub timestamp: i64,
}

impl Envelope {
    pub fn new(channel: &str, session_key: &str, message: Message) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel: channel.to_string(),
            session_key: session_key.to_string(),
            message,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// 内容类型枚举 — 支持多种消息格式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// 纯文本消息
    Text(String),
    /// 结构化消息 (支持多部分内容)
    Structured {
        #[serde(rename = "type")]
        content_type: String,
        text: Option<String>,
        #[serde(default)]
        parts: Vec<ContentPart>,
    },
    /// 图片消息 (Phase 17.5: 支持扫码登录)
    Image {
        /// 图片数据 (Base64编码)
        #[serde(rename = "image_data")]
        image_data: String,
        /// 图片格式 (png, jpeg, gif等)
        #[serde(rename = "image_type", default = "default_image_type")]
        image_type: String,
        /// 图片描述 (可选)
        #[serde(default)]
        description: Option<String>,
    },
}

/// 默认图片类型
fn default_image_type() -> String {
    "png".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }

    /// 创建图片内容 (Phase 17.5)
    pub fn image(data: impl Into<String>, image_type: impl Into<String>) -> Self {
        Content::Image {
            image_data: data.into(),
            image_type: image_type.into(),
            description: None,
        }
    }

    /// 创建带描述的图片内容 (Phase 17.5)
    pub fn image_with_description(
        data: impl Into<String>,
        image_type: impl Into<String>,
        desc: impl Into<String>,
    ) -> Self {
        Content::Image {
            image_data: data.into(),
            image_type: image_type.into(),
            description: Some(desc.into()),
        }
    }
}

#[cfg(test)]
mod tests;
