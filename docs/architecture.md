# RustClaw 技术架构文档

> 版本: v0.9.0
> 日期: 2026-03-09
> 状态: 完整API功能实现

---

## 一、项目概述

### 1.1 项目定位

RustClaw 是一个用 Rust 实现的本地 AI 智能体网关，架构理念参考 OpenClaw，但使用 Rust 实现以获得更高的性能和安全性。

### 1.2 核心特性目标

| 特性 | 描述 | 优先级 |
|------|------|--------|
| 多通道接入 | WebSocket、HTTP、Webhook 等消息入口 | P0 |
| 智能体运行时 | 多会话并发、Agent循环、流式响应 | P0 |
| 工具系统 | 原生Rust工具注册 | P0 |
| 动态Skill | Agent可以自己编写并注册新Skill | P0 |
| 审批流程 | 用户审批后Skill才可使用 | P0 |
| Provider抽象 | 支持多种LLM提供商 | P1 |
| 安全沙箱 | 进程隔离、权限控制、工具策略 | P1 |

---

## 二、系统架构

### 2.1 整体架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                        Ingress (入口层)                          │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐       │
│  │WebSocket│ │  HTTP    │ │ Telegram │ │   Webhook    │  ...  │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘ └──────┬───────┘       │
└───────┼───────────┼────────────┼───────────────┼───────────────┘
        │           │            │               │
        └───────────┴─────┬──────┴───────────────┘
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Gateway (控制平面)                            │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  • 连接管理    • 会话路由    • 认证授权    • 事件广播   │   │
│  │  • 消息队列    • 状态管理    • 协议转换    • API暴露    │   │
│  └──────────────────────────────────────────────────────────┘   │
└───────────────────────────────┬─────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Execution Plane (执行层)                       │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    Agent Runtime                          │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐     │   │
│  │  │ Session │  │  Lane   │  │  Queue  │  │ Stream  │     │   │
│  │  │ Manager │  │ Router  │  │ Handler │  │ Manager │     │   │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘     │   │
│  └──────────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    Tool Executor                          │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐              │   │
│  │  │ Sandbox  │  │  Policy  │  │ Registry  │              │   │
│  │  │ Manager  │  │  Engine  │  │           │              │   │
│  │  └──────────┘  └──────────┘  └──────────┘              │   │
│  └──────────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │               Dynamic Skill System                         │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐              │   │
│  │  │ Generate │  │ Validate │  │ Approve   │              │   │
│  │  └──────────┘  └──────────┘  └──────────┘              │   │
│  └──────────────────────────────────────────────────────────┘   │
└───────────────────────────────┬─────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Capability Layer (能力层)                     │
│  ┌──────────────────┐        ┌────────────────────────┐        │
│  │   Tool Registry  │        │    Provider Manager    │        │
│  │  ┌────────────┐  │        │  ┌─────┐ ┌─────┐      │        │
│  │  │ Native     │  │        │  │OAI  │ │Claude│ ...  │        │
│  │  │ Tools      │  │        │  └─────┘ └─────┘      │        │
│  │  ├────────────┤  │        └────────────────────────┘        │
│  │  │ Dynamic    │  │                                           │
│  │  │ Skills     │  │                                           │
│  │  └────────────┘  │                                           │
│  └──────────────────┘                                           │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Data Layer                                 │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────────┐        │
│  │ Sessions │ │  Config  │ │  Media   │ │   Logs     │        │
│  └──────────┘ └──────────┘ └──────────┘ └────────────┘        │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 核心组件

#### Gateway (网关核心)

- 长期运行的 WebSocket 服务器
- 默认端口: `127.0.0.1:18789`
- 职责: 连接管理、会话路由、认证授权、事件广播

#### Channel (通道层)

消息入口适配器，将各种消息源统一为内部格式。

- WebSocket Channel
- HTTP Channel  
- Webhook Channel
- (未来) Telegram, Discord 等

#### Agent Runtime (智能体运行时)

- Session 管理
- Lane 路由 (sessionKey 隔离)
- Queue 队列
- 流式响应处理 (SSE)
- Agent 循环: message → LLM → tools? → execute → continue → response

**流式响应**:
- OpenAI: 解析 `data: {...}` SSE 块
- Anthropic: 解析 content blocks
- 返回 `Receiver<Result<String, AgentError>>`

#### Tool System (工具系统)

- Tool Registry: 工具注册表
- Sandbox Executor: 沙箱隔离执行
- Policy Engine: 工具调用策略

#### Dynamic Skill System (动态Skill系统)

- Skill Generate: Agent 生成 Skill 代码 (通过 OpenCode CLI)
- Skill Validate: 静态验证语法/类型
- Skill Approve: 用户审批后才能使用

**OpenCode CLI 集成**:
```rust
struct OpenCodeGenerator {
    opencode_path: PathBuf,
    timeout_seconds: u64,  // 默认 120s
}
```

- 自动检测 opencode 路径
- 超时保护
- 返回 SkillDefinition

---

## 三、关键技术决策

### 3.1 技术栈

| 类别 | 选择 | 理由 |
|------|------|------|
| 异步运行时 | tokio | 成熟稳定，生态丰富 |
| Web框架 | axum + tokio-tungstenite | 简洁API，高性能 |
| 配置格式 | TOML | Rust 原生支持 |
| 日志系统 | tracing | 结构化，异步友好 |
| 序列化 | serde_json | 事实标准 |

### 3.2 MCP 策略

**MVP阶段：不实现 MCP 兼容**

理由：
1. MCP 协议有安全风险 (AI 可动态调用任意 MCP 服务器)
2. MVP 聚焦核心：Gateway + Agent + 基础工具
3. 后续可通过 MCP 网关代理或手动实现高频 Skills

### 3.3 Skill 语言支持

多语言支持：
- Rust (闭包，需要编译)
- Python
- Shell/Bash
- JavaScript

### 3.4 审批流程

```
Agent 调用 OpenCode CLI 生成 Skill
    ↓
Skill 保存到 ~/.rustclaw/skills/pending/
    ↓
触发审批请求 (通知用户)
    ↓
用户审批通过
    ↓
移动到 ~/.rustclaw/skills/
    ↓
重新加载 Tool Registry
    ↓
Agent 可使用新 Skill
```

---

## 四、模块划分

### 4.1 Crate 结构

```
rustclaw/
├── Cargo.toml (workspace)
│
├── crates/
│   ├── gateway/      # 网关核心
│   ├── channel/      # 通道抽象
│   ├── agent/        # Agent运行时
│   ├── tool/         # 工具系统
│   ├── provider/     # LLM提供商
│   ├── protocol/     # 通信协议
│   ├── config/       # 配置管理
│   └── storage/      # 数据存储
│
├── tests/
│   └── unit/
│
└── examples/
```

### 4.2 核心数据结构

#### Envelope (消息封装)

```rust
struct Envelope {
    id: String,
    channel: String,
    session_key: String,
    message: Message,
    timestamp: i64,
}

struct Message {
    role: Role,  // user, assistant, system
    content: Content,
}
```

#### Tool Definition

```rust
struct Tool {
    name: String,
    description: String,
    parameters: Schema,
    handler: ToolHandler,
}

enum ToolHandler {
    Native(fn(Context, Params) -> Result<Value>),
    Script(ScriptDefinition),
}
```

### 4.3 关键实现细节

#### Provider 流式响应

```rust
// LlmProvider trait
async fn stream_complete(&self, messages: Vec<Message>) -> TokenStream;
// TokenStream = Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>
```

- OpenAI: 使用 `reqwest` 流式请求，解析 SSE 块
- Anthropic: 解析 `content` 数组中的 `text` 字段

#### SandboxExecutor

```rust
pub struct SandboxExecutor {
    config: SandboxConfig,
}

impl SandboxExecutor {
    pub async fn execute_command(&self, command: &str) -> Result<Value, ToolError> {
        // 1. 检查危险命令
        // 2. 超时控制 (tokio::time::timeout)
        // 3. 执行并返回结果
    }
}
```

#### OpenCodeGenerator

```rust
pub struct OpenCodeGenerator {
    opencode_path: PathBuf,
    timeout_seconds: u64,
}

impl OpenCodeGenerator {
    pub async fn generate_skill(
        &self,
        description: &str,
        language: SkillLanguage,
        parameters: Value,
    ) -> Result<SkillDefinition, SkillError> {
        // 调用 opencode generate CLI
        // 超时保护
        // 返回生成的代码
    }
}
```

---

## 五、安全设计

### 5.1 安全原则

1. **本地优先**: 默认绑定 `127.0.0.1`
2. **设备配对**: 新设备需要显式授权
3. **工具策略**: 白名单/黑名单
4. **沙箱隔离**: 危险操作独立进程
5. **资源限制**: 超时、内存限制

### 5.2 沙箱执行器

```rust
struct SandboxConfig {
    timeout_ms: u64,          // 默认 30000ms
    max_memory_mb: u64,       // 默认 512MB
    allowed_dirs: Vec<String>, // 允许访问的目录
    denied_commands: Vec<String>, // 禁止执行的命令
}
```

**危险命令检测**:
- `rm -rf /` - 递归删除根目录
- `mkfs` - 文件系统格式化
- `dd if=` - 裸磁盘写入
- `:(){:|:&};:` - Fork bomb

**危险模式检测**:
- `curl | sh` - 远程脚本执行
- `wget | sh` - 远程脚本执行
- `chmod 777` - 过度权限
- `chown -R` - 递归修改所有者

### 5.3 审批工作流

- 所有 Agent 生成的 Skills 需要用户审批
- 审批前进行静态验证 (语法检查)
- 审批通过后 Skill 才能被使用

---

## 六、开发路线图

### Phase 1: 核心框架 (2-3周) ✅ 完成

- [x] 项目初始化，workspace 搭建
- [x] 配置系统
- [x] Gateway WebSocket 服务器
- [x] 基础协议实现 (JSON-RPC)
- [x] 日志系统

### Phase 2: Agent 运行时 (2-3周) ✅ 完成

- [x] Session 管理
- [x] Lane/Queue 实现
- [x] Agent 循环逻辑
- [x] 流式响应处理 (SSE)
- [x] 基础 Provider 客户端

### Phase 3: 工具系统 (2-3周) ✅ 完成

- [x] Tool trait 定义
- [x] 工具注册表
- [x] 内置工具实现 (file_read, file_write, shell_exec, opencode_generate)
- [x] 沙箱执行器
- [x] 工具策略引擎

### Phase 4: 动态 Skill (2周) ✅ 完成

- [x] Skill 定义格式
- [x] OpenCode 集成 (外部CLI)
- [x] 静态验证
- [x] 审批流程
- [x] 动态注册

### Phase 5: 测试与优化 (1周) ✅ 完成

- [x] 单元测试 (基础)
- [x] 集成测试
- [x] 审批事件订阅 (SSE)
- [x] 性能优化 (HashMap预分配)
- [x] Skill验证增强
- [x] API文档完善

### Phase 6: OpenCode工具集成 (1天) ✅ 完成

- [x] OpenCodeTool 实现
- [x] Agent自动注册内置工具
- [x] NVIDIA Provider支持

### Phase 11: Agent-Reach核心工具集成 (1天) ✅ 完成

- [x] WebReachTool (Jina Reader API集成)
- [x] VideoReachTool (yt-dlp CLI封装)
- [x] GitHubReachTool (gh CLI封装)
- [x] FeedReachTool (RSS/Atom解析)
- [x] 工具注册和配置支持
- [x] 单元测试覆盖

### Phase 12: 社交平台工具集成 (1天) ✅ 完成

- [x] TwitterReachTool (Twitter/X数据访问)
- [x] RedditReachTool (Reddit JSON API集成)
- [x] WeiboReachTool (微博热搜和用户数据)
- [x] 工具策略更新
- [x] 单元测试覆盖

### Phase 12.5: 速率限制集成 (1天) ✅ 完成

- [x] governor依赖添加
- [x] RateLimiterManager实现
- [x] 平台特定速率配置
- [x] 单元测试覆盖

### Phase 13: MCP桥接集成 (1天) ✅ 完成

- [x] McpClient (JSON-RPC客户端)
- [x] XhsReachTool (小红书MCP桥接)
- [x] DouyinReachTool (抖音MCP桥接)
- [x] SearchReachTool (Exa语义搜索)
- [x] 单元测试覆盖

---

## 七、版本历史

| 版本 | 日期 | 变更 |
|------|------|------|
| v0.1.0 | 2026-03-06 | 初始版本 |
| v0.2.0 | 2026-03-06 | 流式响应(SSE)、沙箱执行增强、OpenCode CLI集成 |
| v0.2.2 | 2026-03-07 | 审批事件通知(SSE)、集成测试覆盖 |
| v0.2.3 | 2026-03-07 | 性能优化(HashMap预分配、Vec预分配) |
| v0.2.4 | 2026-03-07 | Skill验证增强(名称检查、危险模式检测) |
| v0.2.5 | 2026-03-07 | API文档完善(请求/响应示例) |
| v0.3.0 | 2026-03-07 | OpenCode工具集成、NVIDIA Provider支持、配置修复 |
| v0.4.0 | 2026-03-09 | Agent-Reach核心工具集成(WebReach/VideoReach/GitHubReach/FeedReach) |
| v0.5.0 | 2026-03-09 | Agent-Reach社交平台工具集成(TwitterReach/RedditReach/WeiboReach) |
| v0.6.0 | 2026-03-09 | 速率限制功能集成(RateLimiterManager/governor) |
| v0.7.0 | 2026-03-09 | MCP桥接集成(McpClient/XhsReach/DouyinReach/SearchReach) |
| v0.8.0 | 2026-03-09 | Gateway API增强(Session GET/Agent API/Config API) |
| v0.9.0 | 2026-03-09 | 完整API功能(Channel管理/配置保存/模型切换/安全审计) |

---

## 八、API 端点

### Health
- `GET /health` - 健康检查

**响应示例:**
```json
{
  "status": "ok",
  "service": "rustclaw-gateway",
  "version": "0.2.4"
}
```

### Session
- `GET /api/session` - 列出所有会话

**响应示例:**
```json
{
  "success": true,
  "sessions": [
    {
      "key": "uuid-string",
      "lane": "lane_0",
      "channel": "http",
      "message_count": 5,
      "created_at": 1678000000000,
      "updated_at": 1678000001000
    }
  ]
}
```

- `GET /api/session/{id}` - 获取单个会话详情

**响应示例:**
```json
{
  "success": true,
  "session": {
    "key": "uuid-string",
    "lane": "lane_0",
    "channel": "http",
    "message_count": 5,
    "created_at": 1678000000000,
    "updated_at": 1678000001000,
    "tool_calls_enabled": true
  }
}
```

- `DELETE /api/session/{id}` - 删除会话

**响应示例:**
```json
{
  "success": true,
  "deleted": "session-id"
}
```

### Agent
- `GET /api/agent` - 列出所有Agent会话

**响应示例:**
```json
{
  "success": true,
  "agents": [
    {
      "session_key": "uuid-string",
      "lane": "lane_0",
      "channel": "http",
      "status": "active",
      "message_count": 5,
      "created_at": 1678000000000,
      "updated_at": 1678000001000
    }
  ]
}
```

- `GET /api/agent/{id}` - 获取单个Agent详情

**响应示例:**
```json
{
  "success": true,
  "agent": {
    "session_key": "uuid-string",
    "lane": "lane_0",
    "channel": "http",
    "status": "active",
    "message_count": 5,
    "created_at": 1678000000000,
    "updated_at": 1678000001000,
    "tool_calls_enabled": true,
    "messages": [
      {
        "role": "user",
        "content": "Hello, Agent!"
      }
    ]
  }
}
```

### Config
- `GET /api/config` - 获取当前配置

**响应示例:**
```json
{
  "success": true,
  "config": {
    "gateway": {
      "host": "127.0.0.1",
      "port": 18789,
      "device_id": "uuid-string",
      "auth": {
        "require_pairing": true,
        "allowed_devices": []
      }
    },
    "agent": {
      "max_concurrent_sessions": 100,
      "default_model": "claude-3-5-sonnet-20241022",
      "max_loop_iterations": 100,
      "tool_timeout_ms": 60000
    },
    "tools": {
      "enabled_tools": ["file_read", "file_write", "shell_exec", "web_search"],
      "disabled_tools": [],
      "require_approval": ["shell_exec"],
      "sandbox_enabled": true,
      "max_memory_mb": 512,
      "max_timeout_ms": 30000
    },
    "providers": [
      {
        "name": "anthropic",
        "provider_type": "anthropic",
        "default_model": "claude-3-5-sonnet-20241022",
        "base_url": "https://api.anthropic.com"
      }
    ]
  }
}
```

- `POST /api/config` - 更新配置

**请求示例:**
```json
{
  "agent": {
    "max_loop_iterations": 50
  }
}
```

**响应示例:**
```json
{
  "success": true,
  "message": "Configuration update received for: agent",
  "note": "Configuration changes require gateway restart to take effect."
}
```

- `POST /api/config/save` - 保存配置到文件

**请求示例:**
```json
{
  "agent": {
    "default_model": "gpt-4o",
    "max_loop_iterations": 50
  },
  "tools": {
    "enabled_tools": ["file_read", "file_write"]
  }
}
```

**响应示例:**
```json
{
  "success": true,
  "message": "Configuration saved successfully",
  "path": "/home/user/.config/rustclaw/config.toml"
}
```

- `POST /api/config/model` - 切换模型

**请求示例:**
```json
{
  "model": "gpt-4o",
  "provider_type": "openai"
}
```

**响应示例:**
```json
{
  "success": true,
  "message": "Model switched to gpt-4o for openai",
  "note": "Gateway restart required for changes to take effect"
}
```

### Channel
- `GET /api/channel` - 列出所有通道

**响应示例:**
```json
{
  "success": true,
  "channels": [
    {
      "name": "http",
      "type": "http",
      "enabled": true,
      "config": {}
    },
    {
      "name": "websocket",
      "type": "websocket",
      "enabled": true,
      "config": {}
    }
  ]
}
```

- `GET /api/channel/{name}` - 获取单个通道配置

**响应示例:**
```json
{
  "success": true,
  "channel": {
    "name": "http",
    "type": "http",
    "enabled": true,
    "config": {}
  }
}
```

- `POST /api/channel/{name}` - 更新通道配置

**请求示例:**
```json
{
  "enabled": false,
  "config": {
    "timeout": 30000
  }
}
```

**响应示例:**
```json
{
  "success": true,
  "message": "Channel http updated"
}
```

### Security
- `GET /api/security/audit` - 安全审计

**响应示例:**
```json
{
  "success": true,
  "audit": {
    "score": 80,
    "status": "good",
    "findings": [
      {
        "severity": "info",
        "message": "Tool 'shell_exec' requires approval"
      }
    ],
    "checks": {
      "sandbox_enabled": true,
      "auth_required": true,
      "api_key_configured": true,
      "approval_tools": 1,
      "disabled_tools": 0
    }
  }
}
```

### Message
- `POST /api/message` - 发送消息 (JSON-RPC)

**请求示例:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "message.send",
  "params": {
    "channel": "http",
    "sessionKey": "optional-session-key",
    "content": "Hello, Agent!"
  }
}
```

**响应示例:**
```json
{
  "success": true,
  "sessionKey": "generated-or-provided-key",
  "response": "Agent response text",
  "toolCalls": 0
}
```

**方法列表:**
- `message.send` - 发送单次消息
- `message.send_with_loop` - 发送消息并启用Agent循环

### Skill
- `GET /api/skill` - 列出所有Skills

**响应示例:**
```json
{
  "success": true,
  "pending": [
    {
      "id": "skill-uuid",
      "name": "pending_skill",
      "language": "shell",
      "status": "pending"
    }
  ],
  "active": [
    {
      "id": "skill-uuid",
      "name": "active_skill",
      "language": "python",
      "status": "active"
    }
  ]
}
```

- `POST /api/skill` - 创建新Skill

**请求示例:**
```json
{
  "name": "my_skill",
  "description": "A custom skill",
  "language": "shell",
  "source_code": "echo 'Hello from skill'",
  "parameters": {
    "input": "string"
  }
}
```

**响应示例:**
```json
{
  "success": true,
  "skillId": "generated-uuid",
  "message": "Skill submitted for approval"
}
```

- `POST /api/skill/{id}/approve` - 审批Skill

**响应示例:**
```json
{
  "success": true,
  "message": "Skill {id} approved"
}
```

- `POST /api/skill/{id}/reject` - 拒绝Skill

**响应示例:**
```json
{
  "success": true,
  "message": "Skill {id} rejected"
}
```

- `GET /api/skill/approval/subscribe` - 订阅审批事件 (SSE)

**事件格式 (Server-Sent Events):**
```
data: {"skill_id":"uuid","skill_name":"new_skill","skill_definition":{...},"requested_at":1678000000000}
```

---

## 九、测试覆盖

| 模块 | 单元测试 | 集成测试 |
|------|---------|---------|
| config | 7 | - |
| protocol | 9 | - |
| skill | 9 | 4 |
| tool | 33 | - |
| agent | - | 3 |
| gateway | - | - |
| provider | - | - |
| **总计** | **58** | **7** |

---

## 十、性能优化

### HashMap预分配
| 结构 | 预分配容量 | 说明 |
|------|-----------|------|
| SessionManager | max_sessions | 会话存储 |
| LaneRouter | 10 | 固定10个lane |
| Gateway::sessions | 64 | 预期并发连接 |
| ToolRegistry | 16 | 内置工具数量 |
| SkillManager::pending | 8 | 待审批skill |
| SkillManager::active | 32 | 活跃skill |

### Vec预分配
| 结构 | 预分配容量 | 说明 |
|------|-----------|------|
| Session::messages | 16 | 消息历史 |

### 锁优化
- `get_or_create()` 先读后写，减少write锁持有时间

---

## 十一、CLI 与 Gateway 关系

### 11.1 架构设计

RustClaw 采用 CLI + Gateway 分离架构：

```
┌─────────────────────────────────────────────────────────────┐
│                        rustclaw (CLI)                        │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │  进程管理    │  │  配置管理    │  │  运维工具   │     │
│  │  start/stop │  │  config      │  │  doctor     │     │
│  │  restart    │  │  models      │  │  logs       │     │
│  │  status     │  │  providers   │  │  security   │     │
│  └──────┬───────┘  └──────────────┘  └──────────────┘     │
│         │                                                     │
│         ▼                                                     │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              进程管理 (Process Manager)                │  │
│  │  • PID 文件: ~/.local/share/rustclaw.pid             │  │
│  │  • 进程检测: pgrep -f rustclaw-gateway              │  │
│  │  • 信号控制: SIGTERM (15) → SIGKILL (9)             │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   rustclaw-gateway (Gateway)                │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │   HTTP Server │  │  WebSocket   │  │  Agent Loop  │     │
│  │   :18789     │  │  ws://...    │  │  Tool Exec   │     │
│  └──────────────┘  └──────────────┘  └──────────────┘     │
└─────────────────────────────────────────────────────────────┘
```

### 11.2 二进制文件

| 二进制 | 路径 | 作用 |
|--------|------|------|
| `rustclaw` | CLI | 命令行工具，管理Gateway |
| `rustclaw-gateway` | Gateway | 核心服务，运行AI Agent |

### 11.3 启动方式

```bash
# 方式1: 通过CLI管理 (推荐)
rustclaw start           # 启动Gateway
rustclaw stop            # 停止Gateway
rustclaw restart         # 重启Gateway
rustclaw status          # 查看状态

# 方式2: 直接运行Gateway
rustclaw-gateway --host 127.0.0.1 --port 18789
```

### 11.4 进程检测

CLI 通过以下方式检测Gateway是否运行：
1. **PID文件**: `~/.local/share/rustclaw.pid`
2. **进程扫描**: `pgrep -f rustclaw-gateway`

### 11.5 常用命令

```bash
# 启动
rustclaw start                      # 前台启动
rustclaw start --detach             # 后台启动
rustclaw start --port 8080         # 指定端口

# 停止
rustclaw stop                       # 正常停止 (SIGTERM)
rustclaw stop --force               # 强制停止 (SIGKILL)

# 重启
rustclaw restart                    # 重启Gateway

# 状态
rustclaw status                    # 查看运行状态
rustclaw health                    # 健康检查
```

---

## 十二、CLI 命令参考

### 11.1 命令结构

RustClaw CLI 采用分层命令结构，参考 OpenClaw 设计：

```
rustclaw <COMMAND> [SUBCOMMAND] [OPTIONS]
```

### 11.2 主命令

| 命令 | 说明 | 状态 |
|------|------|------|
| `start` | 启动网关服务器 | ✅ |
| `stop` | 停止网关服务器 | ✅ |
| `restart` | 重启网关服务器 | ✅ |
| `config` | 配置管理 | ✅ |
| `sessions` | 会话管理 | ✅ |
| `agent` | Agent管理 | ✅ |
| `gateway` | 网关管理 | ✅ |
| `channels` | 通道管理 | ✅ |
| `skills` | Skill管理 | ✅ |
| `models` | 模型管理 | ✅ |
| `system` | 系统信息 | ✅ |
| `logs` | 日志管理 | ✅ |
| `health` | 健康检查 | ✅ |
| `status` | 状态概览 | ✅ |
| `security` | 安全审计 | ✅ |
| `providers` | 提供商管理 | ✅ |
| `tools` | 工具管理 | ✅ |
| `memory` | 内存管理 | ✅ |
| `open-code` | OpenCode集成 | ✅ |
| `doctor` | 诊断修复 | ✅ |
| `update` | 更新版本 | ✅ |
| `reset` | 重置数据 | ✅ |
| `version` | 版本信息 | ✅ |

### 11.3 子命令示例

#### Gateway 管理
```bash
rustclaw gateway status      # 查看网关状态
rustclaw gateway start       # 启动网关
rustclaw gateway stop        # 停止网关
rustclaw gateway restart    # 重启网关
rustclaw gateway health     # 健康检查
rustclaw gateway call       # 调用API
```

#### Sessions 管理
```bash
rustclaw sessions --list    # 列出所有会话
rustclaw sessions --kill    # 终止会话
rustclaw sessions --clear   # 清理所有会话
```

#### Models 管理
```bash
rustclaw models --list      # 列出可用模型
rustclaw models --status    # 模型状态
rustclaw models --set       # 设置默认模型
rustclaw models auth        # 认证管理
```

#### OpenCode 集成
```bash
rustclaw open-code --run    # 执行OpenCode任务
rustclaw open-code --status # OpenCode状态
rustclaw opencode sessions  # 会话管理
```

### 11.4 全局选项

| 选项 | 说明 |
|------|------|
| `-V, --version` | 打印版本信息 |
| `-h, --help` | 打印帮助信息 |

### 11.5 使用示例

```bash
# 查看帮助
rustclaw --help
rustclaw gateway --help

# 启动网关
rustclaw start --host 127.0.0.1 --port 18789

# 查看状态
rustclaw status --detailed
rustclaw health --verbose

# 管理会话
rustclaw sessions --list

# 管理模型
rustclaw models --list
rustclaw models --set anthropic/claude-sonnet

# OpenCode集成
rustclaw open-code --run "生成一个Hello World程序"
```

---

## 十三、CLI 功能实现状态

### 13.1 服务管理命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `start` | 启动Gateway | ✅ 完整实现 | 支持后台启动、端口配置 |
| `stop` | 停止Gateway | ✅ 完整实现 | 支持强制停止 |
| `restart` | 重启Gateway | ✅ 完整实现 | |
| `status` | 状态概览 | ✅ 完整实现 | 显示运行状态和PID |
| `health` | 健康检查 | ✅ 完整实现 | 支持详细模式 |
| `doctor` | 诊断工具 | ✅ 完整实现 | 系统诊断 |

### 13.2 配置管理命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `config --show` | 显示配置 | ✅ 完整实现 | 显示完整配置 |
| `config --path` | 配置路径 | ✅ 完整实现 | |
| `config --get` | 获取配置项 | 🔄 基础实现 | 显示配置对象 |
| `config --set` | 设置配置项 | 🔄 基础实现 | 需要手动编辑 |

### 13.3 模型管理命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `models list` | 列出模型 | ✅ 完整实现 | 显示可用模型列表 |
| `models status` | 模型状态 | ✅ 完整实现 | 显示当前配置 |
| `models set` | 设置模型 | 🔄 基础实现 | 需要手动编辑配置 |
| `models auth` | 模型认证 | 🔄 基础实现 | 显示配置路径 |

### 13.4 资源管理命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `providers list` | 列出提供商 | ✅ 完整实现 | |
| `providers add` | 添加提供商 | 🔄 基础实现 | 需要手动配置 |
| `tools list` | 列出工具 | ✅ 完整实现 | 显示内置工具 |
| `tools info` | 工具信息 | 🔄 基础实现 | |
| `skills list` | 列出Skills | ✅ 完整实现 | 显示动态Skills |

### 13.5 会话管理命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `sessions --list` | 列出会话 | 🔄 基础实现 | 显示提示信息 |
| `sessions --kill` | 终止会话 | 🔄 基础实现 | 需要Gateway API |
| `sessions --clear` | 清理会话 | 🔄 基础实现 | |
| `agent --list` | 列出Agent | 🔄 基础实现 | |
| `agent --info` | Agent信息 | 🔄 基础实现 | |

### 13.6 系统命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `system info` | 系统信息 | ✅ 完整实现 | |
| `system event` | 事件日志 | ✅ 完整实现 | |
| `system heartbeat` | 心跳状态 | ✅ 完整实现 | |
| `logs` | 日志管理 | ✅ 完整实现 | 支持清理、跟踪 |
| `security audit` | 安全审计 | ✅ 完整实现 | 完整安全检查 |
| `security status` | 安全状态 | ✅ 完整实现 | |
| `memory status` | 内存状态 | ✅ 完整实现 | Gateway连接状态 |

### 13.7 其他命令

| 命令 | 功能 | 实现状态 | 说明 |
|------|------|----------|------|
| `gateway status` | Gateway状态 | ✅ 完整实现 | |
| `gateway start/stop` | Gateway控制 | ✅ 完整实现 | |
| `channels list` | 列出通道 | ✅ 完整实现 | 显示通道类型和配置 |
| `channels status` | 通道状态 | ✅ 完整实现 | |
| `open-code run` | OpenCode执行 | 🔄 基础实现 | 显示参数 |
| `update` | 更新版本 | 🔄 基础实现 | 显示构建命令 |
| `reset` | 重置数据 | ✅ 完整实现 | 支持清理日志 |
| `version` | 版本信息 | ✅ 完整实现 | |

### 13.8 实现状态统计

| 状态 | 数量 | 百分比 |
|------|------|--------|
| ✅ 完整实现 | 40 | 80% |
| 🔄 基础实现 | 10 | 20% |
| ❌ 未实现 | 0 | 0% |
| **总计** | **50** | **100%** |

---

## 十四、待实现功能

### 14.1 高优先级 (P0)

| 功能 | 描述 | 依赖 | 状态 |
|------|------|------|------|
| 会话API | 通过Gateway API管理会话 | Gateway HTTP接口 | ✅ 完成 |
| Agent API | Agent创建和管理 | Gateway HTTP接口 | ✅ 完成 |
| 通道管理 | 实际的通道配置 | Channel模块 | ✅ 完成 |

### 14.2 中优先级 (P1)

| 功能 | 描述 | 依赖 | 状态 |
|------|------|------|------|
| 配置API | 配置获取和更新 | Gateway HTTP接口 | ✅ 完成 |
| 配置编辑 | 直接修改配置文件 | 配置系统 | ✅ 完成 |
| 模型切换 | 运行时切换模型 | Provider管理 | ✅ 完成 |
| 安全审计 | 实际安全检查 | 安全模块 | ✅ 完成 |
| 模型切换 | 运行时切换模型 | Provider管理 | 待实现 |
| 安全审计 | 实际安全检查 | 安全模块 | 待实现 |

### 14.3 Agent-Reach集成进度

| Phase | 功能 | 状态 |
|-------|------|------|
| Phase 11 | WebReachTool (Jina Reader) | ✅ 完成 |
| Phase 11 | VideoReachTool (yt-dlp) | ✅ 完成 |
| Phase 11 | GitHubReachTool (gh CLI) | ✅ 完成 |
| Phase 11 | FeedReachTool (RSS/Atom) | ✅ 完成 |
| Phase 12 | TwitterReachTool (Twitter/X) | ✅ 完成 |
| Phase 12 | RedditReachTool (Reddit) | ✅ 完成 |
| Phase 12 | WeiboReachTool (微博) | ✅ 完成 |
| Phase 13 | McpClient 实现 | ✅ 完成 |
| Phase 13 | XhsReachTool (小红书) | ✅ 完成 |
| Phase 13 | DouyinReachTool (抖音) | ✅ 完成 |
| Phase 13 | SearchReachTool (Exa) | ✅ 完成 |
| Phase 14 | LinkedInReachTool | ⏳ 需要MCP服务 | 需要linkedin-mcp服务运行 |
| Phase 14 | WeChatReachTool | ⏳ 需要MCP服务 | 需要微信公众号MCP服务 |
| Phase 14 | BossZhipinReachTool | ⏳ 需要MCP服务 | 需要mcp-bosszp服务运行 |
| Phase 14 | PodcastReachTool | ⏳ 需要MCP服务 | 需要小宇宙API或Whisper |

### 14.4 低优先级 (P2)

| 功能 | 描述 | 依赖 | 状态 |
|------|------|------|------|
| 内存管理 | Agent内存状态 | Agent运行时 | 待实现 |
| OpenCode集成 | 实际调用OpenCode | OpenCode工具 | ✅ 完成 |
| 自动更新 | 检查并更新版本 | 构建系统 | 待实现 |

---

## 十五、Agent-Reach 集成设计

### 15.1 概述

Agent-Reach 是一个互联网平台访问工具集，为 AI Agent 提供多平台数据读取和搜索能力。本项目计划将其核心能力以 Rust 原生工具形式集成。

**参考**: https://github.com/Panniantong/Agent-Reach

### 15.2 支持的平台

| 平台 | 能力 | 实现方式 |
|------|------|----------|
| Web | 读取任意网页 | Jina Reader (免费无API Key) |
| YouTube | 字幕提取 + 视频搜索 | yt-dlp CLI |
| RSS | RSS/Atom 订阅阅读 | feedparser |
| GitHub | 仓库读取 + 搜索 | gh CLI |
| Twitter/X | 推文读取、搜索、时间线 | xreach CLI |
| Bilibili | 字幕 + 搜索 | yt-dlp + 代理 |
| Reddit | 搜索 + 帖子读取 | JSON API + Exa |
| 小红书 | 阅读、搜索、发帖 | mcporter MCP |
| 抖音 | 视频解析 + 下载 | douyin-mcp-server |
| LinkedIn | Profile + 公司页面 | linkedin-mcp |
| Boss直聘 | 职位搜索 | mcp-bosszp |
| 微信公众号 | 文章搜索 + 阅读 | camoufox + miku |
| 微博 | 热搜、用户动态 | Direct API |
| 小宇宙播客 | 音频转录 | Whisper |

### 15.3 集成架构

```
RustClaw Tool System
├── crates/tool/src/tools/
│   ├── web_reach.rs       # Web读取 (Jina Reader)
│   ├── video_reach.rs     # YouTube/Bilibili (yt-dlp)
│   ├── social_reach.rs    # Twitter/Reddit/微博
│   ├── github_reach.rs    # GitHub (gh CLI)
│   ├── feed_reach.rs      # RSS/Atom
│   ├── xhs_reach.rs       # 小红书 (MCP bridge)
│   ├── douyin_reach.rs    # 抖音 (MCP bridge)
│   └── search_reach.rs    # Exa语义搜索
```

### 15.4 工具实现示例

#### WebReachTool

```rust
pub struct WebReachTool {
    client: reqwest::Client,
    endpoint: String, // https://r.jina.ai
}

impl Tool for WebReachTool {
    fn name(&self) -> &str { "web_reach" }
    
    fn execute(&self, params: Value) -> Result<Value, ToolError> {
        let url = params["url"].as_str().ok_or(ToolError::MissingParam)?;
        let jina_url = format!("{}/{}", self.endpoint, url);
        // 获取并返回Markdown内容
    }
}
```

#### VideoReachTool

```rust
pub struct VideoReachTool {
    executor: SandboxExecutor,
    yt_dlp_path: PathBuf,
}

impl Tool for VideoReachTool {
    fn name(&self) -> &str { "video_reach" }
    
    fn execute(&self, params: Value) -> Result<Value, ToolError> {
        let url = params["url"].as_str().ok_or(ToolError::MissingParam)?;
        let cmd = format!("{} --dump-json --write-sub --skip-download \"{}\"", 
            self.yt_dlp_path.display(), url);
        self.executor.execute_command(&cmd).await
    }
}
```

### 15.5 MCP 桥接架构

对于需要 MCP 服务器的平台（小红书、抖音、LinkedIn）：

```rust
pub struct McpClient {
    endpoint: String,
    client: reqwest::Client,
}

impl McpClient {
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": uuid::Uuid::new_v4()
        });
        self.client.post(&self.endpoint).json(&request).send().await?.json().await
    }
}
```

### 15.6 配置 Schema

```toml
[reach]
enabled = true

[reach.web]
endpoint = "https://r.jina.ai"
timeout_ms = 30000

[reach.video]
yt_dlp_path = "/usr/bin/yt-dlp"
timeout_ms = 60000

[reach.twitter]
xreach_path = "/usr/local/bin/xreach"
cookie_file = "~/.rustclaw/credentials/twitter.json"

[reach.search]
mcp_endpoint = "http://127.0.0.1:8080/mcp"
```

### 15.7 安全设计

#### 沙箱执行

所有外部 CLI 调用通过 `SandboxExecutor`：
- 超时: 视频60s, Web30s, 搜索10s
- 内存限制: 512MB
- 禁止模式: `rm -rf`, fork bombs 等

#### 凭据管理

- Cookie 存储在 `~/.rustclaw/credentials/`
- 文件权限: 600 (仅所有者可读写)
- MCP 端点仅本地绑定 (127.0.0.1)

#### 速率限制

```rust
pub struct RateLimiter {
    requests_per_minute: u32,
    burst_capacity: u32,
}

// Twitter: 30 req/min, burst 10
// Reddit: 60 req/min, burst 20
// Web: 120 req/min, burst 30
```

### 15.8 实现优先级

| Phase | 功能 | 优先级 |
|-------|------|--------|
| Phase 11 | WebReachTool (Jina Reader) | P0 |
| Phase 11 | VideoReachTool (yt-dlp) | P0 |
| Phase 11 | GitHubReachTool (gh CLI) | P0 |
| Phase 11 | FeedReachTool (RSS) | P0 |
| Phase 12 | TwitterReachTool (xreach) | P1 |
| Phase 12 | RedditReachTool (JSON API) | P1 |
| Phase 12 | WeiboReachTool (Direct API) | P1 |
| Phase 13 | McpClient 实现 | P1 |
| Phase 13 | XhsReachTool (小红书) | P1 |
| Phase 13 | DouyinReachTool (抖音) | P1 |
| Phase 13 | SearchReachTool (Exa) | P1 |
| Phase 14 | LinkedInReachTool | P2 |
| Phase 14 | WeChatReachTool | P2 |
| Phase 14 | BossZhipinReachTool | P2 |
| Phase 14 | PodcastReachTool (小宇宙) | P2 |

### 15.9 依赖项

```toml
[dependencies]
# RSS解析
feed-rs = "1.4"

# MCP客户端
jsonrpc-client-transports = "0.6"

# 速率限制
governor = "0.6"
```

### 15.10 测试策略

每个工具需要：
1. Mock 后端测试 (模拟 HTTP 响应)
2. 参数验证测试
3. 错误处理测试

集成测试：
```rust
#[tokio::test]
async fn test_web_reach_integration() {
    let tool = WebReachTool::new(Default::default());
    let result = tool.execute(json!({"url": "https://example.com"})).await;
    assert!(result.is_ok());
}
```

### 15.11 设计文档

详细设计文档位于: `/usr/local/project/lab/myclaw_desigh/AGENTS.md`

---

*文档版本: v0.4.0*
