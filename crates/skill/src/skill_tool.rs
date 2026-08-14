//! X-3: 把已批准的 Skill 包装成可调用的 `Tool`,闭合"生成 → 批准 → 可调用"的环。
//!
//! 只支持 **Shell / Python** 两种语言(Rust 需要编译、JavaScript 需要 node 运行时,
//! 均不在本期范围)。执行一律经 `SandboxExecutor`:
//! - 危险命令黑名单(源码本身也参与匹配)
//! - 30s 超时 + 内存上限
//! - 后端由 `RUSTCLAW_SANDBOX_BACKEND` 选择,生产建议 `docker:<image>`
//!
//! 安全前提:Skill 源码是 **LLM 生成的不可信代码**。因此 `skill_*` 工具在
//! `ToolPolicy::requires_approval` 里被无条件标记为需要审批(见 `rustclaw_tool`)。

use crate::{SkillDefinition, SkillLanguage};
use async_trait::async_trait;
use rustclaw_tool::{
    ExecutionBackend, SandboxConfig, SandboxExecutor, Tool, ToolError, ToolSchema,
    SKILL_TOOL_PREFIX,
};

/// 一个已批准 Skill 的可调用封装。
pub struct SkillTool {
    tool_name: String,
    description: String,
    parameters: serde_json::Value,
    language: SkillLanguage,
    source_code: String,
    sandbox: SandboxExecutor,
}

impl SkillTool {
    /// 从技能定义构造工具。Rust / JavaScript 返回 `None` —— 它们仍可被创建与列出,
    /// 只是不可调用(需要编译器 / node 运行时,本期不做)。
    pub fn from_definition(def: &SkillDefinition) -> Option<Self> {
        match def.language {
            SkillLanguage::Shell | SkillLanguage::Python => {}
            SkillLanguage::Rust | SkillLanguage::JavaScript => return None,
        }

        // G5: 后端可经 RUSTCLAW_SANDBOX_BACKEND 选择,默认 local
        let backend = std::env::var("RUSTCLAW_SANDBOX_BACKEND")
            .ok()
            .map(|s| ExecutionBackend::from_spec(&s))
            .unwrap_or_default();

        let parameters = if def.parameters.get("type").is_some() {
            def.parameters.clone()
        } else {
            // 技能没写 schema 时给一个自由对象,避免 P1-12 的参数校验误杀
            serde_json::json!({ "type": "object" })
        };

        if matches!(backend, ExecutionBackend::Local) {
            tracing::warn!(
                skill = %def.name,
                "skill will run with the LOCAL sandbox backend — generated code executes on this host. \
                 Set RUSTCLAW_SANDBOX_BACKEND=docker:<image> in production."
            );
        }

        Some(Self {
            tool_name: tool_name_for(&def.name, &def.id),
            description: format!(
                "[skill/{}] {} (由 Agent 生成并经人工批准,沙箱内执行)",
                def.language, def.description
            ),
            parameters,
            language: def.language.clone(),
            source_code: def.source_code.clone(),
            sandbox: SandboxExecutor::new(SandboxConfig::default()).with_backend(backend),
        })
    }

    /// 构造实际执行的命令行。参数以 **单个 JSON 字符串实参**传入,
    /// 两种后端都是 `sh -c <command>`,所以这里内联源码、不落临时文件
    /// (docker 后端没有挂载宿主目录,临时文件在容器里不存在)。
    fn build_command(&self, args_json: &str) -> String {
        match self.language {
            // 内层 sh: $0 = 技能名, $1 = 参数 JSON
            SkillLanguage::Shell => format!(
                "sh -c {} {} {}",
                shell_quote(&self.source_code),
                shell_quote(&self.tool_name),
                shell_quote(args_json)
            ),
            // python -c: argv[0] = "-c", argv[1] = 参数 JSON
            SkillLanguage::Python => format!(
                "python3 -c {} {}",
                shell_quote(&self.source_code),
                shell_quote(args_json)
            ),
            _ => unreachable!("from_definition rejects other languages"),
        }
    }
}

/// 技能工具名: `skill_<sanitized>`。
///
/// 前缀是**安全边界**,不是装饰:生成的技能若叫 `shell_exec`,没有前缀就会在
/// `ToolRegistry` 里顶掉内置工具;有前缀则永远落在 `skill_` 命名空间内,
/// 且能被 `ToolPolicy` 一律要求审批。
pub fn tool_name_for(skill_name: &str, skill_id: &str) -> String {
    let sanitized: String = skill_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('_').to_string();
    let base = if sanitized.is_empty() {
        skill_id.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
    } else {
        sanitized
    };
    let base: String = base.chars().take(48).collect();
    format!("{}{}", SKILL_TOOL_PREFIX, base)
}

/// POSIX 单引号转义: 包一层 `'…'`,内部的 `'` 换成 `'\''`。
/// 技能源码与参数 JSON 都要经过它,否则一个引号就能拼出任意命令。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.tool_name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let args_json = serde_json::to_string(&params)
            .map_err(|e| ToolError::InvalidParams(e.to_string()))?;
        let command = self.build_command(&args_json);

        let result = self.sandbox.execute_command(&command).await?;

        // 退出码非 0 视为失败,让模型看到 stderr 并自行修正
        let exit_code = result.get("exit_code").and_then(|v| v.as_i64());
        if exit_code != Some(0) {
            let stderr = result
                .get("stderr")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .take(2000)
                .collect::<String>();
            return Err(ToolError::ExecutionFailed(format!(
                "skill '{}' exited with {:?}: {}",
                self.tool_name, exit_code, stderr
            )));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(language: SkillLanguage, name: &str, source: &str) -> SkillDefinition {
        SkillDefinition {
            id: "abc-123".to_string(),
            name: name.to_string(),
            description: "test skill".to_string(),
            version: "1.0.0".to_string(),
            language,
            source_code: source.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "who": { "type": "string" } },
                "required": ["who"]
            }),
            created_at: 0,
            author: None,
            test_cases: vec![],
            category: None,
            references: vec![],
        }
    }

    #[test]
    fn test_tool_name_is_namespaced_and_sanitized() {
        // 前缀是安全边界: 生成的技能不能顶掉内置工具
        assert_eq!(tool_name_for("shell_exec", "id1"), "dyn_skill_shell_exec");
        assert_eq!(tool_name_for("My Cool Skill!", "id1"), "dyn_skill_my_cool_skill");
        assert_eq!(tool_name_for("中文名", "id42"), "dyn_skill_id42");
        // 工具名不得超过 OpenAI/Anthropic 的 64 字符上限
        assert!(tool_name_for("x".repeat(200).as_str(), "id").len() <= 64);
        // 不能与内置的 skill_manage 撞名空间
        assert!(!tool_name_for("manage", "id").starts_with("skill_manage"));
    }

    #[test]
    fn test_shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_quote("plain"), "'plain'");
    }

    #[tokio::test]
    async fn test_injected_args_are_data_not_code() {
        // 关键安全性质: 参数里的引号/分号不能逃逸成命令,只能原样抵达技能。
        // (只断言"转义后的字符串不含某子串"是无效代理 —— 转义形式里仍会出现该子串。)
        let marker = std::env::temp_dir().join(format!("rustclaw_pwned_{}", uuid::Uuid::new_v4()));
        let evil = format!("'; touch {}; echo '", marker.display());

        let tool = SkillTool::from_definition(&def(
            SkillLanguage::Shell,
            "echoer",
            r#"printf %s "$1""#,
        ))
        .unwrap();

        let out = tool
            .execute(serde_json::json!({ "who": evil }))
            .await
            .expect("skill should run");

        // 注入串原样作为 JSON 参数抵达技能内部
        let stdout = out["stdout"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout).expect("args arrive as JSON");
        assert_eq!(parsed["who"].as_str().unwrap(), evil);
        // 且没有被当成命令执行
        assert!(!marker.exists(), "命令注入成功了: {:?}", marker);
        std::fs::remove_file(&marker).ok();
    }

    #[test]
    fn test_rust_and_js_are_not_callable() {
        // 本期只闭环 shell/python;其余语言仍可创建与列出,但不注册为工具
        assert!(SkillTool::from_definition(&def(SkillLanguage::Rust, "r", "fn main(){}")).is_none());
        assert!(
            SkillTool::from_definition(&def(SkillLanguage::JavaScript, "j", "console.log(1)"))
                .is_none()
        );
        assert!(SkillTool::from_definition(&def(SkillLanguage::Shell, "s", "echo hi")).is_some());
        assert!(SkillTool::from_definition(&def(SkillLanguage::Python, "p", "print(1)")).is_some());
    }

    #[tokio::test]
    async fn test_shell_skill_receives_args_json() {
        let tool = SkillTool::from_definition(&def(
            SkillLanguage::Shell,
            "greet",
            r#"echo "hello $(echo "$1" | tr -d '{}"' )" "#,
        ))
        .unwrap();
        assert_eq!(tool.name(), "dyn_skill_greet");

        let out = tool
            .execute(serde_json::json!({ "who": "world" }))
            .await
            .expect("shell skill should run");
        let stdout = out["stdout"].as_str().unwrap();
        assert!(stdout.contains("world"), "got: {}", stdout);
    }

    #[tokio::test]
    async fn test_python_skill_parses_args_json() {
        let tool = SkillTool::from_definition(&def(
            SkillLanguage::Python,
            "py greet",
            "import json,sys; a=json.loads(sys.argv[1]); print('hi', a['who'])",
        ))
        .unwrap();
        assert_eq!(tool.name(), "dyn_skill_py_greet");

        let out = tool
            .execute(serde_json::json!({ "who": "alice" }))
            .await
            .expect("python skill should run");
        assert!(out["stdout"].as_str().unwrap().contains("hi alice"));
    }

    #[tokio::test]
    async fn test_failing_skill_surfaces_stderr() {
        let tool = SkillTool::from_definition(&def(
            SkillLanguage::Python,
            "boom",
            "import sys; sys.stderr.write('kaboom'); sys.exit(3)",
        ))
        .unwrap();
        let err = tool.execute(serde_json::json!({})).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("kaboom"), "got: {}", msg);
    }

    #[tokio::test]
    async fn test_dangerous_source_is_blocked_by_sandbox() {
        // 源码本身也参与危险命令匹配 —— 沙箱黑名单对生成代码同样生效
        let tool =
            SkillTool::from_definition(&def(SkillLanguage::Shell, "nuke", "rm -rf / --no-preserve-root"))
                .unwrap();
        let err = tool.execute(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::SandboxViolation(_)), "got: {:?}", err);
    }
}
