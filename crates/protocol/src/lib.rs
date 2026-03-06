use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcRequest {
    Standard(Request),
    Notification(Notification),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Success(SuccessResponse),
    Error(ErrorResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: Option<String>,
    pub error: RpcError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    pub fn parse_error() -> Self {
        Self {
            code: -32700,
            message: "Parse error".to_string(),
            data: None,
        }
    }

    pub fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "Invalid Request".to_string(),
            data: None,
        }
    }

    pub fn method_not_found() -> Self {
        Self {
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
    }

    pub fn invalid_params(msg: &str) -> Self {
        Self {
            code: -32602,
            message: format!("Invalid params: {}", msg),
            data: None,
        }
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Structured {
        #[serde(rename = "type")]
        content_type: String,
        text: Option<String>,
        #[serde(default)]
        parts: Vec<ContentPart>,
    },
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
}

#[cfg(test)]
mod tests;
