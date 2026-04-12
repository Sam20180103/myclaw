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
}
