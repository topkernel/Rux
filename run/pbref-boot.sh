#!/bin/bash
# pbref-boot.sh — ONE Linux-reference pbench probe boot (perf-report.md
# 8.4 protocol, reference side). Boots the OH image on its own OHOS
# 6.6.101 bzImage with the machine class MATCHED to the pa series
# (q35 / KVM / -cpu max / SMP2 / 4G — run-oh-linux-ref.sh defaults are
# smp6/8G/-cpu host and are overridden per 8.3), with the private
# pbench-injected system image (pbref-inject.sh) and a private
# per-run userdata copy (no snapshot: probe writes persist).
#
# The probe service starts on bootevent.launcher.ready (~t=10s on the
# reference kernel) and powers the VM off when done; this script waits
# for QEMU exit (timeout backstop), then extracts
# /local/pbench/pbench-results.txt from the userdata copy via debugfs.
#
# Usage: pbref-boot.sh <tag>      (artifacts: run/ref-<tag>/)
# Env:   SECS timeout backstop (default 900)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:?usage: pbref-boot.sh <tag>}"
SECS="${SECS:-900}"
SYS="$ROOT/run/pbref-system.img"
UD="$ROOT/run/ref-$TAG-userdata.img"
RUN="$ROOT/run/ref-$TAG"

"$ROOT/run/pbref-inject.sh" "$SYS" >/dev/null   # idempotent; creates on first call

rm -rf "$RUN"
mkdir -p "$RUN"
if [ ! -f "$UD" ]; then
    cp --reflink=auto "$ROOT/run/oh-img/userdata-tok.img" "$UD"
fi

foreign=$(pgrep -c qemu-system || true)
echo "[boot] ref-$TAG start $(date +%H:%M:%S) machine=q35/kvm/-cpu max/SMP2/4G foreign_qemus=${foreign:-0}"

rc=0
SMP=2 MEM=4096M CPU=max SYSIMG="$SYS" USERDATA_SNAP=0 \
    timeout --signal=TERM --kill-after=15 "${SECS}s" \
    "$ROOT/test/run-oh-linux-ref.sh" "$RUN" "$UD" || rc=$?

echo "[boot] ref-$TAG qemu rc=$rc after at-most ${SECS}s ($(date +%H:%M:%S))"

# results: file channel (authoritative) + serial lines (live copy)
mkdir -p "$RUN"
if debugfs -R "dump /local/pbench/pbench-results.txt $RUN/results.txt" "$UD" >/dev/null 2>&1; then
    echo "[boot] results file extracted: $RUN/results.txt"
    grep -c "^PBENCH" "$RUN/results.txt" || true
else
    echo "[boot] NO results file in userdata (probe did not reach file output)" >&2
fi
grep "PBENCH " "$RUN/serial.log" > "$RUN/pbench-serial-lines.txt" 2>/dev/null || true
grep -q "PBENCH DONE rc=0" "$RUN/serial.log" 2>/dev/null && echo "[boot] DONE marker in serial" || echo "[boot] no DONE marker in serial" >&2
exit 0
