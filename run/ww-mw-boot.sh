#!/bin/bash
# ww-mw-boot.sh — metawatch validation boot (session 13 experiment).
# Bare boot of the WORKTREE kernel (not the frozen pair — this boot is
# about the new dfx=metawatch code, not the hunt) with dfx switches via
# MW_DFX env; metawatch-selftest=1 rides when MW_SELFTEST=1.
# Own PIDs only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${TAG:-ww-mw1}"
SECSN="${SECS:-150}"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
SERIAL="$ROOT/run/$TAG-serial.log"
RAMDISK="/home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img"
DFX="${MW_DFX:-metawatch}"
SELFTEST="${MW_SELFTEST:+metawatch-selftest=1}"

rm -f "$SERIAL"

set -x
exec timeout --signal=TERM --kill-after=10 "${SECSN}s" qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 2G -smp 2 \
    -display none -vga none \
    -serial file:"$SERIAL" \
    -kernel "$KERNEL" \
    -initrd "$RAMDISK" \
    -append "console=ttyS0,115200 loglevel=7 dfx=$DFX $SELFTEST"
