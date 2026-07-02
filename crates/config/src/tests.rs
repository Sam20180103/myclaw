#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::Config;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.gateway.host, "127.0.0.1");
        assert_eq!(config.gateway.port, 18789);
        assert!(config.gateway.auth.require_pairing);
    }

    #[test]
    fn test_gateway_config() {
        let config = Config::default();
        assert!(!config.gateway.device_id.is_empty());
        assert!(config.gateway.auth.allowed_devices.is_empty());
    }

    #[test]
    fn test_agent_config() {
        let config = Config::default();
        assert_eq!(config.agent.max_concurrent_sessions, 100);
        assert!(config.agent.tool_timeout_ms > 0);
    }

    #[test]
    fn test_tool_config() {
        let config = Config::default();
        assert!(config
            .tools
            .enabled_tools
            .contains(&"file_read".to_string()));
        assert!(config
            .tools
            .require_approval
            .contains(&"shell_exec".to_string()));
        assert!(config.tools.sandbox_enabled);
    }

    #[test]
    fn test_provider_config() {
        let config = Config::default();
        assert!(!config.providers.is_empty());
        assert_eq!(config.providers[0].name, "anthropic");
    }

    #[test]
    fn test_skills_dir() {
        let skills_dir = Config::skills_dir();
        assert!(skills_dir.to_string_lossy().contains("rustclaw"));
        assert!(skills_dir.to_string_lossy().contains("skills"));
    }

    #[test]
    fn test_pending_skills_dir() {
        let pending_dir = Config::pending_skills_dir();
        assert!(pending_dir.to_string_lossy().contains("pending"));
    }

    // G9: env 间接引用解析
    #[test]
    fn test_g9_resolve_secret_env_ref() {
        std::env::set_var("RUSTCLAW_TEST_SECRET_G9", "resolved-key");
        let mut config = Config::default();
        config.providers[0].api_key = Some("env:RUSTCLAW_TEST_SECRET_G9".to_string());
        config.gateway.auth.api_key = Some("env:RUSTCLAW_TEST_SECRET_G9".to_string());
        config.resolve_secret_refs();
        assert_eq!(config.providers[0].api_key.as_deref(), Some("resolved-key"));
        assert_eq!(config.gateway.auth.api_key.as_deref(), Some("resolved-key"));
        std::env::remove_var("RUSTCLAW_TEST_SECRET_G9");
    }

    #[test]
    fn test_g9_missing_env_clears_optional() {
        std::env::remove_var("RUSTCLAW_TEST_MISSING_G9");
        let mut config = Config::default();
        config.providers[0].api_key = Some("env:RUSTCLAW_TEST_MISSING_G9".to_string());
        config.resolve_secret_refs();
        assert!(config.providers[0].api_key.is_none(), "missing env should clear optional secret");
    }

    #[test]
    fn test_g9_literal_values_untouched() {
        let mut config = Config::default();
        config.providers[0].api_key = Some("sk-literal-123".to_string());
        config.resolve_secret_refs();
        assert_eq!(config.providers[0].api_key.as_deref(), Some("sk-literal-123"));
    }

    // G7: profile 隔离。env 全局共享,单测里串行设置/清理并合并断言。
    #[test]
    fn test_g7_profile_paths() {
        // 默认 (无 env): 旧路径,不含 profiles
        std::env::remove_var("RUSTCLAW_PROFILE");
        assert!(Config::profile().is_none());
        assert!(!Config::config_path().to_string_lossy().contains("profiles"));
        assert!(Config::data_dir().to_string_lossy().ends_with(".rustclaw"));

        // 命名 profile: 各目录都带 profiles/<name>
        std::env::set_var("RUSTCLAW_PROFILE", "work");
        assert_eq!(Config::profile().as_deref(), Some("work"));
        for p in [Config::config_path(), Config::skills_dir(), Config::logs_dir(), Config::data_dir()] {
            let s = p.to_string_lossy().to_string();
            assert!(s.contains("profiles") && s.contains("work"), "path missing profile: {}", s);
        }

        // "default" 等价于无 profile
        std::env::set_var("RUSTCLAW_PROFILE", "default");
        assert!(Config::profile().is_none());

        // 路径穿越字符被剥离
        std::env::set_var("RUSTCLAW_PROFILE", "../evil");
        assert_eq!(Config::profile().as_deref(), Some("evil"));

        std::env::remove_var("RUSTCLAW_PROFILE");
    }

    // G8: 定时任务配置
    #[test]
    fn test_g8_parse_interval() {
        use crate::parse_interval;
        use std::time::Duration;
        assert_eq!(parse_interval("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_interval("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_interval("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_interval("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_interval(" 15M "), Some(Duration::from_secs(900))); // 空白+大写
        assert_eq!(parse_interval("5s"), None);  // 低于 10s 下限
        assert_eq!(parse_interval("abc"), None);
        assert_eq!(parse_interval("10x"), None);
        assert_eq!(parse_interval(""), None);
    }

    #[test]
    fn test_g8_schedules_toml_roundtrip() {
        let toml_str = r#"
[[schedules]]
name = "morning-hot"
every = "12h"
prompt = "查看小红书热门 Top10 并总结"

[[schedules]]
name = "disabled-one"
every = "30m"
prompt = "x"
enabled = false
session_key = "custom-key"
"#;
        // schedules 可独立反序列化 (serde default 使旧 config.toml 无此段也兼容)
        #[derive(serde::Deserialize)]
        struct Partial {
            #[serde(default)]
            schedules: Vec<crate::ScheduleConfig>,
        }
        let p: Partial = toml::from_str(toml_str).unwrap();
        assert_eq!(p.schedules.len(), 2);
        assert_eq!(p.schedules[0].name, "morning-hot");
        assert!(p.schedules[0].enabled); // 默认 true
        assert!(p.schedules[0].session_key.is_none());
        assert!(!p.schedules[1].enabled);
        assert_eq!(p.schedules[1].session_key.as_deref(), Some("custom-key"));

        // 旧 config (无 schedules 段) 仍可解析
        let old: Partial = toml::from_str("").unwrap();
        assert!(old.schedules.is_empty());
    }
}
