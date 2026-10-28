#!/bin/bash
# s16 WW5 census harvest — one canary round's A2 evidence package.
L="$1"
echo "== strand count ==";        grep -c "WAKE-STRANDED" "$L"
echo "== heal counters (last WW5) =="; grep "WW5:" "$L" | tail -1
echo "== heal counters (max oncpu_heal seen) =="; grep -o "oncpu_heal=[0-9]*" "$L" | sort -t= -k2 -n | tail -1
echo "== heal counters (max claim_heal seen) ==";  grep -o "claim_heal=[0-9]*" "$L" | sort -t= -k2 -n | tail -1
echo "== sub-gate cumulative (last) =="; grep "WW5:" "$L" | tail -1 | grep -o "s[0-9]*=[0-9]*" | tr '\n' ' '; echo
echo "== wtie histogram (50 bins) =="; grep -o "wtie=[0-9]*" "$L" | sed 's/wtie=//' | awk '{b=int($1/50)*50; print b"-"(b+50)}' | sort -t- -k1 -n | uniq -c
echo "== visits stats =="; grep -o "visits=[0-9]*" "$L" | sed 's/visits=//' | awk '{s+=$1;n++;if($1<mn||mn==0)mn=$1;if($1>mx)mx=$1} END{print "n="n" min="mn" avg="int(s/n)" max="mx}'
echo "== strand ms stats =="; grep -o "strand=[0-9]*ms" "$L" | sed 's/strand=//;s/ms//' | awk '{s+=$1;n++;if($1<mn||mn==0)mn=$1;if($1>mx)mx=$1} END{print "n="n" min="mn" avg="int(s/n)" max="mx}'
echo "== SKIPHIST census =="; grep -o "SKIPHIST skips=[0-9]*\|SKIPHIST coll=1" "$L" | sort | uniq -c | sort -rn | head -8
echo "== slotmm census =="; grep -o "slotmm=\[[0-9]*,[0-9]*\]" "$L" | sort | uniq -c | sort -rn | head -6
echo "== why= census =="; grep -o "  pid=[0-9]* why=[0-9]" "$L" | grep -o "why=[0-9]" | sort | uniq -c
echo "== panics =="; grep -ciE "KERNPANIC|Kernel panic|ALLOCTHROW" "$L"
