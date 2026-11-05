#!/bin/bash
# run-repro.sh — batched-read + exit-storm repro for the virtio async
# completion corruption family.
#
# Usage: run-repro.sh <kernel-path> <tag> [duration-seconds]
#
# Load shape: N long-lived concurrent whole-file readers (multi-block
# coalesced chains from 3 different binaries, staggered cache states ->
# mixed 4KB/256KB chains in flight), plus an exit storm of 400 short-lived
# `cat` processes (kernel-stack recycling while batches are in flight),
# on -smp 4 MTTCG.
#
# Discipline: -snapshot + file.locking=off; kills only its own QEMU
# (via timeout); logs to runs/<tag>/serial.log.

set -u
KERNEL=$1
TAG=$2
DUR=${3:-180}
HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/runs/$TAG"
mkdir -p "$OUT"

cat > "$OUT/input.txt" <<'EOF'
echo REPRO-READY
r=0
while [ $r -lt 6 ]; do echo ROUND-$r-BEGIN; head -c 268435456 /dev/zero > /tmp/big; i=0; while [ $i -lt 8 ]; do ( dd if=/tmp/big of=/dev/null bs=1048576 skip=$(( (i*31+r*7) % 240 )) count=8 ) & i=$((i+1)); done; j=0; while [ $j -lt 40 ]; do ( head -c 5242880 /tmp/big > /dev/null ) & j=$((j+1)); done; j=0; while [ $j -lt 20 ]; do ( cat /tmp/big > /dev/null ) & j=$((j+1)); done; wait; rm /tmp/big; echo ROUND-$r-END; r=$((r+1)); done
echo ALL-ROUNDS-DONE
EOF

echo "=== run-repro $TAG kernel=$KERNEL dur=${DUR}s ==="
(cd "$OUT" && (cat "$OUT/input.txt"; sleep "$DUR") | timeout --signal=TERM "$DUR" \
  qemu-system-riscv64 -M virt -accel tcg,thread=multi -cpu rv64 -m 2G -smp 4 \
    -nographic -serial stdio -monitor none \
    -snapshot -drive file=/home/william/Rux/test/rootfs.img,if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  > "$OUT/serial.log" 2>&1; echo "qemu exit: $?" > "$OUT/exit.txt")

# Verdict
{
  echo "=== verdict for $TAG ==="
  for pat in "KERNPANIC" "WAKE-WILD-PTR" "panic" "soft lockup" "SOFT-LOCKUP" "ALLOCTHROW" "trap: stack:" "backtrace" "lost completion" "buffer io wait timeout"; do
    n=$(grep -c "$pat" "$OUT/serial.log" 2>/dev/null || true)
    echo "pattern '$pat': ${n:-0}"
  done
  echo "markers:"
  grep -n "REPRO-READY\|LONG-READERS-UP\|EXIT-STORM-DONE\|SECOND-WAVE-UP" "$OUT/serial.log" | head -8
  echo "serial log lines: $(wc -l < "$OUT/serial.log")"
} > "$OUT/verdict.txt"
cat "$OUT/verdict.txt"
