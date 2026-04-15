# RustClaw

## Project Overview

RustClaw is a high-performance local AI agent gateway implemented in Rust. Its architecture is inspired by OpenClaw and Nous Research Hermes Agent, offering a comprehensive suite of features for running and managing AI agents locally. 

Key features include:
- **Multi-Channel Access:** Supports WebSocket, HTTP, Webhooks, and Feishu.
- **Agent Runtime:** Capable of handling concurrent sessions, agent loops, and streaming responses (SSE).
- **Tool System:** Features a robust registry with 38+ native tools, complete with path traversal protections and safety sandboxing. Includes Agent-Reach tools (Web, Video, GitHub, Social Platforms).
- **Dynamic Skills:** Allows agents to generate and register new skills dynamically, subject to an approval workflow.
- **Provider Abstraction:** Supports multiple LLM providers such as OpenAI, Anthropic, and NVIDIA NIM.
- **Memory System:** Employs a three-layer memory architecture (Instant Memory, Persistent Memory via `MEMORY.md`/`USER.md`, and SQLite Session Store with FTS5 search) for continuous learning and contextual awareness. Fully integrated and active via Dependency Injection in the Gateway (v0.43.0).
- **Context Management:** Features active dual-level context compression and Anthropic prompt caching strategies to optimize token usage. Activated in the main Agent loop.

## Architecture

The project uses a Cargo workspace with a layered architecture divided into several crates:
- `crates/gateway/`: The control plane handling connections, routing, and auth (axum web server).
- `crates/agent/`: The execution plane managing agent runtime, sessions, lanes, and queues.
- `crates/tool/`: Tool registry, sandbox executor, and policy engine.
- `crates/skill/`: Dynamic skill generation, validation, and approval.
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
- **Safety:** Maintain strict sandboxing for external tool execution and directory protections to prevent path traversal attacks. Always ensure prompt injection safety checks are maintained in the memory and context components.