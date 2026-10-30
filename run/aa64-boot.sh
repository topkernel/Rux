#!/usr/bin/env bash
# aarch64 stage-1 boot smoke: QEMU virt, PL011 serial, banner + controlled
# stop point. Resource discipline: -smp 2 -m 1G, TCG, snapshot-free (direct
# kernel boot, no disk), hard timeout.
set -u
KERNEL="${1:-/home/william/rux-agents/aarch64-port/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-/home/william/rux-agents/aarch64-port/run/aa64-boot-serial.log}"
SECS="${3:-25}"

timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" -drive file=/home/william/rux-agents/aarch64-port/run/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=rootfs -append "root=/dev/vda rw console=ttyAMA0 dfx=taskdump" -dtb /tmp/virt.dtb > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout, expected for a running kernel)"
echo "--- serial head ---"
head -40 "${OUT}"
echo "--- serial tail ---"
tail -15 "${OUT}"
