#!/bin/bash
# ww-strand-dump.sh — watch a running oom01-wedge repro serial for a
# LIVE pc=0x100-class strand (session 7): a non-idle pid holding
# pc=256/258 across >=10 consecutive WW3 heartbeat samples means a
# task is stuck INSIDE the do_softirq processing window (SOFTIRQ_OFFSET
# booked, window not closing). Attach gdb to the running QEMU's
# gdbstub and dump both harts immediately — the stuck pc names the
# handler, the stack words name the path. Exits after one dump.
# usage: ww-strand-dump.sh <serial-log> <gdb-port> [max-seconds]
set -u
SER="${1:?serial log}"
PORT="${2:?gdb port}"
MAXS="${3:-2700}"
OUT="${SER}.stranddump.log"
: >> "$OUT"
START=$(date +%s)
while :; do
    NOW=$(date +%s)
    [ $((NOW - START)) -ge "$MAXS" ] && { echo "[$(date +%H:%M:%S)] window elapsed, no dump" >> "$OUT"; exit 1; }
    # last 12 WW3 lines: count consecutive samples of one stuck pid
    RUN=$(tail -400 "$SER" 2>/dev/null | grep -o 'pid=[0-9]* st=0x0 pc=25[68])' | grep -v 'pid=[01] ' | tail -12 | sort | uniq -c | sort -rn | head -1)
    N=$(echo "$RUN" | awk '{print $1}')
    PID=$(echo "$RUN" | awk '{print $2}')
    if [ "${N:-0}" -ge 10 ]; then
        echo "[$(date +%H:%M:%S)] LIVE STRAND $PID x$N — attaching gdb :$PORT" >> "$OUT"
        GDBCMD=$(mktemp)
        cat > "$GDBCMD" <<EOF
set pagination off
set confirm off
target remote :$PORT
echo \n=== HART DUMP (live softirq-window strand) ===\n
info threads
thread 1
echo \n--- hart0 pc/ra/sp/s0 ---\n
p/x \$pc
p/x \$ra
p/x \$sp
p/x \$s0
echo \n--- hart0 stack words ---\n
x/32xg \$sp
thread 2
echo \n--- hart1 pc/ra/sp/s0 ---\n
p/x \$pc
p/x \$ra
p/x \$sp
p/x \$s0
echo \n--- hart1 stack words ---\n
x/32xg \$sp
echo \n=== DUMP END ===\n
detach
quit
EOF
        timeout 120 gdb-multiarch -q -batch -x "$GDBCMD" >> "$OUT" 2>&1
        rm -f "$GDBCMD"
        echo "[$(date +%H:%M:%S)] dump complete" >> "$OUT"
        exit 0
    fi
    sleep 20
done
