#!/bin/bash
# ipc-rv-smoke.sh: riscv64 regression smoke for the perf-ipc tree
# (wake dispatch + sleep-gated wakeup preemption). Minimal rootfs boot,
# scripted serial (cd /proc + ls + sync), gate = prompt seen, commands
# echoed, zero panics.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERIAL="$ROOT/run/ipc-rv-smoke-serial.log"
rm -f "$SERIAL"
( sleep 25; echo "cd /proc"; sleep 4; echo "ls | head -4"; sleep 4; echo "sync"; sleep 3 ) \
  | timeout --signal=TERM --kill-after=5 150s qemu-system-riscv64 \
    -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
    -display none \
    -serial stdio -monitor none \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -drive file="$ROOT/test/rootfs.img",if=virtio,format=raw,file.locking=off,snapshot=on \
    -append "root=/dev/vda rw console=ttyS0 init=/bin/sh" > "$SERIAL" 2>&1 || true
echo "panics: $(grep -cE 'KERNPANIC|ALLOCTHROW|panic' "$SERIAL" || true)"
echo "prompt: $(grep -cE '/ \#|mrsh' "$SERIAL" || true)"
echo "cmd-echo: $(grep -c 'cd /proc' "$SERIAL" || true)"
echo "proc-ls: $(grep -c 'sys\|cpuinfo\|meminfo' "$SERIAL" || true)"
tail -5 "$SERIAL" | cut -c1-120
