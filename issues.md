# RustClaw 项目问题与进展记录 (Issues & Progress)

## 2026-06-05 更新: 待修复问题 (Pending) 🔍

> 暂无未完成的已知功能项。Phase 18 (新功能规划) 待定义。

---

## 已完成的修复 (Completed) ✅

### v0.47.0 小红书自动化

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `vendor/mcp-xhs/` (新) | 缺小红书 MCP 服务器 (原 mcp_endpoint 指向不存在的服务) | Playwright 服务器, 手机号+短信两步登录(复用 BOSS 范式)+cookie 缓存; get_hot/publish/读方法 |
| 2 | `crates/tool/src/reach.rs` | XhsReachTool 仅只读 | 新增 `hot`/`publish`/`login_send_sms`/`login_submit_sms` 动作 + auth_required(sms) 透传; 改用 `xhs_mcp_endpoint` |
| 3 | `crates/tool/src/reach.rs` | 全仓零文生图能力 | 新增通用 `image_gen` 工具 (OpenAI 兼容 /images/generations, 落盘返回路径) |
| 4 | `crates/config` + gateway | 缺 xhs/生图配置 | ReachConfig 增 xhs_mcp_endpoint/xhs_cookie/image_gen_* (serde default 兼容) |
| 5 | `docs/XHS_GUIDE.md` (新) | 缺接入文档 | 用法 + 数据流 + 选择器校准/风控声明 |

### v0.46.0 个人微信交互通道

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `crates/wechat/` (新) | 缺个人微信交互通道 | 新增 `WeChatChannel` (实现 Channel trait, 连接器无关 HTTP 契约): 入站 `ingest()` + 出站连接器回调; 私聊/群聊/图片; 镜像飞书权限范式; +9 tests |
| 2 | `crates/gateway/src/lib.rs` | 网关未接入微信 | 构造 WeChatChannel + `POST /channel/wechat/inbound` 路由 + 事件循环 (仿飞书) + ChannelInfo 注册 |
| 3 | `crates/config/src/lib.rs` | 无微信配置 | `ChannelsConfig.wechat: Option<WeChatChannelConfig>` (serde default 兼容) |
| 4 | `vendor/wechat-connector/` + `docs/WECHAT_CHANNEL.md` | 缺连接器与接入文档 | FastAPI 参考连接器骨架 + 契约/接入/风险文档 |

### v0.45.0 通用 Webhook 入站

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `crates/gateway/src/lib.rs` | 通用 Webhook 入站为空壳 | 新增 `POST /webhook` 与 `POST /webhook/{source}`: 兼容 text/content/message/msg/query/prompt 及 data.{text,content} 字段提取; 同步返回 Agent 响应; 带 `callback_url` 时转异步 (返回 202, 后台处理完 POST 回调)。gateway 新增 reqwest 依赖。 |

### v0.45.0 BOSS 短信登录改造

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `vendor/mcp-bosszp/server.py` | 二维码经飞书发图扫码不可用 (BOSS App 不能扫相册图) | 改为手机号+短信验证码两步登录 `boss.login_send_sms`/`boss.login_submit_sms`，登录页保活 (`get_persistent_login_page`)，遇滑块自动回退二维码 |
| 2 | `vendor/mcp-bosszp/server.py` | `params.get("cookievi ")` 拼写 → cookie 永远为 None；`_start_qr_auth_flow` 缩进错乱 | 修正为 `params.get("cookie")`，重写分发器并清理坏函数 |
| 3 | `crates/tool/src/reach.rs` | BOSS 工具仅支持扫码登录 | 新增 `login_send_sms`/`login_submit_sms` 动作，业务动作透传服务端 `auth_required(sms)` 由 Agent 编排；`wrap_boss_result` |
| 4 | `.env.example` `docs/BOSS_RECRUITMENT.md` `docs/architecture.md` | 文档为扫码登录 | 更新为手机号+短信验证码 (BOSS_PHONE)，二维码兜底 |

### v0.45.0 自我进化 + 视觉 + 通用 WS

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `crates/feishu/src/client.rs` | 令牌刷新 Bug | `CachedToken` 记录 `expires_at`，带 300s buffer 自动刷新 (已核实修复) |
| 2 | `crates/agent/src/builtin_tools.rs` | Memory/Skill 工具未挂载 | 实现 `MemoryTool`(add/replace/remove/list) 与 `SkillManageTool`(create/list/patch/delete)，经 `with_memory`/`with_skill_manager` 注册到 ToolRegistry |
| 3 | `crates/provider/src/lib.rs` | 视觉信息丢失 (`[Image: desc]` 占位) | `openai_content`/`anthropic_content` 将 `Content::Image` 转为真实多模态 (OpenAI image_url / Anthropic base64 source) |
| 4 | `crates/feishu/src/channel.rs` `client.rs` | 飞书入站图片降级为文本 | 新增 `download_message_resource` + `extract_image`，入站图片转 `Content::Image` |
| 5 | `crates/gateway/src/lib.rs` | 通用 WebSocket 入站缺失 | 新增 `GET /ws` 端点，每帧驱动一次 Agent 循环 |
| 6 | `README.md` `docs/architecture.md` `CLAUDE.md` | 文档夸大 (38+ 工具/多通道) | 纠正为实际 21 工具，如实标注通道状态 |

### v0.44.0 安全加固版

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `vendor/mcp-bosszp/server.py` | 浏览器初始化开销过大 | 实现 BrowserManager 单例模式 |
| 2 | `crates/gateway/src/lib.rs` | API 缺乏身份验证与 CORS 过于宽泛 | 添加 API Key 认证中间件 + 配置化 CORS |
| 3 | `crates/tool/src/reach.rs` | 潜在的命令注入风险 | 使用 tokio::process::Command + .arg() |
| 4 | `crates/tool/src/reach.rs` | BossCookie 处理逻辑缺陷 | 添加 None 检查 |
| 5 | `crates/tool/src/reach.rs` | 硬编码端口与超时 | ReachConfig 统一配置 |
| 6 | `auto_vibe.py` | 代理硬编码 | 改为 RUSTCLAW_PROXY 环境变量 |
| 7 | `crates/tool/src/reach.rs` | SSRF 防护 | 添加 URL 验证函数 |
| 8 | `crates/tool/src/lib.rs` | 技能系统沙箱 | SandboxExecutor + 命令黑名单 |
| 9 | `crates/tool/src/reach.rs` | 凭据泄露风险 | 敏感参数过滤 |
| 10 | `crates/config` + `crates/tool` | ReachConfig 不一致 | 统一结构体定义 |
| 11 | `crates/feishu/src/client.rs` | 图片上传类型错误 | 使用 "message" 而非文件后缀 |
| 12 | `vendor/mcp-bosszp/server.py` | 会话不连续导致登录失效 | 同一浏览器上下文 |
| 13 | `vendor/mcp-bosszp/server.py` | asyncio.sleep 等待不可靠 | 使用 wait_for_selector |
| 14 | `vendor/mcp-bosszp/server.py` | boss.apply_job 占位符 | 实现真实投递逻辑 |
| 15 | `vendor/mcp-bosszp/server.py` | 城市代码覆盖不足 | 扩展到 30 个城市 |
| 16 | `vendor/mcp-bosszp/server.py` | MCP 缺乏频率限制 | RateLimiter (30 req/min) |
| 17 | `crates/tool/src/reach.rs` | Cookie 解析逻辑简陋 | 改进解析逻辑 |
| 18 | `crates/feishu/src/channel.rs` | 富文本解析不全 | 支持 link/mention/image |
| 19 | `crates/feishu/src/channel.rs` | 文件下载无效消耗 | 持久化到文件 |
| 20 | `crates/feishu/src/websocket.rs` | 并发实现复杂 | 添加架构说明注释 |
| 21 | `crates/protocol/src/lib.rs` | 缺乏图片消息 | Content::Image 枚举 |
| 22 | `crates/gateway/src/lib.rs` | 图片描述切片无意义 | 使用 "图片消息" |
| 23 | `crates/compression/src/lib.rs` | 上下文压缩对图片负面影响 | 排除 Base64 长度 |

---

## 历史记录 (History)

- **Phase 17**: BOSS直聘智能投递助手 ✅
- **Phase 17.5**: 扫码登录全自动化 ✅
- **v0.44.0**: 安全加固版 (SSRF, Rate Limiting, API Auth, Browser Singleton)
