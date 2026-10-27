#!/bin/bash
# pa55-ab.sh — A/B quick gate for the leg-36 wake/epoll empty fast
# paths (commit 6fdada6b): WaitQueueHead::waiters load-before-lock and
# EPOLL_SHARD_N skip-empty-shard. Protocol = the pa47 series (perf-
# report 8.10.4): pbench_boot.py quiet boots, release-strong kernels,
# TCG thread=multi smp2/2G, BOTH boot orders interleaved so host-load
# drift cancels; med-of-3 per arm (orders A,B,B,A,A,B), controls
# p1_null/p5a_pipe for load noise (p5a exercises the same waitqueue
# fast path only on the wake side; it is a CONTROL here because pipes
# wake the same way both arms).
#   A = run/pa55-a-ctl-bzImage  (bf349926, levers out)
#   B = run/pa55-b-fast-bzImage (6fdada6b, levers in)
# Success criterion (claim, not gate): B median < A median on
# p5b_dgram_send AND p5b_dgram_recv_peek in BOTH adjacent orderings;
# p5c within noise or better; no panics/OOM; DONE rc=0 every boot.
cd /home/william/rux-agents/perf-audit || exit 1
A=run/pa55-a-ctl-bzImage
B=run/pa55-b-fast-bzImage
LOG=run/pa55-ab.log
echo "=== pa55-ab start $(date '+%F %T') orders A,B,B,A,A,B ===" >> "$LOG"
i=0
for arm in A B B A A B; do
  i=$((i+1))
  tag="pa55${i}${arm}"
  k="$A"; [ "$arm" = "B" ] && k="$B"
  echo "--- boot $i arm=$arm tag=$tag $(date '+%F %T') ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa55-pb-${i}${arm}.log" 2>&1
  rc=$?
  echo "boot $i arm=$arm rc=$rc ($(date '+%F %T'))" >> "$LOG"
done
echo "=== pa55-ab complete $(date '+%F %T') ===" >> "$LOG"
