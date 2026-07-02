# 对标 Hermes Agent / OpenClaw 的框架差距分析

> 日期: 2026-06-20 · 基线: RustClaw v0.48.0
> 方法: 调研两个参考框架 (RustClaw CLAUDE.md 自述"架构理念参考 OpenClaw 和 Nous Research Hermes Agent") 的公开架构文档,逐项对照 RustClaw 当前代码 (附 `文件:行号` 证据)。
> 目的: 找出"参考框架有、RustClaw 缺或没接线"的能力,给出优先级。

## 参考框架速览

**Nous Research Hermes Agent** (2026-02, MIT, Python)
- `AIAgent` 同步会话循环: prompt 构造 → provider 选择 → API 调用 → 工具执行 → 循环至终止;循环内置 **retries / fallback / callbacks / compression / persistence**。
- 三种 API 形态: `chat_completions` / `codex_responses` / `anthropic_messages`。
- `runtime_provider.py` 把 `(provider, model)` 映射到 `(api_mode, api_key, base_url)`,覆盖 **18+ provider**;`/model` 可**会话中途切换**且保持 prompt 稳定。
- 中央工具注册表: 70+ 工具 / 28 toolset,**import 时自注册 + `check_fn` 门控**可用性 + 错误包装。
- **可插拔记忆 provider** (`memory_provider.py` 抽象基类,单选激活);SQLite FTS5 + 跨压缩谱系。
- Prompt 分层: **stable(身份/工具) → context(文件) → volatile(记忆/档案/时间戳)**;`prompt_caching.py` 打 Anthropic 缓存断点。
- 沙箱 **六后端**: local / Docker / SSH / Daytona / Modal / Singularity。
- **Profile 隔离**: 每 profile 独立 HERMES_HOME/config/memory/sessions,可并发多 profile。
- **Cron** 挂 skill/script 定时执行;gateway tick cron。
- **Trajectory** 从 session 导出 ShareGPT 格式做训练数据。

**OpenClaw** (开源自托管 agent 网关)
- 七组件: Channel System · Gateway(控制平面/消息 broker) · Plug-ins & Skills · Agent Runtime · Memory & Knowledge · LLM Provider · Local Execution。
- Agent Runtime: 多轮推理循环,工具顺序执行、回灌上下文直到纯文本响应终止。
- **工具调用五级策略级联: global → provider → agent → session → sandbox**,细粒度权限。
- **设备节点** (macOS/iOS/Android) 经 WebSocket 接入,把本地能力**暴露为可远程调用的工具**,跨设备工作流。
- Channel 按 per-channel allowlist 授权 + DM 配对;敏感操作可插入 **human-in-the-loop checkpoint**。
- **Shared Memory Layer**: agent 间上下文共享 (orchestrator + specialized agents)。
- 凭据 **encryption at rest**;持续监控已连接工具、**事件触发主动行动**(非纯请求-响应)。

---

## 对照矩阵

| 能力 | Hermes | OpenClaw | RustClaw 现状 | 差距 |
|------|:---:|:---:|------|:---:|
| 网关控制平面 / 多通道入站 | ✅ | ✅ | ✅ HTTP/WS/Webhook/飞书/微信 | — |
| 多轮工具循环 + 协议化 transcript | ✅ | ✅ | ✅ (v0.48.0 C1 修复) | — |
| SQLite 持久化 + FTS5 + 谱系 | ✅ | ✅ | ✅ storage crate | — |
| 上下文压缩落盘 | ✅ | ✅ | ✅ (v0.48.0 C2 修复) | — |
| 视觉多模态 | ✅ | ✅ | ✅ openai/anthropic | — |
| **工具策略级联 + 人审 checkpoint** | ✅ check_fn | ✅ 五级 | ⚠️ ToolPolicy 存在但**未接线** | 🔴 |
| **Prompt 缓存 (cache_control)** | ✅ | ✅ | ⚠️ CacheStrategy 定义了**没接** | 🔴 |
| **运行时多 Provider 切换/降级/重试** | ✅ 18+ | ✅ | ❌ 取第一个,无 fallback/retry | 🔴 |
| **工具可用性门控 (按凭据/平台)** | ✅ check_fn | ✅ session 级 | ❌ list() 永远全量 | 🟠 |
| **沙箱多后端 (容器/远程)** | ✅ 六后端 | ✅ sandbox 级 | ❌ 仅进程内 + reach/MCP 绕过 | 🟠 |
| **记忆 provider 抽象 (可插向量库)** | ✅ | ✅ knowledge | ❌ 仅 MEMORY.md 文件型 | 🟠 |
| **Profile / 多租户隔离** | ✅ | ⚠️ | ❌ 单一全局目录 | 🟠 |
| **调度 / 主动触发 (cron/事件)** | ✅ cron | ✅ 持续监控 | ❌ 纯请求-响应 | 🟠 |
| **凭据加密落盘** | n/a | ✅ at rest | ❌ config.toml 明文 | 🟠 |
| **子 Agent 编排 / 共享记忆** | ⚠️ | ✅ orchestrator | ❌ 单 agent (仅谱系字段) | 🟡 |
| **Trajectory 训练数据导出** | ✅ | n/a | ❌ 无 | 🟡 |
| 多 API 形态 (Responses 等) | ✅ 3 | — | ⚠️ 缺 OpenAI Responses | 🟡 |

---

## 差距详述

### 🔴 G1. 工具策略与人审从未接线 (最高)

`ToolPolicy` (`crates/tool/src/lib.rs:528`) 已定义 `allowed_tools`/`denied_tools`/`require_approval`,默认还把 `shell_exec` 列为需审批,并有 `is_allowed()`/`requires_approval()`。**但 agent 循环根本不调用它** —— `execute_tool` 直接 `tool_registry.get(name).execute()` (`crates/agent/src/lib.rs`),`ToolExecutor`/`ToolPolicy` 在 agent 与 gateway 中零引用。

后果: 黑/白名单、敏感工具人审 (shell_exec、`xhs publish` 发布、BOSS 投递) 全部失效。对照 OpenClaw 的五级策略级联 + human-in-the-loop checkpoint、Hermes 的 `check_fn` 门控,这是 RustClaw "看似有、实则没生效" 的最大安全/可控性缺口。

**建议**: 在 `execute_tool` 前置 `policy.is_allowed()` 拒绝;对 `requires_approval()` 的工具走"暂停 → 经通道 (飞书/微信) 请求确认 → 继续"流程 (复用现有 SkillApproval 的 broadcast 机制)。后续再做 provider/session 级级联。

### 🔴 G2. Prompt 缓存定义了却没接到请求体

`compression` crate 有 `CacheStrategy` / `calculate_breakpoints` (system + 最近3条),但**全仓只在该 crate 内部出现**;`provider` 的 `AnthropicRequest` / OpenAI 请求体里**没有任何 `cache_control` 字段**。即 README/架构文档宣称的"Anthropic cache_control 降低 75% 输入成本"目前**拿不到**。

对照 Hermes `prompt_caching.py` + "stable→context→volatile" 分层 (保证可缓存前缀稳定)。RustClaw 在 v0.48.0 已让历史前缀稳定 (C1/C2),但缺最后一步:把缓存断点真正写进 Anthropic 请求。

**建议**: Anthropic 请求体支持内容块 `cache_control: {type:"ephemeral"}`,在 system 块与倒数第 N 条消息打断点;按 `CacheStrategy` 计算位置。

### 🔴 G3. 单 Provider,无运行时切换/降级/重试 (评审 A2 升级)

`create_provider_client` (`crates/gateway/src/lib.rs:297`) 只取第一个带 key 的 provider;循环遇 `ProviderError` 直接返回 (`agent` 无 retry/backoff/fallback)。`ProviderManager` 已具多 provider 能力却未被 gateway 使用。

对照 Hermes `runtime_provider` 映射 18+ provider、`/model` 中途切换、循环内置 retries/fallback。这是 RustClaw 与参考框架差距最大的"运行时弹性"维度。

**建议**: gateway 改用 `ProviderManager`;`Envelope` 支持 model 覆盖;循环对 5xx/限流做指数退避重试,失败转移到 fallback provider。

### 🟠 G4. 工具可用性无门控,schema 永远全量

`ToolRegistry::list()` 无条件返回全部 22 个工具的 schema。未配置 key 的 `image_gen`、未部署 MCP 的 `xhs`/`boss` 也照样进 prompt,既浪费 token 又诱导模型调用必然失败的工具。

对照 Hermes 每工具 `check_fn` 门控可见性。

**建议**: `Tool` trait 增 `fn is_available(&self) -> bool` (或 `available(ctx)`),`list()`/schema 组装时过滤;未配置依赖的工具自动隐藏。

### 🟠 G5. 沙箱仅单一进程内后端,且 reach/MCP 绕过

`SandboxExecutor` 只有本地进程内限制 (30s/~512MB/命令黑名单);reach/MCP/HTTP 工具完全不走它 (G3 评审已述超时,A3 已补)。对照 Hermes 六执行后端 (Docker/SSH/Daytona/Modal/Singularity)。

**建议**: 抽象 `ExecutionBackend` trait,至少补一个 Docker 后端用于不可信 skill/shell;为远程/容器执行留扩展点。

### 🟠 G6. 记忆无 provider 抽象

记忆只有文件型 `MemoryManager` (MEMORY.md/USER.md)。对照 Hermes `memory_provider.py` 抽象基类 + 单选激活 (可插向量检索/外部 KV)。RustClaw 难以接入语义检索记忆。

**建议**: 抽 `MemoryProvider` trait,现有文件实现为默认;留向量库 (本地 sqlite-vss / 外部) 实现位。

### 🟠 G7. 无 Profile / 多租户隔离

单一全局 `~/.config/rustclaw` + `~/.rustclaw` (config/memory/sessions/db 共享)。对照 Hermes 每 profile 独立目录、可并发多 profile。多用户/多角色场景会串数据。

**建议**: 引入 profile 维度,数据目录按 profile 隔离;CLI `--profile` 或 env。

### 🟠 G8. 无调度 / 主动触发

RustClaw 纯请求-响应。对照 Hermes cron 挂 skill/script、OpenClaw 持续监控连接工具并事件触发。无法做"定时巡检/到点发布/事件驱动"。

**建议**: 加 scheduler (cron 表达式) + 事件入站,复用现有 Agent 循环执行预设任务 (与已实现的 xhs/boss 自动化天然契合)。

### 🟠 G9. 凭据明文落盘

`api_key` / `boss_cookie` / `xhs_cookie` / `image_gen_api_key` 等以明文存 `config.toml` (`crates/config/src/lib.rs:98,153,159,164`)。对照 OpenClaw encryption at rest。

**建议**: 优先支持从 env / OS keyring 读取;或对敏感字段做对称加密 (主密钥来自 env)。文档明确"勿提交含密钥的 config.toml"。

### 🟡 G10. 单 Agent,无子 Agent 编排 / 共享记忆

`iteration_budget` 注释提到"父子 Agent 共享预算"、`Session.parent_session_id` 有谱系字段,但**没有真正的子 Agent 派生/编排**。对照 OpenClaw orchestrator + specialized agents + shared memory layer。

**建议**: 提供 `spawn_subagent` 工具 (受 iteration_budget 约束),子会话写 parent_session_id,结果回灌父会话。

### 🟡 G11. 无 Trajectory 导出 (契合自我进化目标)

有完整 SQLite session,但无导出。对照 Hermes ShareGPT 格式 trajectory 用于训练。RustClaw 主打"自我进化"却缺这一数据出口。

**建议**: CLI `rustclaw sessions export --format sharegpt`。

### 🟡 G12. 多 API 形态

Hermes 支持 `chat_completions` / `codex_responses` / `anthropic_messages`。RustClaw 有 openai/anthropic/nvidia(OpenAI 兼容),缺 OpenAI Responses API 形态。优先级低,按需。

---

## 建议优先级

| 优先级 | 条目 | 理由 |
|--------|------|------|
| **P0** | G1 工具策略+人审接线 · G2 prompt 缓存接线 · G3 多 Provider/重试 | 安全可控性、已宣称的成本节省、运行时弹性——都是"参考框架核心、RustClaw 缺位"的硬伤 |
| **P1** | G4 工具门控 · G5 沙箱后端 · G9 凭据加密 | token/安全收益高、改动可控 |
| **P2** | G6 记忆 provider · G7 profile 隔离 · G8 调度/触发 | 架构扩展性,体量较大 |
| **P3** | G10 子 Agent · G11 trajectory · G12 多 API | 增值能力,按路线图推进 |

> 说明: G1/G2/G3 与上一轮 `DESIGN_REVIEW.md` 的 A2 / 缓存隐患同源,此处从"对标参考框架"视角进一步量化。RustClaw 在**核心循环正确性、持久化、压缩、视觉**上已与两框架齐平;主要差距集中在**策略治理、运行时弹性、执行隔离、可扩展插件化**四块。

---

## 修复进展 (P0, v0.49.0)

> 日期: 2026-06-24 · `cargo test --workspace` 198 通过 · 改动 crate clippy 干净。

| 条目 | 状态 | 实现要点 |
|------|------|----------|
| **G3** 多 Provider 路由/重试/降级 | ✅ | `ProviderClient` 改持有有序 provider 列表;对可重试错误 (RateLimited / 5xx,经 `is_retryable`) 指数退避 (200ms→5s) 重试,耗尽则降级到下一 provider;`stream` 不降级 (响应已开始)。gateway `create_provider_client` 构造全部已配置 provider (主+备),不再只取第一个。(按请求选模型仍待办) |
| **G2** Anthropic prompt 缓存 | ✅ | 新增 `anthropic_system_blocks` 把前导 System 消息提升为顶层 `system` 数组并在末块打 `cache_control:ephemeral`(并修正"System 被降级成 user"的旧问题);`with_tool_cache` 给工具数组末项打缓存断点 → 缓存 (system+tools) 大块稳定前缀;agent 注入顺序改为 `[base_system, memory, …]` 稳定前缀。OpenAI 自动缓存无需改。 |
| **G1** 工具策略 + 人审接线 | ✅ | `AgentRuntime` 持有 `ToolPolicy` + 异步 `ApprovalHook`;`execute_tool` 前置 deny/allow 校验,对 `require_approval` 工具征求批准 (无 hook → fail-closed 拒绝),拒绝以 `tool_result(error)` 回灌并发 `ToolDenied` 事件。gateway 由 `config.tools.{disabled_tools,require_approval}` 构造策略 (不把过时的 `enabled_tools` 当硬白名单),并接入 `ApprovalRegistry`(挂起+120s 超时默认拒绝)+ REST `GET/POST /api/tools/approvals[/{id}]` 供操作员裁决。 |

### P0 收尾说明
- G1 默认 `require_approval=["shell_exec"]`: 现在 `shell_exec` **必须经 `/api/tools/approvals/{id}` 批准**才执行 (超时/无操作 = 拒绝),这是有意的安全收紧。
- G2 局限: memory 在 system 前缀内,被工具改写时该轮缓存失效;但单次工具循环内 memory 稳定,多轮迭代仍命中 (成本主来源)。
- G3 局限: 暂未支持按请求/Envelope 覆盖 model;为后续项。

> 其余 P1/P2/P3 (G4 工具门控 · G5 沙箱后端 · G6 记忆 provider · G7 profile · G8 调度 · G9 凭据加密 · G10 子 Agent · G11 trajectory · G12 多 API) 仍待办。

---

## 修复进展 (P1, v0.50.0)

> 日期: 2026-06-24 · `cargo test --workspace` 209 通过 · 改动 crate clippy 干净。

| 条目 | 状态 | 实现要点 |
|------|------|----------|
| **G4** 工具可用性门控 | ✅ | `Tool` trait 增 `is_available()`(默认 true);`ToolRegistry::available_schemas()` 过滤后供 Agent 组装发给 LLM 的工具列表 (`list()` 仍返回全量供管理视图)。`image_gen`(需 base_url+api_key)、`search_reach`(需 exa_api_key) 未配置即不出现在 schema,省 token 且不诱导必败调用。 |
| **G9** 凭据 env 间接引用 | ✅ | `Config::resolve_secret_refs()` 在加载后把 `env:VARNAME` 替换为环境变量值,密钥不落 config.toml 明文;覆盖 provider/网关/reach/飞书/微信密钥;缺失 env → 可选字段置空、必填保留并告警。`.env.example` 补用法。 |
| **G5** 执行后端抽象 + Docker | ✅ | 新增 `ExecutionBackend` (`Local` / `Docker{image,network}`);`SandboxExecutor` 经 `with_backend` 选择,`execute_command` 走 `build_invocation`。Docker 后端构造 `docker run --rm [--network none] [--memory Nm] --security-opt no-new-privileges IMAGE sh -c CMD`。经 `RUSTCLAW_SANDBOX_BACKEND=local\|docker:IMG\|docker+net:IMG` 选择 (默认 local)。沙箱内无法实跑 docker,故对命令构造 + 规格解析做单测。 |

### P1 局限
- G4 仅门控有硬凭据要求的工具 (image_gen / search_reach);MCP 类工具 (xhs/boss) 仍常驻 (它们以 `auth_required` 优雅处理未登录),后续可加按会话/平台门控。
- G5 容器/远程实跑未在本环境验证;Hermes 的 SSH/Daytona/Modal/Singularity 后端为后续扩展点。

> 余下 P2 (G6 记忆 provider · G7 profile · G8 调度/触发) 与 P3 (G10 子 Agent · G11 trajectory · G12 多 API) 仍待办。

---

## 修复进展 (P2, v0.51.0)

> 日期: 2026-07-03 · `cargo test --workspace` 213 通过 · 改动 crate clippy 干净。

| 条目 | 状态 | 实现要点 |
|------|------|----------|
| **G6** 记忆 provider 抽象 | ✅ | `memory` crate 新增 `MemoryProvider` trait (add/replace/remove/entries/memory_text/usage/load);文件型 `MemoryManager` 为默认实现;`AgentRuntime.memory_manager` 与 `MemoryTool` 改持 `Arc<dyn MemoryProvider>`——向量检索/外部 KV 后端实现同一 trait 即可整体替换 (Hermes memory_provider 范式)。gateway 传参零改动 (unsize 自动强转)。 |
| **G7** Profile 隔离 | ✅ | `RUSTCLAW_PROFILE=<name>` → config/skills/logs (`~/.config/rustclaw/profiles/<name>/…`) 与数据目录 (`~/.rustclaw/profiles/<name>`) 全部按 profile 隔离;默认/`default` 保持旧路径 (兼容既有数据);profile 名过滤为 `[A-Za-z0-9_-]` 防路径穿越。gateway 改用 `Config::data_dir()`;`image_gen` 落盘目录同步隔离 (tool crate 不依赖 config,内联同一规则)。 |
| **G8** 定时调度 | ✅ | config 新增 `[[schedules]]` (name/every/prompt/enabled/session_key,serde default 兼容);`parse_interval` 支持 `30s/10m/2h/1d` (下限 10s 防打爆 provider);gateway `start()` 为每条启用任务 spawn ticker,到点构造 `Envelope(channel="schedule", session_key="schedule:<name>")` 驱动 `process_with_loop`,结果/错误入日志。同名任务共享会话 → 有跨次上下文。 |

### P2 局限
- G8 为固定间隔,非 cron 表达式 (到点对齐);结果仅入日志,未回推通道 (飞书/微信推送为自然扩展)。
- G7 未提供 CLI `--profile` 旗标 (env 对两个二进制都生效,已够用);profile 间无资源配额。
- G6 暂无第二个 provider 实现;trait 即扩展点。

> 余下 P3 (G10 子 Agent 编排 · G11 trajectory 导出 · G12 多 API 形态) 仍待办。
