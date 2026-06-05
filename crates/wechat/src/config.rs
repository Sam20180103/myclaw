/// 群聊独立配置 (与 config crate 的 GroupChannelConfig 对应)
#[derive(Debug, Clone, Default)]
pub struct WeChatGroupConfig {
    pub group_id: String,
    pub allowed_users: Vec<String>,
    pub system_prompt: Option<String>,
}

/// 个人微信通道运行期配置
#[derive(Debug, Clone, Default)]
pub struct WeChatConfig {
    /// 出站发送目标: 网关把回复 POST 到此连接器地址
    pub connector_send_url: String,
    /// 可选: 调用连接器 send_url 时携带的 Bearer token
    pub send_token: Option<String>,
    /// 机器人自身 wxid
    pub bot_wxid: String,
    /// 群聊是否仅在 @机器人 时响应
    pub require_at_in_group: bool,
    pub dm_enabled: bool,
    pub group_enabled: bool,
    pub allowed_users: Vec<String>,
    pub allowed_groups: Vec<String>,
    pub system_prompt: Option<String>,
    pub group_configs: Vec<WeChatGroupConfig>,
}
