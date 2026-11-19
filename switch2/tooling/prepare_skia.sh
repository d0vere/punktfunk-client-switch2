#!/usr/bin/env bash
# Prepare the exact upstream Skia m153-0.101.2 tree for Rust-Skia 0.153.3.
# This script is sourced by build_official_rust.sh; it can also be executed for preflight.
# No system paths, rustup installation or upstream tracked files are mutated.
set -euo pipefail
PF2_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PF2_WORK="${PF2_WORK:-$PF2_ROOT/work}"
PF2_REPO="$PF2_WORK/official-client"
PF2_SDK="${DEVKITPRO:-/opt/devkitpro}"
PF2_SKIA_REV=m153-0.101.2
PF2_SKIA_VERSION=0.153.3

[[ -f "$PF2_REPO/toolchain/skia/horizon.patch" ]] || { echo 'Missing pinned upstream Horizon Skia patch' >&2; return 23 2>/dev/null || exit 23; }
[[ -f "$PF2_REPO/toolchain/skia/args.gn" ]] || { echo 'Missing upstream Skia GN args' >&2; return 23 2>/dev/null || exit 23; }
[[ -f "$PF2_REPO/Cargo.lock" ]] || { echo 'Official Cargo.lock is missing' >&2; return 23 2>/dev/null || exit 23; }
python3 - "$PF2_REPO/Cargo.lock" "$PF2_SKIA_VERSION" <<'PY'
import sys,tomllib
with open(sys.argv[1],'rb') as f: lock=tomllib.load(f)
matches=[p['version'] for p in lock['package'] if p['name']=='skia-bindings']
if matches!=[sys.argv[2]]:
    raise SystemExit(f'Wrong Skia pin (expected {sys.argv[2]}, got {matches})')
PY
for cmd in curl tar git python3 ninja aarch64-none-elf-gcc aarch64-none-elf-g++; do
    command -v "$cmd" >/dev/null || { echo "Skia prerequisite missing: $cmd" >&2; return 23 2>/dev/null || exit 23; }
done
[[ -f "$PF2_SDK/libounce/include/switch.h" ]] || { echo "No libounce headers: $PF2_SDK/libounce/include" >&2; return 23 2>/dev/null || exit 23; }
[[ -d "$PF2_REPO/toolchain/skia/include" ]] || { echo 'Missing original Skia compatibility includes' >&2; return 23 2>/dev/null || exit 23; }

# Caller can point at already downloaded, pinned Skia sources (must contain the same GN files).
export SKIA_SOURCE_DIR="${PF2_SKIA_SOURCE_DIR:-$PF2_WORK/skia-source/$PF2_SKIA_REV}"
if [[ ! -f "$SKIA_SOURCE_DIR/third_party/zlib/BUILD.gn" || ! -f "$SKIA_SOURCE_DIR/BUILD.gn" || ! -x "$SKIA_SOURCE_DIR/bin/gn" ]]; then
    if [[ -e "$SKIA_SOURCE_DIR" ]]; then
        echo "Skia source incomplete at $SKIA_SOURCE_DIR; refusing to overwrite" >&2
        return 23 2>/dev/null || exit 23
    fi
    mkdir -p "$(dirname "$SKIA_SOURCE_DIR")"
    temp="$(mktemp -d "$(dirname "$SKIA_SOURCE_DIR")/.skia-fetch-XXXXXX")"
    echo "Skia: fetching pinned $PF2_SKIA_REV from rust-skia/skia"
    if ! curl --fail --location --retry 3 --silent --show-error \
           "https://codeload.github.com/rust-skia/skia/tar.gz/$PF2_SKIA_REV" | \
           tar -xz -C "$temp" --strip-components=1 --exclude='*/infra'; then
        rm -rf "$temp"
        echo 'Skia download/extraction failed (nothing installed)' >&2
        return 23 2>/dev/null || exit 23
    fi
    mv "$temp" "$SKIA_SOURCE_DIR"
fi

# Same dependency sync as the official client-switch scripts/build-skia.sh. The marker
# only bypasses it after a successful run on the same extracted source tree.
if [[ ! -f "$SKIA_SOURCE_DIR/.pf2-deps-ok" ]]; then
    echo 'Skia: synchronizing third-party sources'
    ( cd "$SKIA_SOURCE_DIR" && GIT_SYNC_DEPS_QUIET=1 GIT_SYNC_DEPS_SKIP_EMSDK=1 \
      python3 tools/git-sync-deps )
    touch "$SKIA_SOURCE_DIR/.pf2-deps-ok"
fi
patch="$PF2_REPO/toolchain/skia/horizon.patch"
if (cd "$SKIA_SOURCE_DIR" && git apply --reverse --check "$patch" >/dev/null 2>&1); then
    echo 'Skia: upstream Horizon patch already present'
elif (cd "$SKIA_SOURCE_DIR" && git apply --check "$patch"); then
    (cd "$SKIA_SOURCE_DIR" && git apply "$patch")
    echo 'Skia: applied Horizon ARM/zlib and logging patch'
else
    echo 'Skia patch does not match pinned sources; refusing unverified GN edits' >&2
    return 23 2>/dev/null || exit 23
fi

# Skia binds for newlib must use GCC with libounce, not the Switch1 libnx headers.
export PF2_OFFICIAL_REPO="$PF2_REPO"
export DEVKITPRO="$PF2_SDK"
export CLANGCC="${PF2_SKIA_CC:-$PF2_ROOT/tooling/skia-cc.sh}"
export CLANGCXX="${PF2_SKIA_CXX:-$PF2_ROOT/tooling/skia-cxx.sh}"
[[ -x "$CLANGCC" && -x "$CLANGCXX" ]] || { echo "Skia GCC wrappers not executable" >&2; return 23 2>/dev/null || exit 23; }
export SKIA_GN_ARGS="$(cat "$PF2_REPO/toolchain/skia/args.gn")"
export FORCE_SKIA_BUILD=1
# The upstream workaround makes Clang's bindgen see a recognizable AArch64 C++ ABI;
# layout checks on target are still required after a full build.
# A name containing '-' cannot be exported in bash; build_official_rust.sh
# sets this hyphenated key using `env` on the cargo invocation.
# Reproducible bounded parallelism rather than using every CPU on machines with less RAM.
PF2_SKIA_JOBS="${PF2_SKIA_JOBS:-2}"
[[ "$PF2_SKIA_JOBS" =~ ^[1-9][0-9]*$ ]] || { echo 'PF2_SKIA_JOBS must be a positive integer' >&2; return 23 2>/dev/null || exit 23; }
mkdir -p "$PF2_WORK/skia-source"
SKIA_NINJA_COMMAND="$PF2_WORK/skia-source/ninja-limited.sh"
printf '#!/bin/sh\nexec ninja -j%s "$@"\n' "$PF2_SKIA_JOBS" > "$SKIA_NINJA_COMMAND"
chmod +x "$SKIA_NINJA_COMMAND"
export SKIA_NINJA_COMMAND
printf 'Skia source prepared: %s\n' "$SKIA_SOURCE_DIR"
printf 'Skia GN: official Horizon patch + libounce; ninja -j%s\n' "$PF2_SKIA_JOBS"
