#!/bin/bash
# tally-33a.sh — leg-33A draw tally (extends tally-31a.sh with the
# TXN-POOL reader-pool census summary: per-pid burst count, thread-pool
# min..max, eligible-reader min..max, proc-todo max — the exhaustion
# signature is th flat + wr 0 + todo climbing; the spawn-fix signature
# is th climbing / wr recovering).
# Usage: tally-33a.sh <serial-log>
set -euo pipefail
L="${1:?usage: tally-33a.sh <serial-log>}"
[ -f "$L" ] || { echo "no such log: $L"; exit 1; }

lc=$(grep -c "LIFECYCLE_TIMEOUT" "$L" || true)
lc_abil=$(grep -m1 -o "abilityName: [A-Za-z.]*" "$L" | head -1 || true)
abms=$(grep -c "ABMS7918" "$L" || true)
ldef=$(grep -c "Launcher_Default" "$L" || true)
spawn=$(grep -c "CreateAppSpawnReqMsg" "$L" || true)
mainab=$(grep -c "MainAbility" "$L" || true)
mainab_start=$(grep -c "start highest priority extension ability" "$L" || true)
ready=$(grep "bootevent.launcher.ready" "$L" | grep -vc "not set" || true)
rs_ready=$(grep -c "bootevent.renderservice.ready" "$L" || true)
onfg=$(grep -c "OnForeground" "$L" || true)
stopall=$(grep -c "StopAllServices\|stop all services" "$L" || true)
rs_segv=$(grep -c "USEGV-FOR: pid=[0-9]* rip=0x3fbf08ad30" "$L" || true)
usegv=$(grep -c "USEGV-FOR" "$L" || true)
txnhang=$(grep -c "TXN-HANG " "$L" || true)
strand=$(grep -c "WAKE-STRANDED" "$L" || true)
strand_max=$(grep -o "WAKE-STRANDED pid=[0-9]* cpu=[0-9]* strand=[0-9]*ms" "$L" | grep -o "strand=[0-9]*" | grep -o "[0-9]*" | sort -n | tail -1 || true)
leak=$(grep -c "WAKE-RUN-LEAK" "$L" || true)
leak_max=$(grep -o "WAKE-RUN-LEAK tid [0-9]* kicked [0-9]*x" "$L" | grep -o "kicked [0-9]*" | grep -o "[0-9]*" | sort -n | tail -1 || true)
term=$(grep -c "TERM-DESCHED engaged" "$L" || true)
refused=$(grep -c "W3-RUNWAKE-REFUSED" "$L" || true)
ww_final=$(grep "WW: " "$L" | tail -1 | grep -o "stranded=[0-9]* strand_max=[0-9]*ms" || true)
fsw1_final=$(grep "FSW1" "$L" | tail -1 | grep -o "pb=[0-9.]* ps=[0-9.]* pc=[0-9]* ph=[0-9]* pm=[0-9]*" || true)
fsw1_es=$(grep "FSW1" "$L" | tail -1 | grep -o "es=[0-9]*" || true)
fsw1_first_t=$(grep -m1 "FSW1" "$L" | grep -o "t=[0-9]*s" || true)
grid=$(grep -c "1001ms" "$L" || true)
panics=$(grep -c "KERNEL PANIC\|KERNPANIC" "$L" || true)
lines=$(wc -l < "$L")

echo "log=$(basename "$L") lines=$lines"
echo "LC=$lc ($lc_abil) ABMS7918=$abms Launcher_Default=$ldef spawn_req=$spawn MainAbility=$mainab/$mainab_start launcher.ready=$ready rs.ready=$rs_ready OnForeground=$onfg stopall=$stopall"
echo "wedges: TXN-HANG=$txnhang STRANDED=$strand(max=${strand_max:-0}ms) RUN-LEAK=$leak(max=${leak_max:-0}x) USEGV=$usegv rsfault_rip=$rs_segv"
echo "census: TERM=$term REFUSED=$refused $ww_final grid_hits=$grid panics=$panics"
echo "fsw1: first=$fsw1_first_t final=$fsw1_final $fsw1_es"
drain_top=$(grep -o "drained to proc [0-9]*" "$L" | sort | uniq -c | sort -rn | head -5 | tr '\n' ' ' || true)
parked=$(grep -c "parked on node" "$L" || true)
echo "binder-drain: parked=$parked top-receivers: $drain_top"

# TXN-POOL reader-pool census summary (leg-33A): per pid — bursts, th
# min..max, wr min..max, todo max, req max.
echo "txn-pool census:"
grep -o "TXN-POOL pid=[0-9]* th=[0-9]* loop=[0-9]* rd=[0-9]* wr=[0-9]* req=[0-9]* started=[0-9]* max=[0-9]* todo=[0-9]*" "$L" | \
awk '{
    pid=$2; sub("pid=","",pid);
    th=$3; sub("th=","",th); rd=$5; sub("rd=","",rd); wr=$6; sub("wr=","",wr);
    req=$7; sub("req=","",req); todo=$10; sub("todo=","",todo);
    n[pid]++;
    if (!(pid in thmin) || th<thmin[pid]) thmin[pid]=th;
    if (th>thmax[pid]) thmax[pid]=th;
    if (!(pid in wrmin) || wr<wrmin[pid]) wrmin[pid]=wr;
    if (wr>wrmax[pid]) wrmax[pid]=wr;
    if (todo>tdmax[pid]) tdmax[pid]=todo;
    if (req>rqmax[pid]) rqmax[pid]=req;
}
END { for (pid in n) printf "  pid=%s bursts=%d th=%d-%d wr=%d-%d todo<=%d req<=%d\n", pid, n[pid], thmin[pid], thmax[pid], wrmin[pid], wrmax[pid], tdmax[pid], rqmax[pid]; }' | sort || true
tp_n=$(grep -c "TXN-POOL" "$L" || true)
echo "txn-pool lines=$tp_n"
