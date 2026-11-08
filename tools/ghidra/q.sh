#!/bin/bash
# Ask the analysed X8 Ghidra project a question; see x8_query.py.
# Usage: tools/ghidra/q.sh decomp 0x2834e > out.c
# X8_PROJECT picks the project directory (default work/ghidra); give each
# parallel user its own copy, since Ghidra locks a project while it is open.
# GHIDRA_HOME is Ghidra's install folder (default ~/.local/opt/ghidra_10.4_PUBLIC).
P=$(cd "$(dirname "$0")/../.." && pwd)
PROJ=${X8_PROJECT:-$P/work/ghidra}
OUT=$(mktemp)
X8_OUT=$OUT "${GHIDRA_HOME:-$HOME/.local/opt/ghidra_10.4_PUBLIC}/support/analyzeHeadless" "$PROJ" X8 -process v203-app.bin -noanalysis -readOnly \
  -scriptPath "$P/tools/ghidra" -postScript x8_query.py "$@" > "$OUT.log" 2>&1 || { tail -20 "$OUT.log"; exit 1; }
cat "$OUT"; rm -f "$OUT" "$OUT.log"
