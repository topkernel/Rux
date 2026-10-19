#!/bin/bash
# watch-b5.sh <serial-log> — one-line status of the launcher-chain gates
L="$1"
[ -f "$L" ] || { echo "no log"; exit 1; }
echo "--- lines: $(wc -l < "$L")"
grep -c "KERNPANIC" "$L" | sed 's/^/KERNPANIC: /'
grep -c "ALLOCTHROW" "$L" | sed 's/^/ALLOCTHROW: /'
grep -n "OnStart-SA:401 finished\|OnStart-SA:180 finished\|Publish SA:180 \|Publish SA:401 " "$L" | tail -4
grep -n "bootevent.account.ready" "$L" | grep "Handle set param" | tail -2
grep -oE "pid 328 -> pid [0-9]+ h[0-9]+ code 1 oneway" "$L" | wc -l | sed 's/^/samgr OnAdd deliveries: /'
grep -n "ServiceStart started info foundation" "$L" | tail -1
