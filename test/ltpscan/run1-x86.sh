#!/bin/bash
# run1-x86.sh — boot the x86_64 LTP rootfs under QEMU (q35, KVM) and run the
# tests named on the command line (comma-separated) via the ltpscan PID-1
# runner. The x86_64 mirror of run1.sh.
#
# Usage: test/ltpscan/run1-x86.sh [-t SECONDS] [-x "extra boot args"] t1,t2,...
# Logs to .ltp-x86/diag/<test1>.log
set -euo pipefail
cd "$(dirname "$0")/../.."

TESTS="${@: -1}"
TIMEOUT=120
EXTRA=""
ACCEL="${ACCEL:-kvm}"
while getopts "t:x:" o; do
  case "$o" in
    t) TIMEOUT=$OPTARG ;;
    x) EXTRA=$OPTARG ;;
    *) true ;;
  esac
done
for a in "$@"; do TESTS="$a"; done

FIRST="${TESTS%%,*}"
OUTDIR="${LTP_OUT_DIR:-.ltp-x86}"
LOG="${OUTDIR}/diag/${FIRST}.log"
mkdir -p "${OUTDIR}/diag"

IMAGE="${IMAGE:-test/rootfs-x86_64-ltp.img}"
KERNEL="${KERNEL:-target/x86_64-unknown-none/debug/bzImage}"

TMP=$(mktemp /tmp/chunk-XXXX.txt)
echo "$TESTS" | tr ',' '\n' > "$TMP"
DBFS=$(mktemp /tmp/dbfs-XXXX)
printf 'cd /test/linux-ltp\nrm chunk.txt\nwrite %s chunk.txt\n' "$TMP" > "$DBFS"
debugfs -w -f "$DBFS" "$IMAGE" 2>&1 | grep -v "^debugfs" | head -2
rm -f "$TMP" "$DBFS"

timeout "$TIMEOUT" qemu-system-x86_64 -M q35 -accel "$ACCEL" -cpu max \
  -m 2G -smp 2 -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file="$IMAGE",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel "$KERNEL" \
  -append "root=/dev/vda rw init=/test/ltpscan console=ttyS0 $EXTRA" \
  > "$LOG" 2>&1 || true
echo "=== log: $LOG (tail) ==="
grep -E "^RES\||ALLOCTHROW|blocked for more|DEADLOCK|panic" "$LOG" | tail -10 | cut -c1-180
