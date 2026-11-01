#!/bin/bash
# wf-leg.sh — wedge-fix bisect leg: build release at current HEAD, boot the OH
# SA storm 120s, classify.
#   rc 0   = GOOD  (init alive >=100s, no DEADLOCK, no EXT4-BIGLOCK-STALL)
#   rc 1   = BAD   (any DEADLOCK / BIGLOCK-STALL, or init silent before 60s)
#   rc 125 = SKIP  (build fail / early death / samgr never started)
# Usage: wf-leg.sh TAG [kernel]   (kernel optional; default: fresh build)
W=/home/william/rux-agents/wedge-fix
O=/home/william/rux-agents/oh-launcher
TAG=$1
S=$O/run/oh-$TAG-serial.log

cleanup() {
  for p in $(pgrep -f qemu-system); do
    tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null | grep -q "oh-$TAG-mon.sock" && kill $p 2>/dev/null
  done
}
trap cleanup EXIT

if [ $# -ge 2 ]; then
  K=$2
else
  cd "$W" || exit 125
  (cd build && PLATFORM=x86_64 make build-release > /tmp/wf-$TAG-build.log 2>&1) || exit 125
  K="$W/target/x86_64-unknown-none/release/bzImage"
fi
[ -f "$K" ] || exit 125

cd "$O" || exit 125
USERDATA=run/userdata-fresh2.img timeout 200 python3 run/lb-boot.py "$TAG" 120 \
  run/system-lb.img "$K" > /tmp/wf-$TAG-boot.log 2>&1
sleep 2

# SA load must be present for the verdict to count (lb49 lesson).
grep -q "samgr(pid" "$S" 2>/dev/null || exit 125
grep -q "DEADLOCK: spinlock stuck" "$S" && exit 1
grep -q "EXT4-BIGLOCK-STALL" "$S" && exit 1
# init's last kmsg timestamp: strip to seconds, compare to 60.
LAST_INIT=$(grep -o "^\[ *[0-9]*\.[0-9]*\] \[kmsg\] \[pid=1\]" "$S" | tail -1 | grep -o "[0-9]*\.")
LAST_INIT=${LAST_INIT%.}
[ -n "$LAST_INIT" ] || LAST_INIT=0
[ "$LAST_INIT" -lt 60 ] && exit 1
exit 0
