# ASI Code v0.3.0 — Release Notes

A modern terminal coding agent in Rust. Single binary. Multi-provider.
Claude Code–style workflow.

> **中文版本在下方** (Chinese version below)

---

## English

### Highlights

- **Multi-provider** — DeepSeek, OpenAI, Claude (API key + Claude OAuth token).
  One CLI, three providers, same workflow. Switch with `/provider`.
- **Full Claude Code feature parity for the agent loop** — `/work`, `/code`,
  `/secure`, `/review`, `/agent spawn`, `/scan`, `/skills`, `/cron`,
  `/worktree`, `/mcp`, `/plugin`, `/hooks`. 60+ slash commands.
- **8 built-in tools** — `read_file`, `write_file`, `edit_file`, `glob_search`,
  `grep_search`, `bash`, `web_search`, `web_fetch`. Robust to provider
  variations (camelCase / snake_case / Anthropic-style XML / DeepSeek-style
  markdown — all forgiven by the parser).
- **Sandbox + audit** — local sandbox preflight, per-tool permission
  decisions, JSONL audit log, auto-review guard with severity threshold.
- **Stable JSON / JSONL schemas (v1)** for `prompt`, `agent`, `mcp`, `plugin`,
  `hooks`, `review` — drive the CLI from CI without screen-scraping.
- **Single-file Windows installer** + `.zip` portable distribution + signed
  Cargo source build.

### Tested against

- HumanEval-style algorithms: `is_palindrome`, `two_sum`, `roman_to_int` —
  3 / 3 hidden tests pass.
- SWE-bench-style bug fixes: `intervals.merge_intervals` subtle off-by-one —
  PASS, 8 hidden tests.
- Multi-file refactor: mutable-default-arg bug across 3 modules — PASS,
  4 hidden tests.
- Terminal-Bench-style log analysis pipeline — PASS, exact summary match.
- CLI-Tool-Bench: `csvsum --csv X --col Y` — PASS, 2 / 2 cases.
- End-to-end repo driving: clones `karpathy/autoresearch`, patches data URL,
  installs deps via pip, edits `train.py` for an 8 GB GPU, launches training
  and captures logs — all driven by the agent.
- Real bug fix on a 7 KB PyTorch demo: 73 turns / 372 s / $0.02 to read,
  diagnose 4 errors, switch from `edit_file` → `write_file` after 3 failed
  edits, run training, hit **97.5 % test accuracy** end-to-end.

### Install (Windows)

```powershell
cd D:\Code\Rust
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease
```

Or build straight from source:

```powershell
cargo build --release
.\target\release\asi.exe repl --provider deepseek --model deepseek-chat
```

### Quickstart

```powershell
$env:DEEPSEEK_API_KEY = "sk-..."
cargo run --release -- repl --provider deepseek --model deepseek-chat --project D:\your-project
```

The first time you run without `--no-setup`, an interactive wizard asks
for provider, API key (input is hidden), and default model, then saves the
config. Subsequent runs skip the wizard automatically.

In the REPL:

```
ASI Code> /work refactor the auth middleware to use JWT
ASI Code> 帮我把这个 Python 脚本改成异步的
ASI Code> 搜索 GPT-5.5 的最新发布信息
```

### Environment knobs (most useful)

| Variable | Default | Purpose |
|---|---|---|
| `DEEPSEEK_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` | — | Provider keys |
| `ASI_DEEPSEEK_REASONING_EFFORT` | `auto` | `low` / `medium` / `high` / `max` for DeepSeek reasoner models |
| `ASI_SEARCH_BACKEND` | `bing` | `bing` (default) or `duckduckgo` |
| `ASI_BING_HOST` | `cn.bing.com` | Override to `bing.com` for global English-first results |
| `ASI_SANDBOX_ALLOW_NETWORK` | unset | Set to `1` to allow `bash curl/wget/...` |
| `ASI_BASH_TIMEOUT_SECS` | `900` | Per-bash-call timeout |

### Architecture notes

- Single Rust binary, ~14 MB optimized.
- **Native tool-call paths for OpenAI and Claude** — `tools[]` is sent in
  the request body and the SSE stream's structured `tool_calls` /
  `tool_use` blocks are consumed directly. No text parsing needed for
  these two providers.
- **Text-format fallback for DeepSeek** — DeepSeek's chat API does not
  expose a reliable native `tool_use` field, so we parse the model's
  text-format tool calls. The parser is tolerant of every shape we've
  observed (`<tool_call>`, `<tool_calls>`, `<function_calls>`, `<invoke>`,
  `**Calling:**` markdown, self-closing tool tags, fenced code, named or
  positional args).
- Independent `web_search` backend dispatcher — Bing today, Brave / Google
  CSE / SearxNG one PR away.

### Known limitations

- DeepSeek API does not expose native `tool_use`; we parse the model's
  text-format tool calls (multiple shapes supported, tested against
  DeepSeek v4 Pro). When using OpenAI or Claude providers, native tool
  calls are used.
- Web search uses Bing HTML scraping (TOS gray area at commercial scale).
  Replace with Brave Search API in production deployments — `web_search`
  is factored as a backend dispatcher exactly so you can swap in a JSON
  API.
- Long-horizon tasks (> 1 hour continuous reasoning) are not yet
  battle-tested at the same level as Claude Code's published demos.
  Context compaction quality is the bottleneck.

### License

MIT. Use, modify, fork, embed, sell — all OK. Just keep the copyright notice.

---

## 中文

### 亮点

- **多 provider 支持** —— DeepSeek、OpenAI、Claude（API key + Claude OAuth
  token）。一个 CLI，三个 provider，同一套工作流。`/provider` 切换。
- **Claude Code 风格的 agent 循环全套对齐** —— `/work`、`/code`、`/secure`、
  `/review`、`/agent spawn`、`/scan`、`/skills`、`/cron`、`/worktree`、
  `/mcp`、`/plugin`、`/hooks`，60+ 个斜杠命令。
- **8 个内置工具** —— `read_file`、`write_file`、`edit_file`、`glob_search`、
  `grep_search`、`bash`、`web_search`、`web_fetch`。对 provider 输出的各种
  形态（camelCase / snake_case / Anthropic XML / DeepSeek markdown）都能
  容忍。
- **沙箱 + 审计** —— 本地 sandbox 预检、每个工具调用的权限决策、JSONL 审计
  日志、按严重度门控的自动 review。
- **稳定的 JSON / JSONL schema（v1）** —— `prompt`、`agent`、`mcp`、`plugin`、
  `hooks`、`review` 全部有版本化契约，CI 直接消费不用 scrape 终端。
- **单文件 Windows 安装器** + `.zip` 便携包 + Cargo 源码构建。

### 实测过

- HumanEval 风格算法题：`is_palindrome`、`two_sum`、`roman_to_int` ——
  3 / 3 隐藏测试全过。
- SWE-bench 风格 bug 修复：`intervals.merge_intervals` 边界条件 bug ——
  全过 8 个隐藏测试。
- 多文件重构：可变默认参数 bug 散在 3 个模块 —— 全过 4 个隐藏测试。
- Terminal-Bench 风格日志分析流水线 —— 输出格式精确匹配。
- CLI-Tool-Bench：`csvsum --csv X --col Y` —— 2 / 2 用例通过。
- 端到端驱动外部仓库：克隆 `karpathy/autoresearch`、patch 数据 URL、用 pip
  装依赖、改 `train.py` 适配 8 GB GPU、启动训练并捕获日志 —— 全程 agent
  自己驱动。
- 真实 PyTorch demo 修复：7 KB 单文件训练脚本，**73 turn / 372 秒 / $0.02**
  完成读文件 → 定位 4 个错误 → 三次 edit_file 失败后切到 write_file 重写 →
  运行训练 → **测试准确率 97.5 %**。

### 安装（Windows）

```powershell
cd D:\Code\Rust
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease
```

或者直接源码构建：

```powershell
cargo build --release
.\target\release\asi.exe repl --provider deepseek --model deepseek-chat
```

### 快速上手

```powershell
$env:DEEPSEEK_API_KEY = "sk-..."
cargo run --release -- repl --provider deepseek --model deepseek-chat --project D:\你的项目
```

第一次运行（不带 `--no-setup`）会进入交互向导，让你选 provider、输入 API
key（**输入不回显**）、选默认模型，然后存配置。下次运行自动跳过向导。

进 REPL 之后随便聊：

```
ASI Code> /work 把 auth 中间件改成 JWT
ASI Code> 帮我把这个 Python 脚本改成异步的
ASI Code> 搜索 GPT-5.5 的最新发布信息
```

### 常用环境变量

| 变量 | 默认值 | 作用 |
|---|---|---|
| `DEEPSEEK_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` | — | provider 密钥 |
| `ASI_DEEPSEEK_REASONING_EFFORT` | `auto` | DeepSeek reasoner 模型的推理强度 |
| `ASI_SEARCH_BACKEND` | `bing` | `bing`（默认）或 `duckduckgo` |
| `ASI_BING_HOST` | `cn.bing.com` | 改成 `bing.com` 拿全球英文优先结果 |
| `ASI_SANDBOX_ALLOW_NETWORK` | 未设 | 设为 `1` 放行 `bash curl/wget` |
| `ASI_BASH_TIMEOUT_SECS` | `900` | bash 单次调用超时 |

### 架构要点

- 单 Rust 二进制，优化后约 14 MB。
- **OpenAI 和 Claude 走原生工具调用路径** —— 请求体里发 `tools[]`，SSE 流里
  直接消费结构化的 `tool_calls` / `tool_use` block。这两个 provider 完全不
  经过文本解析。
- **DeepSeek 走文本格式兜底** —— DeepSeek 的 chat API 没有稳定可靠的原生
  `tool_use` 字段，所以解析模型的文本输出。解析器对常见形态都做了容忍：
  `<tool_call>`、`<tool_calls>`、`<function_calls>`、`<invoke>`、
  `**Calling:**` markdown、自闭合工具标签、代码 fence、命名或位置参数。
- 独立的 `web_search` 后端分发器 —— 当前 Bing，换 Brave / Google CSE /
  SearxNG 只要一个 PR。

### 已知限制

- DeepSeek API 没有原生 `tool_use` 字段，我们解析模型的文本格式工具调用
  （已支持多种形态，对 DeepSeek v4 Pro 做过实测）。OpenAI / Claude
  provider 下走的是原生 tool calls。
- Web 搜索用的是 Bing HTML 爬取（商业规模下 TOS 灰色）。生产部署建议换成
  Brave Search API —— `web_search` 已经做成 backend 分发器，换底层只动
  一处。
- 长任务（> 1 小时连续推理）还没经过 Claude Code 公开 demo 那种量级的
  压力测试，上下文压缩质量是当前瓶颈。

### License

MIT。用、改、fork、嵌入、商用 —— 都可以，留个版权声明就行。
