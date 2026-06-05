use crate::config::{WeChatConfig, WeChatGroupConfig};

/// 微信权限检查器 — 镜像飞书 DM/群聊访问控制
#[derive(Clone)]
pub struct WeChatPermission {
    config: WeChatConfig,
}

impl WeChatPermission {
    pub fn new(config: WeChatConfig) -> Self {
        Self { config }
    }

    /// 检查发送者是否有权限。chat_type: "p2p" | "group"
    pub fn check_permission(&self, chat_type: &str, chat_id: &str, sender_id: &str) -> bool {
        if chat_type == "p2p" {
            if !self.config.dm_enabled {
                return false;
            }
            if !self.config.allowed_users.is_empty()
                && !self.config.allowed_users.contains(&sender_id.to_string())
            {
                return false;
            }
        } else if chat_type == "group" {
            if !self.config.group_enabled {
                return false;
            }
            if !self.config.allowed_groups.is_empty()
                && !self.config.allowed_groups.contains(&chat_id.to_string())
            {
                return false;
            }
            if let Some(group_config) = self.get_group_config(chat_id) {
                if !group_config.allowed_users.is_empty()
                    && !group_config.allowed_users.contains(&sender_id.to_string())
                {
                    return false;
                }
            }
        }
        true
    }

    pub fn get_group_config(&self, group_id: &str) -> Option<&WeChatGroupConfig> {
        self.config
            .group_configs
            .iter()
            .find(|gc| gc.group_id == group_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WeChatConfig {
        WeChatConfig {
            dm_enabled: true,
            group_enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_dm_allowed_by_default() {
        let p = WeChatPermission::new(cfg());
        assert!(p.check_permission("p2p", "wxid_a", "wxid_a"));
    }

    #[test]
    fn test_dm_disabled() {
        let c = WeChatConfig { dm_enabled: false, ..cfg() };
        assert!(!WeChatPermission::new(c).check_permission("p2p", "wxid_a", "wxid_a"));
    }

    #[test]
    fn test_dm_user_whitelist() {
        let c = WeChatConfig { allowed_users: vec!["wxid_ok".into()], ..cfg() };
        let p = WeChatPermission::new(c);
        assert!(p.check_permission("p2p", "wxid_ok", "wxid_ok"));
        assert!(!p.check_permission("p2p", "wxid_no", "wxid_no"));
    }

    #[test]
    fn test_group_not_in_whitelist() {
        let c = WeChatConfig { allowed_groups: vec!["g2@chatroom".into()], ..cfg() };
        assert!(!WeChatPermission::new(c).check_permission("group", "g1@chatroom", "wxid_a"));
    }
}
