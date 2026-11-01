#!/bin/bash
# sysab-abc.sh — interleaved A/B/C syscall-family comparison.
# Sweeps alternate base -> fix1 -> fix1+2 so host drift affects all
# columns equally (ledger protocol: back-to-back at equal load,
# med-of-sweeps verdict, void any sweep at load1 >= 4).
#
# Usage: run/sysab-abc.sh <nsweeps>
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/.." && pwd)"
N="${1:-3}"
declare -A K=(
    [base]="$WT/run/bzImage-base"
    [fix1]="$WT/run/bzImage-fix1"
    [fix12]="$WT/run/bzImage-fix12"
)
CASES=("lat_syscall null" "lat_syscall read" "lat_syscall write" "lat_syscall fstat" "lat_syscall open")
OUT="${OUT:-$WT/run/sysab-abc.txt}"
: > "$OUT"
for sweep in $(seq 1 "$N"); do
    for tag in base fix1 fix12; do
        load1=$(cut -d' ' -f1 /proc/loadavg)
        fq=$(ps aux | grep -c "[q]emu")
        echo "# sweep $sweep col $tag load1=$load1 foreign_qemus=$fq kernel=$(md5sum "${K[$tag]}" | cut -c1-8) $(date +%H:%M:%S)" >> "$OUT"
        for spec in "${CASES[@]}"; do
            name=$(echo "$spec" | tr ' ' '_')
            val=""
            for attempt in 1 2; do
                log="/tmp/sysab-abc-$name-$$.log"
                timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
                    -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
                    -kernel "${K[$tag]}" \
                    -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$spec" | tr ' ' ',') console=ttyS0" \
                    -drive file="$WT/work/lmbench-x86.img",if=none,id=rootfs,format=raw \
                    -device virtio-blk-pci,drive=rootfs \
                    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
                val=$(tr -d '\r' < "$log" 2>/dev/null | grep -aoE "Simple [a-z/ ]+: [0-9]+\.[0-9]+ microseconds" | tail -1)
                rm -f "$log"
                [ -n "$val" ] && break
            done
            echo "$tag $name | $val" >> "$OUT"
        done
    done
done
echo "sysab-abc: -> $OUT"
