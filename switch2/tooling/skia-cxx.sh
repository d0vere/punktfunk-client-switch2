#!/usr/bin/env bash
# GN cross-compiler shim; rust-skia adds a --target argument GCC does not accept.
set -euo pipefail
: "${DEVKITPRO:?}"
: "${PF2_OFFICIAL_REPO:?}"
args=()
for a in "$@"; do
  case "$a" in --target=*) ;; *) args+=("$a");; esac
done
# ARCH is set by ounce-devkit's switch2vars.sh, not Switch1's compiler profile.
# Intentional word splitting of compiler flags from the trusted SDK.
# shellcheck disable=SC2086
exec aarch64-none-elf-g++ ${ARCH:-} -mtp=soft -fPIE -ftls-model=local-exec \
  -D__SWITCH__ -D__SWITCH2__ -D_GNU_SOURCE \
  -isystem "$PF2_OFFICIAL_REPO/toolchain/skia/include" \
  -isystem "$DEVKITPRO/libounce/include" \
  -isystem "$DEVKITPRO/portlibs/switch2/include" "${args[@]}"
