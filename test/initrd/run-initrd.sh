#!/bin/bash
# Rux initrd boot test (OH Phase 1 prereq).
#
# Boots the kernel with -initrd and root=/dev/ram0 (NO disk at all):
#   FDT /chosen initrd range -> memblock reserve -> gzip+cpio unpack into
#   the ramfs root -> /init runs from the initrd content.
#
# Success markers in the serial log:
#   "initrd" reserve+unpack status rows, "root=/dev/ram0 — initrd ramfs
#   root", "INITTEST: /init from initrd is running", the verbatim cmdline
#   dump (incl. ohos.* tokens), node checks, and dfx=taskdump,periodic rows.
#
# Resource discipline: 1 QEMU, no disk (nothing to snapshot), killed by
# its own `timeout` — no pkill.

set -u

SCRIPT_DIR="$(cd "$(dirname "$BASH_SOURCE")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
KERNEL="$PROJECT_ROOT/target/riscv64gc-unknown-none-elf/debug/rux"
INITRD="${1:-$SCRIPT_DIR/initrd.cpio.gz}"
LOG="$SCRIPT_DIR/initrd-boot.log"
DURATION="${2:-60}"

if [ ! -f "$KERNEL" ]; then
    echo "kernel missing: $KERNEL (run: cargo build --target riscv64gc-unknown-none-elf --features riscv64)" >&2
    exit 1
fi
if [ ! -f "$INITRD" ]; then
    echo "initrd missing: $INITRD (run: python3 test/initrd/mkinitrd.py)" >&2
    exit 1
fi

CMDLINE="root=/dev/ram0 rw init=/init console=ttyS0,115200 dfx=taskdump,periodic ohos.boot.hardware=virt debug_boot_device=a"

echo "booting initrd test (log: $LOG, ${DURATION}s)..."
timeout --signal=TERM --kill-after=5 "$DURATION" qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 2 \
    -nographic \
    -serial stdio -monitor none \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "$CMDLINE" \
    > "$LOG" 2>&1
rc=$?

echo "--- exit $rc; markers ---"
for m in \
    "INITTEST: /init from initrd is running" \
    "INITTEST: cmdline: root=/dev/ram0" \
    "ohos.boot.hardware=virt" \
    "INITTEST: readlink /bin/lnk -> hello" \
    ; do
    if grep -qF "$m" "$LOG"; then
        echo "PASS: $m"
    else
        echo "FAIL: $m"
    fi
done

grep -E "INITTEST: (stat|lstat)" "$LOG" || true
echo "--- kernel initrd rows ---"
grep -E "^initrd:|^fs:.*(initrd|ramfs|ram0)" "$LOG" | head -10 || true
grep -E "init: trying" "$LOG" | head -5 || true
echo "--- taskdump smoke ---"
grep -c "DFX TASK DUMP" "$LOG" || true
