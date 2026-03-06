# RustClaw

[![Version](https://img.shields.io/badge/version-v0.1.0-blue.svg)](https://github.com/your-repo/rustclaw)
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
cargo run --bin rustclaw-gateway
```

默认端口: `127.0.0.1:18789`

## 用法

### 基本使用

1. 启动网关服务器
2. 通过 WebSocket 或 HTTP 连接发送消息
3. Agent 会处理消息并执行工具

### 配置

编辑 `config.toml` 文件进行配置。

## 开发路线图

- **Phase 1**: 核心框架 (项目初始化、配置系统、Gateway WebSocket 服务器)
- **Phase 2**: Agent 运行时 (Session 管理、Agent 循环逻辑)
- **Phase 3**: 工具系统 (Tool trait、注册表、沙箱执行器)
- **Phase 4**: 动态 Skill (Skill 生成、验证、审批流程)
- **Phase 5**: 测试与优化

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