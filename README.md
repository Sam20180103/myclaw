# RustClaw

[![Version](https://img.shields.io/badge/version-v0.43.0-blue.svg)](https://github.com/Sam20180103/myclaw)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-171%20passed-green.svg)](https://github.com/Sam20180103/myclaw)
[![License](https://img.shields.io/badge/license-MIT-yellow.svg)](LICENSE)

RustClaw 是一个用 Rust 实现的本地 AI 智能体网关，架构理念参考 OpenClaw 和 Nous Research Hermes Agent，使用 Rust 实现以获得更高的性能和安全性。

## 核心特性

### 基础架构
- **多通道接入**: HTTP REST、WebSocket (`/ws`)、Webhook (`/webhook`, 支持异步回调)、飞书长连接、个人微信 (经连接器, `/channel/wechat/inbound`)
- **智能体运行时**: 多会话并发、Agent 循环、流式响应
- **工具系统**: 21 个原生 Rust 工具 (5 内置 + 14 Agent-Reach + memory/skill_manage)，含路径安全防护
- **自我进化工具**: `memory` (增删改持久记忆) 与 `skill_manage` (创建/查询/修改/删除 Skill) 直接暴露给 Agent
- **动态 Skill**: Agent 可以自己编写并注册新 Skill
- **审批流程**: 用户审批后 Skill 才可使用
- **Provider 抽象**: 支持 OpenAI、Anthropic、NVIDIA NIM 等多种 LLM
- **视觉理解**: `Content::Image` 经 OpenAI (image_url) / Anthropic (base64 source) 真正传给视觉模型；飞书入站图片自动下载并转多模态

### 记忆系统 (Active v0.43.0)
- **三层 memory 架构**: 即时记忆 + 持久记忆 + 会话搜索
- **MEMORY.md / USER.md**: Agent 笔记 + 用户偏好档案 (2200/1375 chars)
- **Memory 工具**: Agent 自主管理记忆 (add/replace/remove/list)，已注册到 ToolRegistry 可直接调用
- **安全扫描**: Prompt 注入检测、数据泄露防护、不可见字符检测
- **SQLite 持久化**: WAL 模式，支持会话全量持久化、搜索和谱系追踪 (v0.43.0 激活)

### 自我进化 (Learning Loop)
- **Skill 自动创建**: 复杂任务 (5+ tool calls) 后自动建议创建 Skill (已激活回调)
- **SkillManage 工具**: create/list/patch/delete (已注册到 ToolRegistry，创建走用户审批流程)
- **用户纠正记录**: 记录用户纠正行为，持续改进
- **迭代预算**: 父子 Agent 共享预算，防止资源耗尽

### 上下文管理 (Active v0.43.0)
- **双级压缩**: Gateway 层 85% 安全网 + Agent 层 50% 精确压缩 (v0.43.0 激活)
- **Prompt 缓存**: Anthropic cache_control 策略，降低 75% 输入成本
- **回调系统**: tool_progress/step/status/thinking/SkillSuggestion 全事件回调


### Agent-Reach 工具集成
- **Web/视频**: WebReach (Jina Reader), VideoReach (yt-dlp)
- **开发平台**: GitHubReach (gh CLI), SearchReach (Exa 语义搜索)
- **社交/资讯**: TwitterReach, RedditReach, WeiboReach, FeedReach (RSS)
- **中文平台**: XhsReach (小红书), DouyinReach (抖音)
- **高级平台**: LinkedInReach, WeChatReach (公众号), BossZhipinReach (Boss直聘), PodcastReach (小宇宙)

### 飞书通道 (Phase 16 规划中)
- **消息收发**: 纯文本、富文本、图片、文件
- **交互卡片**: 实时状态更新 (Thinking/Generating/Complete)
- **流式响应**: 卡片内实时更新文本
- **权限策略**: DM 白名单、群聊授权、敏感操作确认
- **WebSocket 长连接**: 自动重连、心跳保活

### 个人微信通道 (v0.46.0)
- **连接器无关**: 经稳定 HTTP 契约对接任意微信框架 (wcferry/wechaty/...)，协议层解耦
- **消息收发**: 私聊/群聊文本与图片 (图片走视觉模型)
- **权限策略**: DM/群白名单、群聊默认仅 @机器人 响应 (防刷屏)
- **接入**: 详见 [docs/WECHAT_CHANNEL.md](docs/WECHAT_CHANNEL.md) 与参考连接器 `vendor/wechat-connector/`
- ⚠️ 个人微信自动化违反微信使用条款，有封号风险，请用小号、低频

## 系统架构

RustClaw 采用分层架构设计：

```
┌─────────────────────────────────────────────────────────┐
│  Ingress: HTTP REST, WebSocket, Webhook, 飞书, 个人微信 │
├─────────────────────────────────────────────────────────┤
│  Gateway (控制平面): 连接管理, 会话路由, 认证授权        │
├─────────────────────────────────────────────────────────┤
│  Execution Plane: Agent Runtime, Tool Executor, Skill   │
├─────────────────────────────────────────────────────────┤
│  Capability Layer: Tool Registry (21), Provider Mgr     │
├─────────────────────────────────────────────────────────┤
│  Memory Layer: MEMORY.md, USER.md, SQLite Session Store │
├─────────────────────────────────────────────────────────┤
│  Data Layer: Sessions, Config, Media, Logs              │
└─────────────────────────────────────────────────────────┘
```

## 技术栈

- **异步运行时**: tokio
- **Web 框架**: axum + tokio-tungstenite
- **CLI 框架**: clap
- **配置格式**: TOML
- **日志系统**: tracing
- **序列化**: serde_json
- **数据库**: rusqlite (SQLite WAL + 全文搜索)
- **错误处理**: thiserror + anyhow

## 安装

### 前置要求

- Rust 1.70+
- Cargo

### 构建

```bash
git clone https://github.com/Sam20180103/myclaw.git
cd myclaw
cargo build --release
```

### 运行

RustClaw 由两个二进制文件组成：

| 二进制 | 说明 |
|--------|------|
| `rustclaw` | CLI 命令行工具 |
| `rustclaw-gateway` | Gateway 核心服务 |

```bash
# 构建
cargo build --release

# 启动 Gateway (通过CLI)
./target/release/rustclaw start

# 或直接运行 Gateway
./target/release/rustclaw-gateway
```

默认端口: `127.0.0.1:18789`

## CLI 命令

### 服务管理

```bash
# 查看帮助
rustclaw --help

# 启动/停止/重启网关
rustclaw start              # 启动 Gateway
rustclaw start --detach    # 后台启动
rustclaw start --port 8080 # 指定端口
rustclaw stop               # 停止 Gateway
rustclaw restart            # 重启 Gateway

# 状态检查
rustclaw status            # 查看运行状态
rustclaw health            # 健康检查
rustclaw doctor            # 诊断检查
```

### 配置管理

```bash
rustclaw config --show    # 显示配置
rustclaw config --path    # 显示配置路径
rustclaw config --get KEY # 获取配置项
rustclaw config --set KEY=VALUE # 设置配置项
```

### 常用命令

```bash
# 服务管理
rustclaw start/stop/restart
rustclaw status
rustclaw health

# 系统信息
rustclaw system info      # 系统信息
rustclaw system event    # 查看事件
rustclaw logs            # 日志管理
rustclaw doctor          # 诊断工具

# 配置管理
rustclaw config --show  # 显示配置
rustclaw config --path  # 配置路径

# 模型管理
rustclaw models list    # 列出可用模型
rustclaw models status  # 模型状态
rustclaw models set    # 设置默认模型

# 提供商管理
rustclaw providers list # 列出提供商

# 工具管理
rustclaw tools list    # 列出工具

# OpenCode集成
rustclaw open-code run --prompt "生成Hello World程序"
rustclaw open-code sessions list
```

### 完整命令列表

| 命令 | 说明 |
|------|------|
| `start` | 启动网关服务器 |
| `stop` | 停止网关服务器 |
| `restart` | 重启网关服务器 |
| `status` | 状态概览 |
| `health` | 健康检查 |
| `doctor` | 诊断工具 |
| `config` | 配置管理 |
| `models` | 模型管理 |
| `providers` | 提供商管理 |
| `tools` | 工具管理 |
| `skills` | Skill管理 |
| `sessions` | 会话管理 |
| `agent` | Agent管理 |
| `gateway` | 网关管理 |
| `channels` | 通道管理 |
| `system` | 系统信息 |
| `logs` | 日志管理 |
| `security` | 安全审计 |
| `memory` | 内存管理 |
| `open-code` | OpenCode集成 |
| `doctor` | 诊断修复 |
| `update` | 更新版本 |
| `reset` | 重置数据 |

更多信息请查看 [架构文档](docs/architecture.md)。

## 开发路线图

- **Phase 1**: 核心框架 ✅
- **Phase 2**: Agent 运行时 ✅
- **Phase 3**: 工具系统 ✅
- **Phase 4**: 动态 Skill ✅
- **Phase 5**: 测试与优化 ✅
- **Phase 6**: OpenCode工具集成 ✅
- **Phase 7-10**: CLI完善 ✅
- **Phase 11**: Agent-Reach 核心工具集成 ✅
- **Phase 12**: 社交平台工具集成 ✅
- **Phase 12.5**: 速率限制 ✅
- **Phase 13**: MCP 桥接集成 ✅
- **Phase 14**: 高级平台工具集成 ✅
- **Phase 15**: 记忆系统与自我进化 ✅
- **Phase 15.5**: 代码质量加固 ✅
- **Phase 16**: 飞书通道集成 ✅

## 测试统计

| 模块 | 测试数 | 状态 |
|------|--------|------|
| config | 7 | ✅ |
| protocol | 9 | ✅ |
| skill | 9 | ✅ |
| tool | 39 | ✅ |
| agent | 12 | ✅ |
| provider | 16 | ✅ |
| gateway | 6 | ✅ |
| channel | 11 | ✅ |
| storage | 12 | ✅ |
| memory | 12 | ✅ |
| compression | 10 | ✅ |
| integration | 6 | ✅ |
| **总计** | **171** | **✅** |

## 安全设计

- 本地优先绑定 (127.0.0.1)
- 设备配对授权
- 工具策略 (白名单/黑名单)
- 沙箱隔离执行
- 资源限制 (超时、内存)
- 文件工具路径穿越防护
- 记忆安全扫描 (Prompt 注入/数据泄露检测)

## 项目结构

```
myclaw/
├── crates/
│   ├── gateway/        # HTTP/WS服务器 + API
│   ├── agent/          # Agent运行时 (Session/Lane/Queue)
│   ├── tool/           # 工具系统 (Registry/Sandbox/Policy/Reach)
│   ├── skill/          # 动态Skill系统 + SkillManage
│   ├── provider/       # LLM提供商 (OpenAI/Anthropic/NVIDIA)
│   ├── protocol/       # JSON-RPC协议
│   ├── config/         # 配置管理
│   ├── channel/        # 通道抽象
│   ├── storage/        # SQLite数据存储
│   ├── memory/         # 记忆系统 (MEMORY.md/USER.md/安全扫描)
│   ├── compression/    # 上下文压缩 (SessionHygiene/ContextCompressor)
│   └── cli/            # CLI工具
├── docs/
│   ├── architecture.md # 技术架构文档
│   └── roadmap.md      # 开发任务进度
└── Cargo.toml          # workspace root
```

## 贡献

欢迎贡献！请查看 [CONTRIBUTING.md](CONTRIBUTING.md) 了解详情。

## 许可证

本项目采用 MIT 许可证。详见 [LICENSE](LICENSE) 文件。

## 联系

- 项目主页: [https://github.com/Sam20180103/myclaw](https://github.com/Sam20180103/myclaw)
- 问题反馈: [Issues](https://github.com/Sam20180103/myclaw/issues)