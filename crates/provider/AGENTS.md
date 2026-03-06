# Provider Crate

> Parent: [AGENTS.md](../../AGENTS.md)

## OVERVIEW

LLM Provider抽象层，支持OpenAI和Anthropic客户端，含流式响应。

## KEY STRUCTURES

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| LlmProvider | trait | lib.rs:12 | Provider抽象 |
| OpenAiProvider | struct | lib.rs:100 | OpenAI客户端 |
| AnthropicProvider | struct | lib.rs:252 | Anthropic客户端 |
| ProviderClient | struct | lib.rs:593 | 统一客户端 |

## STREAMING

- `stream_complete()` returns `TokenStream` (SSE)
- OpenAI: parses `data: {...}` chunks
- Anthropic: parses content blocks

## TEST

- No dedicated test file (covered by integration tests)
