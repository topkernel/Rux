#!/bin/bash
# tally-31a.sh — one-line draw tally from a leg-31A serial log.
# Counts the full-chain markers: LC loop, launcher radar/spawn chain,
# launcher.ready bootevent, wedge classes (WAKE-STRANDED /
# WAKE-RUN-LEAK / TXN-HANG / RS faults), sched census, FSW1 final line.
# Usage: tally-31a.sh <serial-log>
set -euo pipefail
L="${1:?usage: tally-31a.sh <serial-log>}"
[ -f "$L" ] || { echo "no such log: $L"; exit 1; }

lc=$(grep -c "LIFECYCLE_TIMEOUT" "$L" || true)
lc_abil=$(grep -m1 -o "abilityName: [A-Za-z]*" "$L" | head -1 || true)
abms=$(grep -c "ABMS7918" "$L" || true)
ldef=$(grep -c "Launcher_Default" "$L" || true)
spawn=$(grep -c "CreateAppSpawnReqMsg" "$L" || true)
ready=$(grep "bootevent.launcher.ready" "$L" | grep -vc "not set" || true)
rs_ready=$(grep -c "bootevent.renderservice.ready" "$L" || true)
onfg=$(grep -c "OnForeground" "$L" || true)
rs_segv=$(grep -c "USEGV-FOR: pid=[0-9]* rip=0x3fbf08ad30" "$L" || true)
usegv=$(grep -c "USEGV-FOR" "$L" || true)
rs_death=$(grep -c "render_service.*die\|SIGSEGV.*render\|render_service.*signal" "$L" || true)
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
echo "LC=$lc ($lc_abil) ABMS7918=$abms Launcher_Default=$ldef spawn_req=$spawn launcher.ready=$ready rs.ready=$rs_ready OnForeground=$onfg"
echo "wedges: TXN-HANG=$txnhang STRANDED=$strand(max=${strand_max:-0}ms) RUN-LEAK=$leak(max=${leak_max:-0}x) USEGV=$usegv rsfault_rip=$rs_segv"
echo "census: TERM=$term REFUSED=$refused $ww_final grid_hits=$grid panics=$panics"
echo "fsw1: first=$fsw1_first_t final=$fsw1_final $fsw1_es"
drain_top=$(grep -o "drained to proc [0-9]*" "$L" | sort | uniq -c | sort -rn | head -5 | tr '\n' ' ' || true)
parked=$(grep -c "parked on node" "$L" || true)
echo "binder-drain: parked=$parked top-receivers: $drain_top"
