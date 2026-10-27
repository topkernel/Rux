#!/bin/bash
# run1.sh — boot the LTP rootfs under QEMU and run the tests named on the
# command line (comma-separated) via the ltpscan PID-1 runner.
#
# Usage: test/ltpscan/run1.sh [-t SECONDS] [-x "extra boot args"] test1,test2,...
# Logs to .ltp/diag/<test1>.log
set -euo pipefail
cd "$(dirname "$0")/../.."

TESTS="${@: -1}"
TIMEOUT=120
EXTRA=""
while getopts "t:x:" o; do
  case "$o" in
    t) TIMEOUT=$OPTARG ;;
    x) EXTRA=$OPTARG ;;
    *) true ;;
  esac
done
# Last non-option arg is the test list
for a in "$@"; do TESTS="$a"; done

FIRST="${TESTS%%,*}"
LOG=".ltp/diag/${FIRST}.log"
mkdir -p .ltp/diag

# IMAGE override: set IMAGE=.ltp/rootfs-XXX.img to run against a private
# copy instead of test/rootfs.img (mandatory while a scan is driving the
# shared image — both writers mutate /test/linux-ltp/chunk.txt).
# SNAP=off additionally drops -snapshot so guest writes persist in $IMAGE
# (post-mortem host-side inspection via debugfs).
IMAGE="${IMAGE:-test/rootfs.img}"
SNAP_ARGS="-snapshot"
if [ "${SNAP:-on}" = "off" ]; then SNAP_ARGS=""; fi

TMP=$(mktemp /tmp/chunk-XXXX.txt)
echo "$TESTS" | tr ',' '\n' > "$TMP"
DBFS=$(mktemp /tmp/dbfs-XXXX)
printf 'cd /test/linux-ltp\nrm chunk.txt\nwrite %s chunk.txt\n' "$TMP" > "$DBFS"
debugfs -w -f "$DBFS" "$IMAGE" 2>&1 | grep -v "^debugfs" | head -2
rm -f "$TMP" "$DBFS"

timeout "$TIMEOUT" qemu-system-riscv64 -M virt -accel tcg,thread=single \
  -cpu rv64,zbb=true,zba=true,zbs=true -m 2G -smp 2 -nographic $SNAP_ARGS \
  -serial mon:stdio \
  -drive file="$IMAGE",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux \
  -append "root=/dev/vda rw init=/test/ltpscan console=ttyS0 $EXTRA" \
  > "$LOG" 2>&1 || true
echo "=== log: $LOG (tail) ==="
grep -E "^RES\||ALLOCTHROW|blocked for more|DEADLOCK|panic" "$LOG" | tail -10 | cut -c1-180
