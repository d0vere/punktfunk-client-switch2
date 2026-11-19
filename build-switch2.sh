#!/usr/bin/env bash
# Incremental production build from the SINGLE Git repository.
set -euo pipefail
ROOT="$(cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK="${DEVKITPRO:-/opt/devkitpro}"
if [[ "${1:-}" == '--test' && $# -eq 1 ]]; then
  bash "$ROOT/switch2/build_full.sh" --host-tests
  exit 0
fi
[[ -f "$ROOT/.pf2-unified-ready" && -d "$ROOT/.git" ]] || { echo 'Not a unified Switch2 Git checkout' >&2; exit 2; }
[[ -d "$ROOT/.pf2-cache/target" && -d "$ROOT/.pf2-cache/cargo-home" && \
   -f "$ROOT/.pf2-cache/skia/m153-0.101.2/BUILD.gn" ]] || { echo 'Cache migration incomplete. Run setup.sh again.' >&2; exit 2; }
[[ -f "$SDK/switch2vars.sh" ]] || { echo 'SDK ounce-devkit not found' >&2; exit 2; }
# shellcheck disable=SC1090
source "$SDK/switch2vars.sh"
export PATH="$SDK/devkitA64/bin:$SDK/tools/bin:$PATH"
export DEVKITPRO="$SDK"
export PF2_WORK="$ROOT/.pf2-work"
export PF2_CARGO_HOME="$ROOT/.pf2-cache/cargo-home"
export PF2_CARGO_TARGET_DIR="$ROOT/.pf2-cache/target"
export PF2_SKIA_SOURCE_DIR="$ROOT/.pf2-cache/skia/m153-0.101.2"
export PF2_NATIVE_OUT="$ROOT/.pf2-work/native-build"
export PF2_SKIA_CC="$ROOT/switch2/tooling/skia-cc.sh"
export PF2_SKIA_CXX="$ROOT/switch2/tooling/skia-cxx.sh"
[[ -L "$PF2_WORK/official-client" ]] || { echo 'Missing stable Git workspace symlink' >&2; exit 2; }
[[ $# == 0 ]] || { echo 'Usage: ./build-switch2.sh [--test]' >&2; exit 2; }
# Do not silently ship from an untested tree; --test is also available offline.
bash "$ROOT/switch2/build_full.sh" --host-tests
mkdir -p "$ROOT/dist" "$ROOT/.pf2-work"
exec > >(tee "$ROOT/.pf2-work/incremental.log") 2>&1
printf 'Native incremental compile (only changed .c)\n'
make --no-print-directory -f "$ROOT/switch2/Makefile.native" -j"${PF2_JOBS:-2}"
printf 'Cargo incremental build (stable target + stable Skia source)\n'
bash "$ROOT/switch2/tooling/build_official_rust.sh"
cp -- "$PF2_WORK/dist/punktfunk-switch2.nro" "$ROOT/dist/punktfunk-switch2.nro"
cp -- "$PF2_WORK/dist/punktfunk-switch2.elf" "$ROOT/dist/punktfunk-switch2.elf"
( cd "$ROOT/dist" && sha256sum punktfunk-switch2.nro punktfunk-switch2.elf > SHA256SUMS.txt )
echo "NRO: $ROOT/dist/punktfunk-switch2.nro"
