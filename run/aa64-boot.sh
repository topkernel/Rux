#!/usr/bin/env bash
# aarch64 GICv3 boot smoke: -machine gic-version=3 (the GICv3 driver and
# the embedded DTB are v3; default -M virt is GICv2). Rootfs is the
# lane's a64-rootfs.img opened snapshot+locking-off (read-only share).
# -append/-dtb are no-ops for ELF -kernel boots (Linux boot protocol
# only) — the kernel uses its embedded DTB and DEFAULT_CMDLINE.
# Resource discipline: -smp 2 -m 1G, TCG, hard timeout.
set -u
KERNEL="${1:-/home/william/rux-agents/aarch64-port/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-/home/william/rux-agents/aa64-signal/run/aa64-boot-serial.log}"
SECS="${3:-30}"

# The embedded DTB blob is untracked by convention (gitignored); build.rs
# regenerates it on the next build if missing (with gic-version=3).
timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file=/home/william/rux-agents/aarch64-port/run/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout, expected for a running kernel)"
echo "--- intc/gic/timer/smp/panic lines ---"
grep -E "intc|gic|timer|smp:|irq|GIC|panic" "${OUT}" | head -30
echo "--- serial tail ---"
tail -15 "${OUT}"
