#!/bin/bash
# ww-boot.sh — wildwrite-hunt OH boot with gdbstub (no -S: guest runs
# until gdb attaches; watchpoints arm post-attach).
#
# Same QEMU geometry as oh-display's oh-boot.sh (q35, 720x1280
# virtio-gpu, six disks, ohos.* tokens), read-only discipline everywhere:
# global -snapshot + file.locking=off on every shared drive.
#
# Env:
#   TAG      log basename under run/ (default ww)
#   GDBPORT  gdbstub port (default 12371)
#   SMP      CPU count (default 2)
#   SECS     wall-clock budget (default 2700) — enforced by ww-hunt.py
#   KERNEL   bzImage path (default: FROZEN copy of the hunt kernel — the
#            worktree may rebuild for unrelated mm work, and a mid-chain
#            kernel change would silently invalidate the hunt; the frozen
#            pair must stay in sync with ww-gdbscript.py's ELF)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="/home/william/rux-agents/oh-display/run/oh-img"
TAG="${TAG:-ww}"
GDBPORT="${GDBPORT:-12374}"
SMPN="${SMP:-2}"
KERNEL="${KERNEL:-$ROOT/run/ww-hunt-frozen-bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/${TAG}-serial.log"

rm -f "$SERIAL"

qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

# userdata: tok-seeded blank image (HDF accessToken chain needs it)
USERDATA="$IMGDIR/userdata-tok.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata-rw.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata.img"

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$USERDATA")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

exec qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp "$SMPN" -snapshot \
    -gdb tcp:127.0.0.1:"$GDBPORT" \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
