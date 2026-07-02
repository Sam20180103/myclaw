//! 内置高阶工具: 把 MemoryManager / SkillManager 暴露为 Agent 可调用的 Tool。
//!
//! 这些工具让 Agent 具备「自我进化」能力:
//! - `memory`: 主动增删改自身的持久记忆 (MEMORY.md)
//! - `skill_manage`: 创建/查询/修改/删除动态 Skill (创建走审批流程)
//!
//! 放在 agent crate 是因为 agent 同时依赖 tool / memory / skill 三者,
//! 避免在底层 crate 之间引入额外依赖。

use std::sync::Arc;

use async_trait::async_trait;
use rustclaw_memory::MemoryProvider;
use rustclaw_skill::{SkillLanguage, SkillManager};
use rustclaw_tool::{Tool, ToolError, ToolSchema};
use serde_json::{json, Value};

/// 记忆管理工具 — Agent 自主管理持久记忆。
/// G6: 依赖 MemoryProvider 抽象,与具体记忆后端解耦。
pub struct MemoryTool {
    memory: Arc<dyn MemoryProvider>,
}

impl MemoryTool {
    pub fn new(memory: Arc<dyn MemoryProvider>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Manage your own long-term memory: add/replace/remove/list persistent notes about the user and ongoing work"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory".to_string(),
            description: "Manage long-term memory. Use 'add' to remember a new fact, 'replace' to update an existing one, 'remove' to forget, 'list' to review.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "replace", "remove", "list"],
                        "description": "Operation to perform"
                    },
                    "content": {
                        "type": "string",
                        "description": "For 'add': the fact to remember"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "For 'replace': substring of the memory to update"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "For 'replace': the replacement text"
                    },
                    "text": {
                        "type": "string",
                        "description": "For 'remove': substring identifying the memory to delete"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let map_err = |e: rustclaw_memory::MemoryError| ToolError::ExecutionFailed(e.to_string());

        match action {
            "add" => {
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'add' requires 'content'".to_string()))?;
                self.memory.add_entry(content).map_err(map_err)?;
                Ok(json!({ "success": true, "action": "add" }))
            }
            "replace" => {
                let old_text = params
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'replace' requires 'old_text'".to_string()))?;
                let new_text = params
                    .get("new_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'replace' requires 'new_text'".to_string()))?;
                self.memory.replace_entry(old_text, new_text).map_err(map_err)?;
                Ok(json!({ "success": true, "action": "replace" }))
            }
            "remove" => {
                let text = params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'remove' requires 'text'".to_string()))?;
                self.memory.remove_entry(text).map_err(map_err)?;
                Ok(json!({ "success": true, "action": "remove" }))
            }
            "list" => {
                let entries: Vec<String> =
                    self.memory.get_entries().into_iter().map(|e| e.content).collect();
                let (used, limit) = self.memory.memory_usage();
                Ok(json!({ "entries": entries, "chars_used": used, "chars_limit": limit }))
            }
            other => Err(ToolError::InvalidParams(format!("Unknown action: {}", other))),
        }
    }
}

/// Skill 管理工具 — Agent 自主创建/查询/修改/删除动态 Skill。
///
/// `create` 会经过 SkillManager 的审批流程 (require_approval=true 时进入 pending,
/// 需用户审批后才激活)。审批本身仍由用户掌控,不暴露给 Agent。
pub struct SkillManageTool {
    skills: Arc<SkillManager>,
}

impl SkillManageTool {
    pub fn new(skills: Arc<SkillManager>) -> Self {
        Self { skills }
    }
}

fn parse_language(s: &str) -> Result<SkillLanguage, ToolError> {
    match s.to_lowercase().as_str() {
        "rust" => Ok(SkillLanguage::Rust),
        "python" | "py" => Ok(SkillLanguage::Python),
        "shell" | "bash" | "sh" => Ok(SkillLanguage::Shell),
        "javascript" | "js" => Ok(SkillLanguage::JavaScript),
        other => Err(ToolError::InvalidParams(format!("Unsupported language: {}", other))),
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn description(&self) -> &str {
        "Create/list/patch/delete reusable Skills. New skills go through the user approval workflow before becoming active."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_manage".to_string(),
            description: "Manage dynamic Skills. 'create' a new skill (submitted for approval), 'list' existing skills, 'patch' a skill by id, 'delete' a skill by id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["create", "list", "patch", "delete"],
                        "description": "Operation to perform"
                    },
                    "name": { "type": "string", "description": "For 'create': skill name" },
                    "description": { "type": "string", "description": "For 'create': what the skill does" },
                    "language": {
                        "type": "string",
                        "enum": ["python", "shell", "javascript", "rust"],
                        "description": "For 'create': implementation language"
                    },
                    "source_code": { "type": "string", "description": "For 'create': the skill source code" },
                    "parameters": { "type": "object", "description": "For 'create': JSON-schema of the skill's parameters" },
                    "id": { "type": "string", "description": "For 'patch'/'delete': the skill id" },
                    "patch": { "type": "object", "description": "For 'patch': fields to update" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParams("Missing 'action' parameter".to_string()))?;

        let map_err = |e: rustclaw_skill::SkillError| ToolError::ExecutionFailed(e.to_string());

        match action {
            "create" => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'create' requires 'name'".to_string()))?;
                let description = params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'create' requires 'description'".to_string()))?;
                let language = parse_language(
                    params
                        .get("language")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| ToolError::InvalidParams("'create' requires 'language'".to_string()))?,
                )?;
                let source_code = params
                    .get("source_code")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'create' requires 'source_code'".to_string()))?;
                let parameters = params.get("parameters").cloned().unwrap_or_else(|| json!({}));

                let definition = self
                    .skills
                    .create_skill(name, description, language, source_code, parameters)
                    .map_err(map_err)?;
                let id = self.skills.submit_skill(definition).await.map_err(map_err)?;
                Ok(json!({
                    "success": true,
                    "action": "create",
                    "skill_id": id,
                    "note": "Skill submitted; awaits user approval before activation."
                }))
            }
            "list" => {
                let active: Vec<Value> = self
                    .skills
                    .get_active_skills()
                    .into_iter()
                    .map(|r| json!({ "id": r.definition.id, "name": r.definition.name, "status": "active" }))
                    .collect();
                let pending: Vec<Value> = self
                    .skills
                    .get_pending_skills()
                    .into_iter()
                    .map(|r| json!({ "id": r.definition.id, "name": r.definition.name, "status": "pending" }))
                    .collect();
                Ok(json!({ "active": active, "pending": pending }))
            }
            "patch" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'patch' requires 'id'".to_string()))?;
                let patch = params
                    .get("patch")
                    .ok_or_else(|| ToolError::InvalidParams("'patch' requires 'patch' object".to_string()))?;
                self.skills.patch_skill(id, patch).await.map_err(map_err)?;
                Ok(json!({ "success": true, "action": "patch", "skill_id": id }))
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::InvalidParams("'delete' requires 'id'".to_string()))?;
                self.skills.delete_skill(id).await.map_err(map_err)?;
                Ok(json!({ "success": true, "action": "delete", "skill_id": id }))
            }
            other => Err(ToolError::InvalidParams(format!("Unknown action: {}", other))),
        }
    }
}

/// G10: spawn_subagent 工具名 (agent 内核注入 `_parent_session_key` 时比对用)。
pub const SUBAGENT_TOOL_NAME: &str = "spawn_subagent";

/// G10: 子 Agent 派生工具 — 把独立子任务交给受限预算的子会话执行。
///
/// - 持 `Weak<AgentRuntime>` 防循环引用 (runtime 拥有注册表,注册表拥有工具)。
/// - 并发上限 (含嵌套派生) 由 `active` 计数约束,防递归爆炸。
/// - 子会话 `sub:<uuid>`,谱系指回父会话;工具结果为子 Agent 最终文本。
pub struct SubAgentTool {
    runtime: std::sync::Weak<crate::AgentRuntime>,
    active: Arc<std::sync::atomic::AtomicUsize>,
}

/// 同时在跑的子 Agent 上限 (含嵌套)。
const MAX_ACTIVE_SUBAGENTS: usize = 4;
/// 单个子 Agent 迭代预算上限。
const MAX_SUBAGENT_BUDGET: u32 = 10;

impl SubAgentTool {
    pub fn new(runtime: std::sync::Weak<crate::AgentRuntime>) -> Self {
        Self {
            runtime,
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &str {
        SUBAGENT_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Delegate an independent subtask to a budget-limited sub-agent. It runs its own tool loop in a child session and returns its final answer."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: SUBAGENT_TOOL_NAME.to_string(),
            description: "Spawn a sub-agent for an independent subtask (own session, limited iterations). Use for parallelizable or exploratory work; give a complete, self-contained task description.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Complete, self-contained task description for the sub-agent"
                    },
                    "max_iterations": {
                        "type": "integer",
                        "description": "Iteration budget (default 5, max 10)"
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, params: Value) -> Result<Value, ToolError> {
        use std::sync::atomic::Ordering;

        let task = params
            .get("task")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::InvalidParams("'task' is required".to_string()))?;
        let budget = params
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(5)
            .clamp(1, MAX_SUBAGENT_BUDGET);
        // 由 agent 内核注入,不来自模型
        let parent = params
            .get("_parent_session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| ToolError::ExecutionFailed("agent runtime dropped".to_string()))?;

        // 并发/嵌套上限 (fetch_add 后检查,失败即回退)
        let prev = self.active.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_ACTIVE_SUBAGENTS {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return Err(ToolError::ExecutionFailed(format!(
                "too many active sub-agents (max {})",
                MAX_ACTIVE_SUBAGENTS
            )));
        }

        let result = runtime.process_subtask(task, &parent, budget).await;
        self.active.fetch_sub(1, Ordering::SeqCst);

        match result {
            Ok((child_key, text)) => Ok(json!({
                "success": true,
                "session_key": child_key,
                "result": text,
            })),
            Err(e) => Err(ToolError::ExecutionFailed(format!("sub-agent failed: {}", e))),
        }
    }
}
