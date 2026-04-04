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
            group_enabled: false,
            allowed_groups: vec![],
            allowed_users: vec![],
        }
    }
}
