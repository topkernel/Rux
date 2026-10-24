#!/bin/bash
# ww-magval.sh — pa16 allocator-magazine validation soak (session 35).
# Controlled OH boot of the WORKTREE bzImage (with the per-CPU magazine)
# — NOT the frozen hunt kernel. Same QEMU geometry and dfx=wildwatch
# instrumentation as the hunt chains, no gdb watchpoints (no hunt set),
# own serial log, 2700 s budget = the 45-min 0-panic gate.
# Discipline: -snapshot + file.locking=off on the shared OH images; this
# QEMU is the session's single experiment slot.

cd "$(dirname "$0")/.."
TAG=magval1
BUDGET=2700
OUT=run/${TAG}-serial.log

rm -f "$OUT"
echo "[magval] $(date +%H:%M:%S) starting budget=${BUDGET}s serial=${OUT}"

timeout --signal=TERM --kill-after=10 "${BUDGET}s" \
    qemu-system-x86_64 -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp 2 \
    -snapshot -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial "file:${OUT}" \
    -kernel target/x86_64-unknown-none/debug/bzImage \
    -initrd /home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img \
    -append "ip=dhcp loglevel=7 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required dfx=wildwatch" \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/updater.img,if=none,id=ohd0,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd0 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/system.img,if=none,id=ohd1,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd1 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/vendor.img,if=none,id=ohd2,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd2 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/sys_prod.img,if=none,id=ohd3,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd3 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/chip_prod.img,if=none,id=ohd4,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd4 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/userdata-tok.img,if=none,id=ohd5,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
rc=$?

echo "[magval] qemu rc=$rc at $(date +%H:%M:%S)"
echo "[magval] serial lines: $(wc -l < ${OUT} 2>/dev/null)"
echo "[magval] KERNPANIC count: $(grep -c KERNPANIC ${OUT} 2>/dev/null)"
echo "[magval] ALLOCTHROW count: $(grep -c ALLOCTHROW ${OUT} 2>/dev/null)"
echo "[magval] heal count: $(grep -cE 'WILDWATCH-CANARY|scribble-heal|WW-HEAL' ${OUT} 2>/dev/null)"
echo "[magval] wildwatch arm lines: $(grep -c 'wildwatch armed' ${OUT} 2>/dev/null)"
echo "[magval] forks: $(grep -c 'fork' ${OUT} 2>/dev/null)"
echo "[magval] last line: $(tail -1 ${OUT} 2>/dev/null | cut -c1-120)"
