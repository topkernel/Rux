#!/bin/bash
# hunt-wedge.sh — Rux kernel wedge/deadlock现场捕获工具 (DFX)
#
# 用途：复现并捕获两类偶发系统级挂起的全量现场（QEMU monitor 寄存器+栈）：
#   A 型：自旋锁死锁 —— guest 打印 "DEADLOCK: spinlock stuck ... holder=N"
#   B 型：静默挂起 —— 管道命令 echo PP | cat 无输出持续 >30s（CPU idle、任务沉睡）
#
# 依赖内核侧 DFX 特性：
#   编译期 feature  dfx-lock-owner   —— 死锁打印含 holder=<cpu>
#   运行时开关      dfx=watchdog     —— boot 参数，DEADLOCK 后自动 dump 全任务状态
#
# 用法：
#   ./test/hunt-wedge.sh [max_rounds]     # 默认 6 轮，任一轮捕获即停
#
# 产物（/tmp/rux-hunt/ 下）：
#   hunt.log   —— guest 串口完整输出（判型依据）
#   hunt.dump  —— QEMU monitor dump：info cpus + 每CPU寄存器/栈字/pc反汇编
#   kernel.elf —— 捕获当轮的内核 ELF 副本（供 addr2line 符号化 dump 中地址）
set -u
cd "$(dirname "$0")/.."

ROUNDS=${1:-6}
WORK=/tmp/rux-hunt
mkdir -p $WORK

# Build with the diagnostics feature (two extra atomic stores per lock).
echo "[hunt] building kernel with dfx-lock-owner ..."
cargo build --target riscv64gc-unknown-none-elf --features riscv64,dfx-lock-owner || exit 1
KRN=$WORK/kernel.elf
RFS=test/rootfs.img
# Snapshot the diagnostic build ONCE: later `cargo build` runs (e.g. a
# parallel gate build without the feature) must never swap the kernel
# under a hunt round mid-flight.
cp target/riscv64gc-unknown-none-elf/debug/rux $KRN

for round in $(seq 1 $ROUNDS); do
  LOG=$WORK/hunt.log; DUMP=$WORK/hunt.dump; MON=$WORK/qmon.sock
  rm -f $MON $LOG $DUMP

  qemu-system-riscv64 -M virt -accel tcg,thread=single -cpu rv64 -m 2G -smp 4 \
    -nographic -serial mon:stdio \
    -monitor unix:$MON,server,nowait \
    -drive file=$RFS,if=none,id=rootfs,format=raw \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel $KRN \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0 dfx=watchdog" \
    > $LOG 2>&1 <<IN &
sleep 2
/test/smoke_test
sleep 42
/test/nettest
sleep 75
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 10
echo PP | cat
sleep 20
poweroff -f
sleep 8
IN
  QPID=$!

  FORM=none
  for i in $(seq 1 100); do
    if grep -aq "DEADLOCK" $LOG 2>/dev/null; then FORM=A; break; fi
    PIPES=$(grep -ac "echo PP | cat" $LOG 2>/dev/null || echo 0)
    PPS=$(grep -ac "^PP" $LOG 2>/dev/null || echo 0)
    # B: a pipe command ran but produced no output for 30s
    if [ "${PIPES:-0}" -gt "${PPS:-0}" ] && [ "$i" -ge 15 ]; then FORM=B; break; fi
    kill -0 $QPID 2>/dev/null || break
    sleep 2
  done

  if [ "$FORM" != "none" ]; then
    echo "form=$FORM at t=$((i*2))s round=$round" >> $DUMP
    if [ -S $MON ]; then
      python3 - "$MON" "$DUMP" <<'PYEOF'
import socket, sys, time
mon, dump = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(mon); time.sleep(0.3)
def cmd(c):
    s.sendall((c + "\n").encode()); time.sleep(0.5)
    try: d = s.recv(262144).decode(errors="replace")
    except Exception: d = ""
    open(dump, "a").write(f"\n===== {c} =====\n{d}\n")
cmd("info cpus")
for cpu in range(4):
    cmd(f"cpu {cpu}")
    cmd("info registers")
    cmd("x/48gx $sp")
    cmd("x/8i $pc-16")
s.close()
PYEOF
    fi
    cp $KRN $WORK/kernel.elf
    echo "[hunt] round $round CAPTURED form=$FORM — see $DUMP / $LOG"
    kill -9 $QPID 2>/dev/null; wait $QPID 2>/dev/null
    exit 0
  fi
  echo "[hunt] round $round: clean (rc=$(wait $QPID 2>/dev/null; echo $?))"
done
echo "[hunt] no wedge in $ROUNDS rounds"
exit 2
