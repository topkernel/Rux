#!/bin/bash
# tr4-overhead-boot.sh — the quiet-host lock: overhead differential
# (see tr4-overhead-init.c). One boot, no aliases, no other QEMUs.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KERNEL="${1:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
LOG="$ROOT/run/tr4-overhead-serial.log"
ROOTFS="$ROOT/run/tr4-ovh-rootfs.img"
gcc -static -O2 -o "$ROOT/run/tr4-ovh-init" "$ROOT/run/tr4-overhead-init.c" || exit 1
rm -f "$ROOTFS"
truncate -s 128M "$ROOTFS"
mkfs.ext4 -q -F -O ^metadata_csum_seed,^orphan_file "$ROOTFS" || exit 1
debugfs -w -R "write $ROOT/run/tr4-ovh-init /init" "$ROOTFS" >/dev/null 2>&1 || exit 1
{
    sleep 150
} | timeout 150 qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 1G -smp 2 \
    -display none -serial stdio -snapshot \
    -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/init console=ttyS0" \
    > "$LOG" 2>&1
grep "OVRDBG" "$LOG"
