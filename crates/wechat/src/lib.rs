//! 个人微信交互通道 (rustclaw-wechat)
//!
//! 通过外部连接器 (wechaty/wcferry/...) 经稳定的自定义 HTTP 契约收发个人微信消息:
//! - 入站: 连接器 POST 到网关 → `WeChatChannel::ingest()`
//! - 出站: `WeChatChannel::send()` → 连接器 send_url
//!
//! 协议层 (易变、灰色) 完全在连接器侧, 本 crate 与具体框架解耦。

pub mod channel;
pub mod client;
pub mod config;
pub mod message;
pub mod permission;

pub use channel::WeChatChannel;
pub use client::{SendTarget, WeChatConnectorClient};
pub use config::{WeChatConfig, WeChatGroupConfig};
pub use message::{parse_session_key, WeChatInbound};
pub use permission::WeChatPermission;
