# ASI Code v0.3.0 — Release Notes

ASI Code is a modern Rust terminal coding agent with multi-provider support, native and text-based tool calling, interactive approvals, sandbox controls, audit logs, and resumable auto-agent loops.

## Highlights

- DeepSeek, OpenAI, and Claude provider integrations.
- Native tool calls for OpenAI-compatible and Claude endpoints.
- Tolerant text-tool parser for DeepSeek and gateway variants.
- Eight core tools: file inspection/editing, search, shell, and web access.
- JSON/JSONL command schemas for automation and CI.
- Windows installer and portable distribution.
- Smart context compaction, checkpoint resume, file synopsis caching, failure memory, and progress-aware loop circuit breakers.
- Model compatibility handling for modern reasoning models and gateway parameter variants.

## Quickstart

```powershell
$env:OPENAI_API_KEY = "sk-..."
cargo run --release -- repl --provider openai --model gpt-4.1-mini --project D:\your-project
```

## Architecture

- OpenAI-compatible providers receive structured `tools[]` definitions and replay assistant tool calls plus matching `role=tool` results.
- Claude receives native `tool_use` and `tool_result` content blocks.
- DeepSeek uses tolerant text-tool parsing when the endpoint does not expose native tool calls.
- Modern GPT and reasoning models use `max_completion_tokens` and omit unsupported sampling fields; a compatibility retry handles gateways that require the older parameter name.

## Known limitations

- Provider gateways can still impose their own model, context, and tool-call limits.
- Web search uses the configured backend and should be reviewed for production terms of service.

## License

MIT.
