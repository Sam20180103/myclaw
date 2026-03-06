use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError>;
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

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read content from a file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_read".to_string(),
            description: "Read content from a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
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

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(serde_json::json!({ "content": content }))
    }
}

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "file_write".to_string(),
            description: "Write content to a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write"
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
        Self {
            sandbox: SandboxExecutor::new(SandboxConfig::default()),
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

impl ToolRegistry {
    pub fn register_builtin_tools(&self) {
        self.register(Arc::new(FileReadTool));
        self.register(Arc::new(FileWriteTool));
        self.register(Arc::new(ShellExecTool::default()));
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
            allowed_tools: vec![
                "file_read".to_string(),
                "file_write".to_string(),
                "shell_exec".to_string(),
            ],
            denied_tools: vec![],
            require_approval: vec!["shell_exec".to_string()],
        }
    }
}

impl ToolPolicy {
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

pub struct SandboxExecutor {
    config: SandboxConfig,
}

impl SandboxExecutor {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
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
        
        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
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
