#!/bin/bash
# fm-ledger-check.sh: 40s minimal-rootfs boot with dfx=ledger to verify
# the MEMLEDGER series prints on the current kernel (fm1 audit-boot
# produced zero series lines vs pt7's 95 — kernel-side isolation).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERIAL="$ROOT/run/fm-ledger-serial.log"
rm -f "$SERIAL"
timeout --signal=TERM --kill-after=5 75s qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 2G -smp 1 \
    -display none -serial file:"$SERIAL" \
    -kernel "$ROOT/target/x86_64-unknown-none/debug/bzImage" \
    -drive file="$ROOT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "console=ttyS0 dfx=ledger" || true
echo "MEMLEDGER lines: $(grep -c MEMLEDGER "$SERIAL" || true)"
grep MEMLEDGER "$SERIAL" | head -3
