use rustclaw_agent::{AgentRuntime, AgentConfig};
use rustclaw_skill::{SkillManager, SkillConfig, SkillLanguage};

#[tokio::test]
async fn test_agent_runtime_basic() {
    let agent = AgentRuntime::new(AgentConfig::default());
    
    let sessions = agent.list_sessions();
    assert!(sessions.is_empty());
    
    let lane = "lane_0";
    let session = agent.session_manager().create("test-channel", lane);
    assert!(!session.key.is_empty());
    
    let sessions = agent.list_sessions();
    assert_eq!(sessions.len(), 1);
    
    agent.session_manager().delete(&session.key);
    let sessions = agent.list_sessions();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_agent_lane_routing() {
    let agent = AgentRuntime::new(AgentConfig::default());
    
    let lanes = ["lane_0", "lane_1", "lane_2"];
    let mut session_keys = Vec::new();
    
    for lane in lanes {
        let session = agent.session_manager().create("test-channel", lane);
        session_keys.push(session.key.clone());
    }
    
    let sessions = agent.list_sessions();
    assert_eq!(sessions.len(), 3);
    
    for key in session_keys {
        agent.session_manager().delete(&key);
    }
    
    let sessions = agent.list_sessions();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_agent_session_ttl() {
    let agent = AgentRuntime::new(AgentConfig::default());
    
    let session = agent.session_manager().create("test-channel", "lane_0");
    let key = session.key.clone();
    
    assert!(agent.session_manager().get(&key).is_some());
    
    agent.session_manager().delete(&key);
    assert!(agent.session_manager().get(&key).is_none());
}

#[tokio::test]
async fn test_skill_manager_lifecycle() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let skills_dir = temp_dir.path().to_path_buf();
    
    let config = SkillConfig {
        skills_dir: skills_dir.clone(),
        pending_dir: skills_dir.join("pending"),
        active_dir: skills_dir.join("active"),
        require_approval: true,
        allowed_languages: vec![SkillLanguage::Python, SkillLanguage::Shell],
    };
    
    let manager = SkillManager::new(config);
    manager.init().await.expect("Failed to init skill manager");
    
    let definition = manager.create_skill(
        "test_skill",
        "A test skill",
        SkillLanguage::Shell,
        "echo hello",
        serde_json::json!({}),
    ).expect("Failed to create skill");
    
    let skill_id = manager.submit_skill(definition).await.expect("Failed to submit skill");
    
    let pending = manager.get_pending_skills();
    assert_eq!(pending.len(), 1);
    
    manager.approve_skill(&skill_id, "test_user").await.expect("Failed to approve skill");
    
    let active = manager.get_active_skills();
    assert_eq!(active.len(), 1);
    
    let pending = manager.get_pending_skills();
    assert!(pending.is_empty());
    
    let stats = manager.hot_reload().await.expect("Failed to hot reload");
    assert_eq!(stats.loaded, 1);
}

#[tokio::test]
async fn test_skill_manager_reject() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let skills_dir = temp_dir.path().to_path_buf();
    
    let config = SkillConfig {
        skills_dir: skills_dir.clone(),
        pending_dir: skills_dir.join("pending"),
        active_dir: skills_dir.join("active"),
        require_approval: true,
        allowed_languages: vec![SkillLanguage::Shell],
    };
    
    let manager = SkillManager::new(config);
    manager.init().await.expect("Failed to init skill manager");
    
    let definition = manager.create_skill(
        "reject_test",
        "A skill to reject",
        SkillLanguage::Shell,
        "echo test",
        serde_json::json!({}),
    ).expect("Failed to create skill");
    
    let skill_id = manager.submit_skill(definition).await.expect("Failed to submit skill");
    
    let pending = manager.get_pending_skills();
    assert_eq!(pending.len(), 1);
    
    manager.reject_skill(&skill_id, "test_user", Some("Test rejection")).await
        .expect("Failed to reject skill");
    
    let pending = manager.get_pending_skills();
    assert!(pending.is_empty());
    
    let active = manager.get_active_skills();
    assert!(active.is_empty());
}

#[tokio::test]
async fn test_skill_stats() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let skills_dir = temp_dir.path().to_path_buf();
    
    let config = SkillConfig {
        skills_dir: skills_dir.clone(),
        pending_dir: skills_dir.join("pending"),
        active_dir: skills_dir.join("active"),
        require_approval: true,
        allowed_languages: vec![SkillLanguage::Shell],
    };
    
    let manager = SkillManager::new(config);
    manager.init().await.expect("Failed to init skill manager");
    
    let stats = manager.stats();
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.active, 0);
    
    let definition = manager.create_skill(
        "stats_test",
        "Stats test skill",
        SkillLanguage::Shell,
        "echo stats",
        serde_json::json!({}),
    ).expect("Failed to create skill");
    
    manager.submit_skill(definition).await.expect("Failed to submit skill");
    
    let stats = manager.stats();
    assert_eq!(stats.pending, 1);
    assert_eq!(stats.active, 0);
}
