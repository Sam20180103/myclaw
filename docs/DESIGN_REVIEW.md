# RustClaw 设计与架构评审 (Design Review)

> 版本基线: v0.47.0 · 评审日期: 2026-06-13
> 方法: 通读核心 crate 源码 (`agent` / `gateway` / `provider` / `tool` / `config`)，结合实际调用链定位。每条问题给出 `文件:行号` 证据，并标注严重度与建议。
> 范围: 仅评审**当前已落地的代码**与架构，不含尚未规划的新功能。
>
> **更新 (v0.48.0):** 下列 P0/P1 + D1/T1 已修复,详见文末 [修复进展](#修复进展-v0480)。
> C1/C2/A3/R1/R3/A1/D1/T1 已闭环;R2/A2/D2–D5/T2 仍待办。

## 评分概览

| 维度 | 评价 | 说明 |
|------|------|------|
| 模块边界 / 分层 | 🟢 良好 | crate 划分清晰，依赖方向单一 (cli/gateway → agent → …) |
| 核心 Agent 循环正确性 | 🔴 有缺陷 | 工具调用未按协议回灌历史 (见 C1) |
| 上下文 / 内存管理 | 🟠 待改进 | 压缩不落盘、写放大、锁泄漏 (C2/R1/R2) |
| 文档与实现一致性 | 🟠 待改进 | Lane 并发、多 Provider 等文档描述与代码不符 (A1/A2) |
| 安全 | 🟢 良好 | v0.44 已加固 (沙箱/认证/SSRF/凭据脱敏/记忆安全扫描) |
| 可测试性 / 可观测性 | 🟠 待改进 | 缺端到端循环测试与成本/延迟指标 (T1/T2) |

---

## 🔴 严重 (Critical) — 影响正确性

### C1. Agent 工具调用未按对话协议回灌历史

**位置**: `crates/agent/src/lib.rs:713-820`

核心循环里，当模型产生 tool_calls 时：
- **承载 tool_calls 的 assistant 消息从未写回会话历史**——只有工具*结果*被追加；
- 工具结果以 `Role::System` 文本写入 (`"Tool {name} result: {...}"`，`lib.rs:788-795`)，而非与 `tool_call_id` 配对的 `tool` / `tool_result` 消息。

**影响**:
1. 模型在下一轮看不到"自己刚才请求过哪个工具、用了什么参数"，多步工具推理质量下降、易重复调用。
2. Anthropic 严格要求 `assistant.tool_use` 与 `tool_result` 成对出现；当前 transcript 对 Anthropic 是非法/降级的。
3. 每轮都把 system + memory 重新 `insert(0, …)` (`lib.rs:637-650`)，**前缀每轮都变**，直接破坏 Anthropic prompt cache (README 宣称可省 75% 输入成本，实际拿不到)。

**建议**: 引入结构化 `ToolResult` 消息类型，按 OpenAI `role:"tool"` / Anthropic `tool_result` 回灌；assistant 的 tool_calls 也要入历史；system/memory 固定放在历史最前且只注入一次，保证缓存前缀稳定。

### C2. 上下文压缩"算了不存"，且按硬编码预算逐轮重算

**位置**: `crates/agent/src/lib.rs:533-538`、`652-657`

```rust
if let Some(ref compressor) = self.context_compressor {
    let result = compressor.compress(&messages, 100000); // 预算硬编码
    if result.compressed { messages = result.messages; }   // 只用于本次调用
}
```

- `messages` 是每轮临时拼装的副本，压缩结果**仅用于当次 LLM 调用，从不写回会话**。`SessionManager` 里的历史持续增长 → 内存与 SQLite 占用**实质无界**，压缩只是给单次调用"打补丁"。
- 预算 `100000` 硬编码，无视 `ContextCompressor::new(0.50, 128000)` 的配置 (`gateway/src/lib.rs:145`)，也无视具体模型上下文窗口。
- 每轮重算压缩，纯属重复劳动。

**建议**: 压缩后通过 `replace_messages` 落盘 (该方法已存在，`lib.rs:292`)；预算从 compressor 配置/模型窗口推导；仅在超阈值时触发而非每轮。

---

## 🟠 架构与文档不符 (Architecture / Reality mismatch)

### A1. Lane 并发子系统是死代码，但文档把它当核心机制

**位置**: `crates/agent/src/lib.rs:368-414`

`LaneRouter` / `Lane` / `LaneTask` / `register_lane` / `get_lane` 全部定义齐全，CLAUDE.md 也把"按 `session_key` 哈希进 `lane_0..lane_9` 并发隔离"写成关键架构事实。但全仓**没有任何地方调用 `register_lane` 或向 `queue_tx` 发送 `LaneTask`**。`route()` 唯一作用是算出一个字符串存进 `Session.lane`，此后再不被读取。

真正的并发控制是 `process_with_loop` 里的 **per-session `tokio::Mutex`** (`session_locks`, `lib.rs:596-601`) + gateway 每请求一个 tokio task。即:文档描述的"车道队列"架构与实现完全脱节，这段代码是误导性的维护负担。

**建议**: 二选一——要么真正接线 Lane 队列 (用于跨会话公平调度/限流)，要么删除 Lane* 并修正 CLAUDE.md，明确并发模型是"每会话串行 + 全局并行"。

### A2. 单一 Provider，无路由/降级/按请求选模型

**位置**: `crates/gateway/src/lib.rs:297-328`、`crates/provider/src/lib.rs:1149`

`create_provider_client` 只取**第一个**带 key 的 provider，其余忽略；`ProviderClient` 内部仅持有 `Option<Arc<dyn LlmProvider>>`。具备多 provider 能力的 `ProviderManager`(register/get_default) 存在却**未被 gateway 使用**。后果:
- 无按请求覆盖模型、无失败转移、无成本/延迟路由;
- `rustclaw models set` 改的是配置文件，需**重启**才生效;
- README/架构文档"支持 OpenAI/Anthropic/NVIDIA 多种 LLM"易被理解为运行时可切换,实际是构建期单选。

**建议**: gateway 改用 `ProviderManager`，支持 `Envelope`/请求级别的 model 选择与 fallback 链；或在文档中明确"单活 provider，切换需重启"。

### A3. `tool_timeout_ms` 配了但从不生效

**位置**: 配置贯穿 `config/src/lib.rs:109` → `agent/src/lib.rs:86` → `gateway/src/lib.rs:108`，但 `execute_tool` (`agent/src/lib.rs:1038-1046`) **没有任何超时包裹**。

只有 `SandboxExecutor` 内部的 30s 对 shell 类工具生效；而 reach/MCP/HTTP (reqwest) 工具**不走沙箱**。一个挂死的 MCP 服务器或慢速文生图调用会**无限期占住该会话的 Mutex**，把整条会话卡死。

**建议**: 在 `execute_tool` 外层套 `tokio::time::timeout(config.tool_timeout_ms)`，超时返回工具错误并继续循环。

---

## 🟠 健壮性与资源管理 (Robustness)

### R1. `session_locks` 无界增长 (内存泄漏)

**位置**: `crates/agent/src/lib.rs:52`、`596-599`

每个 `session_key` 在 `DashMap` 中创建一个 `Arc<Mutex<()>>`，会话被 `delete`/TTL 清理时**锁条目不会被移除**。长驻网关的内存随"历史上出现过的不同会话总数"线性增长。

**建议**: 会话删除/过期清理时同步 `session_locks.remove(key)`；或改用带 LRU/容量上限的结构。

### R2. `add_message` 每次追加都全量拷贝 → 单会话 O(n²)

**位置**: `crates/agent/src/lib.rs:271-289`

```rust
let mut messages = (*session.messages).clone(); // 整段 Vec 拷贝
messages.push(message.clone());
session.messages = Arc::new(messages);
```

`Arc<Vec>` 被注释为"优化减少克隆"，实则因为 Arc 与在途循环的快照共享，**每次写都触发整段拷贝**。100 轮工具循环就是平方级开销。同时该路径还会向 SQLite `add_message` 单条写，而压缩/创建走 `insert` 全量覆盖,两套写法对同一份历史并存,易产生不一致。

**建议**: 会话内部用 `Vec<Message>` + 写锁直接 `push`；需要快照时再克隆。统一 SQLite 写策略 (增量 append)。

### R3. `max_sessions` 未强制，无背压/淘汰

**位置**: `crates/agent/src/lib.rs:128-129`(`#[allow(dead_code)]`)、`357-365`

活跃会话数没有上限与淘汰策略，清理只靠 TTL 扫描 (且需外部触发)。高并发/被刷时活跃会话内存无界。

**建议**: 入口做容量上限 + LRU 淘汰或拒绝 (返回 429)。

---

## 🟡 设计气味与耦合 (Design smells)

### D1. 平台业务逻辑泄漏进通用 Agent 内核

**位置**: `crates/agent/src/lib.rs:728-777`

BOSS 直聘的二维码处理 (硬编码字符串"请使用Boss直聘APP扫描二维码登录"、解析 `qrcode_image` 字段) 直接写在 `process_with_loop` 里。通用运行时不该认识某个具体工具的返回结构。

**建议**: 抽象一个"工具结果 → 对外 Content"适配层 (或让工具返回带 `media`/`render_as` 的结构化 `ToolOutput`)，把平台特例移出内核。

### D2. 三条重复且发散的执行路径

**位置**: `process` (`lib.rs:498`) / `process_with_loop` (`lib.rs:592`) / `process_stream` (`lib.rs:1068`)

三者各自重复拼装 system prompt / memory / 压缩 / tool schema。`process_stream` **完全不支持工具、不注入 memory、不压缩**；`process` 是单轮遗留路径。实际 gateway 全部走 `process_with_loop` (`gateway` 内 7 处调用)。维护时极易只改一处造成漂移。

**建议**: 抽取统一的"消息组装"私有方法；明确 `process`/`process_stream` 的去留 (流式应复用同一组装并支持工具)。

### D3. LLM/工具边界过度 stringly-typed

`complete_with_tools` 返回 `(String, Vec<serde_json::Value>)`，循环里大量 `.get().and_then().unwrap_or("")` (`lib.rs:565-571`)；工具结果以 `result.to_string()` 塞进 System 文本。最刺眼的是 `parse_tool_calls` (`lib.rs:826-1036`) ——**5 种回退策略 + 手写字节级花括号配平**去从自由文本里抠工具调用。这是"边界没有类型契约"的症状,本应由 provider 层用各家原生 tool-calling 解析,而非 agent 兜底刮取文本。

**建议**: 定义跨 crate 的 `ToolResult` / `ToolCall` 强类型;文本回退解析下沉到 provider 且仅作为非原生模型的兜底。

### D4. `ReachConfig` 三处重复定义

`crates/config/src/lib.rs` 的结构体 + `crates/tool/src/reach.rs` 的结构体 + `gateway/src/lib.rs:113-134` 的映射块，外加 config crate 的 2 处 default 构造。每加一个字段要改 5 处 (本仓历史上已因此多次编译失败)。

**建议**: 共享同一类型 (tool crate 定义、config 复用)，或映射改为 serde flatten / `From` 实现，消除手抄。

### D5. 非官方平台自动化的契约是非正式的

`vendor/mcp-bosszp` / `mcp-xhs` / `wechat-connector` 是 Playwright/Python 服务，**不在 cargo 构建内**:不被类型检查、不进 CI、选择器 best-effort 易随线上 DOM 失效;文生图落盘需与网关**同主机共享文件系统**;Rust↔Python 仅靠"约定的 JSON-RPC"。

**建议**: 为关键方法补**契约测试** (mock server 跑 JSON-RPC 往返断言);文档明确同主机假设与跨机部署的替代 (base64/对象存储);把 ToS/封号风险声明保留在显眼处 (已有,保持)。

---

## 🟢 安全现状 (基本良好，仅补注意点)

- 沙箱/命令黑名单、API Key 认证中间件、CORS 收紧、SSRF URL 校验、凭据脱敏均已落地 (v0.44)。
- **记忆写入有安全扫描**: `MemoryTool` → `add_entry`/`replace_entry` → `MemoryScanner::scan` (`memory/src/lib.rs:82,101`)，prompt 注入内容会被拦。✅
- ⚠️ **默认开放姿态**: `/ws`、`/webhook`、`/channel/wechat/inbound` 仅在配置了 api_key 时才校验 Bearer;未配置即开放。建议文档显著标注,并考虑本地默认拒绝外部来源。
- ⚠️ 自我进化工具直连模型:`memory` 增删改无需审批 (仅 `skill_manage create` 走审批)。扫描器能挡注入,但建议对 `replace/remove` 也留审计日志。

---

## 🟡 测试与可观测性 (Testing / Observability)

### T1. 缺端到端工具循环测试
现有测试以单元/构造断言为主 (如 provider 测试只校验结构体字段)。**没有一个测试用 mock provider 跑完整的 `process_with_loop` 工具循环**——C1 的 transcript 缺陷恰好就能被这样一个测试抓住。

**建议**: 加一个 mock `LlmProvider` (先返回 tool_call、再返回终答)，断言会话历史里 assistant.tool_calls 与 tool_result 成对、顺序正确。

### T2. 无成本/延迟/迭代指标
仅有 `info!` 日志,没有 token 用量、每会话成本、工具延迟的结构化指标或 tracing span。而"降低 token 成本"是项目明确目标。

**建议**: 在循环关键点加 tracing span 与计数 (iterations / tool latency / prompt+completion tokens),便于回归 C1/C2 修复后的成本验证。

---

## 建议的修复优先级

| 优先级 | 条目 | 理由 |
|--------|------|------|
| P0 | C1 工具消息回灌 + C2 压缩落盘 | 直接影响多步任务正确性与已宣称的成本节省 |
| P0 | A3 工具超时 + R1 锁泄漏 | 长驻服务的稳定性/可用性 |
| P1 | R2 写放大 + R3 会话上限 | 负载下的性能与内存安全 |
| P1 | A1 Lane 死代码 / A2 多 Provider | 让文档与实现一致,避免误用 |
| P2 | D1–D5 解耦与去重 | 可维护性;随新平台接入收益放大 |
| P2 | T1/T2 测试与指标 | 为上述修复提供回归保障 |

---

## 附:做得好的地方

- crate 分层与依赖方向干净,`thiserror`/`anyhow` 分层得当,几乎无 `unwrap` 滥用。
- v0.44 安全加固扎实 (沙箱、认证、SSRF、脱敏、记忆扫描)。
- "连接器无关 + 稳定 HTTP 契约"接入非官方平台 (微信/小红书/BOSS) 的范式选型务实,把易变/违规风险隔离在 cargo 构建之外。
- 视觉多模态 (`openai_content`/`anthropic_content`) 实现正确且有针对性单测。

---

## 修复进展 (v0.48.0)

> 日期: 2026-06-19 · 全程 `cargo build --workspace` 0 warning、`cargo test --workspace` 188 通过、`cargo clippy -p rustclaw-agent --all-targets` 干净。

| 条目 | 状态 | 实现要点 |
|------|------|----------|
| **C1** 工具调用回灌 | ✅ | 新增 `Role::Tool` + `Content::ToolUse{tool_calls,text}` / `Content::ToolResult{tool_call_id,name,output}`;循环持久化 assistant 工具请求与 tool 结果;provider 新增 `openai_message`/`anthropic_message` 映射为原生 `tool_calls`/`tool_use`/`tool_result`。tool_call id 为空时补 uuid 以保证配对。 |
| **C2** 压缩落盘 | ✅ | 压缩对象改为**纯历史**(不含临时注入的 system/memory),压缩后经 `replace_messages` 落盘;预算改用 `ContextCompressor::context_window()`(修正 `new(0.50,128000)` 中 128000 此前落进无用字段的 bug);`clean_old_tool_results` 适配 `ToolResult` 并修复"保留项被双重 push"的旧 bug。 |
| **A3** 工具超时 | ✅ | `execute_tool` 用 `tokio::time::timeout(tool_timeout_ms)` 包裹,超时返回工具错误并继续循环。 |
| **R1** 锁泄漏 | ✅ | `delete_session` 与新 `maintenance()` 同步移除 `session_locks`;`maintenance()` 还回收 `strong_count==1` 的孤儿锁。 |
| **R3** 会话上限 | ✅ | `cleanup_expired` 返回被移出 key 并按 `last_accessed` LRU 强制 `max_sessions`;gateway `start()` 每 60s 调 `maintenance()`(此前 `cleanup_expired` 无任何调用方)。 |
| **A1** Lane 死代码 | ✅ | 删除未接线的 `LaneRouter`/`Lane`/`LaneTask`,保留 `route_lane()` 仅作 lane 标签;CLAUDE.md 改为如实描述"per-session Mutex + 每请求 task"并发模型。 |
| **D1** 平台特例解耦 | ✅ | BOSS 二维码逻辑抽为通用 `extract_renderable_image()`(识别 `qrcode_image`/`image`),内核不再硬编码平台字符串;早返回前补 `tool_result` 保持配对。 |
| **T1** 端到端测试 | ✅ | 新增 mock `LlmProvider`+`Tool` 跑完整 `process_with_loop`,断言历史 `[user, assistant(ToolUse), tool(ToolResult), assistant(text)]` 成对且 id 配对。 |

### 仍待办 (未在本轮处理)
- **R2** `add_message` 的 O(n²) 写放大 (Arc<Vec> copy-on-write) — 需改会话内部存储结构,改动面较大。
- **A2** 多 Provider 路由/降级/按请求选模型 — 属新功能,需引入 `ProviderManager` 接线。
- **D2** `process`/`process_stream` 与 `process_with_loop` 路径发散;**D3** LLM/工具边界 stringly-typed;**D4** `ReachConfig` 三处重复;**D5** vendor 契约无 CI。
- **T2** token/延迟/成本指标与 tracing span。
- 附带发现:Anthropic 把注入的 `System` 角色历史消息映射进 messages 数组 (应提为顶层 `system` 参数) — 本轮 `anthropic_message` 已将其降级为 `user` 避免 400,但根治需重构 system 注入。
