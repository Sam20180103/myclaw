use crate::config::FeishuConfig;

/// 飞书权限检查器 — 管理 DM/群聊访问控制
pub struct FeishuPermission {
    config: FeishuConfig,
}

impl FeishuPermission {
    /// 创建权限检查器
    pub fn new(config: FeishuConfig) -> Self {
        Self { config }
    }

    /// 检查用户是否有权限访问指定会话
    ///
    /// # Arguments
    /// * `chat_type` - 会话类型 (p2p/group)
    /// * `chat_id` - 会话 ID
    /// * `sender_id` - 发送者 ID
    ///
    /// # Returns
    /// * `true` - 有权限
    /// * `false` - 无权限
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
            // 检查群聊白名单
            if !self.config.allowed_groups.is_empty()
                && !self.config.allowed_groups.contains(&chat_id.to_string())
            {
                return false;
            }
            // 检查群聊独立配置中的用户白名单
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

    /// 获取群聊配置 (如果存在)
    pub fn get_group_config(&self, group_id: &str) -> Option<&crate::config::GroupConfig> {
        self.config
            .group_configs
            .iter()
            .find(|gc| gc.group_id == group_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_permission_dm_enabled() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: false,
            allowed_users: vec![],
            allowed_groups: vec![],
            ..Default::default()
        };
        let permission = FeishuPermission::new(config);
        assert!(permission.check_permission("p2p", "chat_1", "user_1"));
    }

    #[test]
    fn test_check_permission_dm_disabled() {
        let config = FeishuConfig {
            dm_enabled: false,
            ..Default::default()
        };
        let permission = FeishuPermission::new(config);
        assert!(!permission.check_permission("p2p", "chat_1", "user_1"));
    }

    #[test]
    fn test_check_permission_group_enabled() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: true,
            allowed_users: vec![],
            allowed_groups: vec![],
            ..Default::default()
        };
        let permission = FeishuPermission::new(config);
        assert!(permission.check_permission("group", "group_1", "user_1"));
    }

    #[test]
    fn test_check_permission_group_not_in_whitelist() {
        let config = FeishuConfig {
            dm_enabled: true,
            group_enabled: true,
            allowed_groups: vec!["group_2".to_string()],
            ..Default::default()
        };
        let permission = FeishuPermission::new(config);
        assert!(!permission.check_permission("group", "group_1", "user_1"));
    }
}
