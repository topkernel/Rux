#!/usr/bin/env bash
# aa64-tail boot smoke (pattern from aarch64-port/run/aa64-boot.sh):
# -machine gic-version=3 REQUIRED (GIC driver + embedded DTB are v3).
# Shared image from the central store, opened snapshot+locking-off.
# Resource discipline: -smp 2 -m 1G, TCG, one QEMU at a time, hard timeout.
set -u
KERNEL="${1:-/home/william/rux-agents/aa64-tail/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-/home/william/rux-agents/aa64-tail/run/boot-serial.log}"
SECS="${3:-30}"

timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file=/home/william/rux-agents/aa64-tail/run/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout, expected for a running kernel)"
echo "--- panic/OOPS lines ---"
grep -E "panic|Oops|Oops:|KENRPANIC|BUG" "${OUT}" | head -10
echo "--- serial tail ---"
tail -25 "${OUT}"
