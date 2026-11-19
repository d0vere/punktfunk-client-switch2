#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${PF2_WORK:-$HERE/work}"
if [[ -f "$HERE/../.pf2-unified-ready" && "${1:-}" != '--host-tests' ]]; then
  echo 'Legacy full build disabled in unified checkout: use ./build-switch2.sh' >&2
  exit 2
fi
mkdir -p "$WORK"
LOG="$WORK/build_full.log"
exec > >(tee "$LOG") 2>&1
echo "Punktfunk Switch 2 — integrazione Rust/Skia + native source, verifica in corso"
echo "Log: $LOG"
python3 -m unittest discover -s "$HERE/tests" -p 'test_*.py' -v
HOST_CC="${HOST_CC:-clang}"
command -v "$HOST_CC" >/dev/null || { echo 'Clang host mancante' >&2; exit 11; }
for unit in nv12 h264; do
  f="$unit"; [[ "$unit" == h264 ]] && f="h264_units"
  "$HOST_CC" -std=c11 -O1 -Wall -Wextra -Werror -fsanitize=address,undefined \
    -fno-omit-frame-pointer "$HERE/native/pf2_${f}.c" \
    "$HERE/tests/test_${unit}.c" -o "$WORK/test_${unit}"
  "$WORK/test_${unit}"
done
# Mock the SDK ABI to check the direct-NV12 buffer-lending and legacy
# release path; real SDK/hardware testing is still required.
"$HOST_CC" -std=c11 -O1 -Wall -Wextra -Werror -D__SWITCH2__ \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$HERE/tests/mock_sdk" -I"$HERE/native" \
  "$HERE/native/pf2_decoder_switch2.c" "$HERE/tests/test_direct_nv12_native.c" \
  -o "$WORK/test_direct_nv12_native"
"$WORK/test_direct_nv12_native"
"$HOST_CC" -std=c11 -O1 -Wall -Wextra -Werror -I"$HERE/native" \
  "$HERE/tests/test_ffi_layout.c" -o "$WORK/test_ffi_layout"
"$WORK/test_ffi_layout"
# Host test of the exact target compatibility object, extracted by a strong
# symbol reference from its static archive. The weak libc names themselves
# can be intercepted on Linux; functional tests call the internal functions.
"$HOST_CC" -std=c11 -O1 -g -Wall -Wextra -Werror -D__SWITCH2__ \
  -D_POSIX_C_SOURCE=200809L -fsanitize=address,undefined -fno-omit-frame-pointer \
  -pthread -I"$HERE/native" -c "$HERE/native/pf2_skia_compat.c" -o "$WORK/pf2_skia_compat_host.o"
command -v ar >/dev/null || { echo 'ar host mancante' >&2; exit 11; }
ar rcs "$WORK/libpf2compat_host.a" "$WORK/pf2_skia_compat_host.o"
"$HOST_CC" -std=c11 -O1 -g -Wall -Wextra -Werror -D_POSIX_C_SOURCE=200809L \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -pthread "$HERE/tests/test_skia_compat.c" "$WORK/libpf2compat_host.a" -o "$WORK/test_skia_compat"
"$WORK/test_skia_compat"
if [[ "${1:-}" == "--host-tests" ]]; then
  echo 'Test host: superati, NON cross build o test console'; exit 0
fi
python3 "$HERE/tooling/stage.py" --work "$WORK"
python3 "$HERE/tooling/apply_integration.py" --work "$WORK"
bash "$HERE/tooling/build_native.sh"
bash "$HERE/tooling/build_official_rust.sh"
echo 'Build integrata completata (resta obbligatoria la prova sulla console)'
