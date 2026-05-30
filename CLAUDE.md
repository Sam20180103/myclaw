# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

RustClaw (crate: `rustclaw`) is a local AI agent gateway implemented as a Cargo workspace. A long-running gateway accepts messages over multiple channels (WebSocket / HTTP / Webhook / Feishu), routes them to an agent runtime that drives an LLM loop with tool calls, and persists sessions and memory. Architecture is inspired by OpenClaw / Hermes Agent. Documentation is primarily in Chinese; `docs/architecture.md` is the authoritative design doc and tracks the current version (currently v0.44.0).

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
| `gateway` | Axum HTTP/WS server, connection mgmt, session routing, auth, REST API. Entry: `crates/gateway/src/lib.rs`. API endpoints catalogued in `docs/architecture.md` §8. |
| `agent` | Agent runtime — Session / Lane / Queue and the core loop (`process_with_loop`). `crates/agent/src/lib.rs`. |
| `tool` | `ToolRegistry`, `SandboxExecutor`, policy engine, and the Agent-Reach tool family (`crates/tool/src/reach.rs`). |
| `skill` | Dynamic skill generation/validation/approval (`SkillManager`, `OpenCodeGenerator`). |
| `provider` | LLM provider clients: OpenAI, Anthropic, NVIDIA NIM. Streaming responses. |
| `memory` | Three-layer memory: `MEMORY.md` / `USER.md` files + prompt-injection / data-leak safety scanning. |
| `compression` | Two-level context compression (`SessionHygiene`, `ContextCompressor`). |
| `storage` | SQLite (rusqlite, WAL + FTS5) session persistence and search. |
| `channel` | Channel abstraction over message ingress. |
| `feishu` | Feishu/Lark channel (WebSocket long connection, interactive cards). |
| `protocol` | JSON-RPC envelope/message types shared across crates. |
| `config` | TOML config load/save and well-known directory paths. |

## Key architecture facts

- **Lane routing:** sessions are hashed by `session_key` into a fixed set of lanes (lane_0..lane_9) for concurrent processing — relevant when reasoning about ordering and concurrency in `agent`.
- **Tool sandbox:** `SandboxExecutor` enforces a 30s timeout and ~512MB cap, and blocks dangerous commands/patterns (`rm -rf /`, `mkfs`, `dd if=`, fork bombs, `curl|sh`, `chmod 777`, …). New shell-capable tools must go through it, not raw process spawns.
- **Skill approval workflow:** the agent generates a skill (often via the OpenCode CLI) → it lands in `~/.config/rustclaw/skills/pending/` → user approves → it moves to `~/.config/rustclaw/skills/` → the tool registry reloads → the skill becomes callable. Skills can be Rust/Python/Shell/JS.
- **Memory/compression are active (v0.43.0+)** and wired into the gateway via DI and the main agent loop; changes there affect token cost and prompt-cache behavior (Anthropic `cache_control`).
- **MCP bridging** exists for advanced platform tools (e.g. Xiaohongshu/Douyin) via a JSON-RPC `McpClient`. Vendored MCP servers live under `vendor/` (`mcp-bosszp`, `open-lark`) and are not part of the Cargo build.

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
