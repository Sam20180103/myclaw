# RustClaw

[![Version](https://img.shields.io/badge/version-v0.54.0-blue.svg)](https://github.com/Sam20180103/myclaw)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-254%20passed-green.svg)](https://github.com/Sam20180103/myclaw)
![License](https://img.shields.io/badge/license-MIT-yellow.svg)

RustClaw 是一个用 Rust 实现的本地 AI 智能体网关，架构理念参考 OpenClaw 和 Nous Research Hermes Agent。一个长驻网关接收多通道消息，路由给 Agent 运行时驱动 LLM 循环与工具调用，并持久化会话与记忆。

> **上线状态**: 见 [docs/PRODUCTION_READINESS.md](docs/PRODUCTION_READINESS.md) —— 实战阻塞项 (P0/P1/P2 共 17 项 + 3 项额外发现) 已全部修复并验证。

## 核心特性

### 接入与运行时
- **多通道接入**: HTTP REST (`/api/message`)、WebSocket (`/ws`)、Webhook (`/webhook`, 支持 `callback_url` 异步)、飞书长连接、个人微信 (经外部连接器 `/channel/wechat/inbound`)
- **入站幂等**: 平台重推 / Webhook 重试按事件 ID 判重 (SQLite `processed_events`)，不会重复执行发帖、投递等副作用动作
- **并发模型**: 同会话由 per-session `Mutex` 串行，跨会话并行 (每请求一个 tokio task)
- **可中止**: `/stop` (聊天) 或 `POST /api/session/:id/cancel`，与 LLM 调用、工具批次竞速取消，返回已完成的部分结果
- **三重预算**: 迭代上限、墙钟 deadline (`max_loop_duration_ms`)、用户取消 —— 任一触发都返回**部分结果**而非报错
- **进度反馈**: 工具执行期间按会话推送 "正在调用工具 X" (每会话 ≥5s 节流)

### 工具与技能
- **23 个原生工具**: 5 内置 (file_read/file_write/shell_exec/open_code/resume_read) + 15 Agent-Reach (含 `image_gen` 文生图) + `memory` / `skill_manage` / `spawn_subagent`
- **动态技能可调用 (v0.54.0)**: 批准后的 **Shell/Python** 技能自动注册为 `dyn_skill_*` 工具，经沙箱执行；重启自动重新上线，删除即注销。Rust/JS 仍为"可登记不可调用"
- **工具治理**: 黑名单拒绝 + `require_approval` 审批 (fail-closed)；`dyn_skill_*` **无条件需要人工批准**
- **按调用者授权**: `Envelope.principal` (飞书 open_id / 微信 wxid) 选择 `[tools.principals.<id>]` 策略覆盖
- **参数校验**: 调用前按工具 JSON Schema 校验必填与类型，失败以结构化错误回灌让模型自纠
- **并发工具调用**: 同一轮多个工具并发执行，结果按请求顺序回灌
- **沙箱**: 危险命令黑名单 + 30s 超时 + 内存上限；后端 `local` / `docker:<image>` (`RUSTCLAW_SANDBOX_BACKEND`)
- **子 Agent**: `spawn_subagent` 派生受限预算 (≤10 轮) 的子会话，最多 4 个并发

### 上下文与记忆
- **配对安全的压缩**: 旧工具结果只省略正文不删消息，截断后强制 `tool_use`/`tool_result` 配对 (孤儿会让 Provider 返回 400)
- **压缩带摘要**: 被丢弃的历史先经 LLM 摘要再插回队首，避免 Agent 失忆后重复执行副作用动作
- **token 预算**: 按 token 估算 (ASCII ≈ 4 字符/token, CJK ≈ 1)，窗口与阈值可配
- **三层记忆**: `MEMORY.md` / `USER.md` + SQLite 会话持久化 (WAL + FTS5 搜索)，含 Prompt 注入 / 数据泄露扫描
- **记忆后端抽象**: `MemoryProvider` trait，可替换为向量检索等实现

### Provider 与运维
- **多 Provider**: OpenAI、Anthropic、NVIDIA NIM、OpenAI Responses API (`openai-responses`)
- **重试与降级**: 可重试错误指数退避，耗尽后自动降级到下一个 Provider
- **Prompt 缓存**: Anthropic 把稳定的 system + tools 前缀标记 `cache_control`
- **用量与指标**: `GET /api/usage` (token 用量，按模型细分)、`GET /metrics` (Prometheus 文本)
- **Profile 隔离**: `RUSTCLAW_PROFILE=<name>` 隔离 config / skills / logs / 数据目录
- **定时任务**: `[[schedules]]` 按间隔驱动 Agent，结果与失败可推送到指定会话
- **轨迹导出**: `rustclaw sessions --export sharegpt` 直读 SQLite 导出 ShareGPT JSONL
- **密钥不落盘**: 配置里写 `env:VARNAME`，启动时从环境变量解析

### 视觉
`Content::Image` 经 OpenAI (`image_url`) / Anthropic (base64 source) 真正传给视觉模型；飞书入站图片自动下载转多模态；工具返回的可渲染图片 (如登录二维码) 直接回给用户。

## 风险提示

- ⚠️ 技能是 **LLM 生成的代码**。默认 `local` 后端会在宿主机上执行，生产请设 `RUSTCLAW_SANDBOX_BACKEND=docker:<image>`
- ⚠️ 个人微信自动化违反微信使用条款，有封号风险，请用小号、低频
- ⚠️ BOSS 直聘 / 小红书自动化受平台风控约束，选择器需按实际页面校准

## 系统架构

```
┌─────────────────────────────────────────────────────────────┐
│  Ingress: HTTP REST / WebSocket / Webhook / 飞书 / 个人微信   │
│           ↓ 事件判重 (processed_events)                      │
├─────────────────────────────────────────────────────────────┤
│  Gateway (控制平面): 连接管理, 会话路由, 认证, 审批登记        │
│           聊天命令 /clear /stop /approve /reject (先于会话锁) │
├─────────────────────────────────────────────────────────────┤
│  Agent Runtime: 循环 (迭代/墙钟/取消 三重预算)                │
│           工具策略 → 参数校验 → 并发执行 → 结果截断回灌        │
├─────────────────────────────────────────────────────────────┤
│  Capability: ToolRegistry (23 + dyn_skill_*), Provider 降级链 │
├─────────────────────────────────────────────────────────────┤
│  Memory/Compression: MEMORY.md, USER.md, 配对安全压缩 + 摘要  │
├─────────────────────────────────────────────────────────────┤
│  Data: SQLite (会话/事件判重), Config, Skills, Media, Logs    │
└─────────────────────────────────────────────────────────────┘
```

详见 [docs/architecture.md](docs/architecture.md)。

## 技术栈

| 用途 | 选型 |
|------|------|
| 异步运行时 | tokio |
| Web 框架 | axum 0.7 + tokio-tungstenite |
| CLI | clap |
| 配置 | TOML (serde) |
| 日志 | tracing |
| 数据库 | rusqlite (SQLite WAL + FTS5) |
| 错误处理 | thiserror (库) + anyhow (二进制边界) |

## 安装与运行

### 前置要求
- Rust 1.70+
- 可选: Docker (技能/shell 的隔离执行后端)
- 可选: Python 3 (Python 技能)

### 构建

```bash
git clone https://github.com/Sam20180103/myclaw.git
cd myclaw
cargo build --release
```

产出两个二进制:

| 二进制 | 说明 |
|--------|------|
| `rustclaw` | CLI 命令行工具 (crate `rustclaw-cli`) |
| `rustclaw-gateway` | Gateway 核心服务 |

```bash
./target/release/rustclaw start            # 启动网关
./target/release/rustclaw start --detach   # 后台启动
./target/release/rustclaw-gateway          # 或直接运行
```

默认监听 `127.0.0.1:18789`。首次运行会写入默认 `config.toml`。

## 配置

配置路径: `~/.config/rustclaw/config.toml` (`RUSTCLAW_PROFILE=<name>` 时为 `~/.config/rustclaw/profiles/<name>/config.toml`)。

```toml
[agent]
max_concurrent_sessions = 100
default_model = "claude-3-5-sonnet-20241022"
max_loop_iterations = 100
tool_timeout_ms = 60000
max_loop_duration_ms = 600000      # 单轮墙钟预算,超时返回部分结果
max_tool_result_chars = 16000      # 单条工具结果上限 (保头保尾截断)
context_window_tokens = 128000     # 压缩预算基准 (token, 非字节)
compression_threshold = 0.6        # 超过 window*threshold 触发压缩

[tools]
disabled_tools = []                # 黑名单 (dyn_skill_* 也可在此禁用)
require_approval = ["shell_exec"]  # 需审批 (dyn_skill_* 恒需审批,无法豁免)
sandbox_enabled = true

[tools.principals.ou_xxx]          # 按调用者的策略覆盖 (可选)
allowed_tools = ["file_read"]
require_approval = ["file_read"]

[[schedules]]                      # 定时任务 (可选)
name = "morning-hot"
every = "12h"                      # 30s/10m/2h/1d, 最小 10s
prompt = "查看小红书热门 Top10 并总结"
notify_session = "p2p:oc_xxx:ou_yyy"   # 结果/失败推送目标 (不配则只写日志)
notify_channel = "feishu"
```

环境变量见 [.env.example](.env.example)。要点:

| 变量 | 说明 |
|------|------|
| `RUSTCLAW_HOST` / `RUSTCLAW_PORT` / `RUSTCLAW_LOG_LEVEL` | 覆盖网关配置 |
| `RUSTCLAW_PROFILE` | 多角色隔离 (config/skills/logs/data 全部分离) |
| `RUSTCLAW_SANDBOX_BACKEND` | `local` \| `docker:IMAGE` \| `docker+net:IMAGE` |
| `RUST_LOG` | tracing 级别 |

配置里任意密钥可写成 `env:VARNAME`，启动时从环境变量解析，避免明文落盘。

## 聊天内控制命令

飞书 / 微信会话中直接发送 (这些命令在会话锁**之前**处理，正在跑的任务不会挡住它们):

| 命令 | 作用 |
|------|------|
| `/clear` | 清空当前会话上下文 |
| `/stop` 或 `/cancel` | 中止正在运行的任务，返回已完成的部分结果 |
| `/approve <审批ID>` | 批准一次工具调用 |
| `/reject <审批ID>` | 拒绝一次工具调用 |

## API 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | 健康检查 (免鉴权) |
| GET | `/metrics` | Prometheus 指标 (循环/迭代/工具/失败/provider 错误/压缩 + token) |
| GET | `/ws` | 通用 WebSocket 入站,每帧驱动一次 Agent 循环 |
| POST | `/webhook` \| `/webhook/:source` | 通用 Webhook 入站 (同步 / `callback_url` 异步),带 `event_id` 时判重 |
| POST | `/channel/wechat/inbound` | 个人微信连接器入站 |
| POST | `/api/message` | REST 消息入口 (JSON-RPC 信封) |
| GET/DELETE | `/api/session[/:id]` | 会话列表 / 详情 / 删除 |
| POST | `/api/session/:id/cancel` | 中止指定会话正在运行的任务 |
| GET | `/api/usage` | token 用量 (总量 + 按模型) |
| GET | `/api/tools` | **实际可调用**的工具表 (含 `is_skill`/`available`/`allowed`/`requires_approval`) |
| GET/POST | `/api/tools/approvals[/:id]` | 待审批工具调用 / 裁决 |
| GET/POST | `/api/skill` | 技能列表 (active + pending) / 创建 |
| POST | `/api/skill/:id/approve` \| `/reject` | 技能审批 (批准即注册为可调用工具) |
| GET | `/api/skill/approval/subscribe` | 技能审批事件流 (SSE) |
| GET/POST | `/api/config[/save|/model]` | 配置读写 |
| GET/POST | `/api/channel[/:name]` | 通道管理 |
| GET | `/api/agent[/:id]` | Agent 会话视图 |
| GET | `/api/memory/status` \| `/stats` | 记忆状态 |
| GET | `/api/security/audit` | 安全审计 |
| GET | `/api/system/version` \| `/update/check` | 版本 / 更新检查 |

> 路由使用 axum 0.7 的 `:param` 语法。配置了 `[gateway.auth] api_key` 时,除 `/health` 外均需 `Authorization: Bearer <key>`。

## CLI 命令

```bash
rustclaw start|stop|restart|status|health|doctor
rustclaw config --show | --path | --get KEY | --set KEY=VALUE
rustclaw sessions --list | --export sharegpt [--session KEY] [--output FILE]
rustclaw skills --list | info --name X
rustclaw tools list
rustclaw models list | status | set
rustclaw providers list
rustclaw memory | logs | security | system info
rustclaw open-code run --prompt "生成 Hello World"
```

完整命令表见 `rustclaw --help`。

## 技能工作流 (v0.54.0 闭环)

```
Agent 生成 (可经 OpenCode CLI)
   → <skills>/pending/<id>.json
   → 用户批准 (POST /api/skill/:id/approve)
   → <skills>/active/<id>.json + 注册为工具 dyn_skill_<name>
   → 每次调用仍需人工批准 (/approve <id>)
   → 经 SandboxExecutor 执行 (黑名单 + 超时 + 内存上限)
```

- 仅 **Shell / Python** 可调用；Rust / JavaScript 可登记但不注册为工具
- 参数以单个 JSON 实参传入 (`$1` / `sys.argv[1]`)，源码与参数均做 POSIX 单引号转义
- `GET /api/skill` 看"已登记"，`GET /api/tools` 看"真正可调用"

## 开发

```bash
cargo build --release            # 构建全部
cargo test                       # 全量测试
cargo test -p rustclaw-agent     # 单 crate
cargo clippy --all-targets       # lint
```

### 测试统计 (254)

| 模块 | 测试数 | 模块 | 测试数 |
|------|--------|------|--------|
| tool | 55 | storage | 16 |
| agent | 28 | feishu | 16 |
| provider | 28 | config | 15 |
| skill | 24 | compression | 15 |
| memory | 13 | channel | 11 |
| gateway | 9 | protocol | 9 |
| wechat | 9 | integration | 6 |

## 项目结构

```
myclaw/
├── crates/
│   ├── gateway/        # axum HTTP/WS 服务器 + REST API + 通道事件循环
│   ├── agent/          # Agent 运行时 (会话/循环/预算/取消) + metrics
│   ├── tool/           # ToolRegistry / SandboxExecutor / ToolPolicy / Agent-Reach
│   ├── skill/          # 动态技能 (生成/校验/审批) + SkillTool (可调用封装)
│   ├── provider/       # LLM 客户端 (OpenAI/Anthropic/NVIDIA/Responses) + 用量统计
│   ├── memory/         # 三层记忆 + 安全扫描 (MemoryProvider trait)
│   ├── compression/    # 上下文压缩 (配对安全 + 摘要)
│   ├── storage/        # SQLite 会话持久化 + 事件判重
│   ├── channel/        # 通道抽象
│   ├── feishu/         # 飞书通道 (长连接 + 交互卡片)
│   ├── wechat/         # 个人微信通道 (外部连接器契约)
│   ├── protocol/       # JSON-RPC 信封与消息类型
│   ├── config/         # TOML 配置 + profile 路径解析
│   └── cli/            # CLI (二进制名 rustclaw)
├── docs/
│   ├── architecture.md          # 技术架构 (权威设计文档)
│   ├── PRODUCTION_READINESS.md  # 实战上线任务清单与验证记录
│   ├── roadmap.md               # 阶段进度
│   ├── XHS_GUIDE.md             # 小红书接入
│   ├── BOSS_RECRUITMENT.md      # BOSS 直聘招聘 Agent
│   └── WECHAT_CHANNEL.md        # 个人微信通道契约
├── vendor/             # 外部 MCP 服务器与连接器 (不参与 Cargo 构建)
└── Cargo.toml          # workspace root
```

## 安全设计

- 本地优先绑定 (127.0.0.1) + API Key 认证 + 可配置 CORS
- 工具策略: 黑名单 / 白名单 / 强制审批 (fail-closed)，支持按调用者覆盖
- 生成代码隔离: `dyn_skill_*` 恒需人工批准 + 沙箱执行 + 可选 Docker 后端 (`--network none`, `no-new-privileges`)
- 文件工具路径穿越防护、SSRF 校验、敏感参数过滤
- 记忆安全扫描 (Prompt 注入 / 数据泄露 / 不可见字符)
- 密钥 env 间接引用,不落 config.toml 明文

## 开发路线图

| Phase | 名称 | 状态 |
|-------|------|------|
| Phase 1-5 | 核心框架 / Agent 运行时 / 工具 / 动态 Skill / 测试 | ✅ |
| Phase 6-10 | OpenCode 集成 + CLI 完善 | ✅ |
| Phase 11-14 | Agent-Reach 工具 / 社交平台 / MCP 桥接 / 高级平台 | ✅ |
| Phase 15-16 | 记忆系统与自我进化 / 飞书通道 | ✅ |
| Phase 17 | 安全加固 + 扫码登录自动化 | ✅ |
| Phase 18 | 框架补齐 G1-G12 (工具治理/缓存/降级/网关/子 Agent 等) | ✅ |
| Phase 19 | 实战上线加固 P0-P2 (压缩配对/幂等/取消/预算/可观测) | ✅ |
| Phase 20 | 技能执行闭环 (Shell/Python) | ✅ |
| 后续 | JavaScript 技能、技能级资源配额、多租户 | 规划中 |

## 许可证

MIT。(仓库中尚未放置 `LICENSE` 文件,建议补上。)

## 联系

- 项目主页: [https://github.com/Sam20180103/myclaw](https://github.com/Sam20180103/myclaw)
- 问题反馈: [Issues](https://github.com/Sam20180103/myclaw/issues)
