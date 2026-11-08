#!/bin/bash
# r16: OUR column at SMP-8 in the same window as the reference re-anchor
# (run/r16-refanchor-smp8.sh) — same rig (q35, kvm, cpu host, 2G, smp 8),
# our kernel + musl runner, 3 reps per row. Output appended comparison.
set -u
WT="/home/william/rux-agents/perf-gap-hunt"
IMG="$WT/work/lmbench-x86.img"
OURS="${1:-$WT/work/bzImage-r16head}"
OUT="$WT/run/lmbench-ours-smp8.txt"
: > "$OUT"

boot() { # case log outer_cap
    local cap="${3:-120}"
    qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 8 -display none -vga none -serial "file:$2" -monitor none \
        -kernel "$OURS" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$1" | tr ' ' ',') console=ttyS0 budget=120" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1 &
    local qpid=$!
    local i
    for i in $(seq 1 "$cap"); do
        sleep 1
        grep -aq 'reboot: Power down' "$2" 2>/dev/null && break
        kill -0 "$qpid" 2>/dev/null || break
    done
    kill "$qpid" 2>/dev/null
    wait "$qpid" 2>/dev/null
    tr -d '\r' < "$2" 2>/dev/null \
        | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+'
}

for case_spec in "lat_syscall null" "lat_syscall read" "lat_syscall write" "lat_syscall stat" "lat_syscall fstat" "lat_syscall open" "lat_proc fork" "lat_proc exec" "lat_pipe"; do
    for rep in 1 2 3; do
        log="/tmp/r16ours8-$$-$rep.log"
        val="$(boot "$case_spec" "$log")"
        rm -f "$log"
        load="$(cut -d' ' -f1 /proc/loadavg)"
        echo "CASE[$case_spec] OURS-SMP8 rep$rep load=$load VAL=${val:-NONE}" | tee -a "$OUT"
    done
done
# lat_ctx: longer calibration window
for rep in 1 2 3; do
    log="/tmp/r16ours8-ctx-$rep.log"
    val="$(boot "lat_ctx -s 64 2" "$log" 240)"
    rm -f "$log"
    load="$(cut -d' ' -f1 /proc/loadavg)"
    echo "CASE[lat_ctx -s 64 2] OURS-SMP8 rep$rep load=$load VAL=${val:-NONE}" | tee -a "$OUT"
done
