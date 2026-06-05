# WeChat Connector (参考骨架)

把「个人微信框架」桥接到 RustClaw 的稳定 HTTP 契约。**不参与 cargo 构建**，与 `vendor/mcp-bosszp` 同性质。

## 数据流

```
微信 ⇄ [框架: wcferry/wechaty/itchat]
          │ 收到消息 → push_inbound() → POST 网关 /channel/wechat/inbound
          ▲ /send ← 网关回调 (出站契约) ← Agent 回复
```

## 运行

```bash
pip install fastapi uvicorn requests
export GATEWAY_INBOUND_URL=http://127.0.0.1:18789/channel/wechat/inbound
export GATEWAY_API_KEY=...        # 若网关配了 api_key
export CONNECTOR_PORT=8090
python connector.py
```

网关侧 `config.toml`:

```toml
[channels.wechat]
enabled = true
connector_send_url = "http://127.0.0.1:8090/send"
bot_wxid = "wxid_self"
require_at_in_group = true
dm_enabled = true
group_enabled = false
allowed_users = []          # 空 = 允许全部私聊用户
allowed_groups = []
```

## 你要补两处
- **收消息**：在框架的消息回调里调用 `push_inbound(from_user=..., content=..., room_id=..., is_at_bot=...)`。
- **发消息**：在 `do_send()` 里调用框架的发送 API（已给 wcferry 示例注释）。

## HTTP 契约

入站 `POST /channel/wechat/inbound`:
```json
{"msg_type":"text","from_user":"wxid_x","room_id":null,"is_at_bot":false,"content":"你好",
 "image_base64":null,"image_type":null,"from_user_name":"张三","msg_id":"1"}
```
出站 `POST {connector_send_url}`:
```json
{"to_user":"wxid_x","room_id":null,"msg_type":"text","content":"回复"}
```

## ⚠️ 风险
个人微信自动化违反微信使用条款，存在**封号风险**。请使用小号、控制频率，自行承担风险。
协议层完全在本连接器侧，框架/协议变动不影响 RustClaw 网关。
