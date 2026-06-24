use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub mod reach;
pub use reach::*;

pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError>;

    /// G4: 工具当前是否可用 (例如所需凭据/依赖已配置)。
    /// 不可用的工具不会进入发给 LLM 的 schema,避免浪费 token 与诱导必然失败的调用。
    /// 默认 `true`。
    fn is_available(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Timeout")]
    Timeout,
    #[error("Sandbox violation: {0}")]
    SandboxViolation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::with_capacity(16))),
        }
    }

    pub fn register(&self, tool: Arc<dyn Tool>) {
        self.tools.write().insert(tool.name().to_string(), tool);
    }

    pub fn unregister(&self, name: &str) {
        self.tools.write().remove(name);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().get(name).cloned()
    }

    pub fn list(&self) -> Vec<ToolSchema> {
        self.tools
            .read()
            .values()
            .map(|t| t.schema())
            .collect()
    }

    /// G4: 仅返回当前可用工具的 schema (供 Agent 组装发给 LLM 的工具列表)。
    pub fn available_schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .read()
            .values()
            .filter(|t| t.is_available())
            .map(|t| t.schema())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.read().is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FileReadTool {
    allowed_dirs: Vec<String>,
}

impl FileReadTool {
    pub fn new(allowed_dirs: Vec<String>) -> Self {
        Self { allowed_dirs }
    }

    fn validate_path(&self, path: &str) -> Result<(), ToolError> {
        // Reject path traversal
        if path.contains("..") {
            return Err(ToolError::SandboxViolation(
                "Path traversal not allowed".to_string(),
            ));
        }
        // If allowed_dirs is configured, check path is within allowed directories
        if !self.allowed_dirs.is_empty() {
            let canonical = std::path::Path::new(path)
                .canonicalize()
                .map_err(|e| ToolError::InvalidParams(format!("Invalid path: {}", e)))?;
            let path_str = canonical.to_string_lossy();
            let allowed = self.allowed_dirs.iter().any(|dir| path_str.starts_with(dir));
            if !allowed {
                return Err(ToolError::SandboxViolation(format!(
                    "Path '{}' is not within allowed directories",
                    path
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read content from a file (with path restrictions)"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_read".to_string(),
            description: "Read content from a file (path traversal protected)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (must be within allowed directories)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'path' parameter".to_string()))?;

        self.validate_path(path)?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(serde_json::json!({ "content": content }))
    }
}

pub struct FileWriteTool {
    allowed_dirs: Vec<String>,
}

impl FileWriteTool {
    pub fn new(allowed_dirs: Vec<String>) -> Self {
        Self { allowed_dirs }
    }

    fn validate_path(&self, path: &str) -> Result<(), ToolError> {
        // Reject path traversal
        if path.contains("..") {
            return Err(ToolError::SandboxViolation(
                "Path traversal not allowed".to_string(),
            ));
        }
        // If allowed_dirs is configured, check path is within allowed directories
        if !self.allowed_dirs.is_empty() {
            let parent = std::path::Path::new(path)
                .parent()
                .unwrap_or(std::path::Path::new("."));
            let canonical = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
            let path_str = canonical.to_string_lossy();
            let allowed = self.allowed_dirs.iter().any(|dir| path_str.starts_with(dir));
            if !allowed {
                return Err(ToolError::SandboxViolation(format!(
                    "Path '{}' is not within allowed directories",
                    path
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file (with path restrictions)"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_write".to_string(),
            description: "Write content to a file (path traversal protected)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (must be within allowed directories)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'path' parameter".to_string()))?;

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'content' parameter".to_string()))?;

        self.validate_path(path)?;

        tokio::fs::write(path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(serde_json::json!({ "success": true }))
    }
}

pub struct ShellExecTool {
    sandbox: SandboxExecutor,
}

impl Default for ShellExecTool {
    fn default() -> Self {
        // G5: 后端可经 RUSTCLAW_SANDBOX_BACKEND 选择 (local | docker:IMAGE[:net]),默认 local。
        let backend = std::env::var("RUSTCLAW_SANDBOX_BACKEND")
            .ok()
            .map(|s| ExecutionBackend::from_spec(&s))
            .unwrap_or_default();
        Self {
            sandbox: SandboxExecutor::new(SandboxConfig::default()).with_backend(backend),
        }
    }
}

#[async_trait]
impl Tool for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command with sandbox protection"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shell_exec".to_string(),
            description: "Execute a shell command with sandbox protection (timeout: 30s)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Command to execute"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'command' parameter".to_string()))?;

        self.sandbox.execute_command(command).await
    }
}

pub struct OpenCodeTool {
    opencode_path: String,
    timeout_seconds: u64,
}

impl Default for OpenCodeTool {
    fn default() -> Self {
        let opencode_path = std::env::var("OPENCODE_PATH")
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(".opencode/bin/opencode")
                    .canonicalize()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "opencode".to_string())
            });
        Self {
            opencode_path,
            timeout_seconds: 120,
        }
    }
}

impl OpenCodeTool {
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }
}

#[async_trait]
impl Tool for OpenCodeTool {
    fn name(&self) -> &str {
        "opencode_generate"
    }

    fn description(&self) -> &str {
        "Generate code using OpenCode AI. Use this when you need to create new code files, functions, or scripts based on user requirements."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "opencode_generate".to_string(),
            description: "Generate code using OpenCode AI".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "language": {
                        "type": "string",
                        "description": "Programming language: rust, python, javascript, shell, typescript"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Description of what code to generate"
                    }
                },
                "required": ["language", "prompt"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'language' parameter".to_string()))?;

        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'prompt' parameter".to_string()))?;

        let timeout = std::time::Duration::from_secs(self.timeout_seconds);
        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new(&self.opencode_path)
                .arg("generate")
                .arg("--language")
                .arg(language)
                .arg("--prompt")
                .arg(prompt)
                .output(),
        )
        .await
        .map_err(|_| ToolError::Timeout)?
        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to run opencode: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolError::ExecutionFailed(format!("OpenCode error: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        Ok(serde_json::json!({
            "success": true,
            "code": stdout,
            "language": language
        }))
    }
}

pub struct ResumeReadTool {
    allowed_dirs: Vec<String>,
}

impl ResumeReadTool {
    pub fn new(allowed_dirs: Vec<String>) -> Self {
        Self { allowed_dirs }
    }

    fn validate_path(&self, path: &str) -> Result<(), ToolError> {
        if path.contains("..") {
            return Err(ToolError::SandboxViolation(
                "Path traversal not allowed".to_string(),
            ));
        }
        if !self.allowed_dirs.is_empty() {
            let canonical = std::path::Path::new(path)
                .canonicalize()
                .map_err(|e| ToolError::InvalidParams(format!("Invalid path: {}", e)))?;
            let path_str = canonical.to_string_lossy();
            let allowed = self.allowed_dirs.iter().any(|dir| path_str.starts_with(dir));
            if !allowed {
                return Err(ToolError::SandboxViolation(format!(
                    "Path '{}' is not within allowed directories",
                    path
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for ResumeReadTool {
    fn name(&self) -> &str {
        "resume_read"
    }

    fn description(&self) -> &str {
        "Read and structure a resume from a markdown file for LLM analysis."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "resume_read".to_string(),
            description: "Read and structure a resume file (Markdown supported)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the resume markdown file"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'path' parameter".to_string()))?;

        self.validate_path(path)?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        // In a real implementation, we might do some regex or structural parsing here.
        // For now, we return the content as "resume_content" which the LLM can then process.
        Ok(serde_json::json!({
            "success": true,
            "path": path,
            "resume_content": content,
            "format": "markdown"
        }))
    }
}

impl ToolRegistry {
    pub fn register_builtin_tools(&self) {
        self.register(Arc::new(FileReadTool::new(vec![])));
        self.register(Arc::new(FileWriteTool::new(vec![])));
        self.register(Arc::new(ShellExecTool::default()));
        self.register(Arc::new(OpenCodeTool::default()));
        self.register(Arc::new(ResumeReadTool::new(vec![])));
    }

    pub fn register_all_tools(&self) {
        self.register_builtin_tools();
        crate::reach::register_reach_tools(self, None);
    }

    pub fn register_all_tools_with_config(&self, reach_config: Option<ReachConfig>) {
        self.register_builtin_tools();
        crate::reach::register_reach_tools(self, reach_config);
    }

    pub async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, ToolError> {
        let tool = self.get(&tool_call.name)
            .ok_or_else(|| ToolError::NotFound(tool_call.name.clone()))?;

        let result = tool.execute(tool_call.arguments.clone()).await?;

        Ok(ToolResult {
            id: tool_call.id.clone(),
            result: Some(result),
            error: None,
        })
    }
}

#[derive(Clone)]
pub struct ToolPolicy {
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub require_approval: Vec<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            allowed_tools: vec![],
            denied_tools: vec![],
            require_approval: vec!["shell_exec".to_string()],
        }
    }
}

impl ToolPolicy {
    /// 全放行策略 (无白/黑名单、无审批) — 作为运行时默认,保持向后兼容。
    pub fn permissive() -> Self {
        Self {
            allowed_tools: vec![],
            denied_tools: vec![],
            require_approval: vec![],
        }
    }

    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    pub fn with_denied_tools(mut self, tools: Vec<String>) -> Self {
        self.denied_tools = tools;
        self
    }

    pub fn with_require_approval(mut self, tools: Vec<String>) -> Self {
        self.require_approval = tools;
        self
    }

    pub fn is_allowed(&self, tool_name: &str) -> bool {
        if self.denied_tools.contains(&tool_name.to_string()) {
            return false;
        }
        if self.allowed_tools.is_empty() {
            return true;
        }
        self.allowed_tools.contains(&tool_name.to_string())
    }

    pub fn requires_approval(&self, tool_name: &str) -> bool {
        self.require_approval.contains(&tool_name.to_string())
    }
}

#[derive(Clone)]
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
    policy: ToolPolicy,
}

impl ToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, policy: ToolPolicy) -> Self {
        Self { registry, policy }
    }

    pub async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, ToolError> {
        if !self.policy.is_allowed(&tool_call.name) {
            return Err(ToolError::SandboxViolation(format!(
                "Tool '{}' is not allowed by policy",
                tool_call.name
            )));
        }

        self.registry.execute(tool_call).await
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    pub fn policy(&self) -> &ToolPolicy {
        &self.policy
    }
}

#[derive(Clone)]
pub struct SandboxConfig {
    pub timeout_ms: u64,
    pub max_memory_mb: u64,
    pub allowed_dirs: Vec<String>,
    pub denied_commands: Vec<String>,
    pub env_whitelist: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30000,
            max_memory_mb: 512,
            allowed_dirs: vec![],
            denied_commands: vec![
                "rm -rf /".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                ":(){:|:&};:".to_string(),
            ],
            env_whitelist: vec![
                "HOME".to_string(),
                "PATH".to_string(),
                "USER".to_string(),
            ],
        }
    }
}

/// G5: 执行后端 —— 决定 shell 命令在哪里运行。
/// 为不可信 skill/shell 提供容器隔离的扩展点;`Local` 保持现有行为。
#[derive(Debug, Clone, Default)]
pub enum ExecutionBackend {
    /// 本地进程内 (`sh -c`)。
    #[default]
    Local,
    /// Docker 容器 (`docker run --rm`)。需宿主已安装 docker。
    Docker {
        image: String,
        /// 是否允许容器联网 (默认禁网,降低不可信代码外联风险)。
        network: bool,
    },
}

impl ExecutionBackend {
    /// 从规格串解析: `local` | `docker:IMAGE` | `docker+net:IMAGE`。
    /// 用 `docker+net:` 前缀表示允许联网,使镜像名 (含冒号如 `alpine:3.20`) 不歧义。
    pub fn from_spec(spec: &str) -> Self {
        let spec = spec.trim();
        let (scheme, image) = match spec.split_once(':') {
            Some((s, img)) => (s, img),
            None => (spec, ""),
        };
        match scheme {
            "docker" if !image.is_empty() => ExecutionBackend::Docker {
                image: image.to_string(),
                network: false,
            },
            "docker+net" if !image.is_empty() => ExecutionBackend::Docker {
                image: image.to_string(),
                network: true,
            },
            _ => ExecutionBackend::Local,
        }
    }

    /// 构造执行 `command` 的 (程序, 参数)。`mem_mb` 用于容器内存上限。
    fn build_invocation(&self, command: &str, mem_mb: u64) -> (String, Vec<String>) {
        match self {
            ExecutionBackend::Local => {
                ("sh".to_string(), vec!["-c".to_string(), command.to_string()])
            }
            ExecutionBackend::Docker { image, network } => {
                let mut args = vec!["run".to_string(), "--rm".to_string()];
                if !network {
                    args.push("--network".to_string());
                    args.push("none".to_string());
                }
                if mem_mb > 0 {
                    args.push("--memory".to_string());
                    args.push(format!("{}m", mem_mb));
                }
                // 禁止新增权限,降低逃逸面
                args.push("--security-opt".to_string());
                args.push("no-new-privileges".to_string());
                args.push(image.clone());
                args.push("sh".to_string());
                args.push("-c".to_string());
                args.push(command.to_string());
                ("docker".to_string(), args)
            }
        }
    }
}

pub struct SandboxExecutor {
    config: SandboxConfig,
    backend: ExecutionBackend,
}

impl SandboxExecutor {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config, backend: ExecutionBackend::Local }
    }

    /// G5: 选择执行后端 (默认 Local)。
    pub fn with_backend(mut self, backend: ExecutionBackend) -> Self {
        self.backend = backend;
        self
    }

    pub fn backend(&self) -> &ExecutionBackend {
        &self.backend
    }

    pub async fn execute_command(
        &self,
        command: &str,
    ) -> Result<serde_json::Value, ToolError> {
        if self.is_command_denied(command) {
            return Err(ToolError::SandboxViolation(
                "Command is not allowed".to_string(),
            ));
        }

        let timeout = std::time::Duration::from_millis(self.config.timeout_ms);
        let (program, args) = self.backend.build_invocation(command, self.config.max_memory_mb);

        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new(&program)
                .args(&args)
                .output()
        )
        .await
        .map_err(|_| ToolError::Timeout)?
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if !output.status.success() && output.status.code() != Some(0) {
            return Ok(serde_json::json!({
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "exit_code": output.status.code(),
                "killed": false,
                "timeout": false,
            }));
        }

        Ok(serde_json::json!({
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "exit_code": output.status.code(),
            "killed": false,
            "timeout": false,
        }))
    }

    fn is_command_denied(&self, command: &str) -> bool {
        let cmd_lower = command.to_lowercase();
        for denied in &self.config.denied_commands {
            if cmd_lower.contains(&denied.to_lowercase()) {
                return true;
            }
        }
        
        let dangerous_patterns = [
            "curl | sh",
            "wget | sh",
            "chmod 777",
            "chown -R",
            "mkfs",
            "> /dev/sd",
        ];
        
        for pattern in dangerous_patterns {
            if cmd_lower.contains(pattern) {
                return true;
            }
        }
        
        false
    }

    pub fn is_path_allowed(&self, path: &str) -> bool {
        if self.config.allowed_dirs.is_empty() {
            return true;
        }
        
        for allowed in &self.config.allowed_dirs {
            if path.starts_with(allowed) {
                return true;
            }
        }
        
        false
    }
}

#[cfg(test)]
mod tests;
