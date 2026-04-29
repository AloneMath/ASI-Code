# ASI Code v0.3.0-beta.1 Release Notes

## Highlights

- Added interactive tool approvals (`ask` / `on-request`) with session rule memory (`y/n/a/d`).
- Added Windows execution self-heal for common shell/dependency issues (`npm/npx/pnpm` normalization, missing `yarn` recovery).
- Added first wiki command group: `wiki init`, `wiki ingest`, `wiki query`, `wiki lint`.

## Install

- Windows zip: `asi-code-windows-x64-0.3.0-beta.1.zip`
- Installer exe: `asi-code-installer-0.3.0-beta.1.exe`

## Checksums

- `asi-code-windows-x64-0.3.0-beta.1.zip.sha256.txt`
- `asi-code-installer-0.3.0-beta.1.exe.sha256.txt`

## Beta Known Issues

- Unsigned binaries may trigger SmartScreen.
- Some enterprise/proxy networks can block provider endpoints.
- Existing plaintext env vars are still visible via OS environment inspection.

## Validation Snapshot

```powershell
cargo check --release
cargo test --release
```

## Feedback Channels

- Bugs: GitHub Issues
- Ideas and UX feedback: GitHub Discussions
- Security: private advisory report (see SECURITY.md)
