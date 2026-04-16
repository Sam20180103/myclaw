# RustClaw 项目问题与进展记录 (Issues & Progress)

## 2026-04-16 更新: 待修复问题 (Pending) 🔍

### 1. 飞书通道集成 (Feishu Channel)

| # | 问题描述 | 建议方案 | 严重程度 |
|---|---------|---------|----------|
| 1 | **令牌刷新 Bug**: `FeishuClient::get_tenant_token` 缓存了 `tenant_access_token` 但未记录过期时间，也未处理 Token 失效后的重新获取逻辑。 | 添加过期时间记录和自动刷新逻辑。 | 🔴 高 |

### 2. Agent 运行时与 Provider

| # | 问题描述 | 建议方案 | 严重程度 |
|---|---------|---------|----------|
| 1 | **LLM 视觉信息丢失**: 所有 LlmProvider 实现均将 `Content::Image` 简化为文本占位符 `[Image: desc]`。AI 无法理解图片内容。 | 实现真正的视觉理解能力，或使用 vision-capable 模型。 | 🔴 高 |

---

## 已完成的修复 (Completed) ✅

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
