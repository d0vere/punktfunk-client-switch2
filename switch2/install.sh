#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${PF2_WORK:-$HERE/work}"
python3 "$HERE/tooling/stage.py" --work "$WORK" --source "${PF2_UPSTREAM:-https://github.com/punktfunk/client-switch.git}"
python3 "$HERE/tooling/apply_integration.py" --work "$WORK"
echo "Sorgenti ufficiali preparati, integrazione non ancora compilata. Esegui ./build_full.sh"
