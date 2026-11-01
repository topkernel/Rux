#!/bin/bash
# ab-interleave.sh — interleaved A/B boots of two kernels, one case at a
# time, alternating base/fixed per rep so host-load drift hits both
# columns equally. Load-gated per the perf-lane validity rule.
# Usage: ab-interleave.sh <kernel-A> <kernel-B> <label-A> <label-B> <reps> <case...>
set -u
KA="$1"; KB="$2"; LA="$3"; LB="$4"; REPS="$5"; shift 5
WT="/home/william/rux-agents/perf-path"
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/abx-$LA-$LB.txt"
: > "$OUT"
boot() { # kernel case log
    timeout 150 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$3" -monitor none \
        -kernel "$1" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$2" | tr ' ' ',') console=ttyS0 budget=120" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    tr -d '\r' < "$3" 2>/dev/null | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+'
}
wait_quiet() {
    # Gate threshold: default 4 (the lane validity rule). Set GATE=6 when
    # the host is saturated by other agents — interleaving keeps the
    # base/fixed delta valid; each boot records its load anyway.
    local gate="${GATE:-4}"
    local w=0
    while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge "$gate" ] && [ "$w" -lt 300 ]; do
        sleep 20; w=$((w + 20))
    done
}
for case_spec in "$@"; do
    for rep in $(seq 1 "$REPS"); do
        for side in A B; do
            wait_quiet
            k="$KA"; lbl="$LA"
            [ "$side" = B ] && { k="$KB"; lbl="$LB"; }
            name="$(echo "$case_spec" | tr ' ,' '__')"
            log="/tmp/abx-$$-$name-$rep-$side.log"
            val="$(boot "$k" "$case_spec" "$log")"
            load="$(cut -d' ' -f1 /proc/loadavg)"
            echo "CASE[$case_spec] $lbl rep$rep load=$load VAL=${val:-NONE}" >> "$OUT"
            rm -f "$log"
        done
    done
done
echo "ab-interleave done -> $OUT"
