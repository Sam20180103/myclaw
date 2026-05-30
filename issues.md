# RustClaw 项目问题与进展记录 (Issues & Progress)

## 2026-05-30 更新: 待修复问题 (Pending) 🔍

### 1. Agent 自我进化闭环缺失

| # | 问题描述 | 建议方案 | 严重程度 |
|---|---------|---------|----------|
| 1 | **Memory/Skill 工具未挂载**: `MemoryManager`(add/replace/remove)与 `SkillManager`(create) 仅有方法，未实现 `Tool` trait 也未注册到 `ToolRegistry`，Agent 无法主动管理记忆或创建技能。 | 实现 `MemoryTool`/`SkillManageTool` 并在 Agent 构造时注册。 | 🔴 高 |

### 2. 多通道入站

| # | 问题描述 | 建议方案 | 严重程度 |
|---|---------|---------|----------|
| 1 | **通用 WS/Webhook 占位**: `crates/channel` 的 WebSocket/HTTP/Webhook 通道为空壳实现，实际入站仅 HTTP REST 与飞书 WebSocket。 | 实现通用入站或更新文档表述。 | 🟡 中 |

---

## 已完成的修复 (Completed) ✅

### v0.45.0 (进行中)

| # | 模块/文件 | 问题描述 | 修复方案 |
|---|---|---|---|
| 1 | `crates/feishu/src/client.rs` | 令牌刷新 Bug | `CachedToken` 记录 `expires_at`，带 300s buffer 自动刷新 (已核实修复) |

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
