# RustClaw

## Project Overview

RustClaw is a high-performance local AI agent gateway implemented in Rust. Its architecture is inspired by OpenClaw and Nous Research Hermes Agent, offering a comprehensive suite of features for running and managing AI agents locally. 

Key features include:
- **Multi-Channel Access:** HTTP REST, WebSocket (`/ws`), Webhook (sync or `callback_url` async), Feishu long connection, personal WeChat via an external connector. Inbound events are de-duplicated by platform event id, so retries never re-run side-effecting actions.
- **Agent Runtime:** Concurrent sessions (per-session mutex, cross-session parallel) with three termination budgets — iteration cap, wall-clock deadline, and cooperative cancellation — all of which return a **partial result** rather than an error.
- **Tool System:** 23 native tools (5 builtin + 15 Agent-Reach incl. `image_gen` + `memory` / `skill_manage` / `spawn_subagent`), plus dynamically registered skills. Per-call policy: deny-list, forced approval (fail-closed), per-principal overrides, JSON-Schema argument validation, concurrent execution, and head+tail truncation of oversized results.
- **Dynamic Skills (callable since v0.54.0):** approved **Shell/Python** skills are registered as `dyn_skill_*` tools and executed through the sandbox; they re-register on restart and unregister on delete. Rust/JavaScript skills remain catalog-only. Every `dyn_skill_*` call requires human approval — the prefix enforces it and config cannot waive it.
- **Provider Abstraction:** OpenAI, Anthropic, NVIDIA NIM, and the OpenAI Responses API, with exponential-backoff retries and automatic failover across an ordered provider list. Anthropic prompt caching marks the stable system+tools prefix.
- **Memory System:** Three-layer memory (`MEMORY.md` / `USER.md` + SQLite session store with FTS5) behind a `MemoryProvider` trait, with prompt-injection and data-leak scanning.
- **Context Management:** Compression preserves the `tool_use` / `tool_result` pairing invariant (orphans make providers return 400) and summarizes what it drops, so the agent does not forget completed side-effecting work. Budgets are measured in tokens, not bytes.
- **Operations:** `GET /metrics` (Prometheus) and `GET /api/usage` (token usage), profile isolation via `RUSTCLAW_PROFILE`, scheduled tasks with result/failure notifications, ShareGPT trajectory export, and `env:VARNAME` indirection so secrets never sit in `config.toml`.

## Architecture

The project uses a Cargo workspace with a layered architecture divided into several crates:
- `crates/gateway/`: The control plane handling connections, routing, and auth (axum web server).
- `crates/agent/`: The execution plane — agent runtime, sessions, the loop with its budgets/cancellation, and `metrics` counters. (The old Lane/Queue subsystem was removed in v0.48.0; it was never wired.)
- `crates/tool/`: Tool registry, sandbox executor, and policy engine.
- `crates/skill/`: Dynamic skill generation, validation, approval, and `SkillTool` (the callable wrapper around an approved skill).
- `crates/wechat/`: Personal WeChat channel over a connector-agnostic HTTP contract.
- `crates/provider/`: LLM provider integrations.
- `crates/memory/`: Memory management and safety scanning.
- `crates/compression/`: Context compression and session hygiene.
- `crates/feishu/`: Feishu (Lark) channel integration.
- `crates/cli/`: The `rustclaw` command-line interface for managing the gateway and tools.
- Other crates: `channel`, `config`, `protocol`, `storage`.

## Building and Running

### Prerequisites
- Rust 1.70+
- Cargo
- Optional: Docker (isolated execution backend — **recommended in production**, since skills are LLM-generated code)
- Optional: Python 3 (for Python skills)

### Commands
- **Build the project:**
  ```bash
  cargo build --release
  ```
- **Run the CLI to start the Gateway:**
  ```bash
  cargo run --bin rustclaw -- start
  # or
  ./target/release/rustclaw start
  ```
- **Run the Gateway directly:**
  ```bash
  cargo run --bin rustclaw-gateway
  # or
  ./target/release/rustclaw-gateway
  ```
- **Run tests:**
  ```bash
  cargo test
  ```

## Development Conventions

- **Async Runtime:** `tokio` is used extensively for asynchronous operations.
- **Web Framework:** `axum` combined with `tokio-tungstenite` for HTTP and WebSocket servers.
- **Data Serialization:** `serde` and `serde_json` for JSON parsing, and `toml` for configuration.
- **Logging:** Structured logging using the `tracing` crate.
- **Storage:** `rusqlite` for SQLite-based WAL persistent storage and full-text search.
- **Error Handling:** Standardized error handling using `thiserror` and `anyhow`.
- **Routing:** axum 0.7 path syntax is `:id`, **not** `{id}` — the 0.8 form is a literal segment in 0.7 and silently 404s every path-param route.
- **Paths:** always resolve through `Config::config_path()` / `skills_dir()` / `logs_dir()` / `data_dir()`; rebuilding `config_dir()/rustclaw/…` inline bypasses `RUSTCLAW_PROFILE` isolation and leaks credentials across profiles.
- **Safety:** Maintain strict sandboxing for external tool execution and directory protections to prevent path traversal attacks. Keep prompt-injection checks in the memory and context components. Generated skill code must stay behind the `dyn_skill_` prefix (mandatory approval) and the sandbox.

## Verification

```bash
cargo test --workspace       # 254 passed
cargo clippy --all-targets   # 0 warnings
```

See `docs/PRODUCTION_READINESS.md` for the production blocker checklist and how each item was verified.