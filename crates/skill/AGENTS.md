# Skill Crate

> Parent: [AGENTS.md](../../AGENTS.md)

## OVERVIEW

动态Skill系统，支持Skill生成、验证、审批流程。

## KEY STRUCTURES

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| SkillManager | struct | lib.rs:70 | 核心管理器 |
| SkillDefinition | struct | lib.rs:11 | Skill定义 |
| SkillLanguage | enum | lib.rs:33 | 编程语言 |
| OpenCodeGenerator | struct | lib.rs:417 | OpenCode CLI集成 |
| SkillStatus | enum | lib.rs:52 | 状态机 |

## WORKFLOW

```
create_skill() → validate → submit_skill() 
    → pending/ → approve_skill() → active/
```

## OPENCODE CLI

- `OpenCodeGenerator::generate_skill()` 调用opencode CLI
- 超时: 120s
- 自动检测opencode路径

## TEST

- No dedicated test file
