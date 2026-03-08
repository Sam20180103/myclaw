# RustClaw

[![Version](https://img.shields.io/badge/version-v0.3.0-blue.svg)](https://github.com/your-repo/rustclaw)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org/)

RustClaw 是一个用 Rust 实现的本地 AI 智能体网关，架构理念参考 OpenClaw，但使用 Rust 实现以获得更高的性能和安全性。

## 核心特性

- **多通道接入**: 支持 WebSocket、HTTP、Webhook 等消息入口
- **智能体运行时**: 多会话并发、Agent 循环、流式响应
- **工具系统**: 原生 Rust 工具注册
- **动态 Skill**: Agent 可以自己编写并注册新 Skill
- **审批流程**: 用户审批后 Skill 才可使用
- **Provider 抽象**: 支持多种 LLM 提供商
- **安全沙箱**: 进程隔离、权限控制、工具策略

## 系统架构

RustClaw 采用分层架构设计：

1. **Ingress (入口层)**: WebSocket、HTTP、Telegram、Webhook 等通道
2. **Gateway (控制平面)**: 连接管理、会话路由、认证授权、事件广播
3. **Execution Plane (执行层)**: Agent Runtime、Tool Executor、Dynamic Skill System
4. **Capability Layer (能力层)**: Tool Registry、Provider Manager
5. **Data Layer**: Sessions、Config、Media、Logs

## 技术栈

- **异步运行时**: tokio
- **Web 框架**: axum + tokio-tungstenite
- **CLI 框架**: clap
- **配置格式**: TOML
- **日志系统**: tracing
- **序列化**: serde_json

## 安装

### 前置要求

- Rust 1.70+
- Cargo

### 构建

```bash
git clone https://github.com/your-repo/rustclaw.git
cd rustclaw
cargo build --release
```

### 运行

```bash
cargo run -p rustclaw-cli -- start
```

默认端口: `127.0.0.1:18789`

## CLI 命令

### 基本命令

```bash
# 查看帮助
rustclaw --help

# 启动/停止/重启网关
rustclaw start
rustclaw stop
rustclaw restart

# 指定端口
rustclaw start --port 8080

# 配置管理
rustclaw config --show
rustclaw config --path

# 版本信息
rustclaw version
```

### 常用命令

```bash
# 状态检查
rustclaw status
rustclaw status --detailed
rustclaw health
rustclaw health --verbose

# 会话管理
rustclaw sessions --list
rustclaw sessions --kill <session_id>
rustclaw sessions --clear

# 模型管理
rustclaw models --list
rustclaw models --set anthropic/claude-sonnet

# Skill管理
rustclaw skills --list
rustclaw skills --info <skill_name>

# OpenCode集成
rustclaw open-code --run "生成Hello World程序"
rustclaw open-code sessions --list
```

### 完整命令列表

| 命令 | 说明 |
|------|------|
| `start` | 启动网关服务器 |
| `config` | 配置管理 |
| `sessions` | 会话管理 |
| `agent` | Agent管理 |
| `gateway` | 网关管理 |
| `channels` | 通道管理 |
| `skills` | Skill管理 |
| `models` | 模型管理 |
| `system` | 系统信息 |
| `logs` | 日志管理 |
| `health` | 健康检查 |
| `status` | 状态概览 |
| `security` | 安全审计 |
| `providers` | 提供商管理 |
| `tools` | 工具管理 |
| `memory` | 内存管理 |
| `open-code` | OpenCode集成 |
| `doctor` | 诊断修复 |
| `update` | 更新版本 |
| `reset` | 重置数据 |

更多信息请查看 [CLI 命令参考](docs/architecture.md)。

## 开发路线图

- **Phase 1**: 核心框架 (项目初始化、配置系统、Gateway WebSocket 服务器)
- **Phase 2**: Agent 运行时 (Session 管理、Agent 循环逻辑)
- **Phase 3**: 工具系统 (Tool trait、注册表、沙箱执行器)
- **Phase 4**: 动态 Skill (Skill 生成、验证、审批流程)
- **Phase 5**: 测试与优化
- **Phase 6**: OpenCode工具集成
- **Phase 7**: CLI完善 (当前)

## 安全设计

- 本地优先绑定 (127.0.0.1)
- 设备配对授权
- 工具策略 (白名单/黑名单)
- 沙箱隔离执行
- 资源限制 (超时、内存)

## 贡献

欢迎贡献！请查看 [CONTRIBUTING.md](CONTRIBUTING.md) 了解详情。

## 许可证

本项目采用 MIT 许可证。详见 [LICENSE](LICENSE) 文件。

## 联系

- 项目主页: [https://github.com/your-repo/rustclaw](https://github.com/your-repo/rustclaw)
- 问题反馈: [Issues](https://github.com/your-repo/rustclaw/issues)