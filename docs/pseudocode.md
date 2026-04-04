# RustClaw 核心功能模块伪代码

> 本文档用伪代码描述 RustClaw 项目的核心功能模块逻辑，用于人工审核检查。
> 生成日期: 2026-04-01
> 版本: v0.35.0

---

## 目录

1. [SessionManager — 会话管理器](#1-sessionmanager--会话管理器)
2. [AgentRuntime — Agent 运行时](#2-agentruntime--agent-运行时)
3. [ToolRegistry — 工具注册表](#3-toolregistry--工具注册表)
4. [SandboxExecutor — 沙箱执行器](#4-sandboxexecutor--沙箱执行器)
5. [FileReadTool / FileWriteTool — 文件工具](#5-filereadtool--filewritetool--文件工具)
6. [SkillManager — Skill 管理器](#6-skillmanager--skill-管理器)
7. [SQLiteSessionStore — SQLite 会话存储](#7-sqlitesessionstore--sqlite-会话存储)
8. [MemoryManager — 记忆管理器](#8-memorymanager--记忆管理器)
9. [MemoryScanner — 安全扫描器](#9-memoryscanner--安全扫描器)
10. [SessionHygiene — 会话清理](#10-sessionhygiene--会话清理)
11. [ContextCompressor — 上下文压缩器](#11-contextcompressor--上下文压缩器)
12. [CacheStrategy — Prompt 缓存策略](#12-cachestrategy--prompt-缓存策略)
13. [Gateway — 网关服务器](#13-gateway--网关服务器)
14. [LaneRouter — 车道路由器](#14-lanerouter--车道路由器)
15. [ProviderClient — LLM 提供商客户端](#15-providerclient--llm-提供商客户端)

---

## 1. SessionManager — 会话管理器

**文件**: `crates/agent/src/lib.rs`
**职责**: 管理 Agent 会话的生命周期（创建、查询、删除、过期清理）

```
结构体 SessionManager:
    属性:
        sessions: Arc<RwLock<HashMap<会话Key, Session>>>  // 所有活跃会话
        max_sessions: usize                               // 最大会话数
        ttl_seconds: u64                                  // 会话过期时间(秒)

    方法 create(channel, lane) -> Session:
        1. 生成唯一 UUID 作为 session.key
        2. 创建 Session 对象:
           - lane: 指定的车道
           - messages: 空列表 (预分配16容量)
           - metadata: 包含 channel, 创建时间, 更新时间, 工具开关
           - last_accessed: 当前时间
        3. 获取 sessions 写锁
        4. 将 session 插入 HashMap
        5. 返回 session

    方法 get(key) -> Option<Session>:
        1. 获取 sessions 读锁
        2. 从 HashMap 中查找 key
        3. 返回克隆的 session (Arc优化: messages和metadata是Arc包装)

    方法 get_or_create(key, channel, lane) -> Session:
        1. 获取 sessions 写锁 (原子操作,避免竞态条件)
        2. 如果 key 存在:
           a. 克隆 session
           b. 更新 last_accessed 为当前时间
           c. 重新插入 (更新)
           d. 返回 session
        3. 如果 key 不存在:
           a. 创建新 Session (key=传入的key, lane, 空消息)
           b. 插入 HashMap
           c. 返回 session

    方法 add_message(key, message):
        1. 获取 sessions 写锁
        2. 如果 session 存在:
           a. 克隆 messages Arc -> 新 Vec
           b. 追加新消息到 Vec
           c. 用新 Arc 替换旧 messages
           d. 克隆 metadata Arc -> 新 Metadata
           e. 更新 metadata.updated_at
           f. 用新 Arc 替换旧 metadata
           g. 更新 last_accessed

    方法 delete(key):
        1. 获取 sessions 写锁
        2. 从 HashMap 中移除 key

    方法 list() -> Vec<Session>:
        1. 获取 sessions 读锁
        2. 返回所有 session 的克隆列表

    方法 count() -> usize:
        1. 获取 sessions 读锁
        2. 返回 HashMap 长度

    方法 cleanup_expired() -> 移除数量:
        1. 获取 sessions 读锁
        2. 遍历所有 session, 收集过期 key (last_accessed + ttl < 当前时间)
        3. 释放读锁
        4. 获取 sessions 写锁
        5. 逐个移除过期 key
        6. 返回移除数量
```

---

## 2. AgentRuntime — Agent 运行时

**文件**: `crates/agent/src/lib.rs`
**职责**: 执行 Agent 推理循环，处理消息，调用工具，集成记忆和压缩

```
结构体 AgentRuntime:
    属性:
        config: AgentConfig                   // 配置 (最大循环次数, 超时等)
        session_manager: SessionManager       // 会话管理器
        lane_router: LaneRouter               // 车道路由器
        tool_registry: Arc<ToolRegistry>      // 工具注册表
        provider_client: ProviderClient       // LLM 提供商客户端
        memory_manager: Option<MemoryManager> // 可选: 记忆管理器
        context_compressor: Option<ContextCompressor> // 可选: 上下文压缩器
        event_callback: Option<EventCallback>  // 可选: 事件回调
        iteration_budget: Option<u32>          // 可选: 迭代预算

    方法 process(envelope) -> AgentResponse:
        1. 获取 session_key 和 lane
        2. 获取或创建 session
        3. 将用户消息添加到 session
        4. 获取 session 的完整消息历史
        5. [可选] 如果配置了 memory_manager:
           a. 获取记忆文本
           b. 在消息列表头部插入 System 消息: "Memory:\n{记忆文本}"
        6. [可选] 如果配置了 context_compressor:
           a. 执行压缩
           b. 如果压缩发生, 用压缩后的消息替换原消息
        7. 调用 provider_client.complete(messages) 获取 LLM 响应
        8. 解析响应中的 tool_calls
        9. 返回 AgentResponse (包含响应消息和工具调用列表)

    方法 process_with_loop(envelope) -> AgentResponse:
        1. 获取 session_key 和 lane
        2. 获取或创建 session
        3. 将用户消息添加到 session
        4. 发送 Status("started") 事件
        5. 初始化 iterations = 0, total_tool_calls = 0
        6. 循环:
           a. 如果 iterations >= max_iterations:
              - 如果 total_tool_calls >= 5: 发送 SkillSuggestion 事件
              - 发送 Status("max_iterations") 事件
              - 返回 MaxIterationsExceeded 错误
           b. 发送 Step(iteration, message_count) 事件
           c. 获取 session 的完整消息历史
           d. 调用 provider_client.complete(messages) 获取 LLM 响应
           e. 解析响应中的 tool_calls
           f. 如果 tool_calls 为空:
              - 如果 total_tool_calls >= 5: 发送 SkillSuggestion 事件
              - 发送 Status("completed") 事件
              - 返回最终响应
           g. 对每个 tool_call:
              - total_tool_calls += 1
              - 发送 ToolStart(tool_name, args) 事件
              - 执行工具:
                * 成功: 发送 ToolEnd(success=true), 将结果作为 System 消息添加到 session
                * 失败: 发送 ToolEnd(success=false), 将错误作为 System 消息添加到 session
           h. iterations += 1

    方法 parse_tool_calls(response_text) -> Vec<ToolCall>:
        1. 尝试将 response_text 解析为 JSON
        2. 如果解析成功且包含 "tool_calls" 字段:
           a. 遍历 tool_calls 数组
           b. 提取每个 tool_call 的 id, name, arguments
           c. 返回 ToolCall 列表
        3. 否则返回空列表

    方法 execute_tool(tool_call) -> Result<String, Error>:
        1. 从 tool_registry 获取工具
        2. 调用 tool.execute(tool_call.arguments)
        3. 返回工具执行结果
```

---

## 3. ToolRegistry — 工具注册表

**文件**: `crates/tool/src/lib.rs`
**职责**: 注册、管理和执行所有工具

```
结构体 ToolRegistry:
    属性:
        tools: Arc<RwLock<HashMap<工具名, Arc<dyn Tool>>>>

    方法 register(tool):
        1. 获取 tools 写锁
        2. 将工具插入 HashMap (key = tool.name())

    方法 get(name) -> Option<Arc<dyn Tool>>:
        1. 获取 tools 读锁
        2. 从 HashMap 中查找工具

    方法 execute(tool_call) -> ToolResult:
        1. 根据 tool_call.name 查找工具
        2. 如果工具不存在: 返回 NotFound 错误
        3. 调用 tool.execute(tool_call.arguments)
        4. 如果执行成功: 返回包含结果的 ToolResult
        5. 如果执行失败: 返回包含错误的 ToolResult

    方法 register_builtin_tools():
        1. 注册 FileReadTool (allowed_dirs=空, 即无限制)
        2. 注册 FileWriteTool (allowed_dirs=空, 即无限制)
        3. 注册 ShellExecTool (默认沙箱配置)
        4. 注册 OpenCodeTool (默认超时120秒)

    方法 register_all_tools():
        1. 调用 register_builtin_tools()
        2. 调用 register_reach_tools() (注册所有 Reach 工具)
```

---

## 4. SandboxExecutor — 沙箱执行器

**文件**: `crates/tool/src/lib.rs`
**职责**: 在受限环境中执行 shell 命令

```
结构体 SandboxExecutor:
    属性:
        timeout: Duration          // 执行超时 (默认30秒)
        denied_commands: Vec<String> // 禁止的命令列表

    方法 execute_command(command) -> Result<输出, ToolError>:
        1. 检查命令是否被禁止:
           a. 将命令转为小写
           b. 检查是否包含危险模式:
              - "rm -rf /", "mkfs", ":(){:|:&};:" (fork bomb), "> /dev/sd"
              - "curl | sh", "wget | sh", "chmod 777"
              - "nc -l", "nmap", "dd if=", "iptables"
           c. 如果匹配任何模式: 返回 SandboxViolation 错误
        2. 使用 tokio::time::timeout 包装命令执行:
           a. 执行: Command::new("sh").arg("-c").arg(command).output()
           b. 如果超时: 返回 Timeout 错误
           c. 如果执行失败: 返回 ExecutionFailed 错误
        3. 返回命令输出 (stdout, stderr, exit_code)

    方法 is_command_denied(command) -> bool:
        1. 将命令转为小写
        2. 遍历 denied_commands 列表
        3. 如果命令包含任何禁止模式: 返回 true
        4. 返回 false
```

---

## 5. FileReadTool / FileWriteTool — 文件工具

**文件**: `crates/tool/src/lib.rs`
**职责**: 读写文件 (带路径安全验证)

```
结构体 FileReadTool:
    属性:
        allowed_dirs: Vec<String>  // 允许访问的目录白名单

    方法 validate_path(path) -> Result<(), ToolError>:
        1. 如果 path 包含 "..": 返回 SandboxViolation ("路径穿越不允许")
        2. 如果 allowed_dirs 非空:
           a. 规范化 path 为绝对路径
           b. 检查规范化后的路径是否以任一 allowed_dir 开头
           c. 如果不是: 返回 SandboxViolation ("路径不在允许的目录中")
        3. 返回 Ok

    方法 execute(params) -> Result<Value, ToolError>:
        1. 从 params 提取 path
        2. 调用 validate_path(path)
        3. 读取文件内容: tokio::fs::read_to_string(path)
        4. 返回 { "content": 文件内容 }

结构体 FileWriteTool:
    属性:
        allowed_dirs: Vec<String>  // 允许访问的目录白名单

    方法 validate_path(path) -> Result<(), ToolError>:
        1. 如果 path 包含 "..": 返回 SandboxViolation
        2. 如果 allowed_dirs 非空:
           a. 获取 path 的父目录
           b. 规范化父目录为绝对路径
           c. 检查是否以任一 allowed_dir 开头
           d. 如果不是: 返回 SandboxViolation
        3. 返回 Ok

    方法 execute(params) -> Result<Value, ToolError>:
        1. 从 params 提取 path 和 content
        2. 调用 validate_path(path)
        3. 写入文件: tokio::fs::write(path, content)
        4. 返回 { "success": true }
```

---

## 6. SkillManager — Skill 管理器

**文件**: `crates/skill/src/lib.rs`
**职责**: 管理 Skill 的创建、验证、审批、激活流程

```
结构体 SkillManager:
    属性:
        config: SkillConfig                        // 配置 (目录, 语言限制等)
        pending_skills: Arc<RwLock<HashMap<SkillId, SkillRecord>>>  // 待审批
        active_skills: Arc<RwLock<HashMap<SkillId, SkillRecord>>>   // 已激活
        approval_tx: Option<Sender<SkillApprovalRequest>>           // 审批通知通道

    方法 create_skill(name, description, language, source_code, parameters) -> SkillDefinition:
        1. 检查 language 是否在 allowed_languages 中
        2. 创建 SkillDefinition:
           - id: 新生成 UUID
           - version: "1.0.0"
           - created_at: 当前时间戳
           - author: None
           - test_cases: 空列表
           - category: None (或使用 create_skill_with_category 指定)
        3. 调用 validate_skill(definition)
        4. 返回 definition

    方法 validate_skill(definition) -> Result<(), SkillError>:
        1. 检查 name: 非空, 长度<=64, 只允许字母数字下划线连字符
        2. 检查 source_code: 非空, 长度<=1MB
        3. 检查 description: 长度<=512
        4. 检查 language: 是否在 allowed_languages 中
        5. 检查 parameters: 必须是 JSON Object
        6. 调用 validate_language_syntax(language, source_code):
           - Shell: 检查危险模式 (rm -rf /, mkfs, fork bomb等)
           - Python: 检查危险导入 (os, subprocess, sys, __import__)
           - JavaScript: 检查危险模式 (eval, Function, require, process.exit)
           - Rust: 无额外检查
        7. 返回 Ok

    方法 submit_skill(definition) -> Result<SkillId, SkillError>:
        1. 如果 require_approval 为 true:
           调用 submit_for_approval(definition)
        2. 否则:
           调用 activate_skill(definition)

    方法 submit_for_approval(definition) -> Result<SkillId, SkillError>:
        1. 将 definition 序列化为 JSON
        2. 写入 pending_dir/{id}.json
        3. 创建 SkillRecord (status=Pending)
        4. 插入 pending_skills HashMap
        5. 如果配置了 approval_tx: 发送审批请求通知
        6. 返回 skill_id

    方法 approve_skill(skill_id, approver) -> Result<(), SkillError>:
        1. 从 pending_skills 中移除 skill_id
        2. 将文件从 pending_dir/{id}.json 移动到 active_dir/{id}.json
        3. 更新 SkillRecord (status=Active, approved_at, approved_by)
        4. 插入 active_skills HashMap
        5. 返回 Ok

    方法 reject_skill(skill_id, approver, reason) -> Result<(), SkillError>:
        1. 从 pending_skills 中移除 skill_id
        2. 删除 pending_dir/{id}.json 文件
        3. 返回 Ok

    方法 patch_skill(skill_id, patch) -> Result<(), SkillError>:
        1. 在 active_skills 或 pending_skills 中查找 skill
        2. 克隆 definition
        3. 根据 patch 更新字段 (name, description, source_code, parameters)
        4. 调用 validate_skill 验证更新后的 definition
        5. 序列化并写回 JSON 文件
        6. 更新内存中的 SkillRecord
        7. 返回 Ok

    方法 delete_skill(skill_id) -> Result<(), SkillError>:
        1. 尝试从 active_skills 移除, 如果不存在则从 pending_skills 移除
        2. 删除对应的 JSON 文件
        3. 返回 Ok

    方法 create_skill_directory(definition) -> Result<PathBuf, SkillError>:
        1. 确定分类目录: category 或 "general"
        2. 创建目录结构: active_dir/{category}/{name}/
           - references/
           - templates/
           - scripts/
        3. 生成 SKILL.md 文件 (包含名称, 描述, 版本, 语言, 分类, 作者, 源码, 参考)
        4. 返回技能目录路径

    方法 get_skills_by_category() -> HashMap<分类, Vec<SkillRecord>>:
        1. 遍历 active_skills
        2. 按 category 分组 (无分类归为 "general")
        3. 返回分组结果

    方法 hot_reload() -> Result<HotReloadResult, SkillError>:
        1. 记录当前 active_skills 数量
        2. 清空 pending_skills 和 active_skills
        3. 重新从磁盘加载所有技能
        4. 返回 { loaded: 新数量, previous: 旧数量 }
```

---

## 7. SQLiteSessionStore — SQLite 会话存储

**文件**: `crates/storage/src/lib.rs`
**职责**: 将会话持久化到 SQLite 数据库，支持搜索和谱系追踪

```
结构体 SQLiteSessionStore:
    属性:
        conn: Arc<RwLock<Connection>>  // SQLite 连接 (WAL模式)

    方法 new(db_path) -> Result<Self, Error>:
        1. 创建父目录 (如果不存在)
        2. 打开 SQLite 数据库
        3. 执行初始化 SQL:
           - PRAGMA journal_mode=WAL        // WAL模式 (并发读+单写)
           - PRAGMA synchronous=NORMAL      // 正常同步 (性能与安全平衡)
           - CREATE TABLE sessions:
             * key TEXT PRIMARY KEY
             * channel TEXT NOT NULL
             * parent_session_id TEXT       // 父会话ID (谱系追踪)
             * messages TEXT NOT NULL DEFAULT '[]'  // JSON格式消息列表
             * created_at INTEGER NOT NULL
             * updated_at INTEGER NOT NULL
           - CREATE INDEX idx_sessions_updated ON sessions(updated_at)
           - CREATE INDEX idx_sessions_channel ON sessions(channel)
           - CREATE INDEX idx_sessions_parent ON sessions(parent_session_id)
        4. 返回 SQLiteSessionStore

    方法 get(key) -> Option<Session>:
        1. 获取 conn 读锁
        2. 执行 SELECT 查询
        3. 反序列化 messages JSON -> Vec<Message>
        4. 返回 Session (parent_session_id 可能为 None)

    方法 insert(session) -> Result<(), Error>:
        1. 序列化 messages -> JSON 字符串
        2. 获取 conn 写锁
        3. 执行 INSERT OR REPLACE (key, channel, parent_session_id, messages, created_at, updated_at)
        4. 返回 Ok

    方法 add_message(key, message) -> Result<(), Error>:
        1. 获取 conn 写锁
        2. 查询当前 messages JSON
        3. 反序列化 JSON -> Vec<Message>
        4. 追加新消息
        5. 序列化回 JSON
        6. 执行 UPDATE sessions SET messages=?, updated_at=? WHERE key=?
        7. 返回 Ok

    方法 remove(key) -> Result<(), Error>:
        1. 获取 conn 写锁
        2. 执行 DELETE FROM sessions WHERE key=?
        3. 返回 Ok

    方法 list() -> Vec<Session>:
        1. 获取 conn 读锁
        2. 执行 SELECT 所有 session
        3. 反序列化每条记录的 messages
        4. 返回 Session 列表

    方法 count() -> usize:
        1. 获取 conn 读锁
        2. 执行 SELECT COUNT(*) FROM sessions
        3. 返回计数

    方法 cleanup_expired(ttl_seconds) -> Result<移除数量, Error>:
        1. 计算过期时间戳: 当前时间 - ttl_seconds * 1000
        2. 获取 conn 写锁
        3. 执行 DELETE FROM sessions WHERE updated_at < 过期时间戳
        4. 返回删除数量

    方法 search(query, limit) -> Vec<Session>:
        1. 获取 conn 读锁
        2. 构建 LIKE 查询: WHERE messages LIKE '%{query}%'
        3. 按 updated_at DESC 排序
        4. 限制返回数量
        5. 反序列化每条记录的 messages
        6. 返回匹配的 Session 列表

    方法 get_lineage(session_key) -> Vec<Session>:
        1. 初始化 lineage = []
        2. current_key = session_key
        3. while current_key 存在:
           a. 获取 session = get(current_key)
           b. 如果 session 存在:
              - 添加到 lineage
              - current_key = session.parent_session_id
           c. 否则: 退出循环
        4. 返回 lineage (从子到父的链式追踪)
```

---

## 8. MemoryManager — 记忆管理器

**文件**: `crates/memory/src/lib.rs`
**职责**: 管理 Agent 的持久记忆 (MEMORY.md)，支持增删改查和安全扫描

```
结构体 MemoryManager:
    属性:
        memory_entries: Arc<RwLock<VecDeque<MemoryEntry>>>  // 记忆条目队列
        user_profile: Arc<RwLock<UserProfile>>              // 用户偏好档案
        memory_path: PathBuf                                // 记忆文件路径
        memory_char_limit: usize                            // 记忆字符限制 (2200)
        user_char_limit: usize                              // 用户档案字符限制 (1375)

    方法 new(memory_path, memory_char_limit, user_char_limit) -> Self:
        1. 创建父目录 (如果不存在)
        2. 初始化空 memory_entries 和 user_profile
        3. 返回 MemoryManager

    方法 add_entry(content) -> Result<(), MemoryError>:
        1. 调用 MemoryScanner.scan(content) 进行安全扫描
        2. 计算当前记忆总字符数
        3. 如果 当前字符 + 新内容字符 > memory_char_limit:
           返回 LimitExceeded 错误
        4. 创建 MemoryEntry (content, created_at, updated_at)
        5. 获取 memory_entries 写锁
        6. 追加到 VecDeque
        7. 调用 save_memory() 持久化
        8. 返回 Ok

    方法 replace_entry(old_text, new_text) -> Result<(), MemoryError>:
        1. 调用 MemoryScanner.scan(new_text) 进行安全扫描
        2. 获取 memory_entries 写锁
        3. 遍历所有 entry:
           a. 如果 entry.content 包含 old_text:
              - 替换 old_text 为 new_text
              - 更新 updated_at
              - 跳出循环
        4. 如果未找到: 返回 NotFound 错误
        5. 调用 save_entries() 持久化
        6. 返回 Ok

    方法 remove_entry(text) -> Result<(), MemoryError>:
        1. 获取 memory_entries 写锁
        2. 记录初始长度
        3. 移除所有 content 包含 text 的 entry
        4. 如果长度未变: 返回 NotFound 错误
        5. 调用 save_entries() 持久化
        6. 返回 Ok

    方法 get_entries() -> Vec<MemoryEntry>:
        1. 获取 memory_entries 读锁
        2. 返回所有 entry 的克隆

    方法 get_memory_text() -> String:
        1. 获取 memory_entries 读锁
        2. 将所有 entry.content 用 "\n" 连接
        3. 返回拼接后的文本

    方法 current_memory_chars() -> usize:
        1. 获取 memory_entries 读锁
        2. 计算所有 entry.content.len() 之和
        3. 返回总和

    方法 memory_usage() -> (当前字符数, 限制字符数):
        1. 返回 (current_memory_chars(), memory_char_limit)

    方法 save_memory() -> Result<(), MemoryError>:
        1. 获取 memory_entries 读锁
        2. 克隆所有 entry 到 Vec
        3. 调用 save_entries(entries)

    方法 save_entries(entries) -> Result<(), MemoryError>:
        1. 序列化 entries 为格式化 JSON
        2. 写入 memory_path 文件
        3. 返回 Ok

    方法 load_memory() -> Result<(), MemoryError>:
        1. 如果 memory_path 不存在: 返回 Ok
        2. 读取文件内容
        3. 反序列化 JSON -> Vec<MemoryEntry>
        4. 获取 memory_entries 写锁
        5. 替换为加载的 entries
        6. 返回 Ok

    方法 record_correction(original, corrected, context):
        1. 创建 CorrectionEntry (original, corrected, context, created_at)
        2. 获取 user_profile 写锁
        3. 追加到 corrections 列表

    方法 get_corrections() -> Vec<CorrectionEntry>:
        1. 获取 user_profile 读锁
        2. 返回 corrections 的克隆

    方法 clear() -> Result<(), MemoryError>:
        1. 获取 memory_entries 写锁
        2. 清空 VecDeque
        3. 调用 save_memory() 持久化空列表
        4. 返回 Ok
```

---

## 9. MemoryScanner — 安全扫描器

**文件**: `crates/memory/src/lib.rs`
**职责**: 扫描记忆内容，防止 Prompt 注入和数据泄露

```
结构体 MemoryScanner (静态方法):

    方法 scan(content) -> Result<String, MemoryError>:
        1. 去除首尾空白
        2. 如果为空: 返回 InvalidEntry 错误
        3. 如果 contains_invisible_chars(content):
           返回 SecurityViolation ("检测到不可见Unicode字符")
        4. 如果 contains_prompt_injection(content):
           返回 SecurityViolation ("检测到Prompt注入")
        5. 如果 contains_exfiltration_pattern(content):
           返回 SecurityViolation ("检测到数据泄露模式")
        6. 返回清理后的内容

    方法 contains_invisible_chars(text) -> bool:
        1. 遍历 text 的每个字符
        2. 检查是否匹配以下不可见字符:
           - U+200B (零宽空格)
           - U+200C (零宽非连接符)
           - U+200D (零宽连接符)
           - U+FEFF (零宽不换行空格)
           - U+2060 (词连接符)
           - U+180E (蒙古文元音分隔符)
           - U+200E/U+200F (左右至右标记)
           - U+202A-U+202E (双向文本覆盖)
        3. 如果匹配任何: 返回 true
        4. 返回 false

    方法 contains_prompt_injection(text) -> bool:
        1. 将 text 转为小写
        2. 检查是否包含以下注入模式:
           - "ignore previous" / "ignore all previous"
           - "disregard previous" / "disregard all previous"
           - "system prompt:" / "system instruction:"
           - "new instructions:" / "override:"
           - "you are now" / "forget everything"
           - "reset your memory" / "change your role"
           - "act as" / "pretend to be"
        3. 如果匹配任何: 返回 true
        4. 返回 false

    方法 contains_exfiltration_pattern(text) -> bool:
        1. 将 text 转为小写
        2. 检查是否包含以下泄露模式:
           - "send to" / "post to" / "upload to"
           - "exfiltrate" / "leak"
           - "export data" / "copy to clipboard"
           - "write to file" / "save to disk" / "transmit"
        3. 如果匹配任何: 返回 true
        4. 返回 false
```

---

## 10. SessionHygiene — 会话清理

**文件**: `crates/compression/src/lib.rs`
**职责**: Gateway 层的粗略字符级压缩 (85% 阈值安全网)

```
结构体 SessionHygiene:
    属性:
        threshold: f64     // 压缩阈值 (0.85 = 85%)
        max_chars: usize   // 最大字符数 (上下文窗口大小)

    方法 new(threshold, max_chars) -> Self:
        1. 初始化 threshold 和 max_chars
        2. 返回 SessionHygiene

    方法 should_compress(current_chars) -> bool:
        1. 如果 current_chars > max_chars * threshold: 返回 true
        2. 否则: 返回 false

    方法 clean_old_tool_results(messages) -> CompressionResult:
        1. 记录原始消息数量 (original_count)
        2. 初始化 removed = 0, keep_tool_results = true
        3. 从后向前遍历 messages:
           a. 如果消息是工具结果 (以 "Tool " 开头且包含 "result:"):
              - 如果 keep_tool_results 为 true:
                * 保留此消息
                * 设置 keep_tool_results = false (只保留最后一个工具结果)
              - 否则:
                * 跳过此消息 (removed += 1)
           b. 否则: 保留消息
        4. 反转结果列表 (恢复原始顺序)
        5. 返回 CompressionResult (cleaned_messages, compressed, original_count, final_count, removed)

    方法 truncate_to_char_limit(messages, limit) -> Vec<Message>:
        1. 初始化 result = [], total_chars = 0
        2. 从后向前遍历 messages:
           a. 计算消息字符数
           b. 如果 total_chars + msg_chars > limit: 停止
           c. 否则: 添加消息到 result, total_chars += msg_chars
        3. 反转结果列表
        4. 返回截断后的消息列表
```

---

## 11. ContextCompressor — 上下文压缩器

**文件**: `crates/compression/src/lib.rs`
**职责**: Agent 层的精确 token 级压缩 (50% 阈值)

```
结构体 ContextCompressor:
    属性:
        threshold: f64         // 压缩阈值 (0.50 = 50%)
        protect_last_n: usize  // 最少保护的消息数

    方法 new(threshold, protect_last_n) -> Self:
        1. 初始化 threshold 和 protect_last_n
        2. 返回 ContextCompressor

    静态方法 estimate_chars(messages) -> usize:
        1. 遍历所有消息
        2. 累加每条消息的 content 长度
        3. 返回总字符数

    方法 should_compress(current_chars, context_window) -> bool:
        1. 如果 current_chars > context_window * threshold: 返回 true
        2. 否则: 返回 false

    方法 compress(messages, context_window) -> CompressionResult:
        1. 记录原始消息数量
        2. 计算总字符数
        3. 如果不应压缩: 返回原消息 (compressed=false)
        4. 创建 SessionHygiene (threshold=0.85, max_chars=context_window)
        5. 第一阶段: 清理旧工具结果
           a. 调用 hygiene.clean_old_tool_results(messages)
           b. 计算清理后的字符数
        6. 如果清理后已满足阈值: 返回清理后的消息
        7. 第二阶段: 字符级截断
           a. 目标字符数 = context_window * 0.50
           b. 调用 hygiene.truncate_to_char_limit(cleaned_messages, target_chars)
        8. 返回截断后的消息 (compressed=true)

    静态方法 generate_summary_template() -> CompressedSummary:
        1. 返回空的 CompressedSummary:
           - goal: ""
           - constraints: ""
           - progress: ""
           - key_decisions: []
           - relevant_files: []
           - next_steps: []
```

---

## 12. CacheStrategy — Prompt 缓存策略

**文件**: `crates/compression/src/lib.rs`
**职责**: 管理 Anthropic cache_control 断点，降低 75% 输入 token 成本

```
结构体 CacheBreakpoint:
    属性:
        message_index: usize   // 消息索引
        cache_type: CacheType  // 缓存类型 (SystemPrompt / Rolling)

枚举 CacheType:
    - SystemPrompt  // 系统提示 (跨所有轮次缓存)
    - Rolling       // 滚动窗口 (最近3条消息)

结构体 CacheStrategy:
    属性:
        breakpoints: Vec<CacheBreakpoint>  // 缓存断点列表
        ttl_seconds: u64                   // 缓存过期时间

    静态方法 system_and_3(ttl_seconds) -> Self:
        1. 创建4个断点:
           - 索引0: SystemPrompt (系统提示)
           - 索引1: Rolling (倒数第3条)
           - 索引2: Rolling (倒数第2条)
           - 索引3: Rolling (最后1条)
        2. 返回 CacheStrategy

    静态方法 calculate_breakpoints(message_count, ttl_seconds) -> Self:
        1. 如果 message_count <= 4: 返回 system_and_3(ttl_seconds)
        2. 否则:
           a. 创建断点列表:
              - 索引0: SystemPrompt
              - 最后3条消息: Rolling
           b. 返回 CacheStrategy
```

---

## 13. Gateway — 网关服务器

**文件**: `crates/gateway/src/lib.rs`
**职责**: HTTP/WS 服务器，处理 API 请求，管理连接和会话

```
结构体 Gateway:
    属性:
        config: Arc<Config>           // 完整配置
        agent: Arc<AgentRuntime>      // Agent 运行时
        skill_manager: Arc<SkillManager>  // Skill 管理器
        channels: Arc<RwLock<HashMap<通道名, ChannelConfig>>>  // 通道配置
        config_path: PathBuf          // 配置文件路径

    方法 start() -> Result<(), Error>:
        1. 初始化 skill_manager
        2. 解析 host:port 地址
        3. 创建 Axum Router, 注册所有路由:
           - GET /health
           - POST /api/message
           - GET /api/session
           - DELETE /api/session/{id}
           - GET /api/session/{id}
           - GET /api/agent
           - GET /api/agent/{id}
           - GET /api/config
           - POST /api/config
           - POST /api/config/save
           - POST /api/config/model
           - GET /api/channel
           - GET /api/channel/{name}
           - POST /api/channel/{name}
           - GET /api/security/audit
           - GET /api/memory/status
           - GET /api/memory/stats
           - GET /api/system/version
           - GET /api/system/update/check
           - GET /api/skill
           - POST /api/skill
           - POST /api/skill/{id}/approve
           - POST /api/skill/{id}/reject
           - GET /api/skill/approval/subscribe (SSE)
        4. 启动 HTTP 服务器 (axum::serve)
        5. 阻塞直到服务器关闭

    路由处理器 (示例):

    方法 message_handler(State(state), Json(request)):
        1. 从请求中提取 session_key, channel, message
        2. 创建 Envelope (channel, session_key, message)
        3. 调用 state.agent.process(envelope)
        4. 返回 JSON 响应 (success, response, tool_calls)

    方法 session_list_handler(State(state)):
        1. 调用 state.agent.list_sessions()
        2. 构建 JSON 响应 (sessions 列表, 包含 key, lane, message_count, channel)
        3. 返回 JSON 响应

    方法 skill_approve_handler(State(state), Path(id)):
        1. 调用 state.skill_manager.approve_skill(&id, "admin")
        2. 如果成功: 返回 api_success({ "message": "Skill {id} approved" })
        3. 如果失败: 返回 api_error(错误信息)
```

---

## 14. LaneRouter — 车道路由器

**文件**: `crates/agent/src/lib.rs`
**职责**: 将会话路由到固定的车道，实现并发隔离

```
结构体 LaneRouter:
    属性:
        lanes: Arc<RwLock<HashMap<车道名, Lane>>>  // 车道映射
        lane_count: usize                          // 车道数量 (默认10)

    方法 new(lane_count) -> Self:
        1. 初始化空 lanes HashMap
        2. 返回 LaneRouter

    方法 route(session_key) -> 车道名:
        1. 计算 session_key 的哈希值
        2. lane_index = hash % lane_count
        3. 返回 "lane_{lane_index}" (例如 "lane_0" 到 "lane_9")

    方法 register_lane(name, sender) -> Result<(), Error>:
        1. 创建 Lane (name, sender)
        2. 获取 lanes 写锁
        3. 插入 lanes HashMap
        4. 返回 Ok

    方法 get_lane(name) -> Option<Lane>:
        1. 获取 lanes 读锁
        2. 从 HashMap 中查找车道
        3. 返回车道的克隆
```

---

## 15. ProviderClient — LLM 提供商客户端

**文件**: `crates/provider/src/lib.rs`
**职责**: 统一的 LLM 调用接口，支持 OpenAI、Anthropic、NVIDIA NIM

```
结构体 ProviderClient:
    属性:
        provider: Option<Arc<dyn LlmProvider>>  // 当前使用的 LLM 提供商

    方法 new(provider) -> Self:
        1. 初始化 provider
        2. 返回 ProviderClient

    方法 complete(messages) -> Result<String, ProviderError>:
        1. 如果 provider 为 None: 返回 "Provider not configured"
        2. 调用 provider.complete(messages)
        3. 返回 LLM 响应文本

    方法 stream_complete(messages) -> TokenStream:
        1. 如果 provider 为 None: 返回包含 "Provider not configured" 的流
        2. 调用 provider.stream_complete(messages)
        3. 返回 token 流

LlmProvider Trait:
    方法 complete(messages) -> Result<String, ProviderError>
    方法 stream_complete(messages) -> TokenStream
    方法 list_models() -> Result<Vec<String>, ProviderError>

OpenAiProvider 实现:
    方法 complete(messages):
        1. 构建 OpenAI 请求 (model, messages, max_tokens, temperature)
        2. POST {base_url}/v1/chat/completions
        3. 解析响应, 返回 choices[0].message.content

    方法 stream_complete(messages):
        1. 构建流式请求 (stream=true)
        2. POST {base_url}/v1/chat/completions
        3. 读取 SSE 流:
           a. 解析每行 "data: {...}"
           b. 提取 delta.content
           c. 如果 data=[DONE]: 结束流
        4. 返回 token 流

    方法 list_models():
        1. GET {base_url}/v1/models
        2. 解析响应, 返回所有 model.id

AnthropicProvider 实现:
    方法 complete(messages):
        1. 转换消息格式 (role: user/assistant/system)
        2. 提取 system 消息
        3. POST https://api.anthropic.com/v1/messages
           - 头部: x-api-key, anthropic-version: 2024-06-01
           - 体: model, system, messages, max_tokens, temperature
        4. 解析响应, 返回 content[0].text

    方法 stream_complete(messages):
        1. 构建流式请求 (stream=true)
        2. POST https://api.anthropic.com/v1/messages
        3. 读取 SSE 流:
           a. 解析 event: content_block_delta
           b. 提取 delta.text
           c. 如果 event: message_stop: 结束流
        4. 返回 token 流

    方法 list_models():
        1. 返回预定义的 Claude 模型列表:
           - claude-3-5-sonnet-20241022
           - claude-3-opus-20240229
           - claude-3-sonnet-20240229
           - claude-3-haiku-20240307

NvProvider 实现 (NVIDIA NIM, OpenAI兼容):
    方法 complete(messages):
        1. 构建 OpenAI 格式请求
        2. POST https://integrate.api.nvidia.com/v1/chat/completions
           - 头部: Authorization: Bearer {api_key}
        3. 解析响应, 返回 choices[0].message.content

    方法 list_models():
        1. 返回 NVIDIA 支持的模型列表:
           - meta/llama-3.1-405b-instruct
           - meta/llama-3.1-70b-instruct
           - mistralai/mixtral-8x22b-instruct-v0.1
           - google/gemma-2b, google/gemma-7b
           - microsoft/phi-3-mini-128k-instruct
           - nvidia/nemotron-4-340b-instruct
```

---

## 附录: 数据结构定义

```
结构体 Session:
    key: String                    // 会话唯一标识
    lane: String                   // 所属车道
    parent_session_id: Option<String>  // 父会话ID (谱系追踪)
    messages: Arc<Vec<Message>>    // 消息历史 (Arc优化)
    metadata: Arc<SessionMetadata> // 元数据 (Arc优化)
    last_accessed: Instant         // 最后访问时间

结构体 SessionMetadata:
    channel: String                // 通道类型 (http, websocket, etc.)
    created_at: i64                // 创建时间戳 (毫秒)
    updated_at: i64                // 更新时间戳 (毫秒)
    tool_calls_enabled: bool       // 是否启用工具调用

结构体 Message:
    role: Role                     // 角色 (User, Assistant, System)
    content: Content               // 内容 (Text 或 Structured)

结构体 MemoryEntry:
    content: String                // 记忆内容
    created_at: i64                // 创建时间戳
    updated_at: i64                // 更新时间戳

结构体 UserProfile:
    preferences: Vec<MemoryEntry>      // 用户偏好
    communication_style: Vec<MemoryEntry>  // 通信风格
    environment: Vec<MemoryEntry>      // 环境信息
    corrections: Vec<CorrectionEntry>  // 用户纠正记录

结构体 CorrectionEntry:
    original: String               // 原始内容
    corrected: String              // 纠正后内容
    context: String                // 上下文
    created_at: i64                // 创建时间戳

结构体 SkillDefinition:
    id: String                     // Skill 唯一标识
    name: String                   // 名称
    description: String            // 描述
    version: String                // 版本号
    language: SkillLanguage        // 编程语言
    source_code: String            // 源代码
    parameters: JSON               // 参数定义
    created_at: i64                // 创建时间戳
    author: Option<String>         // 作者
    test_cases: Vec<SkillTestCase> // 测试用例
    category: Option<String>       // 分类
    references: Vec<String>        // 参考文件列表

枚举 AgentEvent:
    ToolStart { tool_name, args }      // 工具开始执行
    ToolEnd { tool_name, success }     // 工具执行结束
    Step { iteration, message_count }  // 迭代步骤
    Status { status }                  // 状态变化
    Thinking { content }               // 思考内容
    SkillSuggestion { task_summary, tool_call_count }  // Skill创建建议
    UserCorrection { original, corrected }  // 用户纠正
```

---

## 16. FeishuChannel — 飞书通道

**文件**: `crates/feishu/src/` (新增)
**职责**: 通过飞书 WebSocket 长连接实现与用户的消息交互

### 16.1 FeishuConfig — 飞书配置

```
结构体 FeishuConfig:
    属性:
        app_id: String                    // 飞书应用 ID
        app_secret: String                // 飞书应用密钥
        verification_token: String        // 事件验证 Token
        encrypt_key: Option<String>       // 事件加密密钥 (可选)
        default_model: Option<String>     // 默认模型
        system_prompt: Option<String>     // 系统提示
        dm_enabled: bool                  // 是否启用私聊
        group_enabled: bool               // 是否启用群聊
        allowed_groups: Vec<String>       // 允许的群聊 ID 白名单
        allowed_users: Vec<String>        // 允许的用户 ID 白名单
```

### 16.2 FeishuClient — 飞书 API 客户端

```
结构体 FeishuClient:
    属性:
        app_id: String
        app_secret: String
        tenant_access_token: Arc<RwLock<String>>  // 租户访问令牌 (自动刷新)
        client: reqwest::Client

    方法 get_tenant_token() -> Result<String, FeishuError>:
        1. 获取读锁, 检查 token 是否过期
        2. 如果未过期: 返回缓存的 token
        3. 否则获取写锁:
           a. POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal
           b. 解析响应, 获取新 token
           c. 缓存 token
           d. 返回 token

    方法 send_message(receive_id, msg_type, content) -> Result<String, FeishuError>:
        1. 获取 tenant_token
        2. POST https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id
           - 头部: Authorization: Bearer {token}
           - 体: { receive_id, msg_type, content }
        3. 返回 message_id

    方法 reply_message(message_id, msg_type, content) -> Result<String, FeishuError>:
        1. 获取 tenant_token
        2. POST https://open.feishu.cn/open-apis/im/v1/messages/{message_id}/reply
           - 体: { msg_type, content }
        3. 返回 reply_message_id

    方法 update_message(message_id, content) -> Result<(), FeishuError>:
        1. 获取 tenant_token
        2. PUT https://open.feishu.cn/open-apis/im/v1/messages/{message_id}
           - 体: { content }
        3. 返回 Ok

    方法 download_file(file_key) -> Result<Vec<u8>, FeishuError>:
        1. 获取 tenant_token
        2. GET https://open.feishu.cn/open-apis/im/v1/files/{file_key}
        3. 返回文件字节流

    方法 upload_file(file_type, file_name, data) -> Result<String, FeishuError>:
        1. 获取 tenant_token
        2. POST https://open.feishu.cn/open-apis/im/v1/files
           - multipart/form-data: { file_type, file_name, file }
        3. 返回 file_key
```

### 16.3 FeishuMessage — 飞书消息结构

```
枚举 FeishuMsgType:
    - Text        // 纯文本
    - Post        // 富文本
    - Image       // 图片
    - File        // 文件
    - Interactive // 交互卡片

结构体 FeishuMessage:
    message_id: String           // 消息 ID
    chat_id: String              // 会话 ID
    chat_type: String            // p2p(私聊) / group(群聊)
    sender_id: String            // 发送者 ID
    msg_type: FeishuMsgType      // 消息类型
    content: String              // 消息内容 (JSON)
    mentions: Vec<FeishuMention> // @提及列表
    parent_id: Option<String>    // 父消息 ID (回复)
    create_time: i64             // 创建时间

结构体 FeishuMention:
    key: String       // 占位符 key
    id: String        // 用户 ID
    name: String      // 用户名称
```

### 16.4 FeishuChannel — 飞书通道实现

```
结构体 FeishuChannel:
    属性:
        config: FeishuConfig
        client: Arc<FeishuClient>
        event_tx: Arc<RwLock<Option<Sender<ChannelEvent>>>>
        ws_handle: Option<JoinHandle<()>>  // WebSocket 连接句柄

    方法 connect() -> Result<()>:
        1. 建立 WebSocket 连接到飞书
           a. 获取 tenant_token
           b. 连接 wss://open.feishu.cn/websocket
        2. 启动消息处理循环 (spawn 异步任务):
           a. 接收 WebSocket 消息
           b. 解析为 FeishuEvent
           c. 调用 handle_event(event)
        3. 保存 ws_handle
        4. 返回 Ok

    方法 disconnect() -> Result<()>:
        1. 如果 ws_handle 存在:
           a. 中止任务 (abort)
           b. 关闭 WebSocket 连接
        2. 清空 ws_handle
        3. 返回 Ok

    方法 handle_event(event) -> Result<()>:
        1. 如果 event 是 URL 验证:
           a. 返回 challenge 响应
           b. 返回
        2. 如果 event 是消息事件:
           a. 解析 FeishuMessage
           b. 检查权限:
              - 如果 chat_type == p2p 且 !dm_enabled: 返回
              - 如果 chat_type == group 且 !group_enabled: 返回
              - 检查 allowed_users/allowed_groups 白名单
           c. 提取文本内容:
              - 如果 msg_type == Text: 直接使用 content
              - 如果 msg_type == Post: 解析富文本, 提取纯文本
              - 如果 msg_type == Image: 下载图片, 生成描述
              - 如果 msg_type == File: 下载文件, 生成描述
           d. 处理 @提及:
              - 移除 @机器人 的提及标记
              - 保留其他 @提及 作为上下文
           e. 转换为 Envelope:
              - channel: "feishu"
              - session_key: "{chat_type}_{chat_id}"
              - message: { role: User, content: Text(extracted_text) }
           f. 发送到 AgentRuntime:
              - 调用 agent.process(envelope) 或 agent.process_with_loop(envelope)
           g. 发送响应到飞书:
              - 调用 send_response(response, chat_id)

    方法 send_response(response, chat_id) -> Result<()>:
        1. 提取响应文本
        2. 如果文本长度 > 飞书限制 (15000 字符):
           a. 分割为多条消息
           b. 逐条发送
        3. 否则:
           a. 调用 client.send_message(chat_id, "text", json({text: response_text}))
        4. 返回 Ok

    方法 send_streaming_response(stream, chat_id) -> Result<()>:
        1. 先发送一个 "思考中..." 的交互卡片
        2. 创建空消息缓冲区
        3. 遍历 stream 的每个 token:
           a. 追加 token 到缓冲区
           b. 如果缓冲区长度 > 更新阈值 (200 字符):
              - 调用 client.update_message(message_id, 格式化内容)
              - 清空缓冲区
        4. 发送剩余内容
        5. 更新卡片状态为 "完成"
        6. 返回 Ok

    方法 send_interactive_card(card, chat_id) -> Result<()>:
        1. 序列化 card 为 JSON
        2. 调用 client.send_message(chat_id, "interactive", json)
        3. 返回 Ok

    方法 message_to_envelope(msg) -> Envelope:
        1. session_key = "{msg.chat_type}_{msg.chat_id}"
        2. 提取文本内容 (处理 @提及, 图片描述等)
        3. 创建 Message { role: User, content: Text(extracted_text) }
        4. 返回 Envelope::new("feishu", session_key, message)
```

### 16.5 交互卡片构建

```
结构体 FeishuCard:
    header: CardHeader          // 卡片头部 (标题, 颜色模板)
    elements: Vec<CardElement>  // 卡片元素列表

枚举 CardElement:
    - Markdown { content: String }        // Markdown 文本
    - Action { actions: Vec<CardAction> } // 操作按钮组

结构体 CardAction:
    tag: String           // "button"
    text: CardText        // 按钮文本
    type: String          // "primary" / "danger" / "default"
    value: JSON           // 点击回调值

方法 build_status_card(status, actions) -> FeishuCard:
    1. 创建 CardHeader:
       - title: "🤖 RustClaw Agent"
       - template: 根据 status 选择颜色 (blue=Thinking, green=Complete, red=Error)
    2. 创建元素列表:
       - Markdown 元素: "**状态**: {status}"
       - 如果有 actions: 添加 Action 元素
    3. 返回 FeishuCard
```

### 16.6 权限策略检查

```
结构体 FeishuPermission:
    dm_enabled: bool
    group_enabled: bool
    allowed_users: HashSet<String>
    allowed_groups: HashSet<String>

方法 check_permission(chat_type, chat_id, sender_id) -> bool:
    1. 如果 chat_type == "p2p":
       a. 如果 !dm_enabled: 返回 false
       b. 如果 allowed_users 非空 且 sender_id 不在其中: 返回 false
    2. 如果 chat_type == "group":
       a. 如果 !group_enabled: 返回 false
       b. 如果 allowed_groups 非空 且 chat_id 不在其中: 返回 false
    3. 返回 true
```

---

*文档版本: v0.36.0*
*生成日期: 2026-04-01*
