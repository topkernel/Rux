#!/bin/bash
# Inject the per-bundle data dirs that Rux never creates (offline, debugfs).
#
# sc45 forensics: the FATAL sandbox binds (check-action-status:true in
# appdata-sandbox.json) are
#   /data/app/el1/bundle/public/<b>   (install path creates it - ok)
#   /data/app/el1/<uid>/base/<b>      (install path creates it - ok)
#   /data/app/el1/<uid>/database/<b>  (NEVER created on Rux - inject here)
# for BOTH uid 0 (systemui & co spawn at userId 0) and uid 100 (launcher).
# The el2 leaves are also missing but are NOT fatal and storage_daemon's
# unlock pass (DestroyUserDirs EL2) rmdirs empty el2 children anyway, so
# injecting them is pointless.
# Root cause hunt (ilog tap): installd's CreateBundleDataDir creates base/<b>
# then aborts before database/<b> on Rux - watch BMSInstalld hilog lines.
set -euo pipefail
IMG="$1"
LIST="$2"   # file: one bundle name per line
M() { debugfs -w -R "mkdir $1" "$IMG" >/dev/null 2>&1 || true; }
S() { debugfs -w -R "sif $1 mode=$2" "$IMG" >/dev/null 2>&1 || true; }
U() { debugfs -w -R "sif $1 uid=0 gid=0" "$IMG" >/dev/null 2>&1 || true; }

# skeleton chain (the boot would create these anyway; ensure present so the
# per-bundle leaves can attach)
for d in /app /app/el1 /app/el1/100 /app/el1/100/base /app/el1/100/database \
         /app/el2 /app/el2/100 /app/el2/100/base /app/el2/100/log /app/el2/100/database /app/el2/100/sharefiles \
         /app/el1/0 /app/el1/0/base /app/el1/0/database /app/el2/0 /app/el2/0/base /app/el2/0/log /app/el2/0/database; do
    M "$d"; S "$d" 040755; U "$d"
done

while read -r b; do
    [ -n "$b" ] || continue
    # fatal el1 database leaves, both user ids
    for p in "/app/el1/100/database/$b" "/app/el1/0/database/$b"; do
        M "$p"; S "$p" 040777; U "$p"
    done
done < "$LIST"
echo "injected $(grep -c . "$LIST") bundles into $IMG"
