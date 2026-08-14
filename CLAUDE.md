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
- **Skill approval workflow (closed for Shell/Python since v0.54.0):** the agent generates a skill → `submit_skill` writes `<skills>/pending/<id>.json` → the user approves (`POST /api/skill/:id/approve`) → `approve_skill` renames it to `<skills>/active/<id>.json` **and registers it as a callable tool** named `dyn_skill_<sanitized name>` (`SkillManager::register_skill_tool` → `ToolRegistry`). `SkillManager::init()` (gateway `start()`) re-registers every active skill after a restart; `delete_skill` unregisters. The registry is injected post-Arc via `skill_manager.set_tool_registry(agent.tool_registry().clone())`.
  - *Only Shell and Python are callable.* `SkillTool::from_definition` returns `None` for Rust/JavaScript — those stay catalog-only (no compiler / node runtime). `GET /api/skill` lists what is *registered*; `GET /api/tools` lists what is actually *callable* (with `is_skill` / `available` / `allowed` / `requires_approval`).
  - *Execution*: source is inlined into `sh -c` (`sh -c '<src>' <name> '<args json>'` / `python3 -c '<src>' '<args json>'`) so it works identically on the Local and Docker backends — no temp files, since Docker mounts nothing. Args reach the skill as **one JSON string argv**; both source and args go through POSIX single-quote escaping (`test_injected_args_are_data_not_code` proves args cannot escape into code). Everything runs through `SandboxExecutor` (denied-command list — which also matches the generated source — plus 30s timeout and memory cap). Backend from `RUSTCLAW_SANDBOX_BACKEND`; the Local backend logs a loud warning because generated code then runs on the host — **use `docker:<image>` in production**.
  - *Security*: the tool name prefix `SKILL_TOOL_PREFIX = "dyn_skill_"` is a boundary, not decoration — it keeps generated skills from shadowing builtins, and `ToolPolicy::requires_approval` returns `true` for anything with that prefix **unconditionally** (config cannot waive it; to kill a skill use `disabled_tools`). The prefix deliberately is not `skill_`, which would have swept the builtin `skill_manage` into forced approval. So each call needs a human OK via the P1-9 approval path (`/approve <id>` in chat or `POST /api/tools/approvals/:id`), on top of the one-time skill approval.
- **Memory/compression are active (v0.43.0+)** and wired into the gateway via DI and the main agent loop; changes there affect token cost and prompt-cache behavior (Anthropic `cache_control`).
- **Tool governance (v0.49.0, G1):** `AgentRuntime` enforces a `ToolPolicy` (deny/allow + `require_approval`) in `execute_tool` — denied tools error out, `require_approval` tools call an async `ApprovalHook` (fail-closed: no hook = deny). Gateway builds the policy from `config.tools.{disabled_tools,require_approval}` (NOT `enabled_tools`, whose default is a stale subset) and backs the hook with an `ApprovalRegistry` (120s timeout) exposed via `GET/POST /api/tools/approvals[/:id]`. Default `require_approval=["shell_exec"]` means shell exec now needs operator approval.
- **Prompt caching (v0.49.0, G2):** the Anthropic provider hoists leading `System` messages into the top-level `system` param with `cache_control: ephemeral` on the last block, and marks the last tool with `cache_control` — caching the stable (system + tools) prefix. The agent injects `[base_system, memory, …history]` for a stable cacheable prefix. OpenAI caches automatically (no change).
- **Provider resilience (v0.49.0, G3):** `ProviderClient` holds an ordered provider list (primary + fallbacks); retryable errors (RateLimited / 5xx via `is_retryable`) get exponential backoff, then it fails over to the next provider. Gateway constructs ALL configured providers, not just the first. (Per-request model override still TODO.)
- **Tool availability gating (v0.50.0, G4):** the `Tool` trait has `is_available()` (default true); the agent builds its LLM tool list from `ToolRegistry::available_schemas()` (filtered), while `list()` still returns all for management views. `image_gen` (needs base_url+api_key) and `search_reach` (needs exa_api_key) hide themselves when unconfigured.
- **Secret env indirection (v0.50.0, G9):** `Config::resolve_secret_refs()` (run in `Config::load`) replaces any secret value of the form `env:VARNAME` with the environment variable's value, so keys/cookies need not live in `config.toml` plaintext. Covers provider/gateway/reach/feishu/wechat secrets.
- **Execution backends (v0.50.0, G5):** `ExecutionBackend` (`Local` | `Docker{image,network}`) abstracts where shell commands run; `SandboxExecutor::with_backend` selects it, chosen via `RUSTCLAW_SANDBOX_BACKEND` (`local` | `docker:IMG` | `docker+net:IMG`, default local). Docker runs `docker run --rm [--network none] [--memory Nm] --security-opt no-new-privileges …`.
- **Memory provider abstraction (v0.51.0, G6):** agent and the builtin `memory` tool depend on the `MemoryProvider` trait (`crates/memory`), not the concrete `MemoryManager` (which is the default file-backed impl). Alternative backends (vector search, external KV) implement the same trait.
- **Profile isolation (v0.51.0, G7; fixed in v0.53.0):** `RUSTCLAW_PROFILE=<name>` isolates config/skills/logs (`~/.config/rustclaw/profiles/<name>/…`) and runtime data (`~/.rustclaw/profiles/<name>`, via `Config::data_dir()`). Unset or `default` keeps legacy paths. Profile names are sanitized to `[A-Za-z0-9_-]`. **Always resolve paths through `Config::config_path()` / `skills_dir()` / `logs_dir()` / `data_dir()`** — until v0.53.0 six call sites (`Config::load`, four CLI helpers, `SkillConfig::default`) rebuilt `config_dir()/rustclaw/…` inline, so a named profile still read the default profile's `config.toml` and its Feishu/provider credentials.
- **Scheduled tasks (v0.51.0, G8):** `[[schedules]]` entries in config.toml (name/every/prompt/enabled/session_key) make gateway `start()` spawn a ticker per enabled entry that drives `process_with_loop` on channel `"schedule"` with session key `schedule:<name>` (shared context across firings). Intervals parse as `30s/10m/2h/1d` via `rustclaw_config::parse_interval` (min 10s); results go to logs, plus `notify_session`/`notify_channel` if configured (P2-16).
- **Sub-agents (v0.52.0, G10):** the `spawn_subagent` tool delegates a self-contained subtask to a child session (`sub:<uuid>`, channel `subagent`, lineage via `parent_session_id`). Registered post-Arc via `AgentRuntime::enable_subagent()` (tool holds a `Weak`); budget clamped to ≤10 iterations through `process_with_loop_budget`; max 4 concurrent sub-agents; the kernel injects `_parent_session_key` into the tool args (not model-provided). Sub-agents are still subject to `tool_timeout_ms`.
- **Trajectory export (v0.52.0, G11):** `rustclaw sessions --export sharegpt [--session KEY] [--output FILE]` reads SQLite directly (gateway not required) and emits ShareGPT JSONL via `rustclaw_storage::session_to_sharegpt` (ToolUse/ToolResult serialized as `<tool_calls>`/`<tool_result>` markers).
- **OpenAI Responses API (v0.52.0, G12):** `provider_type = "openai-responses"` selects `OpenAiResponsesProvider` (`/v1/responses`, `input` item array with top-level `function_call`/`function_call_output`, flat tool schema). Streaming is minimal (full text in one chunk).
- **Production hardening (v0.53.0, P0–P2 + X-1)** — see `docs/PRODUCTION_READINESS.md`:
  - *Compression is pairing-safe*: `clean_old_tool_results` now **elides tool-result bodies** (keeps the last 3 in full) instead of deleting messages, and every truncation runs through `rustclaw_compression::sanitize_pairing`, which drops orphan `ToolResult`s and synthesizes placeholders for `tool_use` whose result was cut. Orphans used to make Anthropic/OpenAI return 400 — and the damage was persisted via `replace_messages`.
  - *Compression summarizes*: `CompressionResult.dropped` carries the discarded span; the agent calls `summarize_dropped` (LLM, falling back to `ContextCompressor::local_summary`) and inserts the summary as a System message so the agent does not repeat side-effecting actions after a compaction.
  - *Budgets are tokens, not bytes*: `estimate_tokens` / `estimate_text_tokens` (ASCII ≈ 4 chars/token, CJK ≈ 1). Window + threshold come from `[agent] context_window_tokens` / `compression_threshold`.
  - *Inbound de-duplication*: `SQLiteSessionStore::mark_event_seen` (`processed_events` table, INSERT OR IGNORE) gates Feishu (`message_id`), WeChat (`msg_id`) and Webhook (`event_id`/`id`) ingress; `cleanup_events` runs hourly with a 7-day retention.
  - *Cancellation + budgets*: per-session `watch` channel drives `AgentRuntime::cancel_session`; the loop races cancellation against both the LLM call and the tool batch. `max_loop_duration_ms` (wall clock) and `max_loop_iterations` now return a **partial `AgentResponse`** (prefixed `[已取消]` / `[未完成 · 超时]` / `[未完成 · 达到迭代上限]`) instead of `Err`.
  - *Chat control commands*: `/clear`, `/stop`|`/cancel`, `/approve <id>`, `/reject <id>` are handled by `Gateway::handle_chat_command` **before** the session lock, so they can never queue behind the running task.
  - *Progress + approval push*: `AgentEvent` variants carry `session`; gateway broadcasts them and pushes "⏳ 正在调用工具: X" (≥5s throttle per session) and approval prompts back through the originating channel. `ApprovalHook` now takes `(tool_name, args, session_key)`.
  - *Tool hygiene*: results are truncated head+tail to `max_tool_result_chars` (default 16k) before entering history; args are validated against the tool's JSON Schema (`validate_tool_args`) so bad calls come back as a fixable error; a turn's tool calls now execute **concurrently** (`join_all`) with results replayed in request order.
  - *Observability*: `rustclaw_agent::metrics` (atomic counters) + `rustclaw_provider::{record_usage, usage_snapshot}` back `GET /metrics` (Prometheus text) and `GET /api/usage`. `POST /api/session/:id/cancel` cancels a run.
  - *Per-caller tool policy*: `Envelope.principal` (Feishu `open_id` / WeChat `wxid`) selects `ToolPolicy::for_principal`; overrides live in `[tools.principals.<id>]`.
  - *Dead code removed*: `AgentRuntime::process` and `process_stream` had zero callers and bypassed tools/memory/compression.
  - **Routes use axum 0.7 syntax (`:id`, not `{id}`)** — the crate pins `axum = "0.7"`, where `{id}` is a literal segment; every path-param route was silently 404 until this was fixed.
- **Self-evolution tools (v0.45.0):** the agent can call `memory` (add/replace/remove/list) and `skill_manage` (create/list/patch/delete) directly; `skill_manage create` still routes new skills through the user approval workflow. The tool count is 23 (5 builtin + 15 reach incl. `image_gen` text-to-image + these 2 + `spawn_subagent`), not the "38+" some older docs claim.
- **Vision (v0.45.0):** `Content::Image` is sent as true multimodal content — OpenAI `image_url` data URLs, Anthropic `base64` source blocks (remote URLs downgrade to text for Anthropic). Inbound Feishu images are downloaded via the message-resource endpoint and converted to `Content::Image`. Conversion lives in `provider`'s `openai_content`/`anthropic_content` helpers.
- **Channels reality:** real inbound = HTTP REST (`/api/message`) + WebSocket (`/ws`) + Webhook (`POST /webhook`, `/webhook/:source`, sync or `callback_url` async; `event_id`/`id` in the body is de-duplicated) + Feishu long connection + personal WeChat (`POST /channel/wechat/inbound`, via an external connector; out via `channels.wechat.connector_send_url`). The `channel` crate's WebSocket/HTTP/Webhook impls are still trait placeholders — actual ingress lives in the gateway's axum handlers and the `feishu`/`wechat` channel crates.
- **MCP bridging** exists for advanced platform tools via a JSON-RPC `McpClient`. Vendored Playwright MCP servers live under `vendor/` (`mcp-bosszp` for BOSS直聘, `mcp-xhs` for Xiaohongshu; both use phone+SMS two-step login) plus `wechat-connector` (reference) and `open-lark` — none are part of the Cargo build. `xhs_reach` supports `hot`/`publish` (with `image_gen`-generated images); BOSS uses `login_send_sms`/`login_submit_sms`. See `docs/XHS_GUIDE.md`, `docs/BOSS_RECRUITMENT.md`.

## Config and data locations

Resolved via the `dirs` crate (`config_dir()` → typically `~/.config/rustclaw` on Linux):
- `config.toml` — `Config::config_path()`
- `skills/`, `skills/pending/`, `skills/active/` — `Config::skills_dir()` / `pending_skills_dir()` / `active_skills_dir()`
- logs under the platform local-data dir (`Config::logs_dir()`)

On first run a default `config.toml` is written if none exists. Default session TTL is 3600s; max agent loop iterations is 100.

## Conventions

- All I/O is async on `tokio`; never block the runtime with sync calls.
- Errors use `thiserror` for typed library errors and `anyhow` at the binary boundary. Avoid `unwrap()` in non-test code paths.
- Each crate keeps its unit tests inline under `#[cfg(test)]`; cross-crate tests live in `tests/integration_tests.rs`.
- `protoc-27.1-linux-x86_64.zip` at the repo root is an unused leftover; there is no protobuf in the build (the `protoc` matches are just `rustclaw-protocol`).

## Notes for working here

`opencode-loop.sh` / `gemini-loop.sh` / `merge4gemini.sh` and the various `*-loop.log` / `*.log` files are external automation/agent-driver scripts and their output, not part of the application. `issues.md` and `docs/roadmap.md` track outstanding work and phase status.
