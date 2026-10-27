#!/bin/bash
# pa59-ltp-r1.sh — LTP ext4-metadata + create-loop spot run on the
# perf-audit branch kernel (leg-37 F4 route-1 verification item: the
# null-journal deferral reuses the publication queue, whose history
# warns about create-loop regressions — the owed spot class is
# creat05/fork09 plus the metadata op families route-1 touches).
# Method: private copy of the fleet LTP rootfs with /test/sweep.list
# replaced by 17 rows (mkdir02-05, rmdir01-03, unlink05/07/08,
# rename01/03/04/05, symlink01, creat05, fork09 — indices preserved
# from the fleet 1081-row list, so SWEEP RES indices are comparable),
# sweep from 0. Baseline: the fleet's own recorded results for the
# same indices (main-class kernel, multiple rounds). FAIL/CRASH
# states that match the fleet baseline are known main-class behavior;
# anything NEW (a test the fleet passes that this kernel fails)
# blocks the route-1 merge claim.
# Usage: pa59-ltp-r1.sh <kernel> [tag]
set -u
K="${1:?usage: pa59-ltp-r1.sh <kernel> [tag]}"
TAG="${2:-pa59ltp-r1}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/pa59-ltp-r1-rootfs.img"
LOG="$ROOT/run/pa-$TAG-serial.log"
SECS="${SECS:-900}"

rm -f "$LOG"
setsid nice -n 10 timeout --signal=TERM --kill-after=10 "${SECS}s" \
  qemu-system-riscv64 -M virt -accel tcg,thread=multi \
  -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true \
  -m 2G -smp 2 -display none -monitor none \
  -serial "file:$LOG" \
  -drive "file=$IMG,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel "$K" \
  -append "root=/dev/vda rw init=/test/sweep console=ttyS0 sweepstart=0 dfx=watchdog,taskdump roundtag=$TAG" \
  >/dev/null 2>&1 &
echo "booted $TAG kernel=$K pid=$! (wait with: tail -f $LOG)"
