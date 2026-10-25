#!/bin/bash
# run-oh-linux-ref.sh — boot the OpenHarmony x86_64_virt phone image on the
# REFERENCE kernel (oh-robot's own bzImage) under QEMU/KVM, serial to a
# file, headless, read-only for the shared images.
#
# This is the analysis-lane counterpart of test/run-oh.sh: machine/device/
# cmdline arguments are copied verbatim from oh-robot's qemu_run_user.sh
# (the ground truth for this image), with three deliberate changes:
#   1. -display none instead of "-nographic -display sdl,gl=off" — the run
#      is headless; screen state is captured with QEMU-monitor `screendump`
#      against the named virtio-gpu console (verified to work headless).
#   2. -serial file:<log> instead of stdio — full lossless serial capture.
#   3. -snapshot + file.locking=off on every drive — the shared build-tree
#      images are never written (repo discipline).
#
# Usage:
#   test/run-oh-linux-ref.sh <run-dir> [userdata-img]
#     <run-dir>       output dir (created); serial.log, monitor.sock, qemu.pid
#     [userdata-img]  optional userdata replacement (default: stock populated
#                     userdata.img from the image dir — the reference case)
#
# Environment:
#   QEMU       override the QEMU binary (default: oh-robot's custom build,
#              falling back to system qemu-system-x86_64)
#   KVM=0      force TCG (very slow; allow 30+ min)
#   SMP/MEM    override -smp 6 / -m 8192M
#   CPU        override the cpu model (default host with KVM, max on TCG)
#   SYSIMG     override the system drive image (default the shared
#              $OHOS_IMG/system.img) — probe boots point this at a
#              PRIVATE copy carrying the pbench slot (run/pbref-inject.sh)
#   USERDATA_SNAP=0  drop snapshot=on from the userdata drive so guest
#              writes persist into the (private) userdata image — the
#              probe results channel. Shared images always keep snapshot.
set -euo pipefail

RUNDIR="${1:?usage: run-oh-linux-ref.sh <run-dir> [userdata-img]}"
USERDATA="${2:-}"
OHOS_IMG=/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images

mkdir -p "$RUNDIR"
RUNDIR="$(cd "$RUNDIR" && pwd)"
SERIAL="$RUNDIR/serial.log"
MON="$RUNDIR/monitor.sock"
PIDFILE="$RUNDIR/qemu.pid"

QEMU="${QEMU:-$HOME/.local/qemu/bin/qemu-system-x86_64}"
[ -x "$QEMU" ] || QEMU="$(command -v qemu-system-x86_64)"

ACCEL="-enable-kvm -cpu ${CPU:-host}"
[ "${KVM:-1}" = "0" ] && ACCEL="-accel tcg,thread=multi -cpu ${CPU:-max}"

# drive order is load-bearing: vda updater, vdb system, vdc vendor,
# vdd sys_prod, vde chip_prod, vdf userdata — the ohos.required_mount.*
# cmdline tokens name /dev/block/vdb and /dev/block/vdc explicitly.
drive() { # idx file
    local idx=$1 file=$2 snap=",snapshot=on"
    [ "$idx" = 5 ] && [ "${USERDATA_SNAP:-1}" = "0" ] && snap=""
    [ -n "$file" ] || return 0
    [ -f "$file" ] || { echo "missing disk: $file" >&2; exit 1; }
    printf ' -drive file=%s,if=virtio,media=disk,format=raw,index=%s,file.locking=off%s' "$file" "$idx" "$snap"
}
[ -n "$USERDATA" ] || USERDATA="$OHOS_IMG/userdata.img"

DISKS="$(drive 0 "$OHOS_IMG/updater.img")$(drive 1 "${SYSIMG:-$OHOS_IMG/system.img}")$(drive 2 "$OHOS_IMG/vendor.img")$(drive 3 "$OHOS_IMG/sys_prod.img")$(drive 4 "$OHOS_IMG/chip_prod.img")$(drive 5 "$USERDATA")"

# cmdline copied verbatim from qemu_run_user.sh
APPEND="ip=dhcp loglevel=7 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"

# shellcheck disable=SC2086
exec "$QEMU" \
    -machine q35 -smp "${SMP:-6}" -m "${MEM:-8192M}" $ACCEL -boot c \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=360,yres=720 \
    -rtc base=utc,clock=host \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0,mac=12:22:33:44:55:66 \
    -initrd "$OHOS_IMG/ramdisk.img" -kernel "$OHOS_IMG/bzImage" \
    -append "$APPEND" \
    $DISKS \
    -serial "file:$SERIAL" \
    -monitor "unix:$MON,server,nowait" \
    -pidfile "$PIDFILE"
