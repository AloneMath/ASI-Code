# CLI 与 API 模型协同机制（Rust 版，现状校准 + 升级蓝图）

本文面向当前仓库的 Rust CLI 实现，目标是两件事：

1. 把“CLI 如何和模型协同”讲准确（避免术语漂移）。
2. 给出一套可落地的“更默契”升级方案（从现在就能做的小改到中期架构增强）。

---

## 1. 现状：当前 Rust CLI 的真实协同流程

### 1.1 上下文装配（CLI 先做准备）

当用户输入请求时，CLI 不会裸发文本给模型，而是先构建 Runtime 上下文：

- 项目上下文：`CLAUDE.md`（优先）/`README.md`/`AGENTS.md`
- Git 状态：`git status --short`
- 最近提交：`git log --oneline -5`
- 历史对话消息（session）
- 当前 provider/model/权限模式

对应代码：

- `src/runtime.rs` 中 `load_project_context*`
- `src/runtime.rs` 中 `Runtime::new`

### 1.2 TAOR 循环（Think-Act-Observe-Repeat）

当前实现是“模型思考 + CLI执行 + 结果回灌”的循环，但有两条执行通道：

1. Native Tool Calling 通道  
模型返回 function/tool_use（OpenAI/Claude），Runtime 直接执行工具并把结构化结果回灌。

2. Legacy `/toolcall` 通道  
模型输出 `/toolcall ...` 文本行，由 `OrchestratorEngine` 解析、分批、执行，再追问模型。

对应代码：

- Native 通道：`src/runtime.rs`
- Legacy 编排：`src/orchestrator/engine.rs` + `src/main.rs` 中 auto-loop

### 1.3 工具与权限控制

当前公开工具为 8 个：

- `read_file`
- `write_file`
- `edit_file`
- `glob_search`
- `grep_search`
- `web_search`
- `web_fetch`
- `bash`

权限并非“固定六级体系”，而是由以下组合构成：

- permission mode（`read-only/workspace-write/danger-full-access/ask/on-request`）
- deny/allow 规则
- 路径限制和额外目录白名单
- 交互式审批（按模式触发）

对应代码：

- `src/tools.rs`
- `src/runtime.rs`（`deny_reason` / `request_tool_approval`）

### 1.4 流式呈现

CLI 与 API 模型采用 streaming 输出，工具执行与中间状态实时展示，最终再给出收敛答案。

---

## 2. “更默契”升级：让 CLI 与模型像搭档而不是“问答器”

下面是建议的 8 项升级，按“高收益、低侵入”优先。

## 2.1 升级 1：上下文契约头（Context Contract Header）

在每轮请求前追加一个短结构块，明确告诉模型：

- 当前工具可用性
- 当前权限模式
- 用户硬约束（如 run-only / no-uv / 禁止分支操作）
- 本轮预算（steps/duration/no-progress）
- 上轮停止原因

效果：减少模型“猜环境”导致的错误动作（例如重复 edit_file 被拦截）。

建议落点：

- `src/main.rs`（follow-up prompt 组装处）
- `src/runtime.rs`（system/user 注入策略）

## 2.2 升级 2：工具结果标准化（Tool Result Canonicalization）

将工具结果统一为机器更好消费的短格式（保留人类可读）：

- `status`：ok/error/blocked
- `category`：permission/network/syntax/constraint_block/timeout
- `hint`：一行修复建议
- `next_action`：推荐下一步动作模板

效果：模型更容易“一步修正”而非盲试。

建议落点：

- `src/runtime.rs`（工具结果回灌时）
- `src/main.rs`（strict follow-up hint）

## 2.3 升级 3：计划握手（Plan Handshake）

引入轻量两段式执行：

1. 先让模型给出 3-7 行行动计划（只读检查 + 预期变更文件）  
2. 再执行工具调用

对复杂任务默认启用，对简单任务直通。

效果：减少来回试错和无效大循环。

建议落点：

- `run_auto_tool_loop` / `run_prompt_auto_tool_loop`

## 2.4 升级 4：失败记忆窗口（Failure Memory Window）

把最近 N 次失败摘要压缩后注入 follow-up：

- 最近失败工具
- 失败类型
- 已尝试纠正动作
- 禁止再试动作（短时间内）

效果：避免“同错重试”。

当前已有基础（failure category），可直接扩展。

## 2.5 升级 5：自适应预算（Adaptive Budget）

动态调整：

- 简单任务：低 steps，低 no-progress
- 复杂任务：高 steps，但更强的“阻塞快速退出”

效果：降低成本并提升复杂任务成功率。

## 2.6 升级 6：文件摘要缓存（File Synopsis Cache）

CLI 维护每个文件的短摘要（结构、关键函数、最近改动点），当模型频繁反复 read 同一文件时，优先回摘要 + 精确行段补读。

效果：减少 token 与 IO 开销。

## 2.7 升级 7：置信度门控（Confidence Gate）

模型在关键动作前输出一个简短“置信声明”：

- 目标文件/行号
- 风险点
- 回滚策略

低置信时自动走更保守策略（先读再改、先 dry-run）。

## 2.8 升级 8：双速度模式（Sprint / Deep）

为用户提供明确开关：

- `sprint`：快执行、低解释、预算紧
- `deep`：重验证、重可解释、预算宽

效果：让用户明确预期并主动控成本。

---

## 3. 分阶段落地（建议 3 周）

## 第 1 周（低风险高收益）

1. 上下文契约头  
2. 工具结果标准化  
3. 失败记忆窗口（扩展现有分类）

## 第 2 周（质量提升）

1. 计划握手（仅 strict/复杂任务触发）  
2. 自适应预算  
3. 双速度模式

## 第 3 周（性能与稳定）

1. 文件摘要缓存  
2. 置信度门控  
3. 指标面板（成功率、平均步数、失败重试率）

---

## 4. 建议的关键指标（衡量“更默契”是否成立）

至少跟踪以下指标：

1. 每任务平均工具调用数（越低越好）  
2. 重复失败率（同类错误重复出现）  
3. 约束冲突率（run-only/no-uv 等导致阻塞）  
4. 首次成功率（一次 loop 完成任务）  
5. token/成本与耗时（按任务类型分组）

---

## 5. 一句话总结

当前 Rust CLI 已具备可靠的“本地执行代理”基础；要让 CLI 与 API 模型更默契，关键不是再堆工具，而是提升“协同协议质量”：把约束、失败、预算、计划显式化，让模型在正确边界内稳定收敛。

---

## 产品摘要（对外沟通版）

Rust CLI 现在已经不是“纯问答”工具，而是“可执行的本地代理”。它能读取项目规则、理解代码上下文、调用本地工具并持续迭代直到完成任务。下一阶段的重点不是增加更多按钮，而是提升协同质量：让模型更懂边界、更少走弯路、更稳定收敛。最终用户会直接感受到三件事：成功率更高、成本更低、等待时间更可控。

## 工程摘要（对内执行版）

先做高收益低风险项：`Context Contract Header`、`Canonical Tool Result Schema`、`Failure Memory Window`。这三项能在不破坏主流程的前提下，显著降低重复失败和约束冲突。随后推进 `Plan Handshake` 与 `Adaptive Budget`，并通过指标（平均工具调用数、重复失败率、首次成功率、成本/时延）评估收益，按数据决定后续缓存与门控策略。
