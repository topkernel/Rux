#!/bin/bash
# oh-boot-refw.sh — reference-kernel OH boot with a WRITABLE private userdata
# copy (my own file, not shared) so the post-boot /data tree can be inspected
# offline. Everything else mirrors test/run-oh-linux-ref.sh. hdc port 12056.
# Usage: oh-boot-refw.sh <tag> <userdata-img> [secs]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:?tag}"
USERDATA="${2:?userdata}"
SECS="${3:-360}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"
PIDFILE="$RUN/oh-$TAG-qemu.pid"
OHOS_IMG=/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images
QEMU="${QEMU:-$HOME/.local/qemu/bin/qemu-system-x86_64}"
[ -x "$QEMU" ] || QEMU="$(command -v qemu-system-x86_64)"

rm -f "$SERIAL" "$MON" "$PIDFILE"

sdrive() { # idx file (shared, read-only overlay)
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$2" "$1" "$1"
}

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200 init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"

# shellcheck disable=SC2086
exec "$QEMU" \
    -machine q35 -smp "${SMP:-4}" -m "${MEM:-4096M}" -enable-kvm -cpu host -boot c \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial "file:$SERIAL" \
    -monitor "unix:$MON,server=on,wait=off" \
    -pidfile "$PIDFILE" \
    -initrd "$OHOS_IMG/ramdisk.img" -kernel "$OHOS_IMG/bzImage" \
    -append "$APPEND" \
    $(sdrive 0 "$OHOS_IMG/updater.img")$(sdrive 1 "$OHOS_IMG/system.img")$(sdrive 2 "$OHOS_IMG/vendor.img")$(sdrive 3 "$OHOS_IMG/sys_prod.img")$(sdrive 4 "$OHOS_IMG/chip_prod.img") \
    -drive "if=none,id=ohd5,format=raw,file.locking=off,file=$USERDATA" -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:12056-:10178
