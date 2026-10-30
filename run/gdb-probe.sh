#!/usr/bin/env bash
# gdb-park runner: boot the aarch64 kernel under qemu -gdb -S, run a gdb
# script, kill exactly the qemu we started.
#   usage: gdb-probe.sh <kernel> <image> <gdb-script> <out-log> [qsecs] [gsecs]
set -u
KERNEL="$1"; IMG="$2"; SCRIPT="$3"; OUT="$4"
QSECS="${5:-260}"; GSECS="${6:-240}"
PORT=1235

for p in $(pgrep -f "qemu-system-aarch64.*gdb tcp::${PORT}"); do
    [ "$p" != "$$" ] && kill "$p" 2>/dev/null
done
sleep 1

timeout -k 5 "${QSECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=single \
    -m 1G -smp 1 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file="${IMG}",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs \
    -gdb tcp::${PORT} -S > /tmp/gdb-probe-serial.log 2>&1 &
QPID=$!
sleep 2
timeout -k 5 "${GSECS}" gdb-multiarch -batch -x "${SCRIPT}" > "${OUT}" 2>&1
kill "${QPID}" 2>/dev/null
wait "${QPID}" 2>/dev/null
exit 0
