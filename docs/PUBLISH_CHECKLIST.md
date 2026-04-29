# GitHub Publish Checklist — ASI Code v0.3.0

Tick each box before pushing. Roughly 30–45 minutes total if you've never
done this before.

## 1. Pre-flight (local)

- [ ] `D:\Code` cleaned up: only `Rust/`, `CLAUDE.md`, `.vscode/` remain at the
      root.
- [ ] No API keys in any tracked file:
      ```powershell
      cd D:\Code\Rust
      Select-String -Path *.* -Pattern "sk-[a-zA-Z0-9]{30,}" -Recurse -ErrorAction SilentlyContinue
      ```
      Should return **nothing**. If it does, redact and re-grep.
- [ ] `.gitignore` includes `target/`, `sessions/`, `.asi/`, `.asi_audit.jsonl`,
      and the `_test_cli_*.log` family (already done in v0.3.0).
- [ ] **Revoke and rotate** any DeepSeek / OpenAI / Anthropic key you've
      exposed in chat or screenshots before going public:
      <https://platform.deepseek.com/api_keys>.
- [ ] `cargo build --release` succeeds clean.
- [ ] `cargo test --release --bin asi` ≥ 460 passing tests.

## 2. Repo metadata polish

- [ ] `README.md` first paragraph leads with the modern-CLI positioning
      (multi-provider, single Rust binary, Claude Code–style UX).
- [ ] `Cargo.toml` `repository = "https://github.com/<your-username>/asi-code"`
      points at the real GitHub URL (update after step 3).
- [ ] `LICENSE` file present at repo root (you have it already — MIT).
- [ ] `CONTRIBUTING.md` exists and tells contributors how to file issues / PRs.

## 3. Create the GitHub repo

Go to <https://github.com/new>:

- [ ] Repository name: `asi-code`
- [ ] Description (one line, English):
      `A modern terminal coding agent in Rust. DeepSeek / OpenAI / Claude. Claude Code–style workflow.`
- [ ] **Public** (so people can find it). Private if you want to test the
      water; you can flip it later.
- [ ] **Do NOT** check "Add a README", "Add .gitignore", or "Add a license".
      You already have all three locally.
- [ ] Click **Create repository**.
- [ ] Copy the URL it shows: `https://github.com/<you>/asi-code.git`.

## 4. Local git init + first push

```powershell
cd D:\Code\Rust
git init
git branch -M main
git status                   # look before adding
git add .
git status                   # double-check no junk
git commit -m "Initial public release: ASI Code v0.3.0"
git remote add origin https://github.com/<you>/asi-code.git
git push -u origin main
```

If push asks for credentials:

- **HTTPS + token**: <https://github.com/settings/tokens> → Generate new
  token (classic) → check `repo` scope → use the token as the password.
- **SSH**: replace the URL with `git@github.com:<you>/asi-code.git`.

## 5. Post-push polish (5 min)

- [ ] On the repo page, click ⚙️ next to **About** → set:
  - Description (paste from step 3)
  - Website: leave blank or fill if you have one
  - Topics: `rust`, `cli`, `ai-agent`, `claude-code`, `deepseek`, `openai`,
    `anthropic`, `coding-assistant`, `agent`, `llm`
- [ ] Releases → **Create a new release** → tag `v0.3.0` → title
      `ASI Code v0.3.0 — initial public release` → paste the body of
      `docs/RELEASE_NOTES_v0.3.0.md`. Attach `dist/asi-code-windows-x64-0.3.0.zip`
      and the installer if you have them built.
- [ ] (Optional) Record a 30-second demo:
      - Free tool: <https://asciinema.org> (CLI screencast).
      - Paste the asciinema link at the top of README.

## 6. Announce (optional, do it when you're ready, not now)

When you're rested, pick one or two — you don't need to do all:

- [ ] **Hacker News** Show HN: `Show HN: ASI Code – a modern terminal coding agent in Rust (DeepSeek / OpenAI / Claude)`
- [ ] **Reddit** — `/r/rust`, `/r/LocalLLaMA`, `/r/coding`.
- [ ] **Twitter/X** — tag `@karpathy` (he likes single-author Rust projects),
      plus the provider accounts you support. Keep the post under 280 chars.
- [ ] **lobste.rs** — Rust + CLI fits the audience well.
- [ ] **dev.to** / **Medium** — write a short post about what you built and
      why; embed the asciinema demo.

**Pick one. See if anyone shows up.** That's how Pieter Levels, Linus, and
karpathy started too — one post, one repo, no marketing budget.
