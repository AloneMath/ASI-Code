# GitHub Repo Copy (Open-Core, EN + ZH)

Use this file as ready-to-paste copy for your GitHub repository page, release posts, and discussion pinned posts.

## 1) Repo Description (for GitHub "About")

EN:
`ASI Code is a Rust CLI coding agent with tool-use, approvals, and local automation. Open-core: free local core + commercial team/cloud features.`

ZH:
`ASI Code 是一个 Rust CLI 编程智能体，支持工具调用、交互审批与本地自动化。采用 Open-Core：开源本地核心 + 商业团队/云功能。`

## 2) Repo Short Tagline

EN:
`Modern terminal agent for coding workflows.`

ZH:
`面向现代开发流程的终端智能体。`

## 3) README Intro Block

EN:
`ASI Code is a terminal coding agent built in Rust. It helps developers inspect code, edit files, run commands, and iterate quickly with interactive safety controls.`

ZH:
`ASI Code 是一个用 Rust 构建的终端编程智能体。它可以帮助开发者检查代码、编辑文件、运行命令，并通过交互式安全控制实现快速迭代。`

## 4) Open-Core Positioning (paste into README/website)

EN:
- Community (MIT): local CLI core, REPL, provider integration, tool calls, wiki commands, and core workflows.
- Commercial (roadmap): managed gateway, team workspace, policy packs, enterprise audit dashboard, priority support.
- Goal: keep individual developer productivity features open while monetizing team-scale reliability and governance.

ZH:
- Community（MIT）：本地 CLI 核心、REPL、模型提供商接入、工具调用、wiki 命令与核心工作流。
- Commercial（规划）：托管网关、团队协作空间、策略包、企业审计面板、优先支持。
- 目标：个人开发者生产力能力保持开源，团队级稳定性与治理能力商业化。

## 5) Feature Matrix (for README)

| Capability | Community (Open Source) | Commercial (Planned) |
|---|---|---|
| Local CLI Agent | Yes | Yes |
| Local File/Command Tools | Yes | Yes |
| Interactive Approvals | Yes | Yes |
| Wiki Init/Ingest/Query/Lint | Yes | Yes |
| Managed API Gateway | No | Yes |
| Team Workspaces | No | Yes |
| Policy Packs / Admin Controls | No | Yes |
| Audit Dashboard (Web) | No | Yes |
| SLA / Priority Support | No | Yes |

## 6) Feedback CTA Block

EN:
`Public beta is open. Please report bugs in Issues and share UX/product feedback in Discussions. Your real-world workflows directly shape the roadmap.`

ZH:
`Public Beta 已开放。请在 Issues 提交可复现 Bug，在 Discussions 提交体验与产品建议。你的真实使用反馈会直接影响路线图。`

## 7) Pinned Discussion Post Template

Title:
`Public Beta Feedback Thread (EN + 中文)`

Body:
`Thanks for trying ASI Code beta. Please share:
1) your OS and provider,
2) what task you tried,
3) where it worked well,
4) what blocked you,
5) what feature you want next.

感谢体验 ASI Code Beta。欢迎反馈：
1）系统与模型提供商，
2）你尝试的任务，
3）体验好的地方，
4）卡住的问题，
5）你最希望优先实现的功能。`

## 8) 2.0 Preview Highlights (EN + ZH)

EN:
`ASI Code 2.0 is coming soon. Highlights already merged on main:
- Tier 1 — native computer-control tools: screenshot, find_window, click / click_text, type_text (now with explicit window targeting and clipboard-paste delivery), read_screen_text OCR.
- Tier 2 — live engine bridges: ue5_bridge, blender_bridge, unity_bridge (with action=open|csharp|create_terrain|save for live-editor C# drops).
- Tier 3 — 3D-aware probes and temporal capture: blender_scene_probe, ue5_scene_probe, unity_scene_probe, probe_diff for deterministic structural verification, plus video_capture + video_keyframes (frame-folder design, no MP4 dependency) for temporal context.
- Smarter auto-loop: read-only investigation tool calls now count as progress, no longer trip the no-progress circuit breaker mid-investigation.`

ZH:
`ASI Code 2.0 即将发布。已合入 main 的核心亮点：
- Tier 1 — 原生电脑控制工具：screenshot、find_window、click / click_text、type_text（已支持显式窗口定位与剪贴板粘贴），以及 read_screen_text OCR。
- Tier 2 — 专业软件实时桥接：ue5_bridge、blender_bridge、unity_bridge（action=open|csharp|create_terrain|save，可向已打开的编辑器投放 C# 脚本）。
- Tier 3 — 3D 感知探针与时序采集：blender_scene_probe、ue5_scene_probe、unity_scene_probe、用于确定性结构校验的 probe_diff，以及帧目录式的 video_capture + video_keyframes（无需 MP4 依赖）提供时序上下文。
- 更聪明的自动循环：只读类调查工具调用也计入进度，不再在调查过程中误触发"无进度"熔断。`

## 9) First Public Beta Release Post Template

Title:
`v0.3.0-beta.1: Open-Core CLI Agent Public Beta`

Body (short):
`This is the first public beta of ASI Code.
- Rust CLI agent with REPL and tool-use
- Interactive approvals (`ask` / `on-request`)
- Windows execution self-heal
- Wiki command group (`init/ingest/query/lint`)

Please use it in real projects and report issues with reproducible steps.`
