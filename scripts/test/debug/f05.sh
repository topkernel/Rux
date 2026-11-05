#!/usr/bin/env bash
# R56-heal gate 2b: f05rep2 byte-exact verify on the r56-heal kernel.
# Usage: f05.sh <tag> <seconds> [accel] [smp]
# Own QEMU, own pid file, kills only the PID it started (cmdline-checked).
# Pass: zero FAIL lines (MODE-A completion may be throughput-limited).
set -u
R=/home/william/rux-agents/r56-heal/run
TAG=${1:?tag}
SECS=${2:-150}
ACCEL=${3:-tcg,thread=single}
SMP=${4:-2}
K=/home/william/rux-agents/r56-heal/target/riscv64gc-unknown-none-elf/debug/rux
IMG=/home/william/rux-agents/virtio-wake/run/f05vw.img
rm -f $R/ser-$TAG.sock $R/serial-$TAG.log
setsid /usr/local/bin/qemu-system-riscv64 -M virt -accel $ACCEL -cpu rv64 \
  -m 1G -smp $SMP -snapshot -display none \
  -serial unix:$R/ser-$TAG.sock,server,nowait -monitor none \
  -drive file=$IMG,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel $K \
  -append "root=/dev/vda rw console=ttyS0 init=/root/finit p12=cmd:/f05rep2" \
  >/dev/null 2>&1 &
echo $! > $R/qemu-$TAG.pid
sleep 2
python3 /home/william/rux-agents/r56-race/run/runner.py $R/ser-$TAG.sock $R/serial-$TAG.log $SECS
if [ -f $R/qemu-$TAG.pid ]; then
  PID=$(cat $R/qemu-$TAG.pid)
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    CMD=$(tr '\0' ' ' < /proc/$PID/cmdline 2>/dev/null)
    case "$CMD" in
      *qemu-system-riscv64*$R*) kill "$PID" 2>/dev/null; rm -f $R/qemu-$TAG.pid ;;
      *) echo "REFUSED to kill pid=$PID (cmdline mismatch)" ;;
    esac
  fi
fi
echo "=== f05rep2 results ($TAG) ==="
grep -acE "^FAIL |FAIL\(|FAIL R[0-9]" $R/serial-$TAG.log | sed 's/^/FAIL lines: /'
grep -aE "MODE-|F05REP-DONE|PROBE-START|R56-" $R/serial-$TAG.log | tail -12
