#!/bin/bash
# bmsdump.sh — 28th successor: extract the BMS preinstall-store state from a
# userdata ext4 image (post-boot forensics, no guest needed). Dumps
# /service/el1/public/bms/bundle_manager_service/ listing + bmsdb.db +
# siblings, then (if sqlite3 present) the preinstalled_bundle row count.
# Usage: run/bmsdump.sh <userdata.img> <outprefix>
IMG="$1"; OUT="$2"
[ -f "$IMG" ] || { echo "no image $IMG"; exit 1; }
mkdir -p "$(dirname "$OUT")"
BMS_DIR=/service/el1/public/bms/bundle_manager_service
{
  echo "== ls -l $BMS_DIR (recursive) =="
  debugfs -R "ls -l $BMS_DIR" "$IMG" 2>&1
  echo
  echo "== ls -l /service/el1/public/bms =="
  debugfs -R "ls -l /service/el1/public/bms" "$IMG" 2>&1
  echo
  echo "== ls -l /app/el1 =="
  debugfs -R "ls -l /app/el1" "$IMG" 2>&1
} > "${OUT}-listing.txt"
for f in bmsdb.db bmsdb.db-wal bmsdb.db-shm bmsdb.db-journal; do
  debugfs -R "dump $BMS_DIR/$f ${OUT}-$f" "$IMG" >/dev/null 2>&1
done
echo "== $OUT =="
ls -la "${OUT}"-* 2>/dev/null
if [ -s "${OUT}-bmsdb.db" ] && command -v sqlite3 >/dev/null; then
  echo "== tables =="
  sqlite3 "${OUT}-bmsdb.db" ".tables" 2>&1
  echo "== preinstalled_bundle rows =="
  sqlite3 "${OUT}-bmsdb.db" "select count(*) from preinstalled_bundle;" 2>&1
  echo "== bundle keys (first 40) =="
  sqlite3 "${OUT}-bmsdb.db" "select KEY from preinstalled_bundle limit 40;" 2>&1
fi
