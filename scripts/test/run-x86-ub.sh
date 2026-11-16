#!/bin/bash
# run-x86-ub.sh — boot the Ubuntu amd64 image on the x86_64 kernel (q35).
#
# The x86 mirror of the riscv64 `make ubuntu-run` flow: same rootfs layout
# (work/ubuntu-x86.img, /sbin/init -> udesk-init), same device set
# (virtio-blk rootfs + virtio-gpu + virtio-keyboard/tablet + virtio-net),
# serial console on ttyS0.
#
# Usage: ./scripts/test/run-x86-ub.sh [console|gui|debug]
#   console - headless serial run, exits at kernel stop (default)
#   gui     - native SDL window + serial in this terminal (login: root/rux)
#   debug   - like console but -S -s (GDB attach line printed)
#
# Environment overrides:
#   ROOTFS=path/to/img   image to boot (default work/ubuntu-x86.img)
#   SMP=8                CPUs (unified rig 2026-11-14: KVM SMP8 8G)
#   MEM=8G               guest RAM (unified rig). The old 2G cap and the
#                       ">=3G PCI-hole BAR" warning are STALE: the hole
#                       0xc0000000..4GB is megapage-mapped (mmu_init.rs
#                       setup_device_mappings); verified booting 8G SMP8
#                       KVM to the GNOME stage with zero virtio faults.
#   QEMU=qemu-system-x86_64
#   PROFILE=debug|release
#                       which cargo profile the booted kernel comes from
#                       (default debug). `make ubuntu-run` builds the
#                       RELEASE kernel itself and passes the release
#                       paths in explicitly (KERNEL_ELF/BZIMAGE).
#   KERNEL_ELF / BZIMAGE
#                       explicit kernel paths (override PROFILE)
#   NOBUILD=1            skip the kernel build step
# The script prints the identity of the kernel it boots (path, size,
# mtime) and warns when that binary is OLDER than the kernel sources —
# a stale bzImage is indistinguishable from a fresh one at runtime, and
# a boot that "still shows the old bug" is usually exactly that (ttydev
# lane, 2026-11-16: an x86 bzImage built before the uevent/netlink merge
# was booted with the post-merge tree checked out, and the
# dev-ttyS0.device timeout was blamed on the tree).
#   KERNEL_ELF=path      kernel ELF to boot (default: debug build)
#   BZIMAGE=path         bzImage to boot (default: debug build)
#                       Both default to the debug profile; point them at
#                       target/x86_64-unknown-none/release/... to boot a
#                       RELEASE kernel (the user-facing profile) without
#                       touching this script.set -euo pipefail

cd "$(dirname "$0")"
WT="$PWD"
while [ ! -f "$WT/Cargo.toml" ] || [ ! -d "$WT/kernel" ]; do
    WT="$(dirname "$WT")"
done
cd "$WT"

MODE="${1:-console}"
ROOTFS="${ROOTFS:-work/ubuntu-x86.img}"
SMP="${SMP:-8}"
MEM="${MEM:-8G}"
QEMU="${QEMU:-qemu-system-x86_64}"
PROFILE="${PROFILE:-debug}"
KERNEL_ELF="${KERNEL_ELF:-target/x86_64-unknown-none/$PROFILE/rux}"
BZIMAGE="${BZIMAGE:-target/x86_64-unknown-none/$PROFILE/bzImage}"

if [ ! -f "$ROOTFS" ]; then
    echo "rootfs image missing: $ROOTFS" >&2
    echo "build it with: bash scripts/test/ubuntu-x86/build-img.sh" >&2
    exit 1
fi

if [ "${NOBUILD:-0}" != "1" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    if [ "$PROFILE" = "release" ]; then
        # The Makefile target owns the release profile (it also runs the
        # symbol pass); reuse it instead of rebuilding here.
        make -C build PLATFORM=x86_64 build-release
    else
        cargo build --target x86_64-unknown-none --no-default-features --features x86_64
    fi
    python3 build/mkbzimage.py "$KERNEL_ELF" "$BZIMAGE"
fi

if [ ! -f "$BZIMAGE" ]; then
    echo "kernel not built: $BZIMAGE" >&2
    exit 1
fi

echo "kernel: $BZIMAGE ($(stat -c '%s bytes, built %y' "$BZIMAGE"))"
if [ -n "$(find "$WT/kernel/src" "$WT/kernel/build.rs" "$WT/Cargo.toml" \
            -newer "$BZIMAGE" -print -quit 2>/dev/null)" ]; then
    echo "WARNING: $BZIMAGE is OLDER than the kernel sources — this is a" >&2
    echo "         STALE kernel binary; the boot will not show the current" >&2
    echo "         tree's behaviour. Rebuild it (make build-x86-release)." >&2
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
# virtio-gpu xres/yres pinned to 1280x800 — the pmode the riscv64 flow
# probes (QEMU's default on `virt`), so both arches present the same
# scanout to udesk. virtio-net-pci is claimed by the kernel's PCI net
# driver (eth0: firmware BARs + shared PIC INTx dispatch for completions);
# keyboard and tablet are polled through evdev like on riscv64.
exec "$QEMU" \
    -M q35 -m "$MEM" -smp "$SMP" \
    -accel "${ACCEL:-kvm}" \
    -cpu max \
    -snapshot \
    "${DISPLAY_ARGS[@]}" \
    "${SERIAL_ARGS[@]}" \
    "${EXTRA[@]}" \
    -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -netdev user,id=net0 \
    -device virtio-net-pci,disable-legacy=on,netdev=net0 \
    -device virtio-gpu-pci,xres=1280,yres=800 \
    -device virtio-keyboard-pci,disable-legacy=on \
    -device virtio-tablet-pci,disable-legacy=on \
    -kernel "$BZIMAGE" \
    -append "root=/dev/vda rw init=${INIT:-/sbin/init} console=ttyS0"
