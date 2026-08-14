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


    /// 技能目录布局只能有一处定义: `SkillConfig::default()` 必须与 `Config` 的访问器一致。
    /// 此前 skill 侧自行拼 `skills/active`,config 侧连 active 访问器都没有 —— 文档因此
    /// 把"已批准技能"写成落在 `skills/` 根下。
    #[test]
    fn test_skill_config_default_matches_config_helpers() {
        use rustclaw_config::Config;
        let c = SkillConfig::default();
        assert_eq!(c.skills_dir, Config::skills_dir());
        assert_eq!(c.pending_dir, Config::pending_skills_dir());
        assert_eq!(c.active_dir, Config::active_skills_dir());
        // pending/active 都在 skills base 之下
        assert!(c.pending_dir.starts_with(&c.skills_dir));
        assert!(c.active_dir.starts_with(&c.skills_dir));
        assert_ne!(c.pending_dir, c.active_dir);
    }


    // --- X-3: 生成 → 批准 → 可调用 的闭环 ---

    /// 批准后技能必须真的出现在 ToolRegistry 里并能执行;删除后必须消失。
    #[tokio::test]
    async fn test_approved_skill_becomes_callable_tool() {
        use rustclaw_tool::{Tool, ToolRegistry};
        use std::sync::Arc;

        let (config, _temp) = create_test_config();
        let manager = SkillManager::new(config);
        let registry = Arc::new(ToolRegistry::new());
        manager.set_tool_registry(registry.clone());
        manager.init().await.unwrap();

        let def = manager
            .create_skill(
                "word_count",
                "统计参数里 text 的字符数",
                SkillLanguage::Python,
                "import json,sys; a=json.loads(sys.argv[1]); print(len(a['text']))",
                serde_json::json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }),
            )
            .unwrap();
        let id = manager.submit_skill(def).await.unwrap();

        // 待批准阶段不得注册
        assert!(registry.get("dyn_skill_word_count").is_none(), "未批准的技能不能可调用");

        manager.approve_skill(&id, "tester").await.unwrap();

        let tool = registry
            .get("dyn_skill_word_count")
            .expect("批准后应注册为工具");
        let out = tool
            .execute(serde_json::json!({ "text": "hello" }))
            .await
            .expect("技能应能执行");
        assert_eq!(out["stdout"].as_str().unwrap().trim(), "5");

        // 删除后工具必须一起摘掉,不能留僵尸
        manager.delete_skill(&id).await.unwrap();
        assert!(registry.get("dyn_skill_word_count").is_none(), "删除后工具必须注销");
    }

    /// 重启场景: 新 manager + init() 必须把盘上的已批准技能重新注册回来。
    #[tokio::test]
    async fn test_active_skills_reregister_on_restart() {
        use rustclaw_tool::ToolRegistry;
        use std::sync::Arc;

        let (config, _temp) = create_test_config();
        let first = SkillManager::new(config.clone());
        first.set_tool_registry(Arc::new(ToolRegistry::new()));
        first.init().await.unwrap();
        let def = first
            .create_skill(
                "ping",
                "回一个 pong",
                SkillLanguage::Shell,
                "echo pong",
                serde_json::json!({ "type": "object" }),
            )
            .unwrap();
        let id = first.submit_skill(def).await.unwrap();
        first.approve_skill(&id, "tester").await.unwrap();

        // 模拟重启: 全新 manager,同一目录
        let registry = Arc::new(ToolRegistry::new());
        let second = SkillManager::new(config);
        second.set_tool_registry(registry.clone());
        second.init().await.unwrap();

        assert_eq!(second.get_active_skills().len(), 1);
        assert!(registry.get("dyn_skill_ping").is_some(), "重启后已批准技能应自动重新上线");
    }

    /// Rust/JS 仍可创建与列出,但不进工具表 (本期只闭环 shell/python)。
    #[tokio::test]
    async fn test_unsupported_language_is_catalog_only() {
        use rustclaw_tool::ToolRegistry;
        use std::sync::Arc;

        let (mut config, _temp) = create_test_config();
        config.allowed_languages = vec![SkillLanguage::Rust, SkillLanguage::Shell];
        let manager = SkillManager::new(config);
        let registry = Arc::new(ToolRegistry::new());
        manager.set_tool_registry(registry.clone());
        manager.init().await.unwrap();

        let def = manager
            .create_skill(
                "rusty",
                "rust skill",
                SkillLanguage::Rust,
                "fn main() { println!(\"hi\"); }",
                serde_json::json!({ "type": "object" }),
            )
            .unwrap();
        let id = manager.submit_skill(def).await.unwrap();
        manager.approve_skill(&id, "tester").await.unwrap();

        assert_eq!(manager.get_active_skills().len(), 1, "仍应被登记");
        assert!(registry.get("dyn_skill_rusty").is_none(), "Rust 技能不可调用");
    }
}
