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
    /// G8: 定时任务 (config.toml `[[schedules]]`),serde default 向后兼容
    #[serde(default)]
    pub schedules: Vec<ScheduleConfig>,
}

/// G8: 一条定时任务 — 按固定间隔驱动一次 Agent 循环。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    /// 任务名 (日志 / session_key 派生)
    pub name: String,
    /// 间隔,如 "30s" / "10m" / "2h" / "1d"
    pub every: String,
    /// 每次触发发给 Agent 的指令
    pub prompt: String,
    /// 是否启用 (默认 true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 会话 key (默认 `schedule:<name>`,同名任务共享会话上下文)
    #[serde(default)]
    pub session_key: Option<String>,
    /// P2-16: 结果推送目标会话 (如飞书 `p2p:oc_xxx:ou_xxx`)。
    /// 不配则结果只进日志 —— 定时任务等于黑洞,失败也没人知道。
    #[serde(default)]
    pub notify_session: Option<String>,
    /// P2-16: 推送通道 (`feishu` | `wechat`),默认按 `notify_session` 所属会话推断
    #[serde(default)]
    pub notify_channel: Option<String>,
}

/// G8: 解析 "30s" / "10m" / "2h" / "1d" 形式的间隔;非法返回 None。最小 10s。
pub fn parse_interval(spec: &str) -> Option<std::time::Duration> {
    let spec = spec.trim().to_lowercase();
    if spec.len() < 2 {
        return None;
    }
    let (num, unit) = spec.split_at(spec.len() - 1);
    let n: u64 = num.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return None,
    };
    if secs < 10 {
        return None; // 防误配置打爆 provider
    }
    Some(std::time::Duration::from_secs(secs))
}

/// 通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelsConfig {
    pub feishu: Option<FeishuChannelConfig>,
    #[serde(default)]
    pub wechat: Option<WeChatChannelConfig>,
    pub websocket_enabled: bool,
    pub http_enabled: bool,
    pub webhook_enabled: bool,
}

/// 个人微信通道配置 (经外部连接器收发, 自定义 HTTP 契约)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatChannelConfig {
    pub enabled: bool,
    /// 出站发送目标: 网关把回复 POST 到此连接器地址
    pub connector_send_url: String,
    /// 可选: 调用连接器 send_url 时携带的 Bearer token
    #[serde(default)]
    pub send_token: Option<String>,
    /// 机器人自身 wxid (群聊判断 @ 时可用)
    #[serde(default)]
    pub bot_wxid: String,
    /// 群聊是否仅在 @机器人 时响应 (默认 true, 防刷屏)
    #[serde(default = "default_true")]
    pub require_at_in_group: bool,
    pub dm_enabled: bool,
    pub group_enabled: bool,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub group_configs: Option<Vec<GroupChannelConfig>>,
}

fn default_true() -> bool {
    true
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
    pub group_configs: Option<Vec<GroupChannelConfig>>,
}

/// 群聊通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChannelConfig {
    pub group_id: String,
    pub allowed_users: Vec<String>,
    pub bound_skills: Vec<String>,
    pub system_prompt: Option<String>,
    pub rate_limit_per_minute: Option<u32>,
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
    #[serde(default)]
    pub api_key: Option<String>, // API Key 认证 (Phase 17.5: 安全加固)
    #[serde(default = "default_allowed_origins")]
    pub allowed_origins: Vec<String>, // CORS 允许的来源 (Phase 17.5: 收紧CORS)
}

/// Agent 配置 — 运行时参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_concurrent_sessions: usize, // 最大并发会话数
    pub default_model: String,          // 默认模型
    pub max_loop_iterations: u32,       // 最大循环迭代次数
    pub tool_timeout_ms: u64,           // 工具超时 (毫秒)
    /// P1-6: 单次 Agent 循环的墙钟预算 (毫秒),超时返回部分结果
    #[serde(default = "default_max_loop_duration_ms")]
    pub max_loop_duration_ms: u64,
    /// P1-11: 单条工具结果写入历史前的最大字符数 (超出保头保尾截断)
    #[serde(default = "default_max_tool_result_chars")]
    pub max_tool_result_chars: usize,
    /// P0-3: 上下文预算 (token,非字节)。压缩以此为基准
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: usize,
    /// P0-3: 压缩触发阈值 (占 context_window_tokens 的比例)
    #[serde(default = "default_compression_threshold")]
    pub compression_threshold: f64,
}

fn default_max_loop_duration_ms() -> u64 {
    600_000 // 10 分钟
}

fn default_max_tool_result_chars() -> usize {
    16_000
}

fn default_context_window_tokens() -> usize {
    128_000
}

fn default_compression_threshold() -> f64 {
    0.6
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
    #[serde(default)]
    pub reach: ReachConfig, // Agent-Reach 配置
    /// P2-17: 按调用者的工具策略覆盖,key 为 principal (飞书 open_id / 微信 wxid)。
    /// 缺省为空 = 所有调用者共用全局策略 (旧行为)。
    #[serde(default)]
    pub principals: std::collections::HashMap<String, PrincipalPolicyConfig>,
}

/// P2-17: 单个调用者的工具策略覆盖 (字段缺省表示沿用全局)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrincipalPolicyConfig {
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub denied_tools: Option<Vec<String>>,
    #[serde(default)]
    pub require_approval: Option<Vec<String>>,
}

/// Agent-Reach 配置
/// 注意：此结构体与 crates/tool/src/reach.rs 中的 ReachConfig 保持一致
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReachConfig {
    #[serde(default = "default_web_endpoint")]
    pub web_endpoint: String,
    #[serde(default = "default_web_timeout")]
    pub web_timeout_ms: u64,
    #[serde(default = "default_video_timeout")]
    pub video_timeout_ms: u64,
    #[serde(default = "default_github_timeout")]
    pub github_timeout_ms: u64,
    #[serde(default = "default_social_timeout")]
    pub social_timeout_ms: u64,
    #[serde(default = "default_reddit_user_agent")]
    pub reddit_user_agent: String,
    #[serde(default = "default_weibo_api_base")]
    pub weibo_api_base: String,
    #[serde(default = "default_mcp_endpoint")]
    pub mcp_endpoint: String,
    #[serde(default = "default_linkedin_mcp_endpoint")]
    pub linkedin_mcp_endpoint: String,
    #[serde(default = "default_wechat_mcp_endpoint")]
    pub wechat_mcp_endpoint: String,
    #[serde(default = "default_boss_mcp_endpoint")]
    pub boss_mcp_endpoint: String,
    #[serde(default = "default_podcast_mcp_endpoint")]
    pub podcast_mcp_endpoint: String,
    pub boss_cookie: Option<String>,
    #[serde(default)]
    pub exa_api_key: Option<String>,
    #[serde(default = "default_xhs_mcp_endpoint")]
    pub xhs_mcp_endpoint: String,
    #[serde(default)]
    pub xhs_cookie: Option<String>,
    // 文生图 (OpenAI 兼容 /images/generations)
    #[serde(default)]
    pub image_gen_base_url: Option<String>,
    #[serde(default)]
    pub image_gen_api_key: Option<String>,
    #[serde(default)]
    pub image_gen_model: Option<String>,
    #[serde(default)]
    pub image_gen_output_dir: Option<String>,
}

fn default_web_endpoint() -> String {
    "https://r.jina.ai".to_string()
}
fn default_web_timeout() -> u64 {
    30000
}
fn default_video_timeout() -> u64 {
    60000
}
fn default_github_timeout() -> u64 {
    30000
}
fn default_social_timeout() -> u64 {
    30000
}
fn default_reddit_user_agent() -> String {
    "RustClaw/0.4.0".to_string()
}
fn default_weibo_api_base() -> String {
    "https://weibo.com/ajax".to_string()
}
fn default_mcp_endpoint() -> String {
    "http://127.0.0.1:8080".to_string()
}
fn default_linkedin_mcp_endpoint() -> String {
    "http://127.0.0.1:8081".to_string()
}
fn default_wechat_mcp_endpoint() -> String {
    "http://127.0.0.1:8082".to_string()
}
fn default_boss_mcp_endpoint() -> String {
    "http://127.0.0.1:8083".to_string()
}
fn default_podcast_mcp_endpoint() -> String {
    "http://127.0.0.1:8084".to_string()
}
fn default_xhs_mcp_endpoint() -> String {
    "http://127.0.0.1:8085".to_string()
}

fn default_allowed_origins() -> Vec<String> {
    vec![
        "http://localhost:*".to_string(),
        "http://127.0.0.1:*".to_string(),
    ]
}

impl Default for ReachConfig {
    fn default() -> Self {
        Self {
            web_endpoint: default_web_endpoint(),
            web_timeout_ms: default_web_timeout(),
            video_timeout_ms: default_video_timeout(),
            github_timeout_ms: default_github_timeout(),
            social_timeout_ms: default_social_timeout(),
            reddit_user_agent: default_reddit_user_agent(),
            weibo_api_base: default_weibo_api_base(),
            mcp_endpoint: default_mcp_endpoint(),
            linkedin_mcp_endpoint: default_linkedin_mcp_endpoint(),
            wechat_mcp_endpoint: default_wechat_mcp_endpoint(),
            boss_mcp_endpoint: default_boss_mcp_endpoint(),
            podcast_mcp_endpoint: default_podcast_mcp_endpoint(),
            boss_cookie: None,
            exa_api_key: None,
            xhs_mcp_endpoint: default_xhs_mcp_endpoint(),
            xhs_cookie: None,
            image_gen_base_url: None,
            image_gen_api_key: None,
            image_gen_model: None,
            image_gen_output_dir: None,
        }
    }
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
                    api_key: None,
                    allowed_origins: default_allowed_origins(),
                },
            },
            agent: AgentConfig {
                max_concurrent_sessions: 100,
                default_model: "claude-3-5-sonnet-20241022".to_string(),
                max_loop_iterations: 100,
                tool_timeout_ms: 60000,
                max_loop_duration_ms: default_max_loop_duration_ms(),
                max_tool_result_chars: default_max_tool_result_chars(),
                context_window_tokens: default_context_window_tokens(),
                compression_threshold: default_compression_threshold(),
            },
            tools: ToolConfig {
                enabled_tools: vec![
                    "file_read".to_string(),
                    "file_write".to_string(),
                    "shell_exec".to_string(),
                    "search_reach".to_string(),
                ],
                disabled_tools: vec![],
                require_approval: vec!["shell_exec".to_string()],
                sandbox_enabled: true,
                max_memory_mb: 512,
                max_timeout_ms: 30000,
                reach: ReachConfig::default(),
                principals: std::collections::HashMap::new(),
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
                wechat: None,
                websocket_enabled: true,
                http_enabled: true,
                webhook_enabled: false,
            },
            schedules: vec![],
        }
    }
}

/// G9: 解析单个可选密钥字段的 `env:VAR` 引用。env 缺失 → None。
fn resolve_secret_opt(value: &mut Option<String>) {
    if let Some(v) = value {
        if let Some(var) = v.strip_prefix("env:") {
            match std::env::var(var) {
                Ok(env_val) => *value = Some(env_val),
                Err(_) => {
                    eprintln!("[config] secret env var '{}' not set; clearing field", var);
                    *value = None;
                }
            }
        }
    }
}

/// G9: 解析必填密钥字段的 `env:VAR` 引用。env 缺失 → 保留原样并告警。
fn resolve_secret_str(value: &mut String) {
    if let Some(var) = value.strip_prefix("env:").map(|s| s.to_string()) {
        match std::env::var(&var) {
            Ok(env_val) => *value = env_val,
            Err(_) => eprintln!("[config] secret env var '{}' not set; keeping literal", var),
        }
    }
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        // G7 修复: 必须走 config_path(),它才带 profile 维度。
        // 此前这里就地拼死路径,导致 RUSTCLAW_PROFILE 对 config.toml 完全失效 —— 命名
        // profile 的进程会读到默认 profile 的凭据 (飞书 app_secret / provider key)。
        let config_path = path.unwrap_or_else(Self::config_path);

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let mut config: Config = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))?;
            // G9: 解析密钥的 env 间接引用,避免敏感值落 config.toml 明文
            config.resolve_secret_refs();
            Ok(config)
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

    /// G9: 把形如 `env:VARNAME` 的密钥值替换为对应环境变量的内容。
    ///
    /// 让用户可在 config.toml 写 `api_key = "env:OPENAI_API_KEY"`,真实密钥只存在于
    /// 环境变量,不落明文。env 变量缺失时:`Option<String>` 字段置为 `None`,
    /// 必填 `String` 字段保留原样 (并告警)。覆盖 provider key、网关 api_key、
    /// reach 凭据、飞书/微信通道密钥。
    pub fn resolve_secret_refs(&mut self) {
        for p in self.providers.iter_mut() {
            resolve_secret_opt(&mut p.api_key);
        }
        resolve_secret_opt(&mut self.gateway.auth.api_key);
        resolve_secret_opt(&mut self.tools.reach.boss_cookie);
        resolve_secret_opt(&mut self.tools.reach.exa_api_key);
        resolve_secret_opt(&mut self.tools.reach.xhs_cookie);
        resolve_secret_opt(&mut self.tools.reach.image_gen_api_key);
        if let Some(feishu) = self.channels.feishu.as_mut() {
            resolve_secret_str(&mut feishu.app_secret);
            resolve_secret_str(&mut feishu.verification_token);
            resolve_secret_opt(&mut feishu.encrypt_key);
        }
        if let Some(wechat) = self.channels.wechat.as_mut() {
            resolve_secret_opt(&mut wechat.send_token);
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

    /// G7: 当前 profile 名 (来自 `RUSTCLAW_PROFILE`)。`None` = 默认 profile (兼容旧路径)。
    pub fn profile() -> Option<String> {
        match std::env::var("RUSTCLAW_PROFILE") {
            Ok(p) => {
                let p = p.trim().to_string();
                if p.is_empty() || p == "default" {
                    None
                } else {
                    // 只允许安全字符,防路径穿越
                    let safe: String = p
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    if safe.is_empty() { None } else { Some(safe) }
                }
            }
            Err(_) => None,
        }
    }

    /// G7: 在基准目录上叠加 profile 维度:
    /// 默认 profile → `<base>/rustclaw`;命名 profile → `<base>/rustclaw/profiles/<name>`。
    fn profiled(base: PathBuf) -> PathBuf {
        let root = base.join("rustclaw");
        match Self::profile() {
            Some(p) => root.join("profiles").join(p),
            None => root,
        }
    }

    pub fn config_path() -> PathBuf {
        Self::profiled(dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join("config.toml")
    }

    pub fn skills_dir() -> PathBuf {
        Self::profiled(dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))).join("skills")
    }

    /// 待审批技能目录 (`<skills>/pending`)
    pub fn pending_skills_dir() -> PathBuf {
        Self::skills_dir().join("pending")
    }

    /// 已生效技能目录 (`<skills>/active`)。
    /// 此前只有 `SkillConfig::default()` 里写死 `skills/active`,而 config 侧没有对应
    /// 访问器 —— 两处对"生效技能放哪"各说各话,文档也因此写错。
    pub fn active_skills_dir() -> PathBuf {
        Self::skills_dir().join("active")
    }

    pub fn logs_dir() -> PathBuf {
        Self::profiled(dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."))).join("logs")
    }

    /// G7: 运行时数据目录 (memories / state.db / media)。
    /// 默认 profile → `~/.rustclaw` (兼容旧数据);命名 profile → `~/.rustclaw/profiles/<name>`。
    pub fn data_dir() -> PathBuf {
        let root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".rustclaw");
        match Self::profile() {
            Some(p) => root.join("profiles").join(p),
            None => root,
        }
    }
}

#[cfg(test)]
mod tests;
