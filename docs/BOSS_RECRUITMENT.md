# BOSS直聘智能投递助手使用指南

本指南介绍如何使用 RustClaw 的 BOSS直聘智能投递功能。该功能目前处于 Phase 17 阶段，支持自动搜索、简历匹配（匹配度 >= 90% 自动建议投递）以及手动认证管理。

## 1. 认证管理 (手动注入)

由于 BOSS 直聘的反爬策略，你需要手动从浏览器获取 Cookie。

1.  在浏览器登录 [BOSS直聘](https://www.zhipin.com/)。
2.  打开开发者工具 (F12) -> 网络 (Network)。
3.  刷新页面，找到任意一个请求，在请求头中复制 `Cookie` 字段的完整内容。
4.  使用 `rustclaw` CLI 注入凭证：

```bash
rustclaw tools config boss_zhipin_reach --set cookie="lastCity=101280100; __g=-; Hm_lvt_194df3105ad7148dcf2b98a91b5e727a=1776242970; HMACCOUNT=3ED653E80EC33002; ab_guid=6fe53f6c-14d0-402d-88f1-5261d5eb2cee; wt2=DWOp_HmatsTGy5fWBF2GJm_9iJX6YFRWqzUJAUqablDDYBfPBLHZ_SQOjKJE_KkmfoBjyNbOk2BdEOOXOjG_Czw~~; wbg=0; zp_at=5IRvJ8q9vTgbej-hpXjOOKep4CaWefpVXOBf5UC084o~; Hm_lpvt_194df3105ad7148dcf2b98a91b5e727a=1776243157; __zp_stoken__=24e7gNjjDjcK2xIHCtT0se0A2JDQ5PDY4Qjs%2BNjheKzg5Nz0%2BPxtjw4MjZcOYYMONGMK0NyVCOUA2ODQ5N0MVQj04OTg9ODjEuMK3NTg4wqE5Mh17wrcicMOXX8OJC3PCuQQGwrgRbMK3BR3DggpdwrYmLQZUVVlWCw8OEGMJVwYKBA4GYhBWWQhWDWJdBFYPYg9XGRg4wrrCtcKZwrwRwrvCmMOCC8OAwpbDgzk3PTEoNVfCtTpAQjlBOUA0xLbEvMS3xLnEu8S2xLzDh8ORw7jDosS8xLfEucK4xLbEvMS3w7nDmMS2xLzEt8S5w7nCtys5wpDCucOQwq%2FErcW5xKDFpcOEwrHEm8Oow6DCk8OSw6rDmsKaxLjDuMOUwr7Dq8K3xKbCssS7w4LDu8KixYHCr8OYwp7DlUXDqMKJw5PCuMOowr%2FDrsODwqHCisOXXsOqwrnDpsK8wrDCsMOrW8KZwq7DrUjCvUjCm2jDtUlTW8KuT8KWw4HCu2PCrsK8wrjCtMKwcsK0UmhYwrxUwrJMw4J0woB8YFptWlcSWAwENVtqwo3Dhw%3D%3D; bst=V2RtkvGOX_0lZoVtRuyBoZLSi27DrWxSk~|RtkvGOX_0lZoVtRuyBoZLSi27DrSzS4~; __c=1776242970; __a=68308453.1776242970..1776242970.40.1.40.40"
```

## 2. 准备简历

将你的简历保存为 Markdown 格式（例如 `my_resume.md`），确保包含以下信息以提高匹配准确度：
- 核心技能 (Skills)
- 工作经历 (Experience)
- 教育背景 (Education)
- 项目经验 (Projects)

## 3. 运行自动化投递任务

你可以向 RustClaw 发送类似以下的指令来启动自动化流程：

> "帮我搜索北京的 Rust 开发职位，读取我的简历 `./my_resume.md` 进行匹配分析。如果匹配度超过 90%，请告诉我并准备投递。"

### 内部工作流程：
1.  **搜索**: Agent 调用 `boss_zhipin_reach(action="search", query="Rust", city="北京")`。
2.  **获取详情**: 对搜索结果中的职位，Agent 调用 `boss_zhipin_reach(action="job", job_id="...")` 获取详细 JD。
3.  **读取简历**: Agent 调用 `resume_read(path="./my_resume.md")`。
4.  **智能匹配**: Agent 使用 LLM 对比简历与 JD，输出 0-100 的评分。
5.  **决策**: 
    - 如果评分 >= 90: Agent 会提示你匹配成功，并可以根据指令执行 `boss_zhipin_reach(action="apply", job_id="...")`。
    - 如果评分 < 90: Agent 会说明不匹配的原因并跳过。

## 4. 安全说明

- **限频保护**: 建议每次搜索数量不要超过 20 个，防止触发平台风险控制。
- **凭证有效期**: BOSS 的 Cookie 通常有 1-7 天有效期，过期后需重新注入。
- **自动投递**: 默认为“建议投递”，除非你在指令中明确要求“自动投递”。

---
*RustClaw Team - v0.43.0*
