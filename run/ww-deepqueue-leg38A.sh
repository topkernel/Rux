#!/bin/bash
# ww-deepqueue-leg38A.sh — A2 deep-queue confirmator (leg-38A; the
# follow-up SC-37A sequenced after the tier verdicts). Boots a
# stress-lane-class LTP sweep round on THIS branch's kernel
# (1b8be5a3 = main 2a960538 + wake-placement reservation) with the
# census switch set, and reads:
#   (a) the WW hint counters (onor/steal/exp) across the heartbeat
#       series — the inertness claim says NO reservation stamps while
#       the runnable queue is deep (no idle CPU at wake -> no stamp);
#       the early-boot idle-capacity phase shows a few honors (the
#       leg-37A regression shape onor=6), then the counters must FREEZE
#       while the msgstress region churns. Movement of onor/steal/exp
#       during the deep phase = a real finding for the A2 family.
#   (b) the s16 sub-gate strand census (WW5: s250/s500/s750, wtie=,
#       visits=) — control is the recorded s16 census on the main-class
#       instrument kernels (stress-fleet.md s16: smooth decay
#       s250=18617 s500=8941 s750=2359 at 120 s, wtie mode 300-350,
#       visits avg ~540-580, heals exactly 0). Same shape = unchanged;
#       a cliff or a heal counter > 0 = the surprise case.
# Shared image is booted snapshot=on + file.locking=off only. One own
# QEMU, bounded SECS, own PID recorded.
set -u
ROOT="/home/william/rux-agents/oh-kernel"
IMGDIR="/home/william/rux-agents/wwh-main-kernel/run/fleet/images"
KIMG="$ROOT/target/riscv64gc-unknown-none-elf/debug/rux"
SERIAL="$ROOT/run/ww-deepqueue-leg38A-serial.log"
SECS="${SECS:-600}"
START="${START:-660}"
rm -f "$SERIAL"
nice -n 10 timeout --signal=TERM --kill-after=10 "${SECS}s" \
    qemu-system-riscv64 -M virt -accel tcg,thread=multi \
    -cpu "rv64,zbb=true,zba=true,zbs=true,zcb=true,v=true" \
    -m 2G -smp 2 -display none -monitor none \
    -serial "file:$SERIAL" \
    -drive "file=$IMGDIR/fleet-ltp-rootfs.img,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off" \
    -device "virtio-blk-pci,disable-legacy=on,drive=rootfs" \
    -kernel "$KIMG" \
    -append "root=/dev/vda rw init=/test/sweep console=ttyS0 sweepstart=$START dfx=wwcensus roundtag=ohk-leg38A-dd" &
QPID=$!
echo "$QPID" > "$ROOT/run/ww-deepqueue-leg38A.pid"
wait "$QPID" || true
echo "== verdicts (leg38A deep-queue confirmator: sweepstart=$START secs=$SECS) =="
echo "panics: $(grep -ciE 'KERNPANIC|Kernel panic|ALLOCTHROW' "$SERIAL" || true)"
echo "== hint counter series (every WW: heartbeat) =="
grep -o 'WW: wakes=[0-9]* picks=[0-9]* .*hint=onor=[0-9]* steal=[0-9]* exp=[0-9]*' "$SERIAL" \
  | grep -o 'onor=[0-9]* steal=[0-9]* exp=[0-9]*' | uniq -c
echo "== hint tail =="; grep 'WW: wakes=' "$SERIAL" | tail -1
echo "== WW5 sub-gate tail =="; grep 'WW5:' "$SERIAL" | tail -1
echo "== heal counters (max) =="
echo "oncpu_heal: $(grep -o 'oncpu_heal=[0-9]*' "$SERIAL" | sort -t= -k2 -n | tail -1)"
echo "claim_heal: $(grep -o 'claim_heal=[0-9]*' "$SERIAL" | sort -t= -k2 -n | tail -1)"
echo "== strand census =="
echo "WAKE-STRANDED: $(grep -c 'WAKE-STRANDED' "$SERIAL" || true)"
echo "== wtie histogram (50 bins) =="
grep -o 'wtie=[0-9]*' "$SERIAL" | sed 's/wtie=//' | awk '{b=int($1/50)*50; print b"-"(b+50)}' | sort -t- -k1 -n | uniq -c
echo "== visits stats =="
grep -o 'visits=[0-9]*' "$SERIAL" | sed 's/visits=//' | awk '{s+=$1;n++;if($1<mn||mn==0)mn=$1;if($1>mx)mx=$1} END{print "n="n" min="mn" avg="int(s/n)" max="mx}'
