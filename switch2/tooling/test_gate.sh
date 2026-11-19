#!/usr/bin/env bash
# Gate runner: does not certify the full Rust application.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 -m unittest discover -s "$ROOT/tests" -p 'test_*.py' -v
bash -n "$ROOT"/*.sh "$ROOT"/tooling/*.sh
python3 -m py_compile "$ROOT"/tooling/*.py
python3 "$ROOT/tooling/regenerate_patch.py"
python3 "$ROOT/tooling/regenerate_rust_patch.py"
if command -v git >/dev/null; then
  # Syntax check relative to a disposable fresh repository.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  git -C "$tmp" init -q
  git -C "$tmp" apply --check --whitespace=error "$ROOT/patches/native-additions.patch"
  git -C "$tmp" apply --check --whitespace=error "$ROOT/patches/rust-additions.patch"
fi
