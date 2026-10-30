#!/usr/bin/env bash
# aarch64 signal-delivery smoke: boots the aa64-signal kernel against a
# COPY of the lane rootfs whose /sbin/init is run/sig-loop (raise/catch
# SIGUSR1 x200, restorer-less rt_sigaction -> kernel SIGTRAMP page).
# A self-raised signal delivers on the SVC-return path, so this does
# NOT depend on the (currently broken) IRQ lane.
# Image: a64-sigtest.img is rebuilt by `make -B` style re-inject below
# if missing (copy of the read-only shared a64-rootfs.img + debugfs
# swap of /sbin/init). Resource discipline: -smp 2 -m 1G, snapshot=on.
set -u
RUN=/home/william/rux-agents/aa64-signal/run
KERNEL="${1:-/home/william/rux-agents/aa64-signal/target/aarch64-unknown-none/debug/rux}"
INITBIN="${2:-$RUN/sig-loop}"          # or sig-loop-restorer (SA_RESTORER)
OUT="${3:-$RUN/aa64-sigtest-serial.log}"
SECS="${4:-40}"

if [ ! -f "$RUN/a64-sigtest.img" ]; then
    cp /home/william/rux-agents/aarch64-port/run/a64-rootfs.img "$RUN/a64-sigtest.img"
    debugfs -w -R "rm /sbin/init" "$RUN/a64-sigtest.img" >/dev/null 2>&1
    debugfs -w -R "write $INITBIN /sbin/init" "$RUN/a64-sigtest.img" >/dev/null 2>&1
    debugfs -w -R "sif /sbin/init mode 0100755" "$RUN/a64-sigtest.img" >/dev/null 2>&1
fi

timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file="$RUN/a64-sigtest.img",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout)"
echo "--- signal probe output ---"
grep -E "sig-loop|DEQUEUE|sigreturn|SEGV|panic|SIGSEGV|frames" "${OUT}" | head -20
echo "--- serial tail ---"
tail -8 "${OUT}"
