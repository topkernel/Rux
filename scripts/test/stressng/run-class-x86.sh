#!/bin/bash
# run-class-x86.sh — one stressor class per boot on the x86_64+KVM
# vehicle (stress-port lane). Mirrors test/ltpscan/run1-x86.sh's rig;
# PID 1 is /test/stress-ng/rsrun, which runs the single line of
# /test/stress-ng/cmds.txt. The image is the lane's private copy at
# run/rootfs-stress.img (from the permanent store, injected once with
# rs + rsrun); only cmds.txt is rewritten per boot.
#
# Usage: test/stressng/run-class-x86.sh [-t QEMU_BUDGET] --class-args...
#   e.g. test/stressng/run-class-x86.sh --malloc 4 --timeout 120
# Log: run/stress-runs/<class>.log
set -euo pipefail
cd "$(dirname "$0")"
WT="$PWD"
while [ ! -f "$WT/Cargo.toml" ] || [ ! -d "$WT/kernel" ]; do
    WT="$(dirname "$WT")"
done
cd "$WT"

IMAGE="${IMAGE:-run/rootfs-stress.img}"
KERNEL="${KERNEL:-target/x86_64-unknown-none/debug/bzImage}"
OUTDIR="${OUTDIR:-run/stress-runs}"
DFX="${DFX:-dfx=allocwatch,taskdump,watchdog}"
EXTRA="${EXTRA:-syscost=1}"
BUDGET="${BUDGET:-180}"
mkdir -p "$OUTDIR"

# ---- inject binaries (always rewrite: rs/rsrun may have been rebuilt) ----
echo "inject: building + planting rs/rsrun/renameco/brkchurn/execaudit into $IMAGE"
CC=toolchain/x86_64-rux-linux-musl/bin/musl-gcc
$CC -static -O2 -Wall -o "$OUTDIR/rs" test/stressng/rs.c
$CC -static -O2 -Wall -o "$OUTDIR/rsrun" test/stressng/rsrun.c 2>/dev/null
$CC -static -O2 -Wall -o "$OUTDIR/renameco" test/stressng/renameco.c 2>/dev/null
# glibc-static (host gcc — host and guest are both x86_64): exercises
# the brk paths musl-static binaries cannot (round-1 bigheap finding)
gcc -static -O2 -Wall -o "$OUTDIR/brkchurn" test/stressng/brkchurn.c 2>/dev/null
# r3: rc=254 silent-child-death reproducer (fork storm + exec + audit)
$CC -static -O2 -Wall -o "$OUTDIR/execaudit" test/stressng/execaudit.c 2>/dev/null
INJ=$(mktemp /tmp/inj-XXXX)
printf 'cd /test\nmkdir stress-ng\ncd /test/stress-ng\nrm rs\nrm rsrun\nrm renameco\nrm brkchurn\nrm execaudit\nwrite %s/rs rs\nwrite %s/rsrun rsrun\nwrite %s/renameco renameco\nwrite %s/brkchurn brkchurn\nwrite %s/execaudit execaudit\n' \
    "$OUTDIR" "$OUTDIR" "$OUTDIR" "$OUTDIR" "$OUTDIR" > "$INJ"
debugfs -w -f "$INJ" "$IMAGE" 2>&1 | grep -v "^debugfs" | head -3 || true
rm -f "$INJ"

# ---- cmds.txt: single invocation from "$*", or a file of lines via -f ----
ARGS=""
CLASS=""
CMD=$(mktemp /tmp/cmds-XXXX)
if [ "${1:-}" = "-f" ]; then
    cp "$2" "$CMD"
    CLASS="$(basename "$2" .cmds)"
    ARGS="(file: $2)"
else
    ARGS="$*"
    printf '%s\n' "$ARGS" > "$CMD"
fi
if [ -z "$CLASS" ]; then
CLASS="misc"
for a in "$@"; do
    case "$a" in
        --timeout|--vm-bytes|--fs-root|--shm-bytes|--block) shift_next=1 ;;
        --*) [ "${shift_next:-0}" = 0 ] && CLASS="${a#--}" ;;
        *) shift_next=0 ;;
    esac
done
fi
INJ=$(mktemp /tmp/inj2-XXXX)
printf 'cd /test/stress-ng\nrm cmds.txt\nwrite %s cmds.txt\n' "$CMD" > "$INJ"
debugfs -w -f "$INJ" "$IMAGE" 2>&1 | grep -v "^debugfs" | head -2 || true
rm -f "$CMD" "$INJ"

LOG="$OUTDIR/${CLASS}.log"
echo "boot: class=$CLASS args=$ARGS dfx=$DFX extra=$EXTRA log=$LOG"
timeout "$BUDGET" qemu-system-x86_64 -M q35 -accel kvm -cpu max \
  -m 2G -smp 2 -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file="$IMAGE",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel "$KERNEL" \
  -append "root=/dev/vda rw init=/test/stress-ng/rsrun console=ttyS0 $DFX $EXTRA" \
  > "$LOG" 2>&1 || true

echo "=== verdict input (tail of $LOG) ==="
grep -aE "RES\||RS-RUN|RS-DONE|RS-ALL-DONE|RS-ERR|RS-NOTE|panic|KERNPANIC|DEADLOCK|blocked for more|ALLOCTHROW|watchdog" "$LOG" | tail -15 | cut -c1-200
