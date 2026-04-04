use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 根配置结构 — 包含所有子配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub gateway: GatewayConfig,         // 网关配置
    pub agent: AgentConfig,             // Agent 配置
    pub tools: ToolConfig,              // 工具配置
    pub providers: Vec<ProviderConfig>, // LLM 提供商配置
    pub channels: ChannelsConfig,       // 通道配置
}

/// 通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelsConfig {
    pub feishu: Option<FeishuChannelConfig>,
    pub websocket_enabled: bool,
    pub http_enabled: bool,
    pub webhook_enabled: bool,
}

/// 飞书通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuChannelConfig {
    pub enabled: bool,
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

/// 网关配置 — HTTP/WS 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub host: String,      // 监听地址
    pub port: u16,         // 监听端口
    pub device_id: String, // 设备唯一标识
    pub auth: AuthConfig,  // 认证配置
}

/// 认证配置 — 设备配对授权
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub require_pairing: bool,        // 是否需要设备配对
    pub allowed_devices: Vec<String>, // 允许的设备列表
}

/// Agent 配置 — 运行时参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_concurrent_sessions: usize, // 最大并发会话数
    pub default_model: String,          // 默认模型
    pub max_loop_iterations: u32,       // 最大循环迭代次数
    pub tool_timeout_ms: u64,           // 工具超时 (毫秒)
}

/// 工具配置 — 工具策略和沙箱设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub enabled_tools: Vec<String>,    // 启用的工具列表
    pub disabled_tools: Vec<String>,   // 禁用的工具列表
    pub require_approval: Vec<String>, // 需要审批的工具
    pub sandbox_enabled: bool,         // 是否启用沙箱
    pub max_memory_mb: u64,            // 最大内存 (MB)
    pub max_timeout_ms: u64,           // 最大超时 (毫秒)
}

/// LLM 提供商配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,                  // 提供商名称
    pub provider_type: String,         // 提供商类型 (openai/anthropic/nvidia)
    pub api_key: Option<String>,       // API 密钥
    pub base_url: Option<String>,      // 自定义 API 地址
    pub default_model: Option<String>, // 默认模型
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
            channels: ChannelsConfig {
                feishu: None,
                websocket_enabled: true,
                http_enabled: true,
                webhook_enabled: false,
            },
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

    pub fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let config_path = path.unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rustclaw")
                .join("config.toml")
        });

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustclaw")
            .join("config.toml")
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
