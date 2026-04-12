#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::*;

    #[test]
    fn test_request_creation() {
        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some("test-id".to_string()),
            method: "test.method".to_string(),
            params: serde_json::json!({"key": "value"}),
        };

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "test.method");
        assert!(request.id.is_some());
    }

    #[test]
    fn test_json_rpc_request_creation() {
        let request = JsonRpcRequest::new("test.method", serde_json::json!({}));

        match request {
            JsonRpcRequest::Standard(req) => {
                assert_eq!(req.jsonrpc, "2.0");
                assert_eq!(req.method, "test.method");
                assert!(req.id.is_some());
            }
            _ => panic!("Expected Standard request"),
        }
    }

    #[test]
    fn test_json_rpc_notification() {
        let notif = JsonRpcRequest::notification("test.event", serde_json::json!({"data": "test"}));

        match notif {
            JsonRpcRequest::Notification(n) => {
                assert_eq!(n.jsonrpc, "2.0");
                assert_eq!(n.method, "test.event");
            }
            _ => panic!("Expected Notification"),
        }
    }

    #[test]
    fn test_success_response() {
        let response = JsonRpcResponse::success(
            Some("test-id".to_string()),
            serde_json::json!({"result": "ok"}),
        );

        match response {
            JsonRpcResponse::Success(resp) => {
                assert_eq!(resp.jsonrpc, "2.0");
                assert_eq!(resp.id, Some("test-id".to_string()));
            }
            _ => panic!("Expected Success response"),
        }
    }

    #[test]
    fn test_error_response() {
        let error = RpcError::method_not_found();
        let response = JsonRpcResponse::error(Some("test-id".to_string()), error);

        match response {
            JsonRpcResponse::Error(resp) => {
                assert_eq!(resp.jsonrpc, "2.0");
                assert_eq!(resp.error.code, -32601);
            }
            _ => panic!("Expected Error response"),
        }
    }

    #[test]
    fn test_rpc_errors() {
        let parse_error = RpcError::parse_error();
        assert_eq!(parse_error.code, -32700);

        let invalid_request = RpcError::invalid_request();
        assert_eq!(invalid_request.code, -32600);

        let method_not_found = RpcError::method_not_found();
        assert_eq!(method_not_found.code, -32601);

        let invalid_params = RpcError::invalid_params("missing param");
        assert_eq!(invalid_params.code, -32602);

        let internal = RpcError::internal_error("something failed");
        assert_eq!(internal.code, -32603);
    }

    #[test]
    fn test_envelope_creation() {
        let envelope = Envelope::new(
            "websocket",
            "session-123",
            Message {
                role: Role::User,
                content: Content::text("Hello"),
            },
        );

        assert_eq!(envelope.channel, "websocket");
        assert_eq!(envelope.session_key, "session-123");
        assert!(!envelope.id.is_empty());
    }

    #[test]
    fn test_content_text() {
        let content = Content::text("Hello World");
        match content {
            Content::Text(s) => assert_eq!(s, "Hello World"),
            _ => panic!("Expected Text content"),
        }
    }

    #[test]
    fn test_role_serialization() {
        let user = Role::User;
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, "\"user\"");

        let assistant = Role::Assistant;
        let json = serde_json::to_string(&assistant).unwrap();
        assert_eq!(json, "\"assistant\"");

        let system = Role::System;
        let json = serde_json::to_string(&system).unwrap();
        assert_eq!(json, "\"system\"");
    }
}
