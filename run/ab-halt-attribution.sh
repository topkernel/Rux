#!/bin/bash
# ab-halt-attribution.sh — three-arm interleaved A/B to attribute the
# sync-realign ctx regression: A=pre-realign (ab397dfd, no poll window),
# B=current (w=120us), C=current (w=0, forced halt). Round-robin across
# arms so every arm sees the same load windows. Collects the measurement
# line AND the EXP-RESCHED IPI counter line from each boot.
cd /home/william/rux-agents/code-review-perf
IMG=work/lmbench-x86.img
A=/tmp/bzImage-prerealign
B=/tmp/bzImage-cur-w120
C=/tmp/bzImage-cur-w0
run_one() { # kernel tag spec timeout
  local log=/tmp/attr-$2-$RANDOM.log
  timeout "$4" qemu-system-x86_64 -M q35 -accel kvm -cpu host -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none -kernel "$1" -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$3 console=ttyS0" -drive file=$IMG,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=rootfs >/dev/null 2>&1
  local v c
  v=$(tr -d '\r' < "$log" | grep -aE "Simple|Pipe latency|^[[:space:]]*2[[:space:]]+[0-9]" | grep -avE "POSIX|v0\.1\.0|size=" | tail -1)
  c=$(tr -d '\r' < "$log" | grep -a "EXP-RESCHED" | tail -1)
  echo "[$2] ${v:-TIMEOUT} | ${c:-no-count}"
  rm -f "$log"
}
for i in 1 2 3; do
  for spec in "lat_ctx,-s,64,2" "lat_pipe,"; do
    run_one "$A" preA "$spec" 90
    run_one "$B" curB "$spec" 90
    run_one "$C" w0C "$spec" 90
  done
done
echo BATTERY-DONE
