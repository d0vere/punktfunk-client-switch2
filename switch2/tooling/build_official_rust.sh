#!/usr/bin/env bash
# Cross-build the REAL pf-nx binary with the new Switch2 platform dependency.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${PF2_WORK:-$HERE/work}"
REPO="$WORK/official-client"
SDK="${DEVKITPRO:-/opt/devkitpro}"
[[ -d "$REPO/.git" ]] || { echo 'Official checkout absent' >&2; exit 13; }
[[ -f "$WORK/.pf2_integration.json" ]] || { echo 'Rust integration not installed' >&2; exit 13; }
command -v cargo >/dev/null || { echo 'Cargo non disponibile' >&2; exit 13; }
command -v rustup >/dev/null || { echo 'rustup non disponibile' >&2; exit 13; }
[[ -f "$SDK/switch2vars.sh" ]] || { echo 'ounce-devkit non presente' >&2; exit 13; }
# shellcheck disable=SC1090
source "$SDK/switch2vars.sh"
export PATH="$SDK/devkitA64/bin:$SDK/tools/bin:$PATH"
export DEVKITPRO="$SDK"
export PF2_SWITCH2=1
export PF2_NATIVE_LIB_DIR="${PF2_NATIVE_OUT:-$WORK/native-build}"
[[ -s "$PF2_NATIVE_LIB_DIR/libpf2native.a" ]] || { echo 'Fresh libpf2native.a missing' >&2; exit 13; }
# Isolate all new Cargo downloads/cache from the host's shared cache.
export CARGO_HOME="${PF2_CARGO_HOME:-$WORK/cargo-home}"
mkdir -p "$CARGO_HOME"
# Optional reuse of a previous COMPLETE cargo target directory (including Skia Ninja objects).
export CARGO_TARGET_DIR="${PF2_CARGO_TARGET_DIR:-$REPO/target}"
mkdir -p "$CARGO_TARGET_DIR"
# Override the original Switch1 cc-rs CFLAGS and bindgen include hints for Switch2.
export CFLAGS_aarch64_nintendo_horizon="${ARCH:-} -D__SWITCH__ -D__SWITCH2__ -I$SDK/libounce/include -I$SDK/portlibs/switch2/include"
export CXXFLAGS_aarch64_nintendo_horizon="$CFLAGS_aarch64_nintendo_horizon"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_nintendo_horizon="--target=aarch64-none-elf -D__SWITCH__ -D__SWITCH2__ -isystem $SDK/libounce/include -isystem $SDK/devkitA64/aarch64-none-elf/include -I$SDK/portlibs/switch2/include"
export AWS_LC_SYS_NO_JITTER_ENTROPY_aarch64_nintendo_horizon=1
export AWS_LC_SYS_CFLAGS_aarch64_nintendo_horizon="${ARCH:-} -D__SWITCH__ -D__SWITCH2__ -I$SDK/libounce/include -I$REPO/toolchain/include -D_DEFAULT_SOURCE -DOPENSSL_NO_SOCK -DOPENSSL_STATIC_ARMCAP -DOPENSSL_WASM"
cd "$REPO"
if ! rustup toolchain list | grep -q 'nightly-2026-10-04'; then
    echo 'Required Rust nightly-2026-10-04 not installed; no toolchain auto-change' >&2; exit 13
fi
# -Z build-std uses a SEPARATE Cargo.lock under the pinned rust-src sysroot.
# A patched application libc does not fix std::net if that lock still points at
# crates.io. Never silently update a toolchain or its dependency versions here.
STD_LIBRARY="$(rustc +nightly-2026-10-04 --print sysroot)/lib/rustlib/src/rust/library"
STD_LOCK="$STD_LIBRARY/Cargo.lock"
if [[ -f "$STD_LOCK" ]] && awk '
    /^\[\[package\]\]$/ { in_libc=0 }
    /^name = "libc"$/ { in_libc=1 }
    in_libc && /^source = "registry\+/ { bad=1 }
    END { exit !bad }
' "$STD_LOCK"; then
    echo 'ERROR: rust-src Cargo.lock still selects registry libc for build-std.' >&2
    echo 'The local Horizon ioctl/FIONBIO ABI fix is NOT reaching Rust std::net.' >&2
    echo "Inspect: $STD_LOCK (libc package)." >&2
    echo 'See the network build-std instructions accompanying this patch.' >&2
    exit 14
fi
# The original std cfg patches are preserved. Unlike the original cargo-nx.sh,
# this does not run cargo update against the global rustup sysroot lockfile.
# The public rust-skia binary archive does not exist for Nintendo Horizon;
# Skia must be compiled from the pinned sources after applying the official
# Horizon patch (including the zlib portable-C fallback). The sourced script
# exports SKIA_SOURCE_DIR, compiler wrappers, GN args, and FORCE_SKIA_BUILD.
# This stage never changes the upstream tracked sources.
# shellcheck source=tooling/prepare_skia.sh
source "$HERE/tooling/prepare_skia.sh"
# The previous build successfully compiled all 1370 Skia Ninja steps, but
# libclang then could not find the AArch64 C++ standard header <cassert>.
# Discover devkitA64 GCC C++ paths and preflight them with Clang before Cargo.
source "$HERE/tooling/prepare_bindgen.sh"
# A Skia failure still fails the real build; an incomplete ELF is never packaged.
env "BINDGEN_EXTRA_CLANG_ARGS_aarch64-nintendo-horizon=$PF2_BINDGEN_ARGS" \
  cargo +nightly-2026-10-04 build --locked --release \
   -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
   --target "$REPO/toolchain/aarch64-nintendo-horizon.json" -p pf-nx --bin pf-nx
ELF="$CARGO_TARGET_DIR/aarch64-nintendo-horizon/release/pf-nx"
[[ -s "$ELF" ]] || { echo 'Cargo succeeded without ELF: refusing package' >&2; exit 16; }
python3 "$HERE/tooling/verify_artifacts.py" --elf "$ELF"
mkdir -p "$WORK/dist"
cp "$ELF" "$WORK/dist/punktfunk-switch2.elf"
# Only a successful FULL Rust link reaches elf2nro.
command -v elf2nro >/dev/null || { echo 'elf2nro missing' >&2; exit 16; }
command -v nacptool >/dev/null || { echo 'nacptool missing' >&2; exit 16; }
nacptool --create 'Punktfunk Switch 2' 'Punktfunk' '1.1.0' "$WORK/dist/punktfunk-switch2.nacp"
if [[ -f "$REPO/packaging/icon.jpg" ]]; then
    elf2nro "$ELF" "$WORK/dist/punktfunk-switch2.nro" \
        --nacp="$WORK/dist/punktfunk-switch2.nacp" --icon="$REPO/packaging/icon.jpg"
else
    elf2nro "$ELF" "$WORK/dist/punktfunk-switch2.nro" --nacp="$WORK/dist/punktfunk-switch2.nacp"
fi
python3 "$HERE/tooling/verify_artifacts.py" --elf "$ELF" --nro "$WORK/dist/punktfunk-switch2.nro"
sha256sum "$WORK/dist/punktfunk-switch2.elf" "$WORK/dist/punktfunk-switch2.nro" > "$WORK/dist/SHA256SUMS.txt"
echo "NRO: $WORK/dist/punktfunk-switch2.nro"
