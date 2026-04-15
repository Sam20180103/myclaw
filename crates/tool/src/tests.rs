#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::*;
    use std::sync::Arc;

    #[test]
    fn test_tool_registry_new() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_tool_registry_register() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(FileReadTool::new(vec![])));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_tool_registry_get() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(FileReadTool::new(vec![])));

        let tool = registry.get("file_read");
        assert!(tool.is_some());

        let not_found = registry.get("nonexistent");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_tool_registry_list() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(FileReadTool::new(vec![])));
        registry.register(Arc::new(FileWriteTool::new(vec![])));

        let schemas = registry.list();
        assert_eq!(schemas.len(), 2);
    }

    #[test]
    fn test_tool_registry_unregister() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(FileReadTool::new(vec![])));
        assert_eq!(registry.len(), 1);

        registry.unregister("file_read");
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_file_read_tool_schema() {
        let tool = FileReadTool::new(vec![]);
        assert_eq!(tool.name(), "file_read");
        assert!(tool.description().contains("Read content"));

        let schema = tool.schema();
        assert_eq!(schema.name, "file_read");
        assert!(schema.parameters.is_object());
    }

    #[test]
    fn test_file_write_tool_schema() {
        let tool = FileWriteTool::new(vec![]);
        assert_eq!(tool.name(), "file_write");
        assert!(tool.description().contains("Write content"));
    }

    #[test]
    fn test_shell_exec_tool_schema() {
        let tool = ShellExecTool::default();
        assert_eq!(tool.name(), "shell_exec");
        assert_eq!(
            tool.description(),
            "Execute a shell command with sandbox protection"
        );
    }

    #[test]
    fn test_tool_call_structure() {
        let call = ToolCall {
            id: "call-123".to_string(),
            name: "file_read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };

        assert_eq!(call.id, "call-123");
        assert_eq!(call.name, "file_read");
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult {
            id: "call-123".to_string(),
            result: Some(serde_json::json!({"content": "test"})),
            error: None,
        };

        assert!(result.result.is_some());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult {
            id: "call-123".to_string(),
            result: None,
            error: Some("File not found".to_string()),
        };

        assert!(result.result.is_none());
        assert!(result.error.is_some());
    }

    #[test]
    fn test_tool_policy_default() {
        let policy = ToolPolicy::default();
        assert!(policy.allowed_tools.is_empty());
        assert!(policy.denied_tools.is_empty());
        assert!(policy.is_allowed("file_read"));
        assert!(policy.is_allowed("shell_exec"));
        assert!(policy.requires_approval("shell_exec"));
    }

    #[test]
    fn test_tool_policy_denied_tools() {
        let policy = ToolPolicy::default().with_denied_tools(vec!["rm_rf".to_string()]);
        assert!(!policy.is_allowed("rm_rf"));
        assert!(policy.is_allowed("file_read"));
    }

    #[test]
    fn test_tool_policy_requires_approval() {
        let policy = ToolPolicy::default();
        assert!(policy.requires_approval("shell_exec"));
        assert!(!policy.requires_approval("file_read"));
    }

    #[test]
    fn test_tool_policy_denied() {
        let policy = ToolPolicy {
            allowed_tools: vec!["file_read".to_string()],
            denied_tools: vec!["shell_exec".to_string()],
            require_approval: vec![],
        };

        assert!(policy.is_allowed("file_read"));
        assert!(!policy.is_allowed("shell_exec"));
    }

    #[test]
    fn test_resume_read_tool_schema() {
        let tool = ResumeReadTool::new(vec![]);
        assert_eq!(tool.name(), "resume_read");
        let schema = tool.schema();
        assert_eq!(schema.name, "resume_read");
        assert!(schema.parameters.get("properties").unwrap().get("path").is_some());
    }

    #[test]
    fn test_boss_zhipin_reach_tool_extended_schema() {
        let config = ReachConfig::default();
        let tool = BossZhipinReachTool::new(config);
        assert_eq!(tool.name(), "boss_zhipin_reach");
        let schema = tool.schema();
        let actions = schema.parameters
            .get("properties").unwrap()
            .get("action").unwrap()
            .get("enum").unwrap()
            .as_array().unwrap();
        
        let actions_str: Vec<&str> = actions.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(actions_str.contains(&"apply"));
        assert!(actions_str.contains(&"check_auth"));
    }
}
