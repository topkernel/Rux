#!/bin/bash
# 运行内核（带 rootfs）

qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -smp 4 \
    -nographic \
    -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
    -device virtio-blk-device,drive=rootfs \
    -kernel target/riscv64gc-unknown-none-elf/debug/rux \
    -append "root=/dev/vda rw init=/bin/sh"
