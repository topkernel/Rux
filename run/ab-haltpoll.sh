#!/bin/bash
# ab-haltpoll.sh — interleaved same-window A/B boots for the handoff
# family + null sanity. Interleaving (base,new,base,new,...) cancels
# host-load drift, which moves this family ~15% between load 1 and
# load 5. Usage: BASE=/path/new NEW=/path/new ./run/ab-haltpoll.sh
# (defaults are the 2026-11-04 idle-poll experiment pair).
cd /home/william/rux-agents/code-review-perf
IMG=work/lmbench-x86.img
NEW=/tmp/bzImage-haltpoll5
BASE=/tmp/bzImage-pre-haltpoll
run_one() { # kernel tag spec timeout
  local log=/tmp/ab2-$2-$RANDOM.log
  timeout "$4" qemu-system-x86_64 -M q35 -accel kvm -cpu host -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none -kernel "$1" -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$3 console=ttyS0" -drive file=$IMG,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
  local v
  v=$(tr -d '\r' < "$log" | grep -aE "Simple|Pipe latency|Process|AF_UNIX|^[[:space:]]*2[[:space:]]+[0-9]" | grep -avE "POSIX|v0\.1\.0|size=" | tail -1)
  echo "[$2] ${v:-TIMEOUT}"
  rm -f "$log"
}
for spec in "lat_ctx,-s,64,2" "lat_pipe," "lat_proc,exec" "lat_unix" "lat_syscall,null"; do
  for i in 1 2; do
    run_one "$BASE" base "$spec" 90
    run_one "$NEW" new "$spec" 90
  done
done
for i in 1 2; do
  run_one "$BASE" base "lat_proc,fork" 180
  run_one "$NEW" new "lat_proc,fork" 180
done
echo BATTERY-DONE
