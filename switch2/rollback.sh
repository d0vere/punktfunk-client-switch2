#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${PF2_WORK:-$HERE/work}"
python3 "$HERE/tooling/rollback_integrated.py" --work "$WORK"
python3 "$HERE/tooling/rollback.py" --work "$WORK"
