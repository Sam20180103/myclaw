# PROJECT KNOWLEDGE BASE

**Updated:** 2026-08-14 (v0.54.0)
**Authoritative docs:** `docs/architecture.md` (设计) · `docs/PRODUCTION_READINESS.md` (上线阻塞项) · `CLAUDE.md` (工作约定)

## OVERVIEW

RustClaw — Rust 实现的本地 AI 智能体网关 (Cargo workspace)。长驻网关接收多通道消息 →
Agent 运行时驱动 LLM 循环与工具调用 → 持久化会话与记忆。架构理念参考 OpenClaw / Hermes Agent。

## STRUCTURE

```
./
├── Cargo.toml          # workspace root
├── crates/
│   ├── gateway/        # axum HTTP/WS 服务器 + REST API + 通道事件循环 + 审批登记
│   ├── agent/          # Agent 运行时 (会话/循环/预算/取消) + metrics.rs
│   ├── tool/           # ToolRegistry / SandboxExecutor / ToolPolicy / reach.rs
│   ├── skill/          # 动态技能 (生成/校验/审批) + skill_tool.rs (可调用封装)
│   ├── provider/       # OpenAI / Anthropic / NVIDIA / Responses + 用量统计
│   ├── memory/         # 三层记忆 + 安全扫描 (MemoryProvider trait)
│   ├── compression/    # 上下文压缩 (配对安全 + 摘要 + token 估算)
│   ├── storage/        # SQLite 会话持久化 + processed_events 判重
│   ├── feishu/         # 飞书通道 (长连接 + 卡片)
│   ├── wechat/         # 个人微信通道 (外部连接器契约)
│   ├── channel/        # 通道抽象 (trait 占位, 真实入站在 gateway/feishu/wechat)
│   ├── protocol/       # Envelope / Message / Content / JSON-RPC
│   ├── config/         # TOML 配置 + profile 路径解析
│   └── cli/            # CLI (二进制名 rustclaw)
├── docs/               # architecture / PRODUCTION_READINESS / roadmap / 平台接入指南
├── vendor/             # 外部 MCP 服务器与连接器 (不参与 Cargo 构建)
└── tests/              # 跨 crate 集成测试
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| HTTP/WS 服务与路由 | `crates/gateway/src/lib.rs` | axum router + handlers + 通道事件循环 |
| Agent 循环 | `crates/agent/src/lib.rs` | `process_with_loop_budget` (预算/取消/压缩/工具批次) |
| 运行指标 | `crates/agent/src/metrics.rs` | AtomicU64 计数器 → `GET /metrics` |
| 工具执行与策略 | `crates/tool/src/lib.rs` | ToolRegistry / SandboxExecutor / ToolPolicy |
| Agent-Reach 工具族 | `crates/tool/src/reach.rs` | Web/Video/GitHub/社交/BOSS/XHS/image_gen |
| 技能系统 | `crates/skill/src/lib.rs` | SkillManager + OpenCodeGenerator |
| 技能→工具 | `crates/skill/src/skill_tool.rs` | `SkillTool` (Shell/Python, 沙箱执行) |
| LLM Provider | `crates/provider/src/lib.rs` | 客户端 + 重试降级 + `record_usage` |
| 压缩 | `crates/compression/src/lib.rs` | `sanitize_pairing` / `estimate_tokens` / 摘要素材 |
| 判重与持久化 | `crates/storage/src/lib.rs` | `mark_event_seen` / 会话 CRUD / 搜索 |
| 协议类型 | `crates/protocol/src/lib.rs` | Envelope (含 `principal`) / Content 变体 |
| 路径与配置 | `crates/config/src/lib.rs` | `config_path` / `skills_dir` / `data_dir` / profile |

## CONVENTIONS

- **Async**: 所有 I/O 走 tokio,不得用同步调用阻塞运行时
- **Error**: 库用 thiserror,二进制边界用 anyhow;非测试路径避免 `unwrap()`
- **Logging**: tracing 结构化日志
- **Testing**: 各 crate 内 `#[cfg(test)]`;跨 crate 放 `tests/integration_tests.rs`
- **Config**: TOML + 环境变量覆盖;新增字段一律带 `#[serde(default)]` 保持旧配置可解析

## ANTI-PATTERNS (THIS PROJECT)

- **路由不要写 `{id}`** —— 依赖是 axum 0.7,`{id}` 是字面量路径段,会让接口静默 404。用 `:id`
- **不要就地拼配置/技能/日志路径** —— 必须走 `Config::config_path()` / `skills_dir()` /
  `logs_dir()` / `data_dir()`,否则绕过 `RUSTCLAW_PROFILE` 隔离,导致跨 profile 凭据串号
- **压缩/裁剪历史时不要单独删 `ToolResult`** —— 会产生孤儿 `tool_use`,Provider 返回 400;
  任何裁剪都要过 `sanitize_pairing`
- **新增前缀规则前先查内置工具名** —— `skill_` 会误伤内置的 `skill_manage`,故技能前缀是 `dyn_skill_`
- 不要用 `unwrap()` 吞错误,不要用阻塞调用卡住 async 运行时

## UNIQUE STYLES

- **工具沙箱**: shell 与技能都经 `SandboxExecutor` (命令黑名单 + 30s 超时 + 内存上限);
  后端 `RUSTCLAW_SANDBOX_BACKEND=local|docker:IMG|docker+net:IMG`
- **技能工作流**: Agent 生成 → `skills/pending/` → 批准 → `skills/active/` → **注册为 `dyn_skill_*` 工具**;
  每次调用仍需人工审批 (前缀强制,配置无法豁免)
- **会话并发**: 同会话 per-session `Mutex` 串行,跨会话并行;`route_lane()` 只是可观测性标签
- **三重预算**: 迭代上限 / 墙钟 deadline / 用户取消 —— 都返回部分结果而非 `Err`
- **聊天命令**: `/clear` `/stop` `/approve <id>` `/reject <id>` 在会话锁之前处理
- **入站幂等**: 飞书 `message_id` / 微信 `msg_id` / Webhook `event_id` → `processed_events`

## COMMANDS

```bash
cargo build --release              # 构建
cargo run --bin rustclaw-gateway   # 直接跑网关
cargo run --bin rustclaw -- start  # 经 CLI 启动
cargo test --workspace             # 全量测试 (254)
cargo clippy --all-targets         # lint (0 warnings)
```

## NOTES

- 默认端口 `127.0.0.1:18789`;会话 TTL 3600s;最大迭代 100;单轮墙钟预算 600s
- 工具总数 23 (5 内置 + 15 reach + memory/skill_manage/spawn_subagent) + 动态 `dyn_skill_*`
- Provider: OpenAI / Anthropic / NVIDIA / `openai-responses`,支持重试与降级
- 观测: `GET /metrics` (Prometheus)、`GET /api/usage` (token)、`GET /api/tools` (真正可调用的工具)
- `cargo fmt` 未在全仓执行过,提交前只格式化自己改动的区域,避免混入大片无关 diff
