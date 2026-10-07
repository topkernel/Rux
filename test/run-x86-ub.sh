#!/bin/bash
# run-x86-ub.sh — boot the Ubuntu amd64 image on the x86_64 kernel (q35).
#
# The x86 mirror of the riscv64 `make ubuntu-run` flow: same rootfs layout
# (work/ubuntu-x86.img, /sbin/init -> udesk-init), same device set
# (virtio-blk rootfs + virtio-gpu + virtio-keyboard/tablet + virtio-net),
# serial console on ttyS0.
#
# Usage: ./test/run-x86-ub.sh [console|gui|debug]
#   console - headless serial run, exits at kernel stop (default)
#   gui     - native SDL window + serial in this terminal (login: root/rux)
#   debug   - like console but -S -s (GDB attach line printed)
#
# Environment overrides:
#   ROOTFS=path/to/img   image to boot (default work/ubuntu-x86.img)
#   SMP=1                CPUs (x86 SMP bring-up is not landed: keep 1)
#   MEM=2G               guest RAM — 2G max for now: with RAM >= 3G the
#                       q35 PCI hole starts at 0xc0000000, the first BAR
#                       lands exactly there, and the device-mapping
#                       install leaves it unmapped (write #PF in
#                       VirtIOPCI::reset_device; x86-mm territory)
#   QEMU=qemu-system-x86_64
#   NOBUILD=1            skip the kernel build step
set -euo pipefail

cd "$(dirname "$0")/.."

MODE="${1:-console}"
ROOTFS="${ROOTFS:-work/ubuntu-x86.img}"
SMP="${SMP:-1}"
MEM="${MEM:-2G}"
QEMU="${QEMU:-qemu-system-x86_64}"
KERNEL_ELF="target/x86_64-unknown-none/debug/rux"
BZIMAGE="target/x86_64-unknown-none/debug/bzImage"

if [ ! -f "$ROOTFS" ]; then
    echo "rootfs image missing: $ROOTFS" >&2
    echo "build it with: bash test/ubuntu-x86/build-img.sh" >&2
    exit 1
fi

if [ "${NOBUILD:-0}" != "1" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    cargo build --target x86_64-unknown-none --no-default-features --features x86_64
    python3 build/mkbzimage.py "$KERNEL_ELF" "$BZIMAGE"
fi

if [ ! -f "$BZIMAGE" ]; then
    echo "kernel not built: $BZIMAGE" >&2
    exit 1
fi

EXTRA=()
DISPLAY_ARGS=(-display none)
SERIAL_ARGS=(-nographic -serial mon:stdio)
case "$MODE" in
    gui)
        # Plain `-serial stdio` (NOT mon:stdio): the chardev mux would
        # swallow Ctrl-A as its monitor-escape key, but udesk uses Ctrl-A
        # for About — same reason as the riscv64 ubuntu-run target.
        DISPLAY_ARGS=(-display sdl)
        SERIAL_ARGS=(-serial stdio -monitor none)
        ;;
    debug)
        EXTRA=(-S -s)
        echo "QEMU stopped at entry; attach with: gdb ${KERNEL_ELF} -ex 'target remote :1234'"
        ;;
    console) ;;
    *)
        echo "unknown mode: $MODE (console|gui|debug)" >&2
        exit 1
        ;;
esac

# Shared-discipline boot: -snapshot (writes go to a temp overlay) and
# file.locking=off so concurrent agents can boot the same image.
#
# virtio-gpu xres/yres pinned to the riscv64 default (1024x768, config
# FB_DEFAULT_WIDTH/HEIGHT) so headless runs get the same pmode the riscv
# desktop expects. virtio-net-pci is claimed by the kernel's PCI net driver
# (eth0: firmware BARs + shared PIC INTx dispatch for completions);
# keyboard and tablet are polled through evdev like on riscv64.
exec "$QEMU" \
    -M q35 -m "$MEM" -smp "$SMP" \
    -accel tcg,thread=single \
    -cpu max \
    -snapshot \
    "${DISPLAY_ARGS[@]}" \
    "${SERIAL_ARGS[@]}" \
    "${EXTRA[@]}" \
    -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -netdev user,id=net0 \
    -device virtio-net-pci,disable-legacy=on,netdev=net0 \
    -device virtio-gpu-pci,xres=1024,yres=768 \
    -device virtio-keyboard-pci,disable-legacy=on \
    -device virtio-tablet-pci,disable-legacy=on \
    -kernel "$BZIMAGE" \
    -append "root=/dev/vda rw init=/sbin/init console=ttyS0"
