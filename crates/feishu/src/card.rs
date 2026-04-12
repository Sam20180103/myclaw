use anyhow::Result;
use serde::{Deserialize, Serialize};

/// 交互卡片构建器 — 用于构建飞书交互卡片消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuCard {
    pub config: CardConfig,
    pub header: CardHeader,
    pub elements: Vec<CardElement>,
}

/// 卡片配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardConfig {
    #[serde(rename = "wide_screen_mode")]
    pub wide_screen_mode: bool,
}

/// 卡片头部
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardHeader {
    pub title: CardTitle,
    pub template: String, // blue/green/red/yellow/purple/indigo/turquoise
}

/// 卡片标题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardTitle {
    pub tag: String,
    pub content: String,
}

/// 卡片元素
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tag")]
pub enum CardElement {
    #[serde(rename = "markdown")]
    Markdown { content: String },
    #[serde(rename = "action")]
    Action { actions: Vec<CardAction> },
    #[serde(rename = "hr")]
    Divider,
    #[serde(rename = "note")]
    Note { elements: Vec<CardNoteElement> },
}

/// 卡片操作按钮
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardAction {
    pub tag: String,
    pub text: CardText,
    #[serde(rename = "type")]
    pub action_type: String, // primary/danger/default
    pub value: serde_json::Value,
}

/// 卡片文本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardText {
    pub tag: String,
    pub content: String,
}

/// 卡片备注元素
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tag")]
pub enum CardNoteElement {
    #[serde(rename = "plain_text")]
    PlainText { content: String },
}

impl FeishuCard {
    /// 创建状态卡片
    pub fn status_card(status: &str, template: &str, actions: Option<Vec<CardAction>>) -> Self {
        let mut elements = vec![CardElement::Markdown {
            content: format!("**状态**: {}", status),
        }];

        if let Some(actions) = actions {
            elements.push(CardElement::Action { actions });
        }

        Self {
            config: CardConfig {
                wide_screen_mode: true,
            },
            header: CardHeader {
                title: CardTitle {
                    tag: "plain_text".to_string(),
                    content: "🤖 RustClaw Agent".to_string(),
                },
                template: template.to_string(),
            },
            elements,
        }
    }

    /// 创建思考中卡片
    pub fn thinking_card() -> Self {
        Self::status_card("思考中...", "blue", None)
    }

    /// 创建生成中卡片
    pub fn generating_card() -> Self {
        Self::status_card("生成中...", "yellow", None)
    }

    /// 创建完成卡片
    pub fn complete_card() -> Self {
        Self::status_card("完成", "green", None)
    }

    /// 创建错误卡片
    pub fn error_card(error_msg: &str) -> Self {
        Self::status_card(&format!("错误: {}", error_msg), "red", None)
    }

    /// 创建带确认按钮的卡片
    pub fn confirm_card(message: &str, confirm_action: CardAction) -> Self {
        Self {
            config: CardConfig {
                wide_screen_mode: true,
            },
            header: CardHeader {
                title: CardTitle {
                    tag: "plain_text".to_string(),
                    content: "⚠️ 确认操作".to_string(),
                },
                template: "yellow".to_string(),
            },
            elements: vec![
                CardElement::Markdown {
                    content: message.to_string(),
                },
                CardElement::Action {
                    actions: vec![confirm_action],
                },
            ],
        }
    }

    /// 转换为 JSON 字符串
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| anyhow::anyhow!("Failed to serialize card: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_card() {
        let card = FeishuCard::thinking_card();
        assert_eq!(card.header.template, "blue");
        assert!(card.config.wide_screen_mode);
    }

    #[test]
    fn test_complete_card() {
        let card = FeishuCard::complete_card();
        assert_eq!(card.header.template, "green");
    }

    #[test]
    fn test_error_card() {
        let card = FeishuCard::error_card("test error");
        assert_eq!(card.header.template, "red");
    }

    #[test]
    fn test_card_to_json() {
        let card = FeishuCard::thinking_card();
        let json = card.to_json().unwrap();
        assert!(json.contains("wide_screen_mode"));
        assert!(json.contains("RustClaw Agent"));
    }
}
