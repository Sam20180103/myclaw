# RustClaw 技术架构文档

> 版本: v0.3.0
> 日期: 2026-03-07
> 状态: 核心功能完成，OpenCode工具集成完成

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

- `DELETE /api/session/{id}` - 删除会话

**响应示例:**
```json
{
  "success": true,
  "deleted": "session-id"
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
| tool | 14 | - |
| agent | - | 3 |
| gateway | - | - |
| provider | - | - |
| **总计** | **39** | **7** |

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

## 十一、CLI 命令参考

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

*文档版本: v0.3.0*
