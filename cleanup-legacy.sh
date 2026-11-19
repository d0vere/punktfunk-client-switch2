#!/usr/bin/env bash
# Explicit, narrow cleanup of obsolete packaging directories. Default: plan only.
set -euo pipefail
ROOT="$(cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DL="${PF2_DOWNLOADS:-$HOME/Downloads}"
[[ -f "$ROOT/.pf2-unified-ready" && -d "$ROOT/.git" ]] || { echo 'Not a verified unified repo' >&2; exit 2; }
mode="${1:---plan}"
case "$mode" in
  --plan|--delete) ;;
  *) echo 'Usage: ./cleanup-legacy.sh [--plan|--delete CONFIRM_DELETE_OLD_WORKSPACES]' >&2; exit 2 ;;
esac
if [[ "$mode" == '--delete' ]]; then
  [[ "${2:-}" == CONFIRM_DELETE_OLD_WORKSPACES ]] || { echo 'Explicit confirmation required' >&2; exit 2; }
  [[ ! -s "$ROOT/.git/objects/info/alternates" ]] || { echo "The Git repository still borrows objects from another directory" >&2; exit 2; }
  git -C "$ROOT" fsck --no-reflogs --full >/dev/null || { echo "Git integrity test failed" >&2; exit 2; }
  [[ -s "$ROOT/dist/punktfunk-switch2.nro" ]] || { echo 'First compile a NEW NRO from the unified Git checkout' >&2; exit 2; }
  [[ -f "$ROOT/.pf2-cache/skia/m153-0.101.2/BUILD.gn" && -d "$ROOT/.pf2-cache/target" && -d "$ROOT/.pf2-cache/cargo-home" ]] || { echo 'Cache migration incomplete' >&2; exit 2; }
fi
shopt -s nullglob
# Deliberately exclude ounce-devkit, PX experiments, Azkali and Moonlight references.
for d in "$DL"/Punktfunk_Official_Switch2_*_Work; do
  [[ -d "$d" && ! -L "$d" ]] || continue
  [[ "$d" != "$ROOT" ]] || continue
  if [[ "$mode" == '--plan' ]]; then du -sh -- "$d"; else rm -rf --one-file-system -- "$d"; echo "REMOVED $d"; fi
done
for f in "$DL"/Punktfunk_Official_Switch2_*_Work.zip; do
  [[ -f "$f" && ! -L "$f" ]] || continue
  if [[ "$mode" == '--plan' ]]; then du -sh -- "$f"; else rm -f -- "$f"; echo "REMOVED $f"; fi
done
if [[ "$mode" == '--plan' ]]; then
  echo 'PREVIEW ONLY. After a successful new build, use:'
  echo './cleanup-legacy.sh --delete CONFIRM_DELETE_OLD_WORKSPACES'
fi
