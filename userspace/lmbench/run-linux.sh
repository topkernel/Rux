#!/bin/bash
# run-linux.sh — the reference column: the SAME static lmbench binaries
# executed natively on the host (the reference kernel), one case at a
# time, on the same CPU as the Rux guest boots. Mirrors the guest
# runner: creates the scratch file, starts the "-s" server locally for
# the client/server benchmarks.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/bin"
OUT="${1:-$HERE/../../run/lmbench-linux.txt}"
case "$OUT" in /*) ;; *) OUT="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")";; esac
: > "$OUT"
truncate -s 16M /tmp/lmbench.file
cd /tmp
while read -r spec; do
    [ -n "$spec" ] || continue
    name="${spec%% *}"
    args="${spec#* }"
    [ "$args" = "$spec" ] && args=""
    echo "===CASE $spec" >> "$OUT"
    server=""
    case "$name" in
        lat_tcp|lat_udp|lat_connect|bw_tcp|bw_udp)
            "$BIN/$name" -s >/dev/null 2>&1 &
            server=$!
            sleep 1
            ;;
    esac
    # lat_connect's getopt has no -W option
    opts="-N 3 -W 1"
    [ "$name" = lat_connect ] && opts="-N 3"
    timeout -s KILL 40 "$BIN/$name" $opts $args > "$HERE/.case.log" 2>&1
    cat "$HERE/.case.log" | tr -d '\r' \
        | grep -aE "[0-9]+\.[0-9]+|^[0-9]+k" >> "$OUT"
    # sweep any benchmp workers that outlived a killed case (scoped to
    # this worktree's binary path only)
    pkill -9 -f "$BIN/$name" 2>/dev/null
    if [ -n "$server" ]; then
        kill -9 "$server" 2>/dev/null
        wait "$server" 2>/dev/null
    fi
done < "$HERE/bench-list.txt"
echo "run-linux: results -> $OUT"
