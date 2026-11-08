#!/bin/bash
# r16: re-anchor the REFERENCE lmbench column at SMP-8 (coordinator
# directive 2026-11-08): our gates/pbench moved to the SMP-8 rig but
# every frozen reference number is SMP-2 caliber. Reference side only,
# 3 reps per row, quiet window, same boot shape as g6-refanchor (q35,
# kvm, cpu host, loglevel=4, init_ref, budget=120) with -smp 8.
# Output: run/lmbench-refkvm-smp8.txt (does NOT touch the old tables).
set -u
WT="/home/william/rux-agents/perf-gap-hunt"
RIMG="$WT/run/refkvm/lmbench-ref2.img"
RKLOCAL="$WT/run/refkvm/vmlinuz-7.0.0-38-generic"
# /boot/vmlinuz-* is root:600 (QEMU cannot read it); the local copy was
# extracted with sudo. Prefer the local copy, fall back to /boot only
# for root-run contexts.
if [ -f "$RKLOCAL" ]; then
    RK="$RKLOCAL"
else
    RK=/boot/vmlinuz-7.0.0-38-generic
fi
OUT="$WT/run/lmbench-refkvm-smp8.txt"
: > "$OUT"

boot() { # case log
    local cs
    cs="$(echo "$1" | tr ' ' ',')"
    qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 8 -display none -vga none -serial "file:$2" -monitor none \
        -kernel "$RK" \
        -append "root=/dev/vda rw init=/test/init_ref lmbench=$cs console=ttyS0 budget=120 loglevel=4" \
        -drive file="$RIMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1 &
    local qpid=$!
    local i
    for i in $(seq 1 240); do
        sleep 1
        grep -aq 'reboot: Power down' "$2" 2>/dev/null && break
        kill -0 "$qpid" 2>/dev/null || break
    done
    kill "$qpid" 2>/dev/null
    wait "$qpid" 2>/dev/null
    tr -d '\r' < "$2" 2>/dev/null \
        | grep -aoE '[0-9]+\.[0-9]+ +microseconds' | head -1 | grep -aoE '[0-9]+\.[0-9]+'
}

for case_spec in "$@"; do
    for rep in 1 2 3; do
        log="/tmp/r16ref8-$$-$case_spec-$rep.log"
        val="$(boot "$case_spec" "$log")"
        rm -f "$log"
        load="$(cut -d' ' -f1 /proc/loadavg)"
        echo "CASE[$case_spec] REF-SMP8 rep$rep load=$load VAL=${val:-NONE}" | tee -a "$OUT"
    done
done
echo "== medians ==" | tee -a "$OUT"
awk '{split($1,a,"[\\[\\]]"); c=a[2]; sub(/VAL=/,"",$NF); v[c]=v[c]" "$NF} END {for (k in v) print k, v[k]}' "$OUT" | tee -a "$OUT"
