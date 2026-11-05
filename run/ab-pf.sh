#!/bin/bash
# ab-pf.sh — interleaved A/B boots of two kernels for the perf-gap-hunt
# lane (pagefault 2x push). Alternates base/new per rep so host-load
# drift hits both columns equally. Usage:
#   ab-pf.sh <kernel-A> <kernel-B> <label-A> <label-B> <reps> <case...>
# Extra kernel cmdline via EXTRA_CMD (e.g. mmprof=1).
set -u
KA="$1"; KB="$2"; LA="$3"; LB="$4"; REPS="$5"; shift 5
WT="/home/william/rux-agents/perf-gap-hunt"
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/abpf-$LA-$LB.txt"
: > "$OUT"
boot() { # kernel case log
    # The x86_64 kernel parks (loop+wfi) after "reboot: Power down" —
    # no ACPI-off driver — so QEMU never exits on its own. Poll the
    # serial log for the marker (or a self-exit) and kill our own QEMU
    # instead of burning the full timeout: ~90s per boot vs 280s.
    qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$3" -monitor none \
        -kernel "$1" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$2" | tr ' ' ',') console=ttyS0 budget=120 ${EXTRA_CMD:-}" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1 &
    local qpid=$!
    local i
    for i in $(seq 1 280); do
        sleep 1
        grep -aq 'reboot: Power down' "$3" 2>/dev/null && break
        kill -0 "$qpid" 2>/dev/null || break
    done
    kill "$qpid" 2>/dev/null
    wait "$qpid" 2>/dev/null
    tr -d '\r' < "$3" 2>/dev/null | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+'
}
for case_spec in "$@"; do
    for rep in $(seq 1 "$REPS"); do
        for side in A B; do
            k="$KA"; lbl="$LA"
            [ "$side" = B ] && { k="$KB"; lbl="$LB"; }
            name="$(echo "$case_spec" | tr ' ,/' '___')"
            log="/tmp/abpf-$$-$name-$rep-$side.log"
            val="$(boot "$k" "$case_spec" "$log")"
            load="$(cut -d' ' -f1 /proc/loadavg)"
            echo "CASE[$case_spec] $lbl rep$rep load=$load VAL=${val:-NONE}" >> "$OUT"
            rm -f "$log"
        done
    done
done
echo "ab-pf done -> $OUT"
