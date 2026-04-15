# RustClaw 项目问题与进展记录 (Issues & Progress)

## 2026-04-16 更新: 核心功能代码审查与安全加固建议

### 1. 代码审查发现的问题 (Code Review Findings) 🔍

| # | 模块/文件 | 问题描述 | 建议方案 | 严重程度 |
|---|---|---|---|---|
| 1 | `vendor/mcp-bosszp/server.py` | **浏览器初始化开销过大**: 每次 MCP 请求都会启动/关闭一个新的 Playwright 浏览器实例。 | 实现浏览器实例池或单例模式，复用 BrowserContext。 | 🔴 高 |
| 2 | `crates/gateway/src/lib.rs` | **API 缺乏身份验证与 CORS 过于宽泛**: 所有 `/api/` 接口均无鉴权，且允许 `Any` Origin。 | 引入 API Key 校验中间件，收紧 CORS 策略。 | 🔴 高 |
| 3 | `crates/tool/src/reach.rs` | **潜在的命令注入风险**: `GitHubReachTool` 和 `VideoReachTool` 直接将参数拼接到命令行或 API URL。 | 使用更严格的参数校验和 URL 编码，避免 shell 字符注入。 | 🟡 中 |
| 4 | `crates/tool/src/reach.rs` | **BossCookie 处理逻辑缺陷**: 当 `boss_cookie` 为 `None` 时，Rust 会向 Python 发送 `null`，导致 Python 端解析错误。 | 在 Rust 端增加 `None` 检查，或在 Python 端处理 `None` 输入。 | 🟡 中 |
| 5 | `crates/agent/src/lib.rs` | **工具调用解析逻辑过于复杂**: `parse_tool_calls` 使用大量正则和手动字符串搜索，易出错且存在 ReDoS 风险。 | 规范化 LLM 输出格式，优先使用结构化 JSON 解析，简化 fallback 逻辑。 | 🟡 中 |
| 6 | `crates/tool/src/reach.rs` | **硬编码端口与超时**: MCP 客户端硬编码了 8080-8084 端口，且超时时间在多个地方重复定义。 | 将端口和超时时间统一移至 `ReachConfig` 配置项中。 | 🟢 低 |
| 7 | `auto_vibe.py` | **硬编码代理配置**: 脚本中强制设置了 `127.0.0.1:8118` 代理。 | 改为从环境变量读取或在配置文件中定义。 | 🟢 低 |

### 2. 安全加固建议 (Security Recommendations) 🛡️
- **SSRF 防护**: `WebReachTool` 应验证目标 URL，防止通过 `web_endpoint` 访问内网资源。
- **技能系统沙箱**: 确保 `SkillManager` 运行在隔离的环境中，限制其文件系统和网络访问权限。
- **凭据保护**: 严禁将 `boss_cookie` 或其他 API Key 打印到日志中（目前 `McpClient` 可能在报错时泄露参数）。

---

## 2026-04-15 更新: BOSS直聘功能闭环与系统加固

### 1. 已完成的功能 (Features Completed) ✅

#### **Phase 17: BOSS直聘智能投递助手**
- **工具链增强**: 
    - 扩展 `BossZhipinReachTool` 支持 `apply` (投递简历) 和 `check_auth` (登录态检查) 操作。
    - 所有请求已实现透传 `boss_cookie` 给 MCP 桥接层。
- **简历解析工具**: 
    - 实现了 `ResumeReadTool` (`resume_read`)，专门用于结构化读取本地 Markdown 简历文件。
- **MCP 桥接服务器 (Python 实现)**:
    - 由于外部缺乏现成实现，自主开发了基于 **FastAPI + Playwright** 的桥接服务器 (`vendor/mcp-bosszp/server.py`)。
    - 支持自动化搜索、JD 抓取和登录态验证。
- **认证管理集成**:
    - 在 `rustclaw-config` 中增加了 `tools.reach.boss_cookie` 配置字段。
    - 扩展了 CLI `tools config boss_zhipin_reach --set cookie="..."` 命令支持。

### 2. 已修复的问题 (Fixed Issues) 🛠️

| # | 问题描述 | 修复方案 | 严重程度 |
|---|---------|---------|----------|
| 1 | `rustclaw tools` 缺少 `config` 子命令 | 在 CLI 模块中实现了 `ToolsSubcommands::Config` 处理器 | 🟡 中 |
| 2 | 新增配置字段导致旧 `config.toml` 加载失败 | 为 `ToolConfig` 及其子结构添加了 `#[serde(default)]` 属性 | 🔴 高 |
| 3 | `rustclaw-gateway` 编译报错: 缺少 `rustclaw-tool` | 在 `gateway/Cargo.toml` 中补齐了 path 依赖 | 🔴 高 |
| 4 | Agent 发生错误时飞书无任何反馈 (静默失败) | 在网关的 Feishu 处理循环中添加了异步错误回传逻辑 | 🔴 高 |
| 5 | `rustclaw_protocol::Message` 缺少构造函数 | 改为直接使用结构体字段初始化，修复了编译错误 | 🟢 低 |
| 6 | 异步任务中变量所有权转移导致无法处理错误 | 预先 `clone()` 关键变量（如 `session_key`），修复了借用检查错误 | 🟡 中 |

### 3. 当前剩余/新规划任务 (Pending & New Plans) ⏳

#### **Phase 17.5: 扫码登录全自动化 (极高优先级)**
- **目标**: 彻底解决 Cookie 过期需要手动更新的痛点。
- **计划**:
    - [ ] MCP Server 支持捕获二维码截图并转为 Base64。
    - [ ] `rustclaw-protocol` 扩展 `Image` 消息类型支持。
    - [ ] 飞书通道集成图片上传 (`im/v1/images`)。
    - [ ] Agent 链路自愈逻辑：检测到失效 -> 弹出二维码 -> 自动等待登录 -> 继续任务。

### 4. 环境与配置快照 (Current Env)
- **Gateway**: v0.1.0 (已打通依赖注入)
- **BOSS MCP**: 127.0.0.1:8083 (Python + Playwright)
- **简历路径**: `/usr/local/project/lab/myclaw/张传奇简历及职业信息总结.md`

## 2026-04-16 代码审查发现的问题 (Review Findings) 🔍

### 1. 飞书通道集成 (Feishu Channel)
- **令牌刷新 Bug**: `FeishuClient::get_tenant_token` 缓存了 `tenant_access_token` 但未记录过期时间，也未处理 Token 失效后的重新获取逻辑。这将导致长时间运行后 API 调用失败。
- **富文本解析不全**: `extract_post_text` 仅提取了 `text` 类型元素，忽略了富文本消息中的链接、@提及和图片，导致 AI 丢失关键上下文。
- **文件下载无效消耗**: `download_file` 仅将文件下载到内存后返回确认字符串，未进行持久化或传递给 AI 视觉模块，造成带宽和内存的无效消耗。
- **并发实现复杂**: `WebSocketManager` 使用 `std::thread::spawn` 嵌套 `tokio` 运行时的做法较为繁琐，建议直接使用 `tokio::spawn` 并复用主运行时。

### 2. BOSS直聘 MCP 桥接器 (Boss MCP Bridge)
- **性能严重瓶颈**: 每次 MCP 请求（搜索、获取 JD、检查状态）都会启动并销毁一个新的 Playwright 浏览器实例。建议改为单例模式或浏览器池，以减少延迟和系统负载。
- **安全性隐患**: MCP 接口接收明文 Cookie 且缺乏内部认证机制。若该服务暴露在公网或非受信网络中，可能导致用户凭据泄露。
- **解析逻辑脆弱**: `search_jobs` 中通过 `href.split('/')` 提取 Job ID 的逻辑依赖于特定 URL 结构，若 BOSS 直聘前端变动则会失效。

### 3. 自动化任务 (Auto Vibe)
- **代理硬编码**: `auto_vibe.py` 中硬编码了 `127.0.0.1:8118` 代理，缺乏环境变量或配置文件的灵活性支持。

---

## 历史记录 (2026-04-12 及之前)
*(此处保留之前的联网搜索能力测试报告内容...)*
