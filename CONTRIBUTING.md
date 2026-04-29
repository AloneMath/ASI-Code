# Contributing

Thanks for contributing to ASI Code.

## Development Setup

1. Install Rust stable toolchain.
2. Clone the repository.
3. Build and run checks:

```powershell
cargo check --release
cargo test --release
```

## Pull Request Guidelines

1. Keep changes focused and small.
2. Include tests for behavior changes when possible.
3. Update docs/README when CLI behavior changes.
4. Ensure CI passes before requesting review.

## Commit and Branching

- Use clear commit messages with a short scope.
- Prefer feature branches and open PRs against `main`.

## Bug Reports and Ideas

- Use GitHub Issues for reproducible bugs.
- Use GitHub Discussions for product feedback and ideas.

## Security

Do not open public issues for security vulnerabilities.
Follow the private reporting process in [SECURITY.md](SECURITY.md).
