#!/bin/bash
# fam-ab.sh — interleaved A/B med-of-3 for two kernels (base vs cand).
# For each round, each case: boot base then boot cand back-to-back, so
# host-load drift hits both kernels equally. 8 cases x 2 x 3 = 48 boots.
# Usage: fam-ab.sh <tagA> <bzA> <tagB> <bzB>
set -uo pipefail
WT=/home/william/rux-agents/gap-sched
IMG="$WT/work/lmbench-x86.img"
TA="${1:?tagA}"; KA="${2:?bzA}"; TB="${3:?tagB}"; KB="${4:?bzB}"
OUT="$WT/run/fam-ab-$TA-$TB.txt"

CASES=(
    "lat_ctx,-s,64,2" "lat_ctx,-s,64,8" "lat_pipe," "lat_unix," "lat_sem,"
    "lat_syscall,null" "lat_syscall,read" "lat_syscall,write"
)

echo "=== fam-ab $TA vs $TB $(date +%F_%H:%M:%S)" >> "$OUT"
echo "    A=$KA B=$KB" >> "$OUT"

boot() { # kernel log
    timeout 45 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$2" -monitor none \
        -kernel "$1" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$3 console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
}

for round in 1 2 3; do
    for i in $(seq 0 $((${#CASES[@]}-1))); do
        spec="${CASES[$i]}"
        for side in A B; do
            [ "$side" = A ] && { tag=$TA; ker=$KA; } || { tag=$TB; ker=$KB; }
            log="/tmp/famab-$tag-$i.log"
            boot "$ker" "$log" "$spec"
            if tr -d '\r' < "$log" 2>/dev/null | grep -aqE "[0-9]+\.[0-9]+"; then
                val=$(tr -d '\r' < "$log" | grep -aE "[0-9]+\.[0-9]+" | grep -avE "POSIX|v0\.1\.0" | tail -3 | tr '\n' ' ')
                echo "r$round $tag ${spec//$/, } :: $val" >> "$OUT"
            else
                echo "r$round $tag ${spec//$/, } :: TIMEOUT" >> "$OUT"
            fi
            rm -f "$log"
        done
    done
    echo "--- round $round done load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
done
echo "=== fam-ab done $(date +%H:%M:%S)" >> "$OUT"
cat "$OUT"
