use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::{SkillManager, SkillLanguage};

#[async_trait]
pub trait SkillManageToolTrait: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    async fn execute(&self, params: Value) -> Result<Value, String>;
}

pub struct SkillManageTool {
    skill_manager: Arc<SkillManager>,
}

impl SkillManageTool {
    pub fn new(skill_manager: Arc<SkillManager>) -> Self {
        Self { skill_manager }
    }

    pub fn name() -> &'static str {
        "skill_manage"
    }

    pub fn description() -> &'static str {
        "Manage skills: create, delete, list. Use this to create new skills from complex tasks."
    }

    pub fn schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "delete", "list"],
                    "description": "Action to perform"
                },
                "name": { "type": "string", "description": "Skill name" },
                "description": { "type": "string", "description": "Skill description" },
                "language": {
                    "type": "string",
                    "enum": ["python", "shell", "javascript", "rust"],
                    "description": "Programming language"
                },
                "source_code": { "type": "string", "description": "Skill source code" },
                "parameters": { "type": "object", "description": "Skill parameters" },
                "skill_id": { "type": "string", "description": "Skill ID" }
            },
            "required": ["action"]
        })
    }

    pub async fn execute(&self, params: Value) -> Result<Value, String> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'action' parameter".to_string())?;

        match action {
            "create" => self.create_skill(&params).await,
            "list" => self.list_skills(),
            "delete" => self.delete_skill(&params).await,
            _ => Err(format!("Unknown action: {}", action)),
        }
    }

    async fn create_skill(&self, params: &Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'name' parameter".to_string())?;

        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'description' parameter".to_string())?;

        let language_str = params
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'language' parameter".to_string())?;

        let source_code = params
            .get("source_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'source_code' parameter".to_string())?;

        let language = match language_str {
            "python" => SkillLanguage::Python,
            "shell" => SkillLanguage::Shell,
            "javascript" => SkillLanguage::JavaScript,
            "rust" => SkillLanguage::Rust,
            _ => return Err(format!("Unknown language: {}", language_str)),
        };

        let parameters = params.get("parameters").cloned().unwrap_or(serde_json::json!({}));

        let definition = self.skill_manager
            .create_skill(name, description, language, source_code, parameters)
            .map_err(|e| e.to_string())?;

        let skill_id = self.skill_manager
            .submit_skill(definition)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "success": true,
            "skill_id": skill_id,
            "message": "Skill created and submitted"
        }))
    }

    fn list_skills(&self) -> Result<Value, String> {
        let active = self.skill_manager.get_active_skills();
        let pending = self.skill_manager.get_pending_skills();

        Ok(serde_json::json!({
            "success": true,
            "active": active.iter().map(|s| serde_json::json!({
                "id": s.definition.id,
                "name": s.definition.name,
                "language": s.definition.language.to_string(),
                "description": s.definition.description,
            })).collect::<Vec<_>>(),
            "pending": pending.iter().map(|s| serde_json::json!({
                "id": s.definition.id,
                "name": s.definition.name,
                "language": s.definition.language.to_string(),
            })).collect::<Vec<_>>(),
        }))
    }

    async fn delete_skill(&self, params: &Value) -> Result<Value, String> {
        let skill_id = params
            .get("skill_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'skill_id' parameter".to_string())?;

        self.skill_manager
            .delete_skill(skill_id)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({
            "success": true,
            "message": format!("Skill {} deleted", skill_id)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillConfig, SkillManager};
    use std::path::PathBuf;

    #[test]
    fn test_skill_manage_tool_schema() {
        let schema = SkillManageTool::schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
    }

    #[test]
    fn test_skill_manage_tool_name() {
        assert_eq!(SkillManageTool::name(), "skill_manage");
    }

    #[tokio::test]
    async fn test_skill_manage_tool_list_empty() {
        let temp_dir = std::env::temp_dir().join(format!("rustclaw_skill_test_{}", uuid::Uuid::new_v4()));
        let config = SkillConfig {
            skills_dir: temp_dir.clone(),
            pending_dir: temp_dir.join("pending"),
            active_dir: temp_dir.join("active"),
            require_approval: false,
            allowed_languages: vec![SkillLanguage::Python, SkillLanguage::Shell],
        };
        let manager = Arc::new(SkillManager::new(config));
        manager.init().await.unwrap();
        let tool = SkillManageTool::new(manager);
        let result = tool.list_skills().unwrap();
        assert_eq!(result["active"].as_array().unwrap().len(), 0);
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
