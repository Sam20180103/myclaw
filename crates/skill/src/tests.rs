#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::{SkillManager, SkillConfig, SkillLanguage, SkillDefinition};
    use tempfile::TempDir;

    fn create_test_config() -> (SkillConfig, TempDir) {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let skills_dir = temp_dir.path().to_path_buf();
        
        let config = SkillConfig {
            skills_dir: skills_dir.clone(),
            pending_dir: skills_dir.join("pending"),
            active_dir: skills_dir.join("active"),
            require_approval: true,
            allowed_languages: vec![SkillLanguage::Python, SkillLanguage::Shell],
        };
        
        (config, temp_dir)
    }

    #[tokio::test]
    async fn test_skill_manager_new() {
        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        assert_eq!(manager.get_pending_skills().len(), 0);
        assert_eq!(manager.get_active_skills().len(), 0);
    }

    #[tokio::test]
    async fn test_skill_manager_init() {
        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        
        let result = manager.init().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_skill() {
        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        
        let result = manager.create_skill(
            "test_skill",
            "A test skill",
            SkillLanguage::Python,
            "print('hello')",
            serde_json::json!({"param": "value"}),
        );
        
        assert!(result.is_ok());
        let skill = result.unwrap();
        assert_eq!(skill.name, "test_skill");
        assert_eq!(skill.language, SkillLanguage::Python);
    }

    #[test]
    fn test_skill_language_not_allowed() {
        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        
        let result = manager.create_skill(
            "test_skill",
            "A test skill",
            SkillLanguage::Rust,
            "fn main() {}",
            serde_json::json!({}),
        );
        
        assert!(result.is_err());
    }

    #[test]
    fn test_skill_stats() {
        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        
        let stats = manager.stats();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.active, 0);
    }

    #[test]
    fn test_skill_definition_serialization() {
        let skill = SkillDefinition {
            id: "test-id".to_string(),
            name: "test".to_string(),
            description: "test desc".to_string(),
            version: "1.0.0".to_string(),
            language: SkillLanguage::Python,
            source_code: "print(1)".to_string(),
            parameters: serde_json::json!({}),
            created_at: 0,
            author: None,
            test_cases: vec![],
            category: None,
            references: vec![],
        };
        
        let json = serde_json::to_string(&skill).unwrap();
        let parsed: SkillDefinition = serde_json::from_str(&json).unwrap();
        
        assert_eq!(parsed.id, skill.id);
        assert_eq!(parsed.name, skill.name);
    }

#[test]
fn test_skill_language_display() {
    assert_eq!(format!("{}", SkillLanguage::Python), "python");
    assert_eq!(format!("{}", SkillLanguage::Shell), "shell");
    assert_eq!(format!("{}", SkillLanguage::JavaScript), "javascript");
    assert_eq!(format!("{}", SkillLanguage::Rust), "rust");
}

#[test]
fn test_skill_name_validation() {
    let (config, _temp) = create_test_config();
    let manager = SkillManager::new(config);

    let result = manager.create_skill(
        "",
        "A test skill",
        SkillLanguage::Python,
        "print('hello')",
        serde_json::json!({}),
    );
    assert!(result.is_err());

    let long_name = "a".repeat(65);
    let result = manager.create_skill(
        &long_name,
        "A test skill",
        SkillLanguage::Python,
        "print('hello')",
        serde_json::json!({}),
    );
    assert!(result.is_err());

    let result = manager.create_skill(
        "test@skill!",
        "A test skill",
        SkillLanguage::Python,
        "print('hello')",
        serde_json::json!({}),
    );
    assert!(result.is_err());
}

#[test]
fn test_skill_dangerous_pattern_validation() {
    let (config, _temp) = create_test_config();
    let manager = SkillManager::new(config);

    let result = manager.create_skill(
        "dangerous_skill",
        "A dangerous skill",
        SkillLanguage::Shell,
        "rm -rf /",
        serde_json::json!({}),
    );
    assert!(result.is_err());

    let result = manager.create_skill(
        "dangerous_python",
        "A dangerous skill",
        SkillLanguage::Python,
        "import os; os.system('ls')",
        serde_json::json!({}),
    );
    assert!(result.is_err());

    let result = manager.create_skill(
        "dangerous_js",
        "A dangerous skill",
        SkillLanguage::JavaScript,
        "eval('malicious code')",
        serde_json::json!({}),
    );
    assert!(result.is_err());
}
}
