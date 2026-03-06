use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub gateway: GatewayConfig,
    pub agent: AgentConfig,
    pub tools: ToolConfig,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub device_id: String,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub require_pairing: bool,
    pub allowed_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_concurrent_sessions: usize,
    pub default_model: String,
    pub max_loop_iterations: u32,
    pub tool_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub require_approval: Vec<String>,
    pub sandbox_enabled: bool,
    pub max_memory_mb: u64,
    pub max_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub provider_type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gateway: GatewayConfig {
                host: "127.0.0.1".to_string(),
                port: 18789,
                device_id: uuid::Uuid::new_v4().to_string(),
                auth: AuthConfig {
                    require_pairing: true,
                    allowed_devices: vec![],
                },
            },
            agent: AgentConfig {
                max_concurrent_sessions: 100,
                default_model: "claude-3-5-sonnet-20241022".to_string(),
                max_loop_iterations: 100,
                tool_timeout_ms: 60000,
            },
            tools: ToolConfig {
                enabled_tools: vec![
                    "file_read".to_string(),
                    "file_write".to_string(),
                    "shell_exec".to_string(),
                    "web_search".to_string(),
                ],
                disabled_tools: vec![],
                require_approval: vec!["shell_exec".to_string()],
                sandbox_enabled: true,
                max_memory_mb: 512,
                max_timeout_ms: 30000,
            },
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                provider_type: "anthropic".to_string(),
                api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
                base_url: Some("https://api.anthropic.com".to_string()),
                default_model: Some("claude-3-5-sonnet-20241022".to_string()),
            }],
        }
    }
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let config_path = path.unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rustclaw")
                .join("config.toml")
        });

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str(&content).map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))
        } else {
            let default_config = Self::default();
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = toml::to_string_pretty(&default_config)?;
            std::fs::write(&config_path, content)?;
            Ok(default_config)
        }
    }

    pub fn skills_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustclaw")
            .join("skills")
    }

    pub fn pending_skills_dir() -> PathBuf {
        Self::skills_dir().join("pending")
    }

    pub fn logs_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustclaw")
            .join("logs")
    }
}

#[cfg(test)]
mod tests;
