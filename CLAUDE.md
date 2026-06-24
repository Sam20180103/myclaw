# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

RustClaw (crate: `rustclaw`) is a local AI agent gateway implemented as a Cargo workspace. A long-running gateway accepts messages over multiple channels (WebSocket / HTTP / Webhook / Feishu), routes them to an agent runtime that drives an LLM loop with tool calls, and persists sessions and memory. Architecture is inspired by OpenClaw / Hermes Agent. Documentation is primarily in Chinese; `docs/architecture.md` is the authoritative design doc and tracks the current version (currently v0.45.0).

## Build / test / run

```bash
cargo build --release            # build all crates
cargo test                       # run the whole workspace test suite
cargo test -p rustclaw-agent     # test a single crate (see package names below)
cargo test -p rustclaw-tool test_sandbox   # run tests matching a substring in one crate
cargo clippy --all-targets       # lint (clippy + rustfmt are pinned via rust-toolchain.toml)
cargo fmt
```

Two binaries are produced:
- `rustclaw` (from `crates/cli`, package `rustclaw-cli`) — the CLI used to manage everything.
- `rustclaw-gateway` (from `crates/gateway`) — the core server, also launchable via `rustclaw start`.

```bash
./target/release/rustclaw start [--detach] [--port N]   # start gateway (default 127.0.0.1:18789)
./target/release/rustclaw stop | restart | status | health | doctor
./target/release/rustclaw config --show | --path | --get KEY | --set KEY=VALUE
```

`RUSTCLAW_HOST` / `RUSTCLAW_PORT` / `RUSTCLAW_LOG_LEVEL` env vars override config (see `.env.example`). `RUST_LOG` controls `tracing` verbosity.

## Workspace layout

Crate package names are `rustclaw-<dir>` (e.g. `crates/agent` → `rustclaw-agent`); the CLI crate is `rustclaw-cli` but its binary is `rustclaw`. Rough dependency flow is **cli/gateway → agent → {tool, skill, provider, memory, compression, channel} → {protocol, config, storage}**.

| Crate | Responsibility |
|-------|----------------|
| `gateway` | Axum HTTP/WS server, connection mgmt, session routing, auth, REST API. `GET /ws` is the generic inbound WebSocket endpoint (each text frame → one agent loop). Entry: `crates/gateway/src/lib.rs`. API endpoints catalogued in `docs/architecture.md` §8. |
| `agent` | Agent runtime — Session / Lane / Queue and the core loop (`process_with_loop`). `crates/agent/src/lib.rs`. `builtin_tools.rs` wraps `MemoryManager`/`SkillManager` as the `memory` / `skill_manage` tools (registered via `with_memory` / `with_skill_manager`). |
| `tool` | `ToolRegistry`, `SandboxExecutor`, policy engine, and the Agent-Reach tool family (`crates/tool/src/reach.rs`). |
| `skill` | Dynamic skill generation/validation/approval (`SkillManager`, `OpenCodeGenerator`). |
| `provider` | LLM provider clients: OpenAI, Anthropic, NVIDIA NIM. Streaming responses. |
| `memory` | Three-layer memory: `MEMORY.md` / `USER.md` files + prompt-injection / data-leak safety scanning. |
| `compression` | Two-level context compression (`SessionHygiene`, `ContextCompressor`). |
| `storage` | SQLite (rusqlite, WAL + FTS5) session persistence and search. |
| `channel` | Channel abstraction over message ingress. |
| `feishu` | Feishu/Lark channel (WebSocket long connection, interactive cards). |
| `wechat` | Personal WeChat channel via an external connector over a stable HTTP contract (`POST /channel/wechat/inbound` in, connector `send_url` out). `crates/wechat/`. |
| `protocol` | JSON-RPC envelope/message types shared across crates. |
| `config` | TOML config load/save and well-known directory paths. |

## Key architecture facts

- **Concurrency model:** same-session requests are serialized by a per-session `tokio::Mutex` (`session_locks` in `AgentRuntime`); different sessions run in parallel (one tokio task per request). `route_lane(session_key)` only computes a `lane_0..lane_9` label for grouping/observability (`Session.lane`) — the old Lane/Queue subsystem was removed in v0.48.0 as it was never wired. A periodic task in gateway `start()` calls `AgentRuntime::maintenance()` every 60s to evict expired/over-cap sessions and reclaim their locks.
- **Tool-call transcript (v0.48.0):** the agent loop persists the assistant's tool request as `Content::ToolUse { tool_calls, text }` and each result as a `Role::Tool` message with `Content::ToolResult { tool_call_id, name, output }` — not as `Role::System` text. Providers map these to native structures (OpenAI `tool_calls` + `role:"tool"`, Anthropic `tool_use`/`tool_result`). Tool execution is wrapped in `tool_timeout_ms`. Context compression runs on raw history and is persisted via `replace_messages` (no longer recomputed per-iteration on a throwaway copy). Renderable-image tool results (e.g. login QR) are detected generically by `extract_renderable_image`, not hardcoded per platform.
- **Tool sandbox:** `SandboxExecutor` enforces a 30s timeout and ~512MB cap, and blocks dangerous commands/patterns (`rm -rf /`, `mkfs`, `dd if=`, fork bombs, `curl|sh`, `chmod 777`, …). New shell-capable tools must go through it, not raw process spawns.
- **Skill approval workflow:** the agent generates a skill (often via the OpenCode CLI) → it lands in `~/.config/rustclaw/skills/pending/` → user approves → it moves to `~/.config/rustclaw/skills/` → the tool registry reloads → the skill becomes callable. Skills can be Rust/Python/Shell/JS.
- **Memory/compression are active (v0.43.0+)** and wired into the gateway via DI and the main agent loop; changes there affect token cost and prompt-cache behavior (Anthropic `cache_control`).
- **Tool governance (v0.49.0, G1):** `AgentRuntime` enforces a `ToolPolicy` (deny/allow + `require_approval`) in `execute_tool` — denied tools error out, `require_approval` tools call an async `ApprovalHook` (fail-closed: no hook = deny). Gateway builds the policy from `config.tools.{disabled_tools,require_approval}` (NOT `enabled_tools`, whose default is a stale subset) and backs the hook with an `ApprovalRegistry` (120s timeout) exposed via `GET/POST /api/tools/approvals[/{id}]`. Default `require_approval=["shell_exec"]` means shell exec now needs operator approval.
- **Prompt caching (v0.49.0, G2):** the Anthropic provider hoists leading `System` messages into the top-level `system` param with `cache_control: ephemeral` on the last block, and marks the last tool with `cache_control` — caching the stable (system + tools) prefix. The agent injects `[base_system, memory, …history]` for a stable cacheable prefix. OpenAI caches automatically (no change).
- **Provider resilience (v0.49.0, G3):** `ProviderClient` holds an ordered provider list (primary + fallbacks); retryable errors (RateLimited / 5xx via `is_retryable`) get exponential backoff, then it fails over to the next provider. Gateway constructs ALL configured providers, not just the first. (Per-request model override still TODO.)
- **Tool availability gating (v0.50.0, G4):** the `Tool` trait has `is_available()` (default true); the agent builds its LLM tool list from `ToolRegistry::available_schemas()` (filtered), while `list()` still returns all for management views. `image_gen` (needs base_url+api_key) and `search_reach` (needs exa_api_key) hide themselves when unconfigured.
- **Secret env indirection (v0.50.0, G9):** `Config::resolve_secret_refs()` (run in `Config::load`) replaces any secret value of the form `env:VARNAME` with the environment variable's value, so keys/cookies need not live in `config.toml` plaintext. Covers provider/gateway/reach/feishu/wechat secrets.
- **Execution backends (v0.50.0, G5):** `ExecutionBackend` (`Local` | `Docker{image,network}`) abstracts where shell commands run; `SandboxExecutor::with_backend` selects it, chosen via `RUSTCLAW_SANDBOX_BACKEND` (`local` | `docker:IMG` | `docker+net:IMG`, default local). Docker runs `docker run --rm [--network none] [--memory Nm] --security-opt no-new-privileges …`.
- **Self-evolution tools (v0.45.0):** the agent can call `memory` (add/replace/remove/list) and `skill_manage` (create/list/patch/delete) directly; `skill_manage create` still routes new skills through the user approval workflow. The tool count is 22 (5 builtin + 15 reach incl. `image_gen` text-to-image + these 2), not the "38+" some older docs claim.
- **Vision (v0.45.0):** `Content::Image` is sent as true multimodal content — OpenAI `image_url` data URLs, Anthropic `base64` source blocks (remote URLs downgrade to text for Anthropic). Inbound Feishu images are downloaded via the message-resource endpoint and converted to `Content::Image`. Conversion lives in `provider`'s `openai_content`/`anthropic_content` helpers.
- **Channels reality:** real inbound = HTTP REST (`/api/message`) + WebSocket (`/ws`) + Webhook (`POST /webhook`, `/webhook/{source}`, sync or `callback_url` async) + Feishu long connection + personal WeChat (`POST /channel/wechat/inbound`, via an external connector; out via `channels.wechat.connector_send_url`). The `channel` crate's WebSocket/HTTP/Webhook impls are still trait placeholders — actual ingress lives in the gateway's axum handlers and the `feishu`/`wechat` channel crates.
- **MCP bridging** exists for advanced platform tools via a JSON-RPC `McpClient`. Vendored Playwright MCP servers live under `vendor/` (`mcp-bosszp` for BOSS直聘, `mcp-xhs` for Xiaohongshu; both use phone+SMS two-step login) plus `wechat-connector` (reference) and `open-lark` — none are part of the Cargo build. `xhs_reach` supports `hot`/`publish` (with `image_gen`-generated images); BOSS uses `login_send_sms`/`login_submit_sms`. See `docs/XHS_GUIDE.md`, `docs/BOSS_RECRUITMENT.md`.

## Config and data locations

Resolved via the `dirs` crate (`config_dir()` → typically `~/.config/rustclaw` on Linux):
- `config.toml` — `Config::config_path()`
- `skills/` and `skills/pending/` — `Config::skills_dir()` / `pending_skills_dir()`
- logs under the platform local-data dir (`Config::logs_dir()`)

On first run a default `config.toml` is written if none exists. Default session TTL is 3600s; max agent loop iterations is 100.

## Conventions

- All I/O is async on `tokio`; never block the runtime with sync calls.
- Errors use `thiserror` for typed library errors and `anyhow` at the binary boundary. Avoid `unwrap()` in non-test code paths.
- Each crate keeps its unit tests inline under `#[cfg(test)]`; cross-crate tests live in `tests/integration_tests.rs`.
- `protoc-27.1-linux-x86_64.zip` at the repo root is an unused leftover; there is no protobuf in the build (the `protoc` matches are just `rustclaw-protocol`).

## Notes for working here

`opencode-loop.sh` / `gemini-loop.sh` / `merge4gemini.sh` and the various `*-loop.log` / `*.log` files are external automation/agent-driver scripts and their output, not part of the application. `issues.md` and `docs/roadmap.md` track outstanding work and phase status.
