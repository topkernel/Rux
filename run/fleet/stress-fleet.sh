#!/bin/bash
# stress-fleet.sh — stress-fleet master: launch (or adopt) every fleet
# member under setsid so tool timeouts never kill them, and start the
# periodic central sweep daemon. Standing lane s1 driver root: a future
# session resumes by finding this script and the ledger.
#
# Usage:
#   stress-fleet.sh start [members...]   # default roster, or explicit list
#   stress-fleet.sh stop                 # stop files everywhere (chains NOT touched)
#   stress-fleet.sh status               # one sweep
set -u
FLEET="$(cd "$(dirname "$0")" && pwd)"
LOGS="$FLEET/logs"
mkdir -p "$LOGS"

ROSTER_DEFAULT="ltp-a ltp-b ltp-c ltp-d monkey-a monkey-b memstorm fsfuzz"

kind_of() { case "$1" in ltpm) echo ltpm;; ltp-*) echo ltp;; monkey-*) echo monkey;; memstorm*) echo memstorm;; fsfuzz*) echo fsfuzz;; *) echo "";; esac; }

start_member() {
    local m="$1" kind
    kind=$(kind_of "$m")
    [ -z "$kind" ] && { echo "unknown member $m"; return 1; }
    rm -f "$FLEET/$m.stop"
    # grep -c exits 1 when it prints 0 — don't chain || echo (yields
    # "0\n0" and breaks the arithmetic); classify lines carry R<N>
    # mid-line ("[member] round N: R1 ..."), so no ^ anchor.
    local n
    n=$(grep -cE 'round [0-9]+: R[0-9]+' "$LOGS/$m-rounds.log" 2>/dev/null)
    [ -z "$n" ] && n=0
    local first=$(( n + 1 ))
    if pgrep -f "fleet-member.sh $m " >/dev/null 2>&1; then
        echo "$m already running"
        return 0
    fi
    setsid "$FLEET/fleet-member.sh" "$m" "$kind" 100000 "$first" \
        >> "$LOGS/$m-driver.log" 2>&1 &
    echo "$m launched (kind=$kind, first round $first) pid=$!"
}

start_sweeper() {
    if pgrep -f "fleet-sweeprd.sh" >/dev/null 2>&1; then
        echo "sweeper already running"
        return 0
    fi
    setsid "$FLEET/fleet-sweeprd.sh" >/dev/null 2>&1 &
    echo "sweeper launched pid=$!"
}

case "${1:-}" in
start)
    shift
    members="${*:-$ROSTER_DEFAULT}"
    for m in $members; do start_member "$m"; done
    start_sweeper
    ;;
stop)
    for m in $ROSTER_DEFAULT; do touch "$FLEET/$m.stop"; done
    echo "stop files set (drivers end after current round; QEMU rounds run to timeout)"
    ;;
status)
    "$FLEET/fleet-sweep.sh"
    ;;
*)
    echo "usage: $0 start [members...] | stop | status"
    ;;
esac
