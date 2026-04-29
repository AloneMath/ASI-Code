#!/usr/bin/env bash
set -euo pipefail

# ASI Code one-click launcher for Ubuntu/WSL.
# Usage examples:
#   ./start_asi.sh
#   ./start_asi.sh /mnt/d/Code/Rust
#   ./start_asi.sh /mnt/d/Code/YourProject
#   ./start_asi.sh /mnt/d/Code/Rust --no-setup

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_PATH="${1:-$SCRIPT_DIR}"
shift $(( $# > 0 ? 1 : 0 ))

if [[ ! -d "$PROJECT_PATH" ]]; then
  echo "ERROR: project directory not found: $PROJECT_PATH" >&2
  exit 1
fi

if [[ ! -f "$SCRIPT_DIR/Cargo.toml" ]]; then
  echo "ERROR: Cargo.toml not found in $SCRIPT_DIR" >&2
  exit 1
fi

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1090
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found. Install Rust first: https://rustup.rs" >&2
  exit 1
fi

echo "ASI Code launcher"
echo "Binary project: $SCRIPT_DIR"
echo "Workspace project: $PROJECT_PATH"
echo
echo "Tip: export API keys before running for zero prompts:"
echo "  export OPENAI_API_KEY=..."
echo "  export DEEPSEEK_API_KEY=..."
echo "  export ANTHROPIC_API_KEY=..."
echo

cd "$SCRIPT_DIR"
cargo run -- repl --project "$PROJECT_PATH" "$@"
