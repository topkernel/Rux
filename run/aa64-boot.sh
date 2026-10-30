#!/usr/bin/env bash
# aarch64 GICv3 boot smoke — the aarch64-port lane's boot shape, plus
# -machine gic-version=3 (this branch embeds a gic-version=3 DTB blob;
# the lane's embedded DTB and default -M virt are GICv2). Rootfs is the
# lane's a64-rootfs.img opened snapshot+locking-off (read-only share).
# Resource discipline: -smp 2 -m 1G, TCG, hard timeout.
set -u
KERNEL="${1:-/home/william/rux-agents/aa64-gic/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-/home/william/rux-agents/aa64-gic/run/aa64-boot-serial.log}"
SECS="${3:-30}"

# The embedded DTB blob is untracked by convention (gitignored; build.rs
# rewrites dtb_blob.S's .incbin to kernel/src/arch/aarch64/virt.dtb per
# worktree). Regenerate the gic-version=3 dump if a fresh checkout lacks it.
DTB=/home/william/rux-agents/aa64-gic/kernel/src/arch/aarch64/virt.dtb
if [ ! -f "$DTB" ]; then
    qemu-system-aarch64 -M virt,gic-version=3 -cpu cortex-a72 -accel tcg \
        -m 1G -smp 2 -machine dumpdtb="$DTB" 2>/dev/null
    echo "regenerated $DTB (rebuild needed: make build PLATFORM=aarch64)"
fi

timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file=/home/william/rux-agents/aarch64-port/run/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs \
    -append "root=/dev/vda rw console=ttyAMA0" > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout, expected for a running kernel)"
echo "--- intc/gic/timer/smp/panic lines ---"
grep -E "intc|gic|timer|smp:|irq|GIC|panic|timer check" "${OUT}" | head -30
echo "--- serial tail ---"
tail -15 "${OUT}"
