#!/bin/bash
# verify-healfix-ms.sh — memstorm-surface variant (memstorm rounds never
# wedge; v5 memstorm-r11 A-side: 14 HEAL(pid1) / 14 UNDERFLOW(pid1)).
# Usage: verify-healfix-ms.sh <kernel-elf> <tag> [secs]
set -u
KER="$1"; TAG="$2"; SECS="${3:-1500}"
L="$(cd "$(dirname "$0")" && pwd)/logs"
S="$L/verify-$TAG-serial.log"
IMG="$(cd "$(dirname "$0")" && pwd)/images"
nice -n 10 timeout --signal=TERM --kill-after=10 "${SECS}s" \
  qemu-system-riscv64 -M virt -accel tcg,thread=multi \
  -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true \
  -m 1G -smp 2 -display none -monitor none \
  -serial "file:$S" \
  -drive "file=$IMG/fleet-ltp-rootfs.img,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" \
  -device "virtio-blk-pci,disable-legacy=on,drive=rootfs" \
  -kernel "$KER" \
  -append "root=/dev/vda rw init=/test/memstorm console=ttyS0 dfx=periodic,memwatch,watchdog roundtag=verify-$TAG"
RC=$?
H=$(grep -c "PREEMPT-HEAL" "$S" 2>/dev/null)
UF=$(grep -c "PREEMPT-UNDERFLOW" "$S" 2>/dev/null)
B=$(grep -c "MEMSTORM-BEAT" "$S" 2>/dev/null)
PARK=$(grep -c "PARKED-LEAK" "$S" 2>/dev/null)
{ echo "[$TAG] rc=$RC lines=$(wc -l < "$S" 2>/dev/null) beats=$B HEAL=$H UNDERFLOW=$UF parkedleak=$PARK"; } | tee -a "$L/verify-$TAG-rounds.log"
