#!/bin/bash
# oh-boot.sh — headless OH boot for the oh-display agent.
#
# Same QEMU command as test/run-oh.sh <imgdir> gui (q35, 720x1280
# virtio-gpu, six disks, ohos.* tokens), but:
#   - serial goes to a file (no stdio dependency — this runs as a
#     tracked background task),
#   - the QEMU monitor listens on a unix socket so the agent can send
#     `screendump` at any time (see oh-mon.sh),
#   - -display none: the virtio-gpu device is what the guest sees; the
#     host-side display backend is irrelevant for screendump.
#
# Usage: oh-boot.sh <tag> [kernel]
#   <tag>    log/socket/screenshot basename under run/
#   [kernel] bzImage path (default: target/.../bzImage)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: oh-boot.sh <tag> [kernel]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"

rm -f "$SERIAL" "$MON"

qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
# EXTRA_APPEND (env): appended verbatim (dfx= flags etc).
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$IMGDIR/userdata.img")"

# RAMDISK: prefer the patched copy (adds /etc/ld-musl-x86_64.path, the
# default-library-path file this image only ships for aarch64 — without
# it every setuid/setgid OH service resolves libs against bare
# /lib:/usr/local/lib:/usr/lib and dies at loader stage: LD_LIBRARY_PATH
# is stripped from secure execs, Linux-style). Rebuild: unpack the stock
# ramdisk, drop the .path in /etc (aarch64 list + chipset-sdk-sp/hw/
# passthrough/ndk/llndk/sys_prod dirs), cpio -H newc | gzip.
RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

# GDB=1 adds the gdbstub (-s, port 1234) for post-crash or breakpoint work.
# SMP=n overrides the CPU count. The x86 pick/harvest scheduler races
# (phantom-run wedges, poisoned switch frames — see branch notes) have
# no steal/harvest path on one CPU; display bring-up runs SMP=1.
GDBARGS=""
[ "${GDB:-0}" = "1" ] && GDBARGS="-s"
SMPN="${SMP:-2}"

# shellcheck disable=SC2086
exec qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp "$SMPN" -snapshot \
    $GDBARGS \
    -display none \
    -device virtio-gpu-pci,id=gpu0,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
