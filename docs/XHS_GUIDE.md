# 小红书自动化使用指南 (Xiaohongshu)

模拟用户操作实现：**自动登录** → **查看热门 Top10** → **写文章 + 自动生图 + 发布**。

协议层（易变、灰色）放在外部 Playwright MCP 服务器 `vendor/mcp-xhs/`（镜像 BOSS 服务器），
Rust 侧 `xhs_reach` 工具经 `McpClient` JSON-RPC 桥接；文生图由通用 `image_gen` 工具完成。

## 组件
- **`vendor/mcp-xhs/server.py`**（Playwright，默认端口 8085）：`xhs.login_send_sms/login_submit_sms/check_auth/get_hot/get_note/search/get_user/publish`。
- **`xhs_reach` 工具**（`crates/tool/src/reach.rs`）：动作 `note/search/user/hot/login_send_sms/login_submit_sms/publish`。
- **`image_gen` 工具**：OpenAI 兼容文生图，生成 PNG 落盘并返回路径。

## 配置 (`config.toml`)
```toml
[tools.reach]
xhs_mcp_endpoint = "http://127.0.0.1:8085"
xhs_cookie = ""                 # 可空: 服务端登录后自动缓存
# 文生图 (OpenAI 兼容 /v1/images/generations)
image_gen_base_url = "https://api.openai.com/v1"
image_gen_api_key  = "sk-..."
image_gen_model    = "gpt-image-1"
image_gen_output_dir = ""       # 默认 ~/.rustclaw/media/generated
```
MCP 服务器 env：`XHS_PHONE`（登录手机号）、`XHS_LOGIN_PAGE_TTL`(默认300)、`XHS_MCP_PORT`(默认8085)、`XHS_COOKIE_CACHE_PATH`。

## 启动
```bash
pip install fastapi uvicorn playwright playwright-stealth && playwright install chromium
export XHS_PHONE=138xxxxxxxx
python vendor/mcp-xhs/server.py     # 监听 8085
```

## 使用 (对 Agent 说)
- **看热门**：「看下小红书热门 Top10」→ Agent 调 `xhs_reach(action="hot", limit=10)`。
- **写文发布**：「写一篇关于<主题>的小红书并发布」→ Agent 流程：
  1. （可选）`xhs_reach(action="hot")` 找选题；
  2. LLM 写 标题/正文/话题；
  3. `image_gen(prompt=...)` 生成封面/配图 → 得到图片路径；
  4. `xhs_reach(action="publish", title, body, topics:[...], image_paths:[...])`。
- **登录**：业务调用未登录时返回 `status=auth_required(sms)` → Agent 调 `login_send_sms` →
  飞书/微信提示「已向 138**** 发送验证码，请回复验证码」→ 用户回复 → `login_submit_sms(code)` → 自动重试。

## 数据流注意
`image_gen` 生成的图片**落盘到本地媒体目录**，`publish` 传**文件路径**，由 MCP 服务器读取上传——
要求**网关与 MCP 服务器同主机共享文件系统**（跨机部署需改传 base64/对象存储，非首版）。

## ⚠️ 风险与限制
- **选择器 best-effort**：登录/热门/发布选择器为推测值，**必须对照小红书线上 DOM 校准**；先用 `vendor/mcp-xhs/test_xhs.py` 实测。
- **风控/封号**：小红书对自动化发布风控严格，违反平台规则有**封号风险**；请用小号、低频、内容合规，自担风险。
- **滑块验证**：登录/发布可能触发滑块，首版仅检测并返回 `captcha`，不自动绕过（可改用 cookie 注入）。
- **入站鉴权**：触发发布的指令应来自可信通道（飞书/微信白名单或网关 api_key）。
- **文生图成本/合规**：T2I API 有调用成本；生成图须符合平台规范。
