use serde::{Deserialize, Serialize};

/// 飞书通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub encrypt_key: Option<String>,
    pub default_model: Option<String>,
    pub system_prompt: Option<String>,
    pub dm_enabled: bool,
    pub group_enabled: bool,
    pub allowed_groups: Vec<String>,
    pub allowed_users: Vec<String>,
    pub group_configs: Vec<GroupConfig>,
}

/// 群聊配置 — 每个群聊的独立配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfig {
    pub group_id: String,
    pub allowed_users: Vec<String>,
    pub bound_skills: Vec<String>,
    pub system_prompt: Option<String>,
    pub rate_limit_per_minute: u32,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_secret: String::new(),
            verification_token: String::new(),
            encrypt_key: None,
            default_model: None,
            system_prompt: None,
            dm_enabled: true,
            group_enabled: true,
            allowed_groups: vec![],
            allowed_users: vec![],
            group_configs: vec![],
        }
    }
}
