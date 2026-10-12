#!/bin/sh
# The project's checks. Used by the git hooks, CI, and after any code change.
#   scripts/check.sh          fmt, clippy (warnings are errors), full test suite
#   scripts/check.sh --fast   fmt and clippy only
set -eu
cd "$(dirname "$0")/.."

cargo fmt --check
cargo clippy --all-targets -- -D warnings
if [ "${1:-}" != "--fast" ]; then
    cargo test
fi
