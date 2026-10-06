#!/bin/bash
# Rux x86_64 boot (QEMU q35, multiboot1 ELF via -kernel).
# Usage: ./test/run-x86.sh [console|debug]
#   console - run until timeout/panic (default 90s)
#   debug   - run with -S -s and print the GDB attach line
MODE="${1:-console}"
KERNEL=target/x86_64-unknown-none/debug/rux

if [ ! -f "$KERNEL" ]; then
    echo "kernel not built: run 'make build-x86' first" >&2
    exit 1
fi

EXTRA=""
if [ "$MODE" = "debug" ]; then
    EXTRA="-S -s"
    echo "QEMU stopped at entry; attach with: gdb ${KERNEL} -ex 'target remote :1234'"
fi

exec qemu-system-x86_64 \
    -M q35 -m 2G -smp 1 \
    -accel tcg,thread=single \
    -cpu max \
    -nographic -serial mon:stdio \
    $EXTRA \
    -kernel "$KERNEL"
