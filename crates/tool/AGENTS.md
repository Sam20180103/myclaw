# Tool Crate

> Parent: [AGENTS.md](../../AGENTS.md)

## OVERVIEW

工具系统，含注册表、沙箱执行、策略引擎。

## KEY STRUCTURES

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| ToolRegistry | struct | lib.rs:7 | 工具注册表 |
| Tool | trait | lib.rs:12 | 工具抽象 |
| SandboxExecutor | struct | lib.rs:327 | 沙箱隔离执行 |
| ToolPolicy | struct | lib.rs:258 | 权限策略 |
| ToolExecutor | struct | lib.rs:296 | 执行器 |

## BUILT-IN TOOLS

- `file_read`: 读取文件
- `file_write`: 写入文件
- `shell_exec`: 带沙箱的shell执行

## SANDBOX

- 超时: 30s (默认)
- 拒绝命令: rm -rf /, mkfs, fork bomb
- 危险模式检测: curl|sh, chmod 777

## TEST

- `#[cfg(test)]` at lib.rs:448
