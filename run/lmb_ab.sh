#!/bin/bash
# lmb_ab.sh — A/B lmbench lat_proc fork+exec between two bzImages
# (interleaved, KVM, one boot per case per sweep).
# Usage: lmb_ab.sh <base-bzImage> <fix-bzImage> <sweeps> <out.txt> [case...]
set -u
BASE="${1:?base bzImage}"
FIX="${2:?fix bzImage}"
SWEEPS="${3:?sweeps}"
OUT="${4:?out file}"
shift 4
CASES=("$@")
[ ${#CASES[@]} -eq 0 ] && CASES=("lat_proc fork" "lat_proc exec")
IMG="/home/william/rux-agents/perf-fork/work/lmbench-x86.img"
: > "$OUT"
for sweep in $(seq 1 "$SWEEPS"); do
  for side in base fix; do
    [ "$side" = base ] && K="$BASE" || K="$FIX"
    for spec in "${CASES[@]}"; do
      name="$(echo "$spec" | tr ' ,' '__')"
      log="/tmp/lmbab-${side}-${name}-$$.log"
      qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$K" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$spec" | tr ' ' ',') console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1 &
      QPID=$!
      # wait for the runner to power down (bounded)
      for i in $(seq 1 300); do
        kill -0 $QPID 2>/dev/null || break
        grep -aq "Power down\|PANIC" "$log" 2>/dev/null && sleep 2 && break
        sleep 1
      done
      kill -9 $QPID 2>/dev/null
      wait $QPID 2>/dev/null
      echo "===sweep=$sweep side=$side case=$spec" >> "$OUT"
      tr -d '\r' < "$log" 2>/dev/null | grep -aE "Process fork|Process exec|LMBENCH (TIMEOUT|EXEC-FAIL)" | head -4 >> "$OUT"
      rm -f "$log"
    done
  done
done
echo "lmb_ab: results -> $OUT"
