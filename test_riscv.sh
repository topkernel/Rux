#!/bin/bash
timeout 10 qemu-system-riscv64 -machine virt -cpu rv64 -smp 1 -m 2G -nographic \
  -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux \
  -serial mon:stdio 2>&1 | head -150
