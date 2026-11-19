#!/usr/bin/env bash
# Cross-compile only the actual independent C backend objects with ounce-devkit.
set -euo pipefail
PROJ="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export DEVKITPRO="${DEVKITPRO:-/opt/devkitpro}"
SDK="$DEVKITPRO/libounce/include/switch2/video/decoder.h"
[[ -f "$SDK" ]] || { echo "SDK assente: $SDK" >&2; exit 12; }
[[ -f "$DEVKITPRO/switch2vars.sh" ]] || { echo 'switch2vars.sh mancante' >&2; exit 12; }
for sym in videoDecoderCreate videoDecoderGetFrame videoDecoderCopyFrame videoDecoderReleaseFrame ViddecCodec_Av1 ViddecCodec_H264; do
  grep -q "\b$sym\b" "$SDK" "$DEVKITPRO/libounce/include/switch2/services/viddec.h" || { echo "API SDK non presente: $sym" >&2; exit 12; }
done
[[ -f "$DEVKITPRO/libounce/include/switch.h" ]] || { echo 'switch.h mancante' >&2; exit 12; }
[[ -f "$DEVKITPRO/libounce/include/vulkan/vulkan.h" || -f "$DEVKITPRO/portlibs/switch2/include/vulkan/vulkan.h" || -f "$DEVKITPRO/portlibs/switch/include/vulkan/vulkan.h" ]] || echo 'Nota: Vulkan header potrebbe essere in un altro prefisso del devkit' >&2
# shellcheck disable=SC1091
source "$DEVKITPRO/switch2vars.sh"
: "${CC:=aarch64-none-elf-gcc}"
: "${AR:=aarch64-none-elf-ar}"
command -v "$CC" >/dev/null || { echo "Compiler non disponibile: $CC" >&2; exit 12; }
command -v "$AR" >/dev/null || { echo "Archiver non disponibile: $AR" >&2; exit 12; }
OUT="${PF2_NATIVE_OUT:-$PROJ/work/native-build}"
# Official upstream supplies a dedicated sys/mman.h for newlib/Horizon.
SKIA_COMPAT_INCLUDE="${PF2_WORK:-$PROJ/work}/official-client/toolchain/skia/include"
[[ -f "$SKIA_COMPAT_INCLUDE/sys/mman.h" ]] || { echo "Missing pinned Skia mman compatibility header: $SKIA_COMPAT_INCLUDE/sys/mman.h" >&2; exit 12; }
mkdir -p "$OUT"
# Reject stale objects: each archive must derive solely from this build.
rm -f "$OUT"/*.o "$OUT/libpf2native.a"
for f in pf2_nv12 pf2_h264_units pf2_decoder_switch2 pf2_audio_switch2 pf2_input_switch2 pf2_vulkan_switch2 pf2_runtime_switch2 pf2_skia_compat; do
  echo "Compilazione: $f.c"
  # Avoid including Skia's newlib overrides in unrelated libounce modules.
  compat_includes=()
  if [[ "$f" == pf2_skia_compat ]]; then compat_includes=(-I"$SKIA_COMPAT_INCLUDE"); fi
  "$CC" -c -std=c11 -O2 -g -fPIC -Wall -Wextra -D__SWITCH2__ -D__SWITCH__ \
    ${CFLAGS:-} ${CPPFLAGS:-} -I"$PROJ/native" "${compat_includes[@]}" \
    "$PROJ/native/$f.c" -o "$OUT/$f.o"
done
"$AR" rcs "$OUT/libpf2native.a" "$OUT"/*.o
printf '\nLibreria nativa generata: %s\n' "$OUT/libpf2native.a"
# A static archive by itself is NOT the client and must never be converted to NRO.
