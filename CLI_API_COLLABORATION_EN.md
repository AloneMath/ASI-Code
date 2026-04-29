# CLI and API Model Collaboration (Rust CLI: Current Reality + Upgrade Blueprint)

This document has two goals:

1. Describe how the current Rust CLI actually collaborates with the model (accurate to code).
2. Propose practical upgrades to make CLI↔API collaboration more aligned, stable, and efficient.

---

## 1. Current State: How the Rust CLI Collaborates Today

### 1.1 Context Assembly (CLI prepares first)

Before sending a user request, the CLI builds runtime context, including:

- Project policy docs: `CLAUDE.md` (preferred), fallback to `README.md` / `AGENTS.md`
- Git snapshot: `git status --short`
- Recent commits: `git log --oneline -5`
- Session history
- Provider/model/permission mode

Code references:

- `src/runtime.rs` (`load_project_context*`)
- `src/runtime.rs` (`Runtime::new`)

### 1.2 TAOR Loop (Think-Act-Observe-Repeat)

The architecture is model-driven, CLI-executed, and result-fed-back. There are two execution lanes:

1. Native tool-calling lane  
Model returns function/tool_use calls; Runtime executes and feeds structured results back.

2. Legacy `/toolcall` lane  
Model emits `/toolcall ...` lines; `OrchestratorEngine` parses/plans/batches execution and follows up.

Code references:

- Native path: `src/runtime.rs`
- Legacy orchestration: `src/orchestrator/engine.rs`, `src/main.rs` auto loops

### 1.3 Tools and Safety Controls

Current public tools are 8:

- `read_file`
- `write_file`
- `edit_file`
- `glob_search`
- `grep_search`
- `web_search`
- `web_fetch`
- `bash`

Safety/approval is not a fixed “6-level gate”; it is a combination of:

- permission mode (`read-only/workspace-write/danger-full-access/ask/on-request`)
- allow/deny rules
- path restrictions + additional directories
- interactive approval flow when required

Code references:

- `src/tools.rs`
- `src/runtime.rs` (`deny_reason`, `request_tool_approval`)

### 1.4 Streaming UX

CLI uses streaming with the provider so users see incremental reasoning/output and tool progress in real time.

---

## 2. Making CLI↔API Collaboration More “In Sync”

Below are 8 upgrades prioritized for high impact with low-to-medium intrusion.

## 2.1 Upgrade 1: Context Contract Header

Inject a short structured header into each turn:

- available tools/features
- effective permission constraints
- user hard constraints (run-only, no-uv, no-branch-ops)
- current execution budget (steps/duration/no-progress)
- previous loop stop reason

Benefit: fewer environment/constraint mismatches and fewer invalid actions.

## 2.2 Upgrade 2: Canonical Tool Result Schema

Standardize tool outputs for model consumption:

- `status`: ok/error/blocked
- `category`: permission/network/syntax/constraint_block/timeout
- `hint`: one-line corrective hint
- `next_action`: recommended action template

Benefit: better one-step corrections, fewer trial-and-error loops.

## 2.3 Upgrade 3: Plan Handshake

Use a two-stage flow for complex tasks:

1. model emits a 3-7 line action plan (read checks + expected file edits)
2. execute tool calls

Benefit: fewer blind retries and better task convergence.

## 2.4 Upgrade 4: Failure Memory Window

Feed a compressed summary of last N failures into follow-up prompts:

- failing tool(s)
- failure category
- attempted fixes
- short-term “do not retry” hints

Benefit: avoids repeated identical failures.

## 2.5 Upgrade 5: Adaptive Budgets

Dynamically tune steps/no-progress thresholds by task complexity:

- simple task: tight budget
- complex task: broader budget + stronger block-fast exits

Benefit: better cost-performance balance.

## 2.6 Upgrade 6: File Synopsis Cache

Maintain lightweight per-file summaries (structure/key functions/recent edits) and use targeted range reads.

Benefit: lower token and IO overhead, especially on repeated reads.

## 2.7 Upgrade 7: Confidence Gate

Before risky actions, require a tiny confidence declaration:

- target file/line
- risk note
- rollback expectation

Low confidence triggers conservative mode (read-first, dry-run-first).

## 2.8 Upgrade 8: Dual-Speed Modes (Sprint / Deep)

Expose explicit operation modes:

- `sprint`: faster, lower explanation, tighter budget
- `deep`: stronger validation/explainability, wider budget

Benefit: predictable user expectations and better control of latency/cost.

---

## 3. Suggested Rollout (3 Weeks)

## Week 1 (Low risk, high impact)

1. Context Contract Header  
2. Canonical Tool Result Schema  
3. Failure Memory Window

## Week 2 (Quality upgrades)

1. Plan Handshake  
2. Adaptive Budgets  
3. Sprint/Deep modes

Status note (implemented in current Rust CLI):

- `repl --speed <sprint|deep>` and `prompt --speed <sprint|deep>`
- runtime `/speed` command in REPL
- `speed` visible in `/status` and `/auto status`
- `speed` included in Context Contract Header and prompt JSON agent metadata
- adaptive budgets incorporate both complexity and speed
- `execution_speed` persisted in `config.json` (also configurable via `config --execution-speed <sprint|deep>`)

## Week 3 (Performance + robustness)

1. File Synopsis Cache  
2. Confidence Gate  
3. Metrics panel

Status note (implemented in current Rust CLI):

- `File Synopsis Cache` is now active inside auto-agent loops
- cache source is successful `read_file` tool results (path/range/total-lines + compact summary)
- follow-up prompts now include a compact "File Synopsis Cache" block to reduce repeated full reads
- `/auto` and `/auto status` now print cache stats (`entries/hits/misses/inserts`)

---

## 4. Metrics to Validate Better Collaboration

Track at least:

1. average tool calls per task (lower is better)
2. repeated failure rate
3. constraint conflict rate
4. first-pass completion rate
5. token/cost/latency by task class

---

## 5. One-Line Summary

The Rust CLI already has a strong local-agent foundation. To become truly “in sync” with API models, the next step is not more tools, but better collaboration contracts: explicit constraints, explicit failure memory, explicit budgets, and explicit plans.

---

## Product Summary (External Communication)

The Rust CLI is already beyond a simple chat assistant: it is an executable local agent that can read project rules, understand repository context, invoke tools, and iterate to completion. The next phase is not “more features for the sake of features,” but better collaboration quality between CLI and model. Users should see clear outcomes: higher completion reliability, lower wasted cost, and more predictable turnaround time.

## Engineering Summary (Internal Execution)

Start with high-impact, low-risk upgrades: `Context Contract Header`, `Canonical Tool Result Schema`, and `Failure Memory Window`. These improve convergence without rewriting the core loop. Then add `Plan Handshake` and `Adaptive Budgets`, and evaluate with operational metrics (tool calls per task, repeated-failure rate, first-pass completion, cost/latency). Use those metrics to prioritize deeper optimizations such as synopsis cache and confidence gating.
