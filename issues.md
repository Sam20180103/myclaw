# RustClaw 项目问题与进展记录 (Issues & Progress)

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

---

## 历史记录 (2026-04-12 及之前)
*(此处保留之前的联网搜索能力测试报告内容...)*
