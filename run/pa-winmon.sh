#!/bin/bash
# pa-winmon.sh <out-log> <duration-secs> — sample load1 + foreign qemu count every 30s.
OUT="$1"; DUR="$2"
END=$(( $(date +%s) + DUR ))
echo "window sample start $(date '+%F %T')" >> "$OUT"
while [ "$(date +%s)" -lt "$END" ]; do
    L1=$(cut -d' ' -f1 /proc/loadavg)
    NQ=$(ps aux | grep -c "[q]emu-system")
    echo "$(date '+%T') load1=$L1 qemus=$NQ" >> "$OUT"
    sleep 30
done
echo "window sample end $(date '+%F %T')" >> "$OUT"
