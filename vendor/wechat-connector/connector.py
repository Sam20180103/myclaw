"""个人微信连接器 — 参考骨架 (connector-agnostic).

它在「网关稳定 HTTP 契约」与「具体微信框架 (wcferry / wechaty / itchat ...)」之间做桥接:

  微信框架  --(收到消息回调)-->  push_inbound()  --POST-->  网关 /channel/wechat/inbound
  网关  --POST /send-->  本服务 /send  --(调用框架发送 API)-->  微信

你需要做的:
  1. 选一个能登录个人微信的框架 (wcferry 需 Windows+指定微信版本; wechaty 需 puppet)。
  2. 在该框架的「收到消息」回调里, 转成入站契约调用 push_inbound()。
  3. 在 do_send() 里, 把出站契约翻译成该框架的发送调用。

环境变量:
  GATEWAY_INBOUND_URL  网关入站地址 (默认 http://127.0.0.1:18789/channel/wechat/inbound)
  GATEWAY_API_KEY      若网关配了 api_key, 这里带上 (Bearer)
  CONNECTOR_HOST/PORT  本服务监听 (默认 0.0.0.0:8090)

风险: 个人微信自动化违反微信使用条款, 有封号风险。请用小号、低频, 自担风险。
"""
import base64
import os

import requests
from fastapi import FastAPI, Request
import uvicorn

GATEWAY_INBOUND_URL = os.getenv(
    "GATEWAY_INBOUND_URL", "http://127.0.0.1:18789/channel/wechat/inbound"
)
GATEWAY_API_KEY = os.getenv("GATEWAY_API_KEY", "").strip()

app = FastAPI()


def push_inbound(
    *,
    from_user: str,
    content: str | None = None,
    room_id: str | None = None,
    is_at_bot: bool = False,
    msg_type: str = "text",
    image_base64: str | None = None,
    image_type: str | None = None,
    from_user_name: str | None = None,
    msg_id: str | None = None,
) -> dict:
    """把一条收到的微信消息推给网关 (入站契约)。在你框架的消息回调里调用。"""
    payload = {
        "msg_id": msg_id,
        "msg_type": msg_type,
        "from_user": from_user,
        "from_user_name": from_user_name,
        "room_id": room_id,
        "is_at_bot": is_at_bot,
        "content": content,
        "image_base64": image_base64,
        "image_type": image_type,
    }
    headers = {}
    if GATEWAY_API_KEY:
        headers["Authorization"] = f"Bearer {GATEWAY_API_KEY}"
    resp = requests.post(GATEWAY_INBOUND_URL, json=payload, headers=headers, timeout=20)
    resp.raise_for_status()
    return resp.json()


def do_send(target: dict, msg_type: str, content: str | None, image_bytes: bytes | None):
    """TODO: 用你选定的框架真正发送。

    target: {"to_user": "wxid_..."} 或 {"room_id": "...@chatroom"}
    msg_type: "text" | "image"

    示例 (wcferry):
        from wcferry import Wcf
        wcf = Wcf()
        receiver = target.get("room_id") or target.get("to_user")
        if msg_type == "text":
            wcf.send_text(content, receiver)
        else:
            path = save_tmp(image_bytes)
            wcf.send_image(path, receiver)
    """
    receiver = target.get("room_id") or target.get("to_user")
    if msg_type == "text":
        detail = content
    else:
        detail = "<image %d bytes>" % len(image_bytes or b"")
    print("[connector] TODO send %s to %s: %s" % (msg_type, receiver, detail))


@app.post("/send")
async def send(request: Request):
    """网关回调: 接收出站契约并发送到微信。"""
    body = await request.json()
    target = {k: body[k] for k in ("to_user", "room_id") if body.get(k)}
    msg_type = body.get("msg_type", "text")
    if msg_type == "image":
        b64 = body.get("image_base64", "")
        do_send(target, "image", None, base64.b64decode(b64) if b64 else b"")
    else:
        do_send(target, "text", body.get("content", ""), None)
    return {"ok": True}


@app.get("/health")
async def health():
    return {"status": "ok", "service": "wechat-connector"}


if __name__ == "__main__":
    host = os.getenv("CONNECTOR_HOST", "0.0.0.0")
    port = int(os.getenv("CONNECTOR_PORT", "8090"))
    print(f"WeChat connector reference on {host}:{port}; inbound -> {GATEWAY_INBOUND_URL}")
    uvicorn.run(app, host=host, port=port)
