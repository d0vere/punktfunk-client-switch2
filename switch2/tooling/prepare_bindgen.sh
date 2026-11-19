#!/usr/bin/env bash
# Bindgen needs the AArch64 devkitA64 C++ standard headers separately from the
# headers Ninja sees via aarch64-none-elf-g++.  Skia's generated C++ bindings
# compile with GCC, but libclang does NOT automatically know GCC's cross paths.
# Sourced after switch2vars.sh and prepare_skia.sh. Never edits SDK files.
set -euo pipefail
: "${DEVKITPRO:?DEVKITPRO is required}"

pf2_cxx_prefix="$DEVKITPRO/devkitA64/aarch64-none-elf/include/c++"
if [[ ! -d "$pf2_cxx_prefix" ]]; then
    echo "Bindgen: cannot find target C++ headers under $pf2_cxx_prefix" >&2
    return 26 2>/dev/null || exit 26
fi
# The installed GCC version is discovered, never assumed from the upstream pin.
pf2_cxx_ver=$(find "$pf2_cxx_prefix" -mindepth 1 -maxdepth 1 -type d -name '[0-9]*' -printf '%f\n' | sort -V | tail -n 1)
pf2_cxx_root="$pf2_cxx_prefix/$pf2_cxx_ver"
if [[ -z "$pf2_cxx_ver" || ! -f "$pf2_cxx_root/cassert" ]]; then
    echo "Bindgen: no usable <cassert> in devkitA64 target include/c++" >&2
    return 26 2>/dev/null || exit 26
fi
pf2_cxx_config=""
for pf2_candidate in "$pf2_cxx_root/aarch64-none-elf" "$pf2_cxx_root/aarch64-unknown-elf"; do
    if [[ -f "$pf2_candidate/bits/c++config.h" ]]; then
        pf2_cxx_config="$pf2_candidate"
        break
    fi
done
if [[ -z "$pf2_cxx_config" ]]; then
    # GCC layout can use another multiarch directory, but it must actually
    # contain the target's c++config.h; never default to host x86_64 headers.
    while IFS= read -r pf2_candidate; do
        case "$pf2_candidate" in *x86_64*|*i686*) continue;; esac
        pf2_cxx_config="${pf2_candidate%/bits/c++config.h}"
        break
    done < <(find "$pf2_cxx_root" -mindepth 2 -maxdepth 3 -type f -path '*/bits/c++config.h' | sort)
fi
if [[ -z "$pf2_cxx_config" ]]; then
    echo "Bindgen: no AArch64 bits/c++config.h under $pf2_cxx_root" >&2
    return 26 2>/dev/null || exit 26
fi
pf2_newlib="$DEVKITPRO/devkitA64/aarch64-none-elf/include"
if [[ ! -f "$pf2_newlib/stdint.h" ]]; then
    echo "Bindgen: missing target newlib stdint.h in $pf2_newlib" >&2
    return 26 2>/dev/null || exit 26
fi

# Clang with the target's newlib/GCC headers is less likely to silently use the
# build machine's x86-64 C++ ABI. Override the Horizon triple which libclang
# cannot natively resolve. This does NOT change the actual Rust target JSON.
pf2_bindgen_target="${PF2_BINDGEN_TARGET:-aarch64-none-elf}"
case "$pf2_bindgen_target" in
    aarch64-none-elf|aarch64-unknown-linux-gnu) ;;
    *) echo "Bindgen: unsupported PF2_BINDGEN_TARGET=$pf2_bindgen_target" >&2; return 26 2>/dev/null || exit 26 ;;
esac
pf2_bindgen_parts=(
    "--target=$pf2_bindgen_target" "-std=c++20" '-nostdinc++'
    '-D__SWITCH__' '-D__SWITCH2__' '-D_GNU_SOURCE'
    "-isystem$pf2_cxx_root"
    "-isystem$pf2_cxx_config"
    "-isystem$pf2_newlib"
    "-isystem$DEVKITPRO/libounce/include"
)
if [[ -d "$pf2_cxx_root/backward" ]]; then
    pf2_bindgen_parts+=("-isystem$pf2_cxx_root/backward")
fi
if [[ -d "$DEVKITPRO/portlibs/switch2/include" ]]; then
    pf2_bindgen_parts+=("-isystem$DEVKITPRO/portlibs/switch2/include")
fi

# A cheap independent gate: catch missing C++ stdlib headers BEFORE Cargo's
# thousands of dependencies and Skia's 1370 Ninja steps. Clang is required by
# bindgen anyway; one invocation uses the same target and include candidates.
pf2_clang="${PF2_BINDGEN_CLANG:-clang++}"
command -v "$pf2_clang" >/dev/null || { echo "Bindgen: $pf2_clang not found" >&2; return 26 2>/dev/null || exit 26; }
pf2_bindgen_error=$(mktemp)
if ! printf '%s\n' '#include <cassert>' '#include <memory>' '#include <vector>' 'static_assert(sizeof(void*) == 8);' | \
     "$pf2_clang" -x c++ -fsyntax-only "${pf2_bindgen_parts[@]}" - 2> "$pf2_bindgen_error"; then
    echo "Bindgen: target C++ preflight FAILED; first diagnostics:" >&2
    sed -n '1,65p' "$pf2_bindgen_error" >&2
    echo 'Bindgen: no NRO was produced. Check SDK libstdc++, Clang and target compatibility.' >&2
    rm -f "$pf2_bindgen_error"
    return 26 2>/dev/null || exit 26
fi
rm -f "$pf2_bindgen_error"

# A hyphenated Cargo target cannot be an exported Bash identifier; caller will
# inject this string with env "BINDGEN_EXTRA_CLANG_ARGS_aarch64-nintendo-horizon=...".
PF2_BINDGEN_ARGS="${pf2_bindgen_parts[*]}"
export PF2_BINDGEN_ARGS
printf 'Bindgen preflight: PASS %s with GCC C++ %s\n' "$pf2_bindgen_target" "$pf2_cxx_ver"
printf 'Bindgen target stdlib: %s\n' "$pf2_cxx_root"
printf 'Bindgen target c++config: %s\n' "$pf2_cxx_config"
