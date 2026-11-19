#!/usr/bin/env bash
# R56-heal gate 2a: ftest01 (LTP idx 1422) on the r56-heal kernel.
# Own QEMU, own pid file, kills only the PID it started (cmdline-checked).
# Pass: "[TO] ftest01" + SWEEP-DONE + zero D-state hangs.
set -u
R=/home/william/rux-agents/r56-heal/run
K=/home/william/rux-agents/r56-heal/target/riscv64gc-unknown-none-elf/debug/rux
IDX=1422
TAG=${1:?tag}
SECS=${2:-150}
IMG=/home/william/rux-agents/ltp-scan/.ltp/ltp2.img
rm -f $R/ser-$TAG.sock $R/serial-$TAG.log
setsid /usr/local/bin/qemu-system-riscv64 -M virt -accel tcg,thread=single -cpu rv64 \
  -m 1G -smp 2 -snapshot -display none \
  -serial unix:$R/ser-$TAG.sock,server,nowait -monitor none \
  -drive file=$IMG,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel $K \
  -append "root=/dev/vda rw init=/sbin/runit console=ttyS0 ltpchunk=$IDX ltpstart=$IDX ltpcount=1" \
  >/dev/null 2>&1 &
echo $! > $R/qemu-$TAG.pid
sleep 2
python3 /home/william/rux-agents/r56-race/run/runner.py $R/ser-$TAG.sock $R/serial-$TAG.log $SECS
if [ -f $R/qemu-$TAG.pid ]; then
  PID=$(cat $R/qemu-$TAG.pid)
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    CMD=$(tr '\0' ' ' < /proc/$PID/cmdline 2>/dev/null)
    case "$CMD" in
      *qemu-system-riscv64*$R*) kill "$PID" && rm -f $R/qemu-$TAG.pid ;;
      *) echo "PID $PID cmdline mismatch, not killing" ;;
    esac
  fi
fi
echo "=== ftest01 ($TAG) ==="
grep -aE "ftest01|TPass|TFAIL|TBROK|TWARN|SWEEP-DONE|SIGDEATH|blocked for more than|D-state|uninterruptible" $R/serial-$TAG.log | tail -20
