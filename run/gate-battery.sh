#!/bin/bash
# gate-battery.sh — waits for a quiet host window (load1 < 8) and runs
# the perf-gap-hunt measurement battery in order:
#   1. lat_pagefault interleaved A/B: base vs backwin (3 reps each)
#   2. mmprof attribution boot on backwin (mmprof=1)
#   3. pbench Tier-2 A/B: base vs head (1 rep each, extend if marginal)
# Every boot records load. Usage: gate-battery.sh [max_wait_secs]
set -u
WT="/home/william/rux-agents/perf-gap-hunt"
MAXW="${1:-3600}"
LOG="$WT/run/gate-battery.log"
echo "battery start $(date +%H:%M:%S)" >> "$LOG"

quiet() { [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -lt 8 ]; }

w=0
until quiet; do
    sleep 30; w=$((w + 30))
    [ "$w" -ge "$MAXW" ] && { echo "window never opened, giving up" >> "$LOG"; exit 1; }
done
echo "window open at $(date +%H:%M:%S) load=$(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"

# 1. lat_pagefault A/B
bash "$WT/run/ab-pf.sh" "$WT/work/bzImage-lm-base" "$WT/work/bzImage-lm-backwin" \
    base backwin 3 "lat_pagefault,/tmp/lmbench.file" >> "$LOG" 2>&1
echo "ab-pf done $(date +%H:%M:%S)" >> "$LOG"

# 2. mmprof attribution boot (backwin, one boot)
if quiet; then
    timeout 150 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial file:/tmp/pf-mmprof.log -monitor none \
        -kernel "$WT/work/bzImage-lm-backwin" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=lat_pagefault,/tmp/lmbench.file console=ttyS0 budget=60 mmprof=1" \
        -drive file="$WT/work/lmbench-x86.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    grep -aE "MMPROF (MU_|MP_|M_)|microseconds" /tmp/pf-mmprof.log | tail -40 > "$WT/run/pf-mmprof-backwin.txt"
    echo "mmprof boot done $(date +%H:%M:%S)" >> "$LOG"
fi

# 3. pbench Tier-2 A/B (base vs HEAD gate kernel)
if quiet; then
    bash "$WT/run/ab-pbench.sh" "$WT/work/bzImage-lm-base" "$WT/work/bzImage-lm-head" \
        base head 1 >> "$LOG" 2>&1
    echo "ab-pbench done $(date +%H:%M:%S)" >> "$LOG"
fi
echo "battery complete $(date +%H:%M:%S)" >> "$LOG"
