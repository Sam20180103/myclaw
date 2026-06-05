# 个人微信交互通道 (WeChat Channel)

让用户用**个人微信**私聊/群聊直接与 Agent 对话（收发文本与图片）。

## 设计要点

个人微信**没有官方 API**，协议层不稳定（需 Hook PC 微信 / wechaty puppet 等灰色方案）。因此协议层放在
**外部连接器**，RustClaw 网关只对接一份**稳定的自定义 HTTP 契约**，与具体框架解耦：协议/框架变动不影响网关。

- 入站：连接器把收到的微信消息 `POST` 到网关 `POST /channel/wechat/inbound`。
- 出站：网关把 Agent 回复 `POST` 到连接器的 `connector_send_url`（如 `/send`）。

```
微信 ⇄ [连接器: wcferry/wechaty/...] ⇄ (HTTP 契约) ⇄ [Gateway WeChatChannel] ⇄ Agent
```

会话键沿用飞书方案 `"{chat_type}:{chat_id}:{sender}"`（私聊 `p2p:{wxid}:{wxid}`，群聊 `group:{room}:{wxid}`）。

## 配置 (`config.toml`)

```toml
[channels.wechat]
enabled = true
connector_send_url = "http://127.0.0.1:8090/send"  # 网关回调连接器发送
send_token = ""                # 可选: 调用连接器时的 Bearer token
bot_wxid = "wxid_self"
require_at_in_group = true      # 群聊仅 @机器人 才响应 (防刷屏)
dm_enabled = true
group_enabled = false
allowed_users = []              # 空 = 允许全部私聊用户; 否则白名单
allowed_groups = []
# [[channels.wechat.group_configs]]
# group_id = "xxx@chatroom"
# allowed_users = ["wxid_a"]
# system_prompt = "你是群助手"
```

## HTTP 契约

### 入站 `POST /channel/wechat/inbound`
```json
{
  "msg_id": "123",
  "msg_type": "text" | "image",
  "from_user": "wxid_sender",
  "from_user_name": "张三",
  "room_id": "xxx@chatroom",   // 群聊存在; 私聊 null/缺省
  "is_at_bot": true,            // 群聊: 是否 @机器人
  "content": "你好",
  "image_base64": "...", "image_type": "png"
}
```
返回 `200 {"accepted": true|false}`（被权限/规则忽略时 `false`）。

### 出站 `POST {connector_send_url}`
```json
{ "to_user": "wxid_sender", "room_id": null, "msg_type": "text", "content": "回复" }
```
图片：`"msg_type":"image","image_base64":"...","image_type":"png"`。

## 行为
- **私聊**：受 `dm_enabled` + `allowed_users` 白名单控制。
- **群聊**：受 `group_enabled` + `allowed_groups` + 群级 `allowed_users` 控制；默认仅 `@机器人` 响应。
- **图片**：入站图片 → `Content::Image` 走视觉模型（需配置 vision provider）；出站图片回传 base64。
- **`/clear`**：清空当前会话。
- 图片视觉链路复用 provider 的 `openai_content` / `anthropic_content`（v0.45.0）。

## 连接器
参考骨架见 `vendor/wechat-connector/`（FastAPI），补两处即可：消息回调里 `push_inbound()`，`do_send()` 里调框架发送 API。

## 安全
- `/channel/wechat/inbound` 会触发 Agent，**务必在可信网络或配置 `gateway.auth.api_key`（Bearer）**，勿公网裸暴露。
- **封号风险**：个人微信自动化违反微信使用条款，请用小号、低频，自担风险。
