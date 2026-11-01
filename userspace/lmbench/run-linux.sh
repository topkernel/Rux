#!/bin/bash
# run-linux.sh — the reference column: the SAME static lmbench binaries
# executed natively on the host (the reference kernel), one case at a
# time, on the same CPU as the Rux guest boots.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/bin"
OUT="${1:-$HERE/../../run/lmbench-linux.txt}"
case "$OUT" in /*) ;; *) OUT="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")";; esac
: > "$OUT"
cd /tmp
while read -r spec; do
    [ -n "$spec" ] || continue
    name="${spec%% *}"
    args="${spec#* }"
    [ "$args" = "$spec" ] && args=""
    echo "===CASE $spec" >> "$OUT"
    if [ -z "$args" ]; then
        timeout -s KILL 25 "$BIN/$name" -N 3 -W 1 2>&1 | tr -d '\r' \
            | grep -aE "[0-9]+\.[0-9]+" >> "$OUT"
    else
        timeout -s KILL 25 "$BIN/$name" -N 3 -W 1 $args 2>&1 | tr -d '\r' \
            | grep -aE "[0-9]+\.[0-9]+" >> "$OUT"
    fi
done < "$HERE/bench-list.txt"
echo "run-linux: results -> $OUT"
