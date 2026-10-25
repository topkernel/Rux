#!/bin/bash
# pa48-ltpsock.sh — LTP socket-family subset on the perf-audit branch
# kernel (leg-35 owed item: b3757d92 socket fast path was med-of-3
# measured but never LTP-run; the fleet covers main-class only).
# Method: private copy of the fleet LTP rootfs with /test/sweep.list
# replaced by the 32 socket-family rows (indices preserved from the
# fleet 1081-row list, so SWEEP RES indices are comparable), sweep
# from 0. Baseline: the fleet's own recorded results for the same
# indices (main-class kernel, multiple rounds — grep the ltp-*-serial
# logs). FAIL/CRASH states that match the fleet baseline are known
# main-class behavior; anything NEW (a test the fleet passes that
# this kernel fails) blocks the merge claim.
# Usage: pa48-ltpsock.sh <kernel> [tag]
set -u
K="${1:?usage: pa48-ltpsock.sh <kernel> [tag]}"
TAG="${2:-pa48sock-r1}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/pa48-ltpsock-rootfs.img"
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
