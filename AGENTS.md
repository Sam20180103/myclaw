# PROJECT KNOWLEDGE BASE

**Generated:** 2026-03-06
**Commit:** 2d484fd
**Branch:** (current)

## OVERVIEW

RustClaw - Rust实现的本地AI智能体网关，参考OpenClaw架构，支持多通道接入、Agent运行时、工具系统、动态Skill和审批流程。

## STRUCTURE

```
./
├── Cargo.toml          # workspace root
├── crates/
│   ├── gateway/        # HTTP/WS服务器 + API
│   ├── agent/          # Agent运行时 (Session/Lane/Queue)
│   ├── tool/           # 工具系统 (Registry/Sandbox/Policy)
│   ├── skill/          # 动态Skill系统
│   ├── provider/       # LLM提供商 (OpenAI/Anthropic)
│   ├── protocol/       # JSON-RPC协议
│   ├── config/         # 配置管理
│   ├── channel/        # 通道抽象
│   ├── storage/        # 数据存储
│   └── cli/            # CLI工具
├── docs/
│   ├── roadmap.md      # 任务进度
│   └── architecture.md # 技术架构
└── tests/
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| HTTP Server | crates/gateway/src/lib.rs | Axum router + handlers |
| Agent Loop | crates/agent/src/lib.rs | Session/Lane/Queue + process_with_loop |
| Tool Execution | crates/tool/src/lib.rs | ToolRegistry + SandboxExecutor |
| Skill System | crates/skill/src/lib.rs | SkillManager + OpenCodeGenerator |
| LLM Providers | crates/provider/src/lib.rs | OpenAI + Anthropic clients |
| Protocol | crates/protocol/src/lib.rs | Envelope/Message/JsonRpc |

## CONVENTIONS

- **Async**: All I/O uses tokio
- **Error**: thiserror for error types
- **Logging**: tracing with structured logs
- **Testing**: #[cfg(test)] in each crate
- **Config**: TOML + env var override

## ANTI-PATTERNS (THIS PROJECT)

- DO NOT use `as any`, `@ts-ignore` (no TypeScript)
- DO NOT suppress errors with `unwrap()` in production code
- DO NOT block async runtime with blocking calls

## UNIQUE STYLES

- **Tool sandbox**: ShellExecTool uses SandboxExecutor with timeout (30s) + denied commands
- **Skill workflow**: Agent generates → pending/ → approval → active/
- **Lane routing**: session_key hash → 10 lanes (lane_0 to lane_9)

## COMMANDS

```bash
# Build
cargo build --release

# Run gateway
cargo run --bin rustclaw-gateway

# Run CLI
cargo run --bin rustclaw

# Test
cargo test

# Check
cargo check
```

## NOTES

- Default port: 127.0.0.1:18789
- Session TTL: 3600s (configurable)
- Max loop iterations: 100
- Provider: OpenAI or Anthropic (configurable)
