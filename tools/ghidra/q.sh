#!/bin/bash
# Ask the analysed X8 Ghidra project a question; see x8_query.py.
# Usage: tools/ghidra/q.sh decomp 0x2834e > out.c
P=$(cd "$(dirname "$0")/../.." && pwd)
OUT=$(mktemp)
X8_OUT=$OUT ~/.local/opt/ghidra_10.4_PUBLIC/support/analyzeHeadless "$P/work/ghidra" X8 -process v203-app.bin -noanalysis -readOnly \
  -scriptPath "$P/tools/ghidra" -postScript x8_query.py "$@" > "$OUT.log" 2>&1 || { tail -20 "$OUT.log"; exit 1; }
cat "$OUT"; rm -f "$OUT" "$OUT.log"
