#!/bin/bash
# r16: GB flamegraph official run — sampler=1, quiet window (load<3).
WT=/home/william/rux-agents/perf-gap-hunt
LOG="$WT/run/r16-gb-flame.log"
: > "$LOG"
qemu-system-x86_64 -M q35 -accel kvm -cpu host -smp 4 -m 4G \
    -display none -vga none -serial "file:$LOG" -monitor none -snapshot \
    -kernel "$WT/work/bzImage-r16head" \
    -append "root=/dev/vda rw init=/test/geekbench_run geekbench=cpu gtimeout=1200 sampler=1 console=ttyS0" \
    -drive file="$WT/work/geekbench-x86.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 &
QPID=$!
echo "$QPID" > "$WT/run/r16-gb-flame.pid"
for i in $(seq 1 2400); do
    sleep 5
    grep -aq "GBSAMP done" "$LOG" 2>/dev/null && break
    grep -aqE "KERNEL PANIC" "$LOG" 2>/dev/null && break
    kill -0 $QPID 2>/dev/null || break
done
sleep 5
kill $QPID 2>/dev/null
echo "== flame run: DONE=$(grep -ac 'GEEKBENCH DONE' $LOG) SAMPLER=$(grep -ac 'GBSAMP start' $LOG) samples=$(grep -ac '^GBSAMP ' $LOG)"
