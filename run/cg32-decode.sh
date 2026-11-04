#!/bin/bash
# cg32-decode.sh — 32nd successor per-boot classifier for the post-fix
# roll series. Extracts from a boot's serial log: first hung binder txn
# (id + from->to pair), SA insert count + freeze point, install-branch
# discriminator (BMSSCAN install_list open / bundle_manager_service
# mkdirs), RDB ctor storm span, journal-less line, pixel md5 set.
TAG=$1
SER="run/oh-${TAG}-serial.log"
cd /home/william/rux-agents/oh-launcher || exit 1
[ -f "$SER" ] || { echo "no serial for $TAG"; exit 1; }
echo "== $TAG =="
F=$(grep -a -m1 'TXN-HANG txn' "$SER")
[ -n "$F" ] && echo "first-hang: $(echo "$F" | sed 's/.*TXN-HANG \(txn[0-9]*\).*from pid \([0-9]*\) tid [0-9]* (\([^)]*\)) to pid \([0-9]*\) tid [0-9]* (\([^)]*\)).*exec \([0-9]*\) ms.*/\1 \2(\3)->\4(\5) exec=\6ms @\t/' | tr -d '\t')$(echo "$F" | grep -o '^\[ *[0-9.]*\]')"
SA=$(grep -a -c 'insert SA:' "$SER")
SAL=$(grep -a 'insert SA:' "$SER" | tail -1 | grep -o '\[ *[0-9.]*\]')
echo "SA inserts: $SA (last $SAL)"
ILL=$(grep -a -c 'BMSSCAN-OPEN.*install_list' "$SER")
BMD=$(grep -a -c 'BMSSCAN-MKDIR.*bundle_manager_service' "$SER")
echo "install-branch: install_list_opens=$ILL bms_mkdirs=$BMD"
RDB=$(grep -a -c 'BMSSCAN' "$SER")
JL=$(grep -a -c 'journal inode empty' "$SER")
echo "bmsscan-lines=$RDB journal-less=$JL"
CU=$(grep -a -c 'CreateNewUser' "$SER")
STU=$(grep -a -c 'startUser' "$SER")
LS=$(grep -a -c 'lockstatus_100 SET' "$SER")
echo "createNewUser=$CU startUser=$STU lockstatus100=$LS"
for P in run/oh-${TAG}-shot-*.ppm; do
  [ -f "$P" ] || continue
  M=$(md5sum "$P" | cut -d' ' -f1 | cut -c1-8)
  echo "pixel $(basename $P) $M"
done | sort -u -k3 | awk '{print}' | tail -6
