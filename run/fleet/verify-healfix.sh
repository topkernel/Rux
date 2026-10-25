#!/bin/bash
# verify-healfix.sh — A/B discriminator for the PREEMPT-HEAL base-only
# clear fix (session 3). Boots ONE riscv64 LTP-surface QEMU with the
# given kernel, runs SECS, then reports heal/underflow/bracket counts.
# Usage: verify-healfix.sh <kernel-elf> <tag> [secs]
set -u
KER="$1"; TAG="$2"; SECS="${3:-1800}"
L="$(cd "$(dirname "$0")" && pwd)/logs"
S="$L/verify-$TAG-serial.log"
IMG="$(cd "$(dirname "$0")" && pwd)/images"
START=$(( (RANDOM * 32768 + RANDOM) % 1080 ))
nice -n 10 timeout --signal=TERM --kill-after=10 "${SECS}s" \
  qemu-system-riscv64 -M virt -accel tcg,thread=multi \
  -cpu rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true \
  -m 2G -smp 2 -display none -monitor none \
  -serial "file:$S" \
  -drive "file=$IMG/fleet-ltp-rootfs.img,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" \
  -device "virtio-blk-pci,disable-legacy=on,drive=rootfs" \
  -kernel "$KER" \
  -append "root=/dev/vda rw init=/test/sweep console=ttyS0 sweepstart=$START dfx=watchdog,taskdump roundtag=verify-$TAG"
RC=$?
HEALS=$(grep -c "PREEMPT-HEAL" "$S" 2>/dev/null)
UF=$(grep -c "PREEMPT-UNDERFLOW" "$S" 2>/dev/null)
UF64=$(grep -c "sub=00010000" "$S" 2>/dev/null)
BRK=$(grep -c "IRQ-BRACKET-LEAK" "$S" 2>/dev/null)
PLEAK=$(grep -c "PREEMPT-LEAK" "$S" 2>/dev/null")
RES=$(grep -c "SWEEP RES" "$S" 2>/dev/null)
{ echo "[$TAG] rc=$RC lines=$(wc -l < "$S" 2>/dev/null) sweepRES=$RES HEAL=$HEALS UNDERFLOW=$UF sub64k=$UF64 bracket=$BRK preemptleak=$PLEAK"; } | tee -a "$L/verify-$TAG-rounds.log"
