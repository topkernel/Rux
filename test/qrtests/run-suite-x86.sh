#!/bin/bash
# run-suite-x86.sh — one suite class per boot on the x86_64+KVM vehicle
# (qa-suites lane; the stress-port run-class-x86.sh rig, retargeted at
# the onboarded industry suites). PID 1 is /test/qa/qsrun, which runs
# the lines of /test/qa/cmds.txt sequentially with TERM/KILL guards
# (TIMEOUT = P0, house rule). The image is the lane's private copy at
# run/rootfs-qa.img (from the permanent store, injected once); only
# cmds.txt is rewritten per boot.
#
# Usage: test/qrtests/run-suite-x86.sh [-t QEMU_BUDGET] "<cmds line>"
#        test/qrtests/run-suite-x86.sh -f <cmds-file>
#   cmds line: "[budget-secs] <prog> [args...]"  (prog under /test/qa)
#   e.g. test/qrtests/run-suite-x86.sh "hackbench 20 process"
#        test/qrtests/run-suite-x86.sh "60 cyclictest -D 30 -i 1000 -m"
# Log: run/qa-runs/<class>.log
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT_DIR="$(pwd)"

IMAGE="${IMAGE:-run/rootfs-qa.img}"
KERNEL="${KERNEL:-target/x86_64-unknown-none/debug/bzImage}"
OUTDIR="${OUTDIR:-run/qa-runs}"
DFX="${DFX:-dfx=taskdump,watchdog}"
EXTRA="${EXTRA:-}"
SMP="${SMP:-2}"
MEM="${MEM:-2G}"
BUDGET="${BUDGET:-300}"
mkdir -p "$OUTDIR"

# ---- build + inject (always rewrite: binaries may have been rebuilt) ----
echo "inject: building rt-tests suite + qsrun into $IMAGE"
userspace/tests/rttests/build-x86.sh > "$OUTDIR/build.log" 2>&1 || {
    cat "$OUTDIR/build.log" >&2
    exit 1
}
toolchain/x86_64-rux-linux-musl/bin/musl-gcc -static -O2 -Wall \
    -o "$OUTDIR/qsrun" test/qrtests/qsrun.c 2>/dev/null
toolchain/x86_64-rux-linux-musl/bin/musl-gcc -static -O2 -Wall \
    -o "$OUTDIR/schedprobe" test/qrtests/schedprobe.c 2>/dev/null
BIN="$OUTDIR"            # qsrun/schedprobe built here
SUITE="$ROOT_DIR/run/rttests-bin"  # suite binaries (build-x86.sh output)
INJ=$(mktemp /tmp/qa-inj-XXXX)
{
    echo "cd /test"
    echo "mkdir qa"
    echo "cd qa"
    for f in hackbench cyclictest pi_stress; do
        echo "rm $f"
        echo "write $SUITE/$f $f"
    done
    for f in qsrun schedprobe; do
        echo "rm $f"
        echo "write $BIN/$f $f"
    done
} > "$INJ"
debugfs -w -f "$INJ" "$IMAGE" 2>&1 | grep -aE "not found|error|Bad|invalid" || true
rm -f "$INJ"

# ---- cmds.txt: single line from "$*", or a file of lines via -f ----
if [ "${1:-}" = "-t" ]; then
    BUDGET="$2"
    shift 2
fi
ARGS=""
CLASS=""
CMD=$(mktemp /tmp/qa-cmds-XXXX)
if [ "${1:-}" = "-f" ]; then
    cp "$2" "$CMD"
    CLASS="$(basename "$2" .cmds)"
    ARGS="(file: $2)"
else
    ARGS="$*"
    printf '%s\n' "$ARGS" > "$CMD"
    CLASS="$(printf '%s' "$ARGS" | awk '{print $1}')"
    [ "$CLASS" -eq "$CLASS" ] 2>/dev/null && CLASS="$(printf '%s' "$ARGS" | awk '{print $2}')"
fi
CLASS="${CLASS:-misc}"
INJ=$(mktemp /tmp/qa-inj2-XXXX)
printf 'cd /test/qa\nrm cmds.txt\nwrite %s cmds.txt\n' "$CMD" > "$INJ"
debugfs -w -f "$INJ" "$IMAGE" 2>&1 | grep -av "^debugfs" | head -2 || true
rm -f "$CMD" "$INJ"

LOG="$OUTDIR/${CLASS}.log"
echo "boot: class=$CLASS args=$ARGS dfx=$DFX extra=$EXTRA log=$LOG"
timeout "$BUDGET" qemu-system-x86_64 -M q35 -accel kvm -cpu max \
  -m "$MEM" -smp "$SMP" -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file="$IMAGE",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel "$KERNEL" \
  -append "root=/dev/vda rw init=/test/qa/qsrun console=ttyS0 $DFX $EXTRA" \
  > "$LOG" 2>&1 || true

echo "=== verdict input (protocol lines + failure markers) ==="
grep -aE "QSRUN|QS-RUN|RES\||QS-ALL-DONE|panic|KERNPANIC|DEADLOCK|blocked for more|watchdog|ALLOCTHROW" "$LOG" \
    | tail -20 | cut -c1-200
