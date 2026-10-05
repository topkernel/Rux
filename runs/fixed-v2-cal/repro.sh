#!/bin/sh
# v2 load shape: the head_desc-collision corruption family needs
# CONCURRENT batch submitters on MANY DISTINCT inodes with MIXED chain
# sizes, plus fork/exec/exit churn (kernel-stack recycling while batches
# are in flight) — the GNOME-session shape, not a single-stream IO burn.
echo REPRO-SCRIPT-START
FILES="/bin/toybox /bin/mrsh /lib/ld-musl-riscv64.so.1 /app/vshell /app/calculator /app/desktop /app/clock /test/smoke_test"
r=0
while [ $r -lt 3 ]; do
  echo ROUND-$r-BEGIN
  # Fresh multi-inode uncached data, recreated each round (4 x 16MB).
  head -c 16777216 /dev/zero > /tmp/f1
  head -c 16777216 /dev/zero > /tmp/f2
  head -c 16777216 /dev/zero > /tmp/f3
  head -c 16777216 /dev/zero > /tmp/f4
  # Fresh toybox copy under an applet name: exec reads a COLD inode's
  # pages through the page-cache batch path every round.
  rm -rf /tmp/e
  mkdir /tmp/e
  cp /bin/toybox /tmp/e/cat
  cp /bin/toybox /tmp/e/head
  # (1) concurrent whole-file readers on 8 distinct large inodes
  #     (coalesced multi-block chains, one batch per task).
  for f in $FILES; do cat $f > /dev/null & done
  # (2) concurrent window readers on the fresh files (mixed 4MB reads,
  #     staggered offsets so tasks race on overlapping ranges).
  for f in /tmp/f1 /tmp/f2 /tmp/f3 /tmp/f4; do dd if=$f of=/dev/null bs=1048576 skip=$(( (r * 3) % 12 )) count=4 & done
  # (3) fork/exec/exit storm: short-lived processes whose execs batch-read
  #     cold pages while other batches are in flight, and whose exits
  #     recycle kernel stacks.
  e=0
  while [ $e -lt 100 ]; do
    /tmp/e/cat /etc/hostname > /dev/null 2>&1
    /tmp/e/head -c 4096 /bin/toybox > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  e=0
  while [ $e -lt 100 ]; do
    /tmp/e/cat /etc/passwd > /dev/null 2>&1
    /tmp/e/head -c 8192 /bin/mrsh > /dev/null 2>&1
    e=$(( e + 1 ))
  done &
  # (4) small-reader churn: short-lived forks interleaved with the batches.
  j=0
  while [ $j -lt 30 ]; do head -c 65536 /tmp/f2 > /dev/null & j=$(( j + 1 )); done
  wait
  rm -rf /tmp/f1 /tmp/f2 /tmp/f3 /tmp/f4 /tmp/e
  echo ROUND-$r-END
  r=$(( r + 1 ))
done
echo REPRO-SCRIPT-ALL-DONE
