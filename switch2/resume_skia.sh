#!/usr/bin/env bash
# Resume a successful Skia Ninja build from the preceding Skia_Fix_Work package.
# The previous checkout/target cache is READ and reused, not overwritten with a
# foreign binary. Cargo decides what must be rebuilt; the real ELF gate remains.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREVIOUS="${PF2_PREVIOUS_SKIA:-$ROOT/../Punktfunk_Official_Switch2_Skia_Fix_Work}"
INTEGRATION="${PF2_PREVIOUS_INTEGRATION:-$ROOT/../Punktfunk_Official_Switch2_Integration_Work}"
if [[ ! -f "$PREVIOUS/work/skia-source/m153-0.101.2/BUILD.gn" || \
      ! -d "$PREVIOUS/work/official-client/target" || \
      ! -x "$PREVIOUS/tooling/skia-cc.sh" || \
      ! -x "$PREVIOUS/tooling/skia-cxx.sh" ]]; then
    echo "No verified previous Skia build found under: $PREVIOUS" >&2
    echo 'Use ./build_full.sh for a clean attempt; no existing files were changed.' >&2
    exit 27
fi
export PF2_SKIA_SOURCE_DIR="${PF2_SKIA_SOURCE_DIR:-$PREVIOUS/work/skia-source/m153-0.101.2}"
export PF2_CARGO_TARGET_DIR="${PF2_CARGO_TARGET_DIR:-$PREVIOUS/work/official-client/target}"
export PF2_SKIA_CC="${PF2_SKIA_CC:-$PREVIOUS/tooling/skia-cc.sh}"
export PF2_SKIA_CXX="${PF2_SKIA_CXX:-$PREVIOUS/tooling/skia-cxx.sh}"
if [[ -d "$INTEGRATION/work/cargo-home" ]]; then
    export PF2_CARGO_HOME="${PF2_CARGO_HOME:-$INTEGRATION/work/cargo-home}"
fi
printf 'Reusing Skia source: %s\n' "$PF2_SKIA_SOURCE_DIR"
printf 'Reusing Cargo/Ninja target: %s\n' "$PF2_CARGO_TARGET_DIR"
if [[ -n ${PF2_CARGO_HOME:-} ]]; then printf 'Reusing Cargo registry: %s\n' "$PF2_CARGO_HOME"; fi
exec "$ROOT/build_full.sh" "$@"
