#!/bin/bash
# Inject the USER_UNLOCKED-event-created per-bundle data dirs into a clean
# tok-family userdata copy (offline, debugfs). Rux never publishes
# USER_UNLOCKED so libbms's user_unlocked_event_subscriber never creates
# el1/100/database/<b> (fatal, check-action-status:true) nor the el2 leaves.
set -euo pipefail
IMG="$1"
LIST="$2"   # file: one bundle name per line
M() { debugfs -w -R "mkdir $1" "$IMG" >/dev/null 2>&1 || true; }
S() { debugfs -w -R "sif $1 mode=$2" "$IMG" >/dev/null 2>&1 || true; }
U() { debugfs -w -R "sif $1 uid=0 gid=0" "$IMG" >/dev/null 2>&1 || true; }

# skeleton chain (sc45's boot would create these anyway; ensure present so
# the per-bundle leaves can attach)
for d in /app /app/el1 /app/el1/100 /app/el1/100/database /app/el2 /app/el2/100 \
         /app/el2/100/base /app/el2/100/log /app/el2/100/database /app/el2/100/sharefiles \
         /app/el1/0 /app/el1/0/database /app/el2/0 /app/el2/0/base /app/el2/0/log /app/el2/0/database; do
    M "$d"; S "$d" 040755; U "$d"
done

while read -r b; do
    [ -n "$b" ] || continue
    for p in "/app/el1/100/database/$b" "/app/el2/100/base/$b" "/app/el2/100/log/$b" \
             "/app/el2/100/database/$b" "/app/el2/100/sharefiles/$b"; do
        M "$p"; S "$p" 040777; U "$p"
    done
done < "$LIST"
echo "injected $(grep -c . "$LIST") bundles into $IMG"
