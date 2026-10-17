#!/bin/bash
K=/home/william/rux-agents/sched-riscv/target/riscv64gc-unknown-none-elf/debug/rux
R=/home/william/rux-agents/sched-riscv/test/rootfs.img
qemu-system-riscv64 -M virt -accel tcg,thread=single \
  -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp 2 -nographic \
  -drive file=$R,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel $K -append "root=/dev/vda rw console=ttyS0" \
  -gdb tcp::1236 -S >/dev/null 2>&1 &
Q=$!
sleep 1
gdb-multiarch -q -batch \
  -ex "set pagination off" \
  -ex "file $K" \
  -ex "target remote :1236" \
  -ex "break *0xffffffff802f4908
  -ex "break *0xffffffff802f46c2"" \
  -ex "continue" \
  -ex "printf \"=== PANIC CAUGHT pc=%p ra=%p sp=%p\\n\", \$pc, \$ra, \$sp" \
  -ex "bt 12" \
  -ex "x/40gx \$sp" \
  -ex "info registers ra s0 sp" 2>&1
kill $Q 2>/dev/null
