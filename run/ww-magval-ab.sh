#!/bin/bash
# ww-magval-ab.sh — pa16 magazine A/B lock-traffic measurement boot.
# Same geometry as ww-magval.sh but dfx=wildwatch,periodic (30s
# 30s taskdumps (dfx=periodic, PERIODIC 3000 jiffies) carry the SMALLALLOC census with the new locks= column)
# and a 900s budget — the LAST census block in the serial gives the
# cumulative per-class ops vs class-lock acquisitions ratio.
# Pre-magazine, every op took the lock: before-holds == a+f on the same
# line, so locks/(a+f) is the direct traffic-ratio readout.

cd "$(dirname "$0")/.."
TAG=magval-ab
BUDGET=900
OUT=run/${TAG}-serial.log

rm -f "$OUT"
echo "[ab] $(date +%H:%M:%S) starting budget=${BUDGET}s serial=${OUT}"

timeout --signal=TERM --kill-after=10 "${BUDGET}s" \
    qemu-system-x86_64 -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp 2 \
    -snapshot -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial "file:${OUT}" \
    -kernel target/x86_64-unknown-none/debug/bzImage \
    -initrd /home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img \
    -append "ip=dhcp loglevel=7 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required dfx=wildwatch,periodic" \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/updater.img,if=none,id=ohd0,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd0 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/system.img,if=none,id=ohd1,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd1 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/vendor.img,if=none,id=ohd2,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd2 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/sys_prod.img,if=none,id=ohd3,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd3 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/chip_prod.img,if=none,id=ohd4,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd4 \
    -drive file=/home/william/rux-agents/oh-display/run/oh-img/userdata-tok.img,if=none,id=ohd5,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
rc=$?

echo "[ab] qemu rc=$rc at $(date +%H:%M:%S)"
echo "[ab] serial lines: $(wc -l < ${OUT} 2>/dev/null)"
echo "[ab] KERNPANIC: $(grep -c KERNPANIC ${OUT} 2>/dev/null) ALLOCTHROW: $(grep -c ALLOCTHROW ${OUT} 2>/dev/null)"
echo "[ab] census blocks: $(grep -c SMALLALLOC ${OUT} 2>/dev/null)"
echo "[ab] last census block:"
awk '/SMALLALLOC cls/{n=NR} {lines[NR]=$0} END{for(i=n;n<=NR&&i<n+9;i++) print lines[i]}' "$OUT" 2>/dev/null
