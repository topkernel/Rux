#!/bin/bash
# riscv64 regression boot: minimal rootfs, scripted serial (cd /proc then halt)
WT=/home/william/rux-agents/lmbench-fix
LOG=$WT/run/rv-boot.log
timeout 90 qemu-system-riscv64 -M virt -accel tcg,thread=multi -cpu rv64 -m 1G -smp 2 \
  -nographic -serial mon:stdio -monitor none \
  -kernel $WT/target/riscv64gc-unknown-none-elf/debug/rux \
  -append "root=/dev/vda rw console=ttyS0" \
  -drive file=$WT/test/rootfs.img,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
  -device virtio-blk-pci,drive=rootfs > $LOG 2>&1 <<'IN'
cd /proc
ls
poweroff
IN
echo "rc=$?"
grep -acE "panic|Oops" $LOG
