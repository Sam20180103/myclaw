# 实战上线任务清单 (Production Readiness)

> 来源: 2026-08-14 对 Agent 架构的实测代码审查。按"会不会炸 / 能不能用 / 看不看得见"分三级。
> 逐项修复,全部打勾即可实战上线。

## P0 — 上线阻塞项 (会造成数据损坏 / API 硬错误 / 重复副作用)

- [x] **P0-1 压缩产生孤儿 `tool_use` → Provider 400**
  `ContextCompressor::compress` 阶段1 只删 `ToolResult` 不删配对的 assistant `ToolUse`;阶段2 从尾部截断可能让历史以 `Role::Tool` 开头。Anthropic/OpenAI 都要求 tool_use 与 tool_result 严格配对 → 400。且结果经 `replace_messages` 落盘,会话被永久损坏。
  修复: 旧工具结果改为**内容省略**而非整条删除;新增 `sanitize_pairing()` 在压缩结果落盘前强制配对不变量(丢弃孤儿 ToolResult、为缺失结果合成占位)。
  验证: `test_compress_never_orphans_tool_use`、`test_sanitize_drops_orphan_tool_result`、`test_sanitize_synthesizes_missing_tool_result`。

- [x] **P0-2 压缩是纯丢弃,没有摘要 → Agent 长任务失忆**
  `generate_summary_template()` 全仓无调用,`SessionHygiene` 未被外部接线。截断直接砍掉前半段历史,Agent 会重复执行已完成的有副作用动作(重复发帖/重复投递)。
  修复: `CompressionResult.dropped` 带回被丢弃的消息;Agent 循环在落盘前调用 LLM 生成结构化摘要,作为一条 System 消息插入历史头部(LLM 失败时降级为本地摘要)。
  验证: `test_compress_reports_dropped`、`test_local_summary_mentions_dropped_tools`。

- [x] **P0-3 上下文预算单位是字节,且硬编码**
  `estimate_chars` 用 `String::len()`(UTF-8 中文 1 字 = 3 字节),窗口 `ContextCompressor::new(0.50, 128000)` 在 gateway 写死,与模型无关。中文会话预算被高估 3 倍 → 过早压缩狂丢历史。
  修复: 改为 `estimate_tokens()`(ASCII ≈ 4 字符/token,CJK ≈ 1 字符/token);窗口/阈值移入 `[compression]` 配置段。
  验证: `test_estimate_tokens_cjk_vs_ascii`。

- [x] **P0-4 无幂等/去重 → 重复执行有副作用的动作**
  飞书事件重推、Webhook 重试都会再跑一次 Agent 循环。`message_id` 只用于下载图片资源,没有去重表。叠加 `xhs publish` / `boss apply_job` = 重复发帖/重复投简历。
  修复: SQLite 新增 `processed_events` 表 + `mark_event_seen()` (INSERT OR IGNORE 原子判重);飞书/微信/Webhook 入站统一走 `Envelope.id` 判重。
  验证: `test_mark_event_seen_is_idempotent`。

## P1 — 实战体验阻塞项

- [x] **P1-5 无取消/打断**
  无 `CancellationToken`。用户说"停"的消息会阻塞在 per-session Mutex 上,最坏等 100 迭代 × 60s。
  修复: `AgentRuntime` 持 per-session `CancellationToken`;`cancel_session()` + `POST /api/session/{id}/cancel`;循环每轮检查,工具执行与取消 race。
  验证: `test_cancel_session_stops_loop`。

- [x] **P1-6 整轮无 wall-clock 预算**
  只有单工具 `tool_timeout_ms`,循环本身只数迭代次数,可跑 100 分钟。
  修复: `AgentConfig.max_loop_duration_ms`(默认 600s),循环每轮检查 deadline,超时返回**部分结果**而非错误。
  验证: `test_deadline_returns_partial`。

- [x] **P1-7 达到 max_iterations 直接返回 `Err`**
  全部中间成果丢弃,用户只拿到一句报错。
  修复: 返回"已完成进展 + 未完成说明"的 `AgentResponse`,并标记 `truncated`。
  验证: `test_max_iterations_returns_partial_response`。

- [x] **P1-8 循环期间零反馈**
  `send_response` 只在循环结束后调一次,多工具任务用户静默等几分钟。
  修复: 复用已有 `AgentEvent::ToolStart/Step` 回调,飞书/微信通道订阅并推送进度(带节流,首个工具即发)。
  验证: 事件回调装配的单测 + 手动观察。

- [x] **P1-9 审批链路在聊天场景不可用**
  `require_approval=["shell_exec"]` 默认开,审批只能走 REST,120s 超时期间还占着 session 锁。聊天用户无法审批 → shell_exec 等于永久禁用。
  修复: 审批请求经通道推送(飞书卡片/文本指令 `/approve <id>`),`POST /api/tools/approvals/{id}` 保留;审批等待不再持有 session 锁的语义说明。
  验证: `test_approval_notifier_invoked`。

- [x] **P1-10 流式路径是另一个残废 Agent**
  `process_stream` 不带工具/记忆/压缩/落盘,`process` 是 `process_with_loop` 的退化重复实现。
  修复: 删除 `process`;`process_stream` 改为循环的薄包装(工具轮走完整循环,最终答案流式吐出)。
  验证: 编译期(调用点收敛)+ `test_process_stream_uses_loop`。

- [x] **P1-11 工具输出无上限**
  工具返回值原样进历史,一次大页面抓取就能顶爆窗口。
  修复: `AgentConfig.max_tool_result_chars`(默认 16000),超限保头保尾并标注截断。
  验证: `test_tool_result_truncated`。

- [x] **P1-12 工具参数不校验**
  模型给的 JSON 原样丢进 `tool.execute`,参数错只能靠工具自己报错,没有修复轮。
  修复: 执行前按工具 schema 校验 `required` 字段与类型,失败时把结构化错误作为 tool_result 回灌,让模型自行修正。
  验证: `test_validate_args_missing_required`。

## P2 — 运维可观测性

- [x] **P2-13 零 token/成本统计**
  `Usage { prompt_tokens, total_tokens }` 定义了但全仓无人写入/读取。
  修复: provider 解析响应时回填 `Usage`;`ProviderClient` 累加到进程级 `UsageStats`;`GET /api/usage` 暴露。
  验证: `test_usage_accumulates`。

- [x] **P2-14 无 metrics 端点**
  只有 `/health`,没有迭代数/工具成功率/provider 失败率/时延。
  修复: `GET /metrics` (Prometheus 文本格式),计数器在 Agent 循环与 ProviderClient 内埋点。
  验证: `test_metrics_render`。

- [x] **P2-15 工具串行执行**
  一轮里多个只读工具也得排队。
  修复: 同一轮的多个工具调用并发执行(`join_all`),结果按原顺序回灌。
  验证: `test_parallel_tool_calls_preserve_order`。

- [x] **P2-16 定时任务是黑洞**
  共享会话永久增长,结果只写日志,失败无通知。
  修复: `[[schedules]]` 增 `notify_channel` / `notify_session`,结果与错误推送到指定通道。
  验证: `test_schedule_notify_config`。

- [x] **P2-17 权限粒度只到通道**
  `ToolPolicy` 是全局单例,不区分调用者。
  修复: `ToolPolicy` 支持 per-principal 覆盖,`Envelope` 带 `principal`,循环按调用者取策略。
  验证: `test_policy_principal_override`。

---

## 进度

| 阶段 | 状态 |
|------|------|
| P0 (4 项) | ✅ 全部完成 |
| P1 (8 项) | ✅ 全部完成 |
| P2 (5 项) | ✅ 全部完成 |
| 额外发现 (3 项) | ✅ 全部修复 |

全量校验: `cargo test --workspace` 241 passed / `cargo clippy --all-targets` 0 warnings。

## 额外发现 (修复过程中暴露)

- [x] **X-1 所有带路径参数的 REST 路由全部 404 (axum 版本语法错配)**
  路由用 axum 0.8 的 `{id}` 语法注册,但依赖是 `axum = "0.7"` —— 在 0.7 里 `{id}` 是**字面量路径段**,
  于是 `/api/session/{id}` (GET/DELETE)、`/api/agent/{id}`、`/api/channel/{name}`、
  `/api/skill/{id}/approve|reject`、`/api/tools/approvals/{id}`、`/webhook/{source}` 全部 404 ——
  包括 P1-9 依赖的工具审批 REST 接口。冒烟测试时发现。
  修复: 10 条路由改为 0.7 的 `:id` 语法。
  验证: 起网关逐条 curl,6 条代表性路由由 404 变 200。

- [x] **X-2 `RUSTCLAW_PROFILE` 对 config.toml / skills / logs 完全失效 (凭据串号)**
  `Config::profile()` / `profiled()` 本身是对的,但 6 处调用方**绕开了这些 helper、就地拼死路径**:
  `Config::load()`(gateway 读配置的唯一入口)、CLI 的 `load_config_file` / `get_config_path` /
  `ensure_config_dir` / `get_log_path`、以及 `SkillConfig::default()`(技能目录)。
  后果:`RUSTCLAW_PROFILE=<name>` 的进程只隔离了 `data_dir`,**照旧读默认 profile 的 config.toml**,
  连带其中的飞书 `app_secret` / provider key —— 一个"隔离"的实例会连上生产的飞书长连接。
  (本次冒烟测试就踩到:测试网关连上了真实飞书 WS。)
  修复: 6 处一律改走 `Config::config_path()` / `Config::logs_dir()` / `Config::skills_dir()`。
  验证: `test_g7_profile_paths` 增加 `Config::load(None)` 落盘路径断言;并起网关实测 ——
  配置写到 `.config/rustclaw/profiles/smoke/config.toml`,日志里飞书相关行数为 0。

- [x] **X-3 技能目录布局两处定义、文档写错;技能"批准后可调用"的闭环并不存在**
  三个层次的问题:
  1. *布局重复定义(已修)*: `SkillConfig::default()` 自行拼 `skills/pending` + `skills/active`,
     而 config 侧只有 `skills_dir()` / `pending_skills_dir()`、**没有 active 访问器** ——
     两边对"生效技能放哪"各说各话。修复: 新增 `Config::active_skills_dir()`,
     `SkillConfig::default()` 改为从三个 helper 派生;`test_skill_config_default_matches_config_helpers` 钉住。
  2. *文档写错(已修)*: CLAUDE.md 原文说批准后技能"移到 `~/.config/rustclaw/skills/`",
     实际是 `<skills>/active/<id>.json`。已更正。
  3. *闭环缺失(已修 — v0.54.0,仅 Shell/Python)*: `approve_skill` 之后原本**没有任何代码**把技能
     注册进 `ToolRegistry` —— `get_active_skills()` 只被用于列表展示,技能永远调不起来。
     现在: `SkillManager` 持有 `ToolRegistry`(gateway 在 Agent 构造后注入),批准即注册为
     `dyn_skill_<name>` 工具,`init()` 在重启后重新上线,`delete_skill` 同步注销。
     执行经 `SandboxExecutor`(黑名单 + 30s 超时 + 内存上限),源码内联进 `sh -c`,
     Local / Docker 后端行为一致;Local 后端会打印醒目告警(生成代码跑在宿主机上)。
     参数以单个 JSON 实参传入并做 POSIX 单引号转义 —— `test_injected_args_are_data_not_code`
     用真实执行 + 落地标记文件证明注入串只能是数据。
     安全前提: 前缀 `dyn_skill_` 使技能顶不掉内置工具,且 `requires_approval` 对该前缀
     **无条件返回 true**(配置无法豁免)。前缀特意不用 `skill_` —— 那会把内置的
     `skill_manage` 一起拖进强制审批(冒烟时踩到并修正)。
     Rust / JavaScript 仍是"可登记不可调用",需要编译器 / node 运行时,不在本期。
     新增 `GET /api/tools` 区分"已登记"与"真正可调用"。
     验证: 单测 (批准→可调用→删除注销、重启重新上线、Rust 仅登记、注入防护、审批强制) +
     网关实测 (创建 23 → 批准 24 个工具,重启后仍在)。

## 实现说明 (与原计划的偏差)

- **P1-7 未新增 `truncated` 字段**: `AgentResponse` 在 4 个 crate 里有 12+ 处构造,为一个标记位改签名不划算。
  改为在返回文本里带明确前缀 (`[未完成 · 达到迭代上限]` / `[未完成 · 超时]` / `[已取消]`),用户与日志都能看见。
- **P1-10 直接删除而非重写**: `process()` 与 `process_stream()` 全仓零调用方 (纯死代码),
  重写一个没人用的流式路径属于投机开发,删掉即可。真需要流式时在 `process_with_loop` 上加。
- **P2-13 未改 `LlmProvider` trait 签名**: 改签名要波及 4 个 provider × 2 个方法及全部 mock。
  改为 provider 解析响应时写进程级全局计数器,效果相同、blast radius 为零。
