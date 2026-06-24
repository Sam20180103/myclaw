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
}
