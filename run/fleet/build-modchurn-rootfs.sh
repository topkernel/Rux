#!/bin/bash
# build-modchurn-rootfs.sh — regenerate images/fleet-modchurn-rootfs.img
# (session 9, modchurn member). Reflink of the dirtyscan rootfs (same
# tree: toybox + ld-musl + probes, 1G ext4 + 256MB unformatted swap
# tail the kernel carves swap from) + debugfs injection of the dynamic
# modchurn binary, its churned .so, and the sf-name dynamic linker
# (the musl install under devfs-grow is soft-float; its Scrt1.o bakes
# interp /lib/ld-musl-riscv64-sf.so.1 while mkrootfs installs the same
# libc under the plain name — both names must exist in the image).
set -euo pipefail
cd "$(dirname "$0")/../.."
T=test
GCC=run/fleet/bin/riscv64-musl-gcc
IMG=run/fleet/images/fleet-modchurn-rootfs.img
MUSLLIB=/home/william/rux-agents/devfs-grow/toolchain/riscv64-rux-linux-musl/lib/libc.so

$GCC -shared -fPIC -O1 -o /tmp/libmodchurn.so $T/modchurn_lib.c
$GCC -O1 -o /tmp/modchurn $T/modchurn.c -ldl

cp -f --reflink=auto run/fleet/images/fleet-dirtyscan-rootfs.img "$IMG"
debugfs -w -R "write /tmp/modchurn /test/modchurn" "$IMG" >/dev/null 2>&1
debugfs -w -R "write /tmp/libmodchurn.so /test/libmodchurn.so" "$IMG" >/dev/null 2>&1
debugfs -w -R "write $MUSLLIB /lib/ld-musl-riscv64-sf.so.1" "$IMG" >/dev/null 2>&1
debugfs -w -R "sif /test/modchurn mode 0100755" "$IMG" >/dev/null 2>&1
debugfs -w -R "sif /test/libmodchurn.so mode 0100644" "$IMG" >/dev/null 2>&1
debugfs -w -R "sif /lib/ld-musl-riscv64-sf.so.1 mode 0100755" "$IMG" >/dev/null 2>&1
echo "rebuilt $IMG:"
debugfs -R "ls -l /test" "$IMG" 2>/dev/null | tail -4
