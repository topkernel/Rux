#!/bin/bash
# stress-fleet3.sh — the standing 3-QEMU stress fleet (user directive
# 2026-11-10: "say 'run stress tests' and 3 QEMUs spin up and keep
# going"). One class per boot, x86_64+KVM, round-robin across 3 slots,
# continuous until `stop`. Self-rebuilding: if the worktree was cleaned,
# `start` recreates it from origin/qa-suites first (the rebuild chain).
#
# Usage: stress-fleet3.sh start|status|stop|log
#   start — launch 3 detached workers (idempotent: running → no-op)
#   status — worker/QEMU state + last verdict lines
#   stop — stop files + reap ONLY this fleet's QEMUs (cwd match)
#   log — tail the rounds ledger (run/fleet3/rounds.log)
#
# Host budget discipline: mem floor AVAIL_MIN_GB (default 12) checked
# before every boot — below it the worker sleeps and retries (sheds).
# Fleet QEMUs: -snapshot + file.locking=off, niced, 2G/smp2 each.
set -uo pipefail

FLEET_DIR="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$FLEET_DIR/../.." && pwd)"
BRANCH=qa-suites
QUEUE_FILE="$FLEET_DIR/queue.txt"
ROUNDS="$FLEET_DIR/rounds.log"
STOP="$FLEET_DIR/stop"
AVAIL_MIN_GB="${AVAIL_MIN_GB:-12}"
OUTBASE="$WT/run/qa-fleet"
mkdir -p "$FLEET_DIR" "$OUTBASE"

rebuild_worktree() {
    echo "$(date +%H:%M:%S) rebuild: worktree missing at $WT — rebuilding from origin/$BRANCH" >&2
    cd /home/william/workspace/Rux || return 1
    git worktree prune
    git worktree add "$WT" "$BRANCH" || return 1
    cd "$WT" || return 1
    # musl toolchain (tarball top dir is toolchain/, flatten it)
    [ -x toolchain/x86_64-rux-linux-musl/bin/musl-gcc ] || {
        (cd toolchain && tar xzf x86_64-rux-linux-musl.tar.gz &&
         [ -d toolchain ] && mv toolchain/x86_64-rux-linux-musl . && rmdir toolchain)
    }
    [ -x toolchain/x86_64-rux-linux-musl/bin/musl-gcc ] || return 1
    bash userspace/tests/extract.sh >/dev/null 2>&1
    # guest image from the permanent store (plain cp; reflink unsupported cross-fs)
    [ -f run/rootfs-qa.img ] || cp /home/william/rux-images/testimages/ltp/rootfs-x86_64-ltp.img run/rootfs-qa.img
    # suites + kernel
    userspace/tests/fio/build-x86.sh >/dev/null 2>&1
    userspace/tests/futex-selftests/build-x86.sh >/dev/null 2>&1
    PLATFORM=x86_64 make build >/dev/null 2>&1 || return 1
    [ -f run/fio-bin/fio ] && [ -f run/futex-selftests-bin/futex_wait ] \
        && [ -f target/x86_64-unknown-none/debug/bzImage ]
}

injbins_for() {  # per-class binary injection dir
    case "$1" in
        r5-*|r5b-*) echo "$WT/run/fio-bin" ;;
        r6-*)       echo "$WT/run/futex-selftests-bin" ;;
        r7-*)       echo "$WT/run/fsfuzz-bin" ;;
        *)          echo "" ;;
    esac
}

build_queue() {  # every one-boot class we own, in rotation order
    {   ls "$WT"/run/r5-*.cmds "$WT"/run/r5b-*.cmds "$WT"/run/r6-*.cmds "$WT"/run/r7-*.cmds 2>/dev/null | sort
        printf '%s\n' "L|hackbench 20 process|200" "L|hackbench -p 20|200" \
            "L|60 cyclictest -D 30 -i 1000 -m|200" \
            "L|60 cyclictest -D 30 -i 1000 -m -p 98|200" \
            "L|45 pi_stress -D 15 -g 2|120" \
            "L|60 memfd|200" "L|60 threadtest -t 20|300" "L|150 posix_timers|200"
    } | sed "s|$WT/run/||"
}

classify() {  # one ledger line per finished boot
    local log="$1" cls="$2" slot="$3" rc_lines anom verdict
    rc_lines=$(grep -ac '^RES|' "$log" 2>/dev/null); rc_lines=${rc_lines:-0}
    local fails=0; fails=$(grep -a '^RES|' "$log" 2>/dev/null | grep -acv 'RES|.*|0|' 2>/dev/null); fails=${fails:-0}
    anom=$(grep -acE 'KERNPANIC|ALLOCTHROW|DEADLOCK' "$log" 2>/dev/null); anom=${anom:-0}
    local wedge=0; wedge=$(grep -acE 'unreapable|TIMEOUT-KILL' "$log" 2>/dev/null); wedge=${wedge:-0}
    local warn=0; warn=$(grep -acE 'KOT-KILL|close completion stalled' "$log" 2>/dev/null); warn=${warn:-0}
    verdict=CLEAN
    [ "$anom" -gt 0 ] && verdict=CRITICAL
    [ "$wedge" -gt 0 ] && verdict=WEDGE
    [ "$verdict" = CLEAN ] && { [ "$warn" -gt 0 ] && verdict=WARN; }
    [ "$verdict" = CLEAN ] && [ "$fails" -gt 0 ] && verdict=FAILRC
    echo "$(date +%m%d-%H:%M:%S)|slot$slot|$cls|res=$rc_lines failrc=$fails anom=$anom wedge=$wedge warn=$warn|$verdict" >> "$ROUNDS"
}

worker() {
    local slot="$1"
    local myq=() all=() i=0
    mapfile -t all < "$QUEUE_FILE"
    for e in "${all[@]}"; do [ $((i % 3)) -eq "$slot" ] && myq+=("$e"); i=$((i+1)); done
    [ ${#myq[@]} -eq 0 ] && return 0
    cd "$WT" || return 1
    # Per-slot private image: 3 workers injecting cmds.txt into ONE
    # shared image would clobber each other (debugfs -w races). Boots
    # are -snapshot, so per-slot base copies are the cheap isolation.
    local IMG="$WT/run/rootfs-fleet-s$slot.img"
    [ -f "$IMG" ] || cp "$WT/run/rootfs-qa.img" "$IMG"
    export IMAGE="$IMG"
    local n=0
    while [ ! -f "$STOP" ]; do
        local e="${myq[$((n % ${#myq[@]}))]}"; n=$((n+1))
        # host mem floor — shed, don't pile on
        while [ ! -f "$STOP" ]; do
            local avail; avail=$(LC_ALL=C free -g | awk 'NR==2{print $7}')
            [ "${avail:-0}" -ge "$AVAIL_MIN_GB" ] && break
            sleep 60
        done
        [ -f "$STOP" ] && break
        local budget cls inj
        if [[ "$e" == L\|* ]]; then
            IFS='|' read -r _ cls budget <<< "$e"; inj=""
        else
            cls="${e%.cmds}"
            case "$cls" in *seqrw*|*randrw*) budget=600;; *verify*|*multi*|*bigfile*) budget=700;; *) budget=650;; esac
            inj="$(injbins_for "$cls")"
        fi
        local outdir="$OUTBASE/s$slot"
        mkdir -p "$outdir"
        if [ -n "$inj" ]; then
            INJBINS="$inj" BUDGET="$budget" OUTDIR="$outdir" \
                nice -n 10 test/qrtests/run-suite-x86.sh -f "run/$e" > /dev/null 2>&1
        else
            BUDGET="$budget" OUTDIR="$outdir" \
                nice -n 10 test/qrtests/run-suite-x86.sh "$cls" > /dev/null 2>&1
        fi
        classify "$outdir/$cls.log" "$cls" "$slot"
        sleep 5
    done
    echo "$(date +%m%d-%H:%M:%S)|slot$slot|worker-exit|after $n boots|-" >> "$ROUNDS"
}

case "${1:-}" in
start)
    [ -f "$STOP" ] && rm -f "$STOP"
    if ! { [ -d "$WT" ] && [ -f "$WT/run/rootfs-qa.img" ] && [ -f "$WT/target/x86_64-unknown-none/debug/bzImage" ]; }
    then rebuild_worktree || { echo "rebuild FAILED" >&2; exit 1; }
    fi
    build_queue > "$QUEUE_FILE"
    if pgrep -f "stress-fleet3.sh worker" >/dev/null 2>&1; then
        echo "fleet already running: $(pgrep -fc 'stress-fleet3.sh worker') workers"; exit 0
    fi
    echo "$(date +%m%d-%H:%M:%S)|-|fleet-start|queue=$(wc -l < "$QUEUE_FILE") classes, 3 slots|-" >> "$ROUNDS"
    for s in 0 1 2; do
        setsid bash "$0" worker "$s" </dev/null >/dev/null 2>&1 &
    done
    sleep 2
    echo "fleet started: 3 workers, $(wc -l < "$QUEUE_FILE") classes; logs → $ROUNDS"
    ;;
status)
    echo "== workers: $(pgrep -fc 'stress-fleet3.sh worker' 2>/dev/null || echo 0)/3"
    local_pids=$(pgrep -f "stress-fleet3.sh worker" 2>/dev/null || true)
    for p in $local_pids; do echo "  worker pid=$p alive"; done
    echo "== fleet QEMUs:"
    for p in $(pgrep -f 'qemu-system-x86_64' 2>/dev/null); do
        cwd=$(readlink /proc/$p/cwd 2>/dev/null)
        [ "$cwd" = "$WT" ] && echo "  qemu pid=$p (mine)"
    done
    echo "== last verdicts:"; tail -8 "$ROUNDS" 2>/dev/null
    ;;
stop)
    touch "$STOP"
    pkill -f "stress-fleet3.sh worker" 2>/dev/null
    sleep 3   # workers dead first — no new QEMU launches can race past us
    for p in $(pgrep -f 'qemu-system-x86_64' 2>/dev/null); do
        cwd=$(readlink /proc/$p/cwd 2>/dev/null)
        [ "$cwd" = "$WT" ] && kill "$p" 2>/dev/null && echo "killed qemu $p"
    done
    echo "$(date +%m%d-%H:%M:%S)|-|fleet-stop|-|-" >> "$ROUNDS"
    echo "stopped"
    ;;
log) tail -n "${2:-30}" "$ROUNDS" ;;
worker) worker "$2" ;;
*) echo "usage: $0 start|status|stop|log [n]"; exit 1 ;;
esac
