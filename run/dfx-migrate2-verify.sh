#!/bin/bash
# dfx-migrate2-verify.sh — round 5 boot verification (x86_64 KVM, probe
# image). Three boots:
#   A: trace_event=<all 40 new rux: events>  (canonical arming path)
#   B: dfx=<all 39 retired tokens + 3 dead tokens>  (alias path — old rigs)
#   C: no tokens                             (probe-retire guarantee)
#
# Reuses run/dfx_mig_probe.c (forks/exits, kill, signal, tracefs dump).
# Usage: run/dfx-migrate2-verify.sh [kernel] ; env: ACCEL=kvm|tcg, CAP
set -uo pipefail
HERE="$(cd "$(dirname .. "$(dirname "$0")" 2>/dev/null || echo .)" &>/dev/null; pwd)"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$HERE/target/x86_64-unknown-none/debug/bzImage}"
ACCEL="${ACCEL:-kvm}"
CAP="${CAP:-150}"
OUT="$HERE/run"
MKFS=/usr/sbin/mkfs.ext4
[ -x "$MKFS" ] || MKFS=$(command -v mkfs.ext4)

mkdir -p "$OUT/work"
gcc -static -O1 -o "$OUT/work/dfx_mig_probe" "$HERE/run/dfx_mig2_probe.c" || { echo "FAIL: probe build"; exit 1; }
rm -rf "$OUT/work/mig-root"
mkdir -p "$OUT/work/mig-root/test"
cp "$OUT/work/dfx_mig_probe" "$OUT/work/mig-root/test/probe"
chmod 755 "$OUT/work/mig-root/test/probe"
rm -f "$OUT/work/mig.img"
"$MKFS" -q -F -O '^metadata_csum' -d "$OUT/work/mig-root" "$OUT/work/mig.img" 64M || { echo "FAIL: mkfs"; exit 1; }

# The 38 quiet new rux: events (round 5). scribble (one PUB line per
# context switch) and steptrace (two lines per syscall) are verified by
# their own short boots D/E — arming them here floods the UART and the
# probe never reaches its tracefs dump.
NEW_EVENTS="rux:schb,rux:parkdump,rux:sleepsnap,rux:ksprobe,rux:wwcensus,rux:bindertrace,rux:syscalltrace,rux:inputtrace,rux:exthunt,rux:sfddbg,rux:dbtrace,rux:datatap,rux:ams46,rux:vmap,rux:poolcensus,rux:slowtask,rux:execcost,rux:tfdcensus,rux:scmtap,rux:sysforensic,rux:filerd,rux:lostpark,rux:allocwatch,rux:r83scan,rux:leakhunt,rux:acqdiv,rux:metawatch,rux:monowatch,rux:epollscan,rux:treehunt,rux:tcptrace,rux:strace1,rux:ripsample,rux:socksnap,rux:writetrace,rux:ledger,rux:pid1spin,rux:pctlmon"
# Retired tokens minus the two loud ones + the dead trio.
RETIRED="schb,parkdump,sleepsnap,ksprobe,wwcensus,bindertrace,syscalltrace,inputtrace,exthunt,sfddbg,dbtrace,datatap,ams46,vmap,poolcensus,slowtask,execcost,tfdcensus,scmtap,sysforensic,filerd,lostpark,allocwatch,r83scan,leakhunt,acqdiv,metawatch,monowatch,epollscan,treehunt,tcptrace,strace1,ripsample,socksnap,writetrace,ledger,pid1spin,pctlmon"
DEAD="thpark,syncguard,syncwatch"

# Hunt-line shapes this round's families print on this probe (the
# echo families with reachable faces on a minimal boot).
HUNT_RE='SC-HB t=|SC-ANAT t=|SC-PROC |RWFRZ-PARK|APP-PARK|SLEEP-SNAP|EXT-HUNT|WW: wakes=|SLOWTASK|MONOWATCH-JUMP|POOL |POOL-SPAWN|LEDGER|TREE-|PID1SPIN|ALLOC-STUCK|LOSTPARK|scribble|SCRIBBLE'

boot() {  # boot <name> <append-extra>
    local name="$1"
    local extra="$2"
    local log="$OUT/mig2-$name-serial.log"
    rm -f "$log"
    timeout "$CAP" qemu-system-x86_64 \
        -M q35 -accel "$ACCEL" -cpu max -m 2G -smp 2 \
        -display none -serial stdio -snapshot \
        -drive file="$OUT/work/mig.img",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/probe console=ttyS0 $extra" \
        > "$log" 2>&1 || true
    echo "$log"
}

echo "== boot A: trace_event= (canonical, all 40 new events) =="
A_LOG=$(boot a "trace_event=$NEW_EVENTS")
echo "== boot B: dfx= retired + dead tokens (alias path) =="
B_LOG=$(boot b "dfx=$RETIRED,$DEAD")
echo "== boot C: no tokens (clean) =="
C_LOG=$(boot c "")
echo "== boot D: trace_event=rux:scribble only (30s — the PUB flood family) =="
D_LOG=$(boot d "trace_event=rux:scribble")
echo "== boot E: dfx=scribble,steptrace (alias, 30s — the loud families) =="
E_LOG=$(boot e "dfx=scribble,steptrace")

echo
echo "===== ANALYSIS ====="
for f in "$A_LOG" "$B_LOG" "$C_LOG"; do
    echo "--- $f"
    echo "probe-end markers: $(grep -ac '===MARK: probe-end' "$f" || true)"
    echo "hunt-lines: $(grep -acE "$HUNT_RE" "$f" || true)"
    echo "panics: $(grep -acE 'KERNPANIC|Kernel panic|Oops:' "$f" || true)"
done
echo "--- A: new rux: events in available_events (expect 40)"
sed -n '/===TRACE: available-rux/,/===TRACE: set-event/p' "$A_LOG" | grep -ac "^rux:"
echo "--- A: per-family sample lines"
grep -aE "$HUNT_RE" "$A_LOG" | head -6
echo "--- A: ring rows (trace pipe)"
sed -n '/===TRACE: trace-rux/,/===TRACE: rux-record-count/p' "$A_LOG" | grep -aE "schb:|wwcensus:|sleepsnap:|pid1spin:|exthunt:|poolcensus:|slowtask:|ledger:|treehunt:" | head -12
echo "--- B: alias notices (expect 39 retired + 1 thpark + 2 dead)"
grep -a "dfx: token" "$B_LOG" | sed 's/(first: .*)//' | sort | uniq -c | sort -rn | head -8
grep -ac "retired; armed trace events" "$B_LOG"
grep -a "retired; instrument removed" "$B_LOG" | sort | uniq -c
echo "--- B: runtime switches still live (expect 0 enabled: none passed)"
grep -a "dfx: runtime switch" "$B_LOG" | head -3
echo "--- D: scribble via event (PUB lines expected, no panic)"
echo "PUB lines: $(grep -ac "PUB" "$D_LOG")  probe-armed: $(grep -ac "scribble" "$D_LOG" | head -1)  panic: $(grep -acE 'KERNPANIC|Kernel panic' "$D_LOG")"
echo "--- E: scribble+steptrace via alias dfx= (same PUB flood + step lines)"
echo "PUB lines: $(grep -ac "PUB" "$E_LOG")  alias notices: $(grep -ac "retired; armed" "$E_LOG")  panic: $(grep -acE 'KERNPANIC|Kernel panic' "$E_LOG")"

norm() { grep -aE "$HUNT_RE" "$1" | sed -E 's/[0-9a-fx]+/N/g' | sort -u; }
norm "$A_LOG" > "$OUT/mig2-A-shapes.txt"
norm "$B_LOG" > "$OUT/mig2-B-shapes.txt"
norm "$C_LOG" > "$OUT/mig2-C-shapes.txt"
echo "--- shape diff A vs B (numbers-normalized; empty = identical)"
diff "$OUT/mig2-A-shapes.txt" "$OUT/mig2-B-shapes.txt" && echo "A/B SHAPES IDENTICAL"
echo "--- C shapes (must be empty): $(wc -l < "$OUT/mig2-C-shapes.txt") lines"
