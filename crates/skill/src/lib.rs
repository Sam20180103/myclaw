pub mod skill_tool;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub language: SkillLanguage,
    pub source_code: String,
    pub parameters: serde_json::Value,
    pub created_at: i64,
    pub author: Option<String>,
    pub test_cases: Vec<SkillTestCase>,
    pub category: Option<String>,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTestCase {
    pub input: serde_json::Value,
    pub expected_output: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillLanguage {
    Rust,
    Python,
    Shell,
    JavaScript,
}

impl std::fmt::Display for SkillLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillLanguage::Rust => write!(f, "rust"),
            SkillLanguage::Python => write!(f, "python"),
            SkillLanguage::Shell => write!(f, "shell"),
            SkillLanguage::JavaScript => write!(f, "javascript"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SkillStatus {
    Pending,
    Approved,
    Rejected,
    Active,
    Disabled,
}

#[derive(Debug, Clone)]
pub struct SkillRecord {
    pub definition: SkillDefinition,
    pub status: SkillStatus,
    pub path: PathBuf,
    pub created_at: Instant,
    pub approved_at: Option<i64>,
    pub approved_by: Option<String>,
}

pub struct SkillManager {
    config: SkillConfig,
    pending_skills: Arc<RwLock<HashMap<String, SkillRecord>>>,
    active_skills: Arc<RwLock<HashMap<String, SkillRecord>>>,
    approval_tx: Option<mpsc::Sender<SkillApprovalRequest>>,
    /// X-3: 工具注册表 —— 已批准的 Shell/Python 技能注册进去后才真正可被 Agent 调用。
    /// 用 RwLock 是因为注册表要等 `AgentRuntime` 建好之后才能注入。
    tool_registry: RwLock<Option<Arc<rustclaw_tool::ToolRegistry>>>,
}

#[derive(Clone)]
pub struct SkillConfig {
    pub skills_dir: PathBuf,
    pub pending_dir: PathBuf,
    pub active_dir: PathBuf,
    pub require_approval: bool,
    pub allowed_languages: Vec<SkillLanguage>,
}

impl Default for SkillConfig {
    fn default() -> Self {
        // G7 修复: 走 Config 的访问器,才带 profile 维度,且技能目录布局只有一处定义
        // (此前这里自行拼 pending/active,与 config 侧的 helper 各说各话)。
        use rustclaw_config::Config;

        Self {
            pending_dir: Config::pending_skills_dir(),
            active_dir: Config::active_skills_dir(),
            skills_dir: Config::skills_dir(),
            require_approval: true,
            allowed_languages: vec![
                SkillLanguage::Python,
                SkillLanguage::Shell,
                SkillLanguage::JavaScript,
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillApprovalRequest {
    pub skill_id: String,
    pub skill_name: String,
    pub skill_definition: SkillDefinition,
    pub requested_at: i64,
}

pub struct SkillApprovalResponse {
    pub skill_id: String,
    pub approved: bool,
    pub approver: String,
    pub reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("Skill not found: {0}")]
    NotFound(String),
    #[error("Skill already exists: {0}")]
    AlreadyExists(String),
    #[error("Invalid skill definition: {0}")]
    InvalidDefinition(String),
    #[error("Language not allowed: {0}")]
    LanguageNotAllowed(String),
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Skill is pending approval")]
    PendingApproval,
    #[error("Skill execution failed: {0}")]
    ExecutionFailed(String),
}

impl SkillManager {
    pub fn new(config: SkillConfig) -> Self {
        Self {
            config: config.clone(),
            pending_skills: Arc::new(RwLock::new(HashMap::with_capacity(8))),
            active_skills: Arc::new(RwLock::new(HashMap::with_capacity(32))),
            tool_registry: RwLock::new(None),
            approval_tx: None,
        }
    }

    pub fn with_approval_channel(mut self, tx: mpsc::Sender<SkillApprovalRequest>) -> Self {
        self.approval_tx = Some(tx);
        self
    }

    pub async fn init(&self) -> Result<(), SkillError> {
        tokio::fs::create_dir_all(&self.config.skills_dir)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        tokio::fs::create_dir_all(&self.config.pending_dir)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        tokio::fs::create_dir_all(&self.config.active_dir)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        
        self.load_existing_skills().await?;

        // X-3: 重启后把盘上的已批准技能重新注册为可调用工具
        let registered = self.register_active_skill_tools();
        if registered > 0 {
            info!("{} active skill(s) registered as callable tools", registered);
        }

        Ok(())
    }

    /// X-3: 注入工具注册表。必须在 `AgentRuntime` 构造完成后调用 (注册表由它持有)。
    pub fn set_tool_registry(&self, registry: Arc<rustclaw_tool::ToolRegistry>) {
        *self.tool_registry.write() = Some(registry);
    }

    /// X-3: 把一个已批准技能注册为可调用工具。返回工具名;语言不受支持则返回 None。
    fn register_skill_tool(&self, definition: &SkillDefinition) -> Option<String> {
        use rustclaw_tool::Tool as _;
        let registry = self.tool_registry.read().clone()?;
        let tool = skill_tool::SkillTool::from_definition(definition)?;
        let name = tool.name().to_string();
        registry.register(Arc::new(tool));
        info!("skill '{}' registered as tool '{}'", definition.name, name);
        Some(name)
    }

    /// X-3: 撤销注册 (删除技能时调用),避免工具表里留下指向已删技能的僵尸条目。
    fn unregister_skill_tool(&self, definition: &SkillDefinition) {
        if let Some(registry) = self.tool_registry.read().clone() {
            registry.unregister(&skill_tool::tool_name_for(&definition.name, &definition.id));
        }
    }

    /// X-3: 批量注册当前所有已批准技能,返回成功注册的数量。
    fn register_active_skill_tools(&self) -> usize {
        let defs: Vec<SkillDefinition> = self
            .active_skills
            .read()
            .values()
            .map(|r| r.definition.clone())
            .collect();
        defs.iter()
            .filter(|d| self.register_skill_tool(d).is_some())
            .count()
    }

    async fn load_existing_skills(&self) -> Result<(), SkillError> {
        let pending = self.load_skills_from_dir(&self.config.pending_dir, SkillStatus::Pending).await?;
        let active = self.load_skills_from_dir(&self.config.active_dir, SkillStatus::Active).await?;
        
        for (id, record) in pending {
            self.pending_skills.write().insert(id, record);
        }
        
        for (id, record) in active {
            self.active_skills.write().insert(id, record);
        }
        
        Ok(())
    }

async fn load_skills_from_dir(
    &self,
    dir: &Path,
    status: SkillStatus,
) -> Result<HashMap<String, SkillRecord>, SkillError> {
    let mut skills = HashMap::with_capacity(16);

    if !dir.exists() {
        return Ok(skills);
    }

        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| SkillError::IoError(e.to_string()))? {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    if let Ok(definition) = serde_json::from_str::<SkillDefinition>(&content) {
                        let record = SkillRecord {
                            definition: definition.clone(),
                            status: status.clone(),
                            path: path.clone(),
                            created_at: Instant::now(),
                            approved_at: if status == SkillStatus::Active {
                                Some(chrono::Utc::now().timestamp_millis())
                            } else {
                                None
                            },
                            approved_by: None,
                        };
                        skills.insert(definition.id.clone(), record);
                    }
                }
            }
        }

        Ok(skills)
    }

    pub fn create_skill(
        &self,
        name: &str,
        description: &str,
        language: SkillLanguage,
        source_code: &str,
        parameters: serde_json::Value,
    ) -> Result<SkillDefinition, SkillError> {
        if !self.config.allowed_languages.contains(&language) {
            return Err(SkillError::LanguageNotAllowed(language.to_string()));
        }

        let definition = SkillDefinition {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: description.to_string(),
            version: "1.0.0".to_string(),
            language,
            source_code: source_code.to_string(),
            parameters,
            created_at: chrono::Utc::now().timestamp_millis(),
            author: None,
            test_cases: vec![],
            category: None,
            references: vec![],
        };

        self.validate_skill(&definition)?;
        
        Ok(definition)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_skill_with_category(
        &self,
        name: &str,
        description: &str,
        language: SkillLanguage,
        source_code: &str,
        parameters: serde_json::Value,
        category: &str,
        references: Vec<String>,
    ) -> Result<SkillDefinition, SkillError> {
        if !self.config.allowed_languages.contains(&language) {
            return Err(SkillError::LanguageNotAllowed(language.to_string()));
        }

        let definition = SkillDefinition {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: description.to_string(),
            version: "1.0.0".to_string(),
            language,
            source_code: source_code.to_string(),
            parameters,
            created_at: chrono::Utc::now().timestamp_millis(),
            author: None,
            test_cases: vec![],
            category: Some(category.to_string()),
            references,
        };

        self.validate_skill(&definition)?;
        
        Ok(definition)
    }

    pub async fn create_skill_directory(&self, definition: &SkillDefinition) -> Result<PathBuf, SkillError> {
        let category = definition.category.as_deref().unwrap_or("general");
        let skill_dir = self.config.active_dir.join(category).join(&definition.name);
        
        tokio::fs::create_dir_all(&skill_dir)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        
        tokio::fs::create_dir_all(skill_dir.join("references"))
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        
        tokio::fs::create_dir_all(skill_dir.join("templates"))
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;
        
        tokio::fs::create_dir_all(skill_dir.join("scripts"))
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        let skill_md = format!(
            "# {}\n\n{}\n\n- **Version**: {}\n- **Language**: {}\n- **Category**: {}\n- **Author**: {}\n\n## Usage\n\n{}\n\n## References\n\n{}\n",
            definition.name,
            definition.description,
            definition.version,
            definition.language,
            definition.category.as_deref().unwrap_or("general"),
            definition.author.as_deref().unwrap_or("unknown"),
            definition.source_code,
            definition.references.iter().map(|r| format!("- {}", r)).collect::<Vec<_>>().join("\n"),
        );

        tokio::fs::write(skill_dir.join("SKILL.md"), skill_md)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        Ok(skill_dir)
    }

    pub fn get_skills_by_category(&self) -> HashMap<String, Vec<SkillRecord>> {
        let mut categories: HashMap<String, Vec<SkillRecord>> = HashMap::new();
        for record in self.active_skills.read().values() {
            let category = record.definition.category.clone().unwrap_or_else(|| "general".to_string());
            categories.entry(category).or_default().push(record.clone());
        }
        categories
    }

    pub async fn submit_skill(&self, definition: SkillDefinition) -> Result<String, SkillError> {
        if self.config.require_approval {
            self.submit_for_approval(&definition).await
        } else {
            self.activate_skill(definition).await
        }
    }

    async fn submit_for_approval(&self, definition: &SkillDefinition) -> Result<String, SkillError> {
        let path = self.config.pending_dir.join(format!("{}.json", definition.id));
        let content = serde_json::to_string_pretty(definition)
            .map_err(|e| SkillError::SerializationError(e.to_string()))?;
        
        tokio::fs::write(&path, &content)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        let record = SkillRecord {
            definition: definition.clone(),
            status: SkillStatus::Pending,
            path,
            created_at: Instant::now(),
            approved_at: None,
            approved_by: None,
        };

        self.pending_skills.write().insert(definition.id.clone(), record);

        if let Some(ref tx) = self.approval_tx {
            let request = SkillApprovalRequest {
                skill_id: definition.id.clone(),
                skill_name: definition.name.clone(),
                skill_definition: definition.clone(),
                requested_at: chrono::Utc::now().timestamp_millis(),
            };
            let _ = tx.send(request).await;
        }

        info!("Skill {} submitted for approval", definition.id);
        Ok(definition.id.clone())
    }

    pub async fn approve_skill(
        &self,
        skill_id: &str,
        approver: &str,
    ) -> Result<(), SkillError> {
        let record = self.pending_skills.write().remove(skill_id)
            .ok_or_else(|| SkillError::NotFound(skill_id.to_string()))?;

        let pending_path = record.path.clone();
        let active_path = self.config.active_dir.join(format!("{}.json", skill_id));

        tokio::fs::rename(&pending_path, &active_path)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        let mut approved_record = record;
        approved_record.status = SkillStatus::Active;
        approved_record.path = active_path;
        approved_record.approved_at = Some(chrono::Utc::now().timestamp_millis());
        approved_record.approved_by = Some(approver.to_string());

        let definition = approved_record.definition.clone();
        self.active_skills.write().insert(skill_id.to_string(), approved_record);

        // X-3: 批准即上线 —— 此前批准只是改了状态,技能永远调不起来
        match self.register_skill_tool(&definition) {
            Some(tool) => info!("Skill {} approved by {} → tool '{}'", skill_id, approver, tool),
            None => info!(
                "Skill {} approved by {} (language {} is catalog-only, not callable)",
                skill_id, approver, definition.language
            ),
        }
        Ok(())
    }

    pub async fn reject_skill(
        &self,
        skill_id: &str,
        _approver: &str,
        reason: Option<&str>,
    ) -> Result<(), SkillError> {
        let record = self.pending_skills.write().remove(skill_id)
            .ok_or_else(|| SkillError::NotFound(skill_id.to_string()))?;

        tokio::fs::remove_file(&record.path)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        warn!("Skill {} rejected: {:?}", skill_id, reason);
        Ok(())
    }

    /// 直接激活 (仅当 `require_approval = false`)。同样会注册为可调用工具。
    async fn activate_skill(&self, definition: SkillDefinition) -> Result<String, SkillError> {
        let path = self.config.active_dir.join(format!("{}.json", definition.id));
        let content = serde_json::to_string_pretty(&definition)
            .map_err(|e| SkillError::SerializationError(e.to_string()))?;
        
        tokio::fs::write(&path, &content)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        let record = SkillRecord {
            definition: definition.clone(),
            status: SkillStatus::Active,
            path,
            created_at: Instant::now(),
            approved_at: Some(chrono::Utc::now().timestamp_millis()),
            approved_by: None,
        };

        let id = definition.id.clone();
        self.active_skills.write().insert(id.clone(), record);
        self.register_skill_tool(&definition);

        info!("Skill {} activated directly", id);
        Ok(id)
    }

fn validate_skill(&self, definition: &SkillDefinition) -> Result<(), SkillError> {
    if definition.name.is_empty() {
        return Err(SkillError::InvalidDefinition("Name cannot be empty".to_string()));
    }
    if definition.name.len() > 64 {
        return Err(SkillError::InvalidDefinition("Name cannot exceed 64 characters".to_string()));
    }
    if !definition.name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err(SkillError::InvalidDefinition("Name can only contain alphanumeric, underscore, and hyphen characters".to_string()));
    }
    if definition.source_code.is_empty() {
        return Err(SkillError::InvalidDefinition("Source code cannot be empty".to_string()));
    }
    if definition.source_code.len() > 1024 * 1024 {
        return Err(SkillError::InvalidDefinition("Source code cannot exceed 1MB".to_string()));
    }
    if definition.description.len() > 512 {
        return Err(SkillError::InvalidDefinition("Description cannot exceed 512 characters".to_string()));
    }
    if !self.config.allowed_languages.contains(&definition.language) {
        return Err(SkillError::LanguageNotAllowed(definition.language.to_string()));
    }

    match &definition.parameters {
        serde_json::Value::Object(_) => {}
        _ => return Err(SkillError::InvalidDefinition("Parameters must be a JSON object".to_string())),
    }

    self.validate_language_syntax(&definition.language, &definition.source_code)?;

    Ok(())
}

fn validate_language_syntax(&self, language: &SkillLanguage, source_code: &str) -> Result<(), SkillError> {
    match language {
        SkillLanguage::Shell => {
            let dangerous_patterns = ["rm -rf /", "mkfs", ":(){:|:&};:", "> /dev/sd"];
            for pattern in dangerous_patterns {
                if source_code.contains(pattern) {
                    return Err(SkillError::ValidationFailed(format!("Dangerous pattern detected: {}", pattern)));
                }
            }
        }
        SkillLanguage::Python => {
            let dangerous_imports = ["import os", "import subprocess", "import sys", "__import__"];
            for imp in dangerous_imports {
                if source_code.contains(imp) {
                    return Err(SkillError::ValidationFailed(format!("Potentially dangerous import: {}", imp)));
                }
            }
        }
        SkillLanguage::JavaScript => {
            let dangerous_patterns = ["eval(", "Function(", "require(", "process.exit"];
            for pattern in dangerous_patterns {
                if source_code.contains(pattern) {
                    return Err(SkillError::ValidationFailed(format!("Dangerous pattern detected: {}", pattern)));
                }
            }
        }
        SkillLanguage::Rust => {}
    }
    Ok(())
}

    pub fn get_pending_skills(&self) -> Vec<SkillRecord> {
        self.pending_skills.read().values().cloned().collect()
    }

    pub fn get_active_skills(&self) -> Vec<SkillRecord> {
        self.active_skills.read().values().cloned().collect()
    }

    pub fn get_skill(&self, id: &str) -> Option<SkillRecord> {
        self.pending_skills.read().get(id).cloned()
            .or_else(|| self.active_skills.read().get(id).cloned())
    }

    pub async fn delete_skill(&self, id: &str) -> Result<(), SkillError> {
        let record = {
            if let Some(record) = self.active_skills.write().remove(id) {
                Some(record)
            } else {
                self.pending_skills.write().remove(id)
            }
        };

        if let Some(record) = record {
            tokio::fs::remove_file(&record.path)
                .await
                .map_err(|e| SkillError::IoError(e.to_string()))?;
            // X-3: 同步摘掉工具,否则删掉的技能还能被调用
            self.unregister_skill_tool(&record.definition);
            return Ok(());
        }

        Err(SkillError::NotFound(id.to_string()))
    }

    pub async fn patch_skill(&self, id: &str, patch: &serde_json::Value) -> Result<(), SkillError> {
        let (path, def) = {
            let mut active = self.active_skills.write();
            if let Some(record) = active.get_mut(id) {
                let mut def = record.definition.clone();
                if let Some(name) = patch.get("name").and_then(|v| v.as_str()) {
                    def.name = name.to_string();
                }
                if let Some(desc) = patch.get("description").and_then(|v| v.as_str()) {
                    def.description = desc.to_string();
                }
                if let Some(code) = patch.get("source_code").and_then(|v| v.as_str()) {
                    def.source_code = code.to_string();
                }
                if let Some(params) = patch.get("parameters") {
                    def.parameters = params.clone();
                }
                self.validate_skill(&def)?;
                let path = record.path.clone();
                (path, def)
            } else {
                drop(active);
                let mut pending = self.pending_skills.write();
                if let Some(record) = pending.get_mut(id) {
                    let mut def = record.definition.clone();
                    if let Some(name) = patch.get("name").and_then(|v| v.as_str()) {
                        def.name = name.to_string();
                    }
                    if let Some(desc) = patch.get("description").and_then(|v| v.as_str()) {
                        def.description = desc.to_string();
                    }
                    if let Some(code) = patch.get("source_code").and_then(|v| v.as_str()) {
                        def.source_code = code.to_string();
                    }
                    if let Some(params) = patch.get("parameters") {
                        def.parameters = params.clone();
                    }
                    self.validate_skill(&def)?;
                    let path = record.path.clone();
                    (path, def)
                } else {
                    return Err(SkillError::NotFound(id.to_string()));
                }
            }
        };

        let content = serde_json::to_string_pretty(&def)
            .map_err(|e| SkillError::SerializationError(e.to_string()))?;
        tokio::fs::write(&path, &content)
            .await
            .map_err(|e| SkillError::IoError(e.to_string()))?;

        {
            let mut active = self.active_skills.write();
            if let Some(record) = active.get_mut(id) {
                record.definition = def;
                return Ok(());
            }
        }
        {
            let mut pending = self.pending_skills.write();
            if let Some(record) = pending.get_mut(id) {
                record.definition = def;
                return Ok(());
            }
        }

        Err(SkillError::NotFound(id.to_string()))
    }

    pub async fn hot_reload(&self) -> Result<HotReloadResult, SkillError> {
        let before_count = self.active_skills.read().len();
        
        self.pending_skills.write().clear();
        self.active_skills.write().clear();
        
        self.load_existing_skills().await?;
        
        let after_count = self.active_skills.read().len();
        
        info!(
            "Hot reload completed: {} -> {} skills",
            before_count,
            after_count
        );
        
        Ok(HotReloadResult {
            loaded: after_count,
            previous: before_count,
        })
    }

    pub fn stats(&self) -> SkillStats {
        SkillStats {
            pending: self.pending_skills.read().len(),
            active: self.active_skills.read().len(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HotReloadResult {
    pub loaded: usize,
    pub previous: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillStats {
    pub pending: usize,
    pub active: usize,
}

pub struct OpenCodeGenerator {
    opencode_path: PathBuf,
    timeout_seconds: u64,
}

impl OpenCodeGenerator {
    pub fn new(opencode_path: Option<PathBuf>) -> Self {
        let opencode_path = opencode_path.unwrap_or_else(|| {
            which::which("opencode")
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| PathBuf::from("opencode"))
        });
        
        Self {
            opencode_path,
            timeout_seconds: 120,
        }
    }

    pub async fn generate_skill(
        &self,
        description: &str,
        language: SkillLanguage,
        parameters: serde_json::Value,
    ) -> Result<SkillDefinition, SkillError> {
        let prompt = format!(
            "Generate a {} skill that: {}\n\
            The skill should be a complete, working implementation.\n\
            Return ONLY the code, no explanations.\n\
            Parameters schema: {}",
            language,
            description,
            parameters
        );

        let timeout = std::time::Duration::from_secs(self.timeout_seconds);
        
        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new(&self.opencode_path)
                .arg("generate")
                .arg("--language")
                .arg(language.to_string())
                .arg("--prompt")
                .arg(&prompt)
                .output()
        )
        .await
        .map_err(|_| SkillError::ExecutionFailed("OpenCode timeout".to_string()))?
        .map_err(|e| SkillError::ExecutionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SkillError::ExecutionFailed(format!(
                "OpenCode failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        let skill_name = format!("generated_skill_{}", &Uuid::new_v4().to_string()[..8]);

        Ok(SkillDefinition {
            id: Uuid::new_v4().to_string(),
            name: skill_name,
            description: description.to_string(),
            version: "1.0.0".to_string(),
            language,
            source_code: stdout.to_string(),
            parameters,
            created_at: chrono::Utc::now().timestamp_millis(),
            author: Some("opencode".to_string()),
            test_cases: vec![],
            category: None,
            references: vec![],
        })
    }

    pub fn is_available(&self) -> bool {
        std::path::Path::new(&self.opencode_path).exists()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillGenerateRequest {
    pub description: String,
    pub language: SkillLanguage,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillGenerateResponse {
    pub success: bool,
    pub skill: Option<SkillDefinition>,
    pub error: Option<String>,
}

pub mod skill_manage;
pub use skill_manage::*;

#[cfg(test)]
mod tests;
