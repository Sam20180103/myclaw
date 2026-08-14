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
            principals: Default::default(),
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

    // --- G5: 执行后端抽象 ---

    #[test]
    fn test_g5_backend_from_spec() {
        assert!(matches!(ExecutionBackend::from_spec("local"), ExecutionBackend::Local));
        match ExecutionBackend::from_spec("docker:alpine:3.20") {
            ExecutionBackend::Docker { image, network } => {
                assert_eq!(image, "alpine:3.20");
                assert!(!network);
            }
            _ => panic!("expected docker backend"),
        }
        match ExecutionBackend::from_spec("docker+net:python:3.12") {
            ExecutionBackend::Docker { image, network } => {
                assert_eq!(image, "python:3.12");
                assert!(network);
            }
            _ => panic!("expected docker backend with net"),
        }
        // 缺 image → 退回 Local
        assert!(matches!(ExecutionBackend::from_spec("docker:"), ExecutionBackend::Local));
    }

    #[test]
    fn test_g5_local_invocation() {
        let (prog, args) = ExecutionBackend::Local.build_invocation("echo hi", 512);
        assert_eq!(prog, "sh");
        assert_eq!(args, vec!["-c".to_string(), "echo hi".to_string()]);
    }

    #[test]
    fn test_g5_docker_invocation_builds_isolated_run() {
        let backend = ExecutionBackend::Docker { image: "alpine".to_string(), network: false };
        let (prog, args) = backend.build_invocation("echo hi", 256);
        assert_eq!(prog, "docker");
        let joined = args.join(" ");
        assert!(joined.contains("run --rm"));
        assert!(joined.contains("--network none"), "no-net by default: {}", joined);
        assert!(joined.contains("--memory 256m"));
        assert!(joined.contains("no-new-privileges"));
        assert_eq!(args.last().unwrap(), "echo hi");
        assert!(joined.contains("alpine sh -c"));
    }

    #[test]
    fn test_g5_docker_with_network_omits_none() {
        let backend = ExecutionBackend::Docker { image: "alpine".to_string(), network: true };
        let (_p, args) = backend.build_invocation("ls", 0);
        let joined = args.join(" ");
        assert!(!joined.contains("--network none"));
        assert!(!joined.contains("--memory"), "mem 0 omits limit: {}", joined);
    }

    #[test]
    fn test_g5_sandbox_default_backend_is_local() {
        let sandbox = SandboxExecutor::new(SandboxConfig::default());
        assert!(matches!(sandbox.backend(), ExecutionBackend::Local));
    }
    #[test]
    fn test_policy_principal_override() {
        // P2-17: 工具权限必须能按调用者收紧,而不是全局一份
        let base = ToolPolicy::permissive()
            .with_denied_tools(vec!["dangerous".to_string()])
            .with_principal(
                "ou_guest",
                PrincipalPolicy {
                    allowed_tools: Some(vec!["file_read".to_string()]),
                    denied_tools: None,
                    require_approval: Some(vec!["file_read".to_string()]),
                },
            );

        // 未配置覆盖的调用者沿用全局策略
        let admin = base.for_principal(Some("ou_admin"));
        assert!(admin.is_allowed("shell_exec"));
        assert!(!admin.is_allowed("dangerous"));
        assert!(!admin.requires_approval("file_read"));

        // 受限调用者: 白名单外一律拒绝, 白名单内也要审批
        let guest = base.for_principal(Some("ou_guest"));
        assert!(guest.is_allowed("file_read"));
        assert!(!guest.is_allowed("shell_exec"));
        assert!(guest.requires_approval("file_read"));
        // 覆盖未指定 denied_tools -> 继承全局
        assert!(!guest.is_allowed("dangerous"));

        // 无 principal 时等价于全局策略
        assert!(base.for_principal(None).is_allowed("shell_exec"));
    }


    #[test]
    fn test_skill_tools_always_require_approval() {
        // X-3: 技能是 LLM 生成的不可信代码,即便策略全放行也必须人工批准
        let permissive = ToolPolicy::permissive();
        assert!(!permissive.requires_approval("shell_exec"));
        assert!(permissive.requires_approval("dyn_skill_anything"));
        assert!(permissive.requires_approval(&format!("{}x", SKILL_TOOL_PREFIX)));
        // 内置的技能管理工具不得被前缀规则误伤 (它叫 skill_manage)
        assert!(!permissive.requires_approval("skill_manage"));

        // per-principal 覆盖也不能把技能的审批要求摘掉
        let per_principal = ToolPolicy::permissive().with_principal(
            "trusted",
            PrincipalPolicy {
                allowed_tools: None,
                denied_tools: None,
                require_approval: Some(vec![]),
            },
        );
        assert!(per_principal
            .for_principal(Some("trusted"))
            .requires_approval("dyn_skill_anything"));

        // 但仍可整体禁用某个技能工具
        let denied = ToolPolicy::permissive().with_denied_tools(vec!["dyn_skill_bad".to_string()]);
        assert!(!denied.is_allowed("dyn_skill_bad"));
    }
}
