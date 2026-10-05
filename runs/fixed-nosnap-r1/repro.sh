#!/bin/sh
# v3 load shape: the head_desc-collision corruption family needs
# CONCURRENT batch submitters on MANY DISTINCT inodes with MIXED chain
# sizes, plus fork/exec/exit churn (kernel-stack recycling while batches
# are in flight) — the GNOME-session shape. The 300MB /tmp/rdata file
# exceeds what stays cached between rounds on a 512M guest, so the dd
# window readers do REAL cold batch reads every round; its random
# content is checksummed per round (a miscompleted read shows up as a
# CKSUM-MISMATCH, not just a panic).
echo REPRO-SCRIPT-START
FILES="/bin/toybox /bin/mrsh /lib/ld-musl-riscv64.so.1 /app/vshell /app/calculator /app/desktop /app/clock /test/smoke_test"
ref=""
r=0
while [ $r -lt 3 ]; do
  echo ROUND-$r-BEGIN
  # (1) data-integrity probe: fixed 8MB window of /tmp/rdata, cksum.
  c=$(dd if=/tmp/rdata bs=1048576 skip=7 count=8 2>/dev/null | cksum)
  if [ -z "$ref" ]; then ref="$c"; fi
  if [ "$c" != "$ref" ]; then
    echo "CKSUM-MISMATCH r=$r got=$c want=$ref"
    # Transient vs persistent: re-read immediately. A second read matching
    # the reference means the corruption was a one-shot DELIVERY failure
    # (page-cache/virtio race); a persistent mismatch means the data on
    # the (snapshot) disk image itself was damaged — a write-path/alloc
    # aliasing bug.
    c2=$(dd if=/tmp/rdata bs=1048576 skip=7 count=8 2>/dev/null | cksum)
    if [ "$c2" = "$ref" ]; then echo "CKSUM-REREAD-CLEAN (transient delivery)"; else echo "CKSUM-REREAD-STILL-BAD (persistent on-disk)"; fi
    # Narrow to the damaged 1MB chunk (4 pages granularity for forensics).
    m=0
    while [ $m -lt 8 ]; do
      cm=$(dd if=/tmp/rdata bs=1048576 skip=$(( 7 + m )) count=1 2>/dev/null | cksum)
      echo "CHUNK r=$r mb=$m $cm"
      m=$(( m + 1 ))
    done
  fi
  # (2) fresh /tmp churn: 4 x 16MB writes (dirty writeback = SYNC chains
  #     mixing with the async read batches).
  head -c 16777216 /dev/zero > /tmp/f1
  head -c 16777216 /dev/zero > /tmp/f2
  head -c 16777216 /dev/zero > /tmp/f3
  head -c 16777216 /dev/zero > /tmp/f4
  # (3) concurrent cold window readers on /tmp/rdata (rotating windows,
  #     coalesced multi-block async chains, 8 tasks racing).
  w=$(( (r * 8) % 280 ))
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$w count=8 &
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$(( w + 12 )) count=8 &
  dd if=/tmp/rdata of=/dev/null bs=524288 skip=$(( (w * 2) + 40 )) count=24 &
  dd if=/tmp/rdata of=/dev/null bs=4096 skip=$(( (w * 512) + 70000 )) count=3000 &
  dd if=/tmp/rdata of=/dev/null bs=1048576 skip=$(( w + 60 )) count=4 &
  dd if=/tmp/rdata of=/dev/null bs=262144 skip=$(( (w * 4) + 900 )) count=80 &
  # (4) concurrent whole-file readers on 8 distinct large on-disk inodes.
  for f in $FILES; do cat $f > /dev/null & done
  # (5) fork/exec/exit storm: short-lived processes interleaved with the
  #     batches; execs walk + read ELF/libc pages, exits recycle stacks.
  e=0
  while [ $e -lt 60 ]; do
    /bin/cat /etc/hostname > /dev/null 2>&1
    /bin/head -c 4096 /bin/toybox > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  e=0
  while [ $e -lt 60 ]; do
    /bin/ls /usr/bin > /dev/null 2>&1
    /bin/head -c 8192 /bin/mrsh > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  # (6) small-reader churn on the fresh files.
  j=0
  while [ $j -lt 20 ]; do head -c 65536 /tmp/f2 > /dev/null & j=$(( j + 1 )); done
  wait
  rm -f /tmp/f1 /tmp/f2 /tmp/f3 /tmp/f4
  echo ROUND-$r-END
  r=$(( r + 1 ))
done
echo REPRO-SCRIPT-ALL-DONE
