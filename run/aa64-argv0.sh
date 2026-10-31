#!/usr/bin/env bash
# aa64-tail task-3 probe: kernel-started init argv[0] must be the
# CANDIDATE path tried (POSIX: the kernel must not rewrite the caller's
# argv[0] to the resolved target). Boots the shared image and feeds the
# PID-1 shell two probes over serial:
#   echo ZERO:$0            — the shell's live argv[0]
#   cat /proc/1/cmdline     — the mm arg-bounds record + aarch64 walker
# Resource discipline: -smp 2 -m 1G, one QEMU, snapshot+locking-off.
set -u
RUN=/home/william/rux-agents/aa64-tail/run
KERNEL="${1:-/home/william/rux-agents/aa64-tail/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-$RUN/argv0-serial.log}"
SECS="${3:-75}"

( sleep 28
  printf 'echo ZERO_BEGIN:$0:ZERO_END\n'
  sleep 4
  printf 'cat /proc/1/cmdline; echo CMDLINE_DONE\n'
  sleep 4
  printf 'cat /proc/1/cmdline | tr "\\000" "@" ; echo TR_DONE\n'
  sleep 6
) | timeout -k 5 "${SECS}" qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "${KERNEL}" \
    -drive file=$RUN/a64-rootfs.img,if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "${OUT}" 2>&1
rc=$?
echo "qemu rc=${rc} (124 = timeout, expected)"
echo "--- argv0 probe output ---"
grep -aE "ZERO_|CMDLINE_DONE|TR_DONE|bin/sh|panic" "${OUT}" | head -10
echo "--- serial tail ---"
tail -6 "${OUT}"
