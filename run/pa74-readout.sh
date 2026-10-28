#!/bin/bash
# pa74-readout.sh — extract p5 (and p1/p2 sanity) medians from the four
# pa74 pbench boot logs and print the M/B comparison with % delta.
# Gate reminder (IPC domain): P5 rows within +-20% of control or
# improved; the lever's target = p5c2/p5d_rt_read toward the 5-8 us
# class; p5c3/p5d_rt_recv/p5f are recvfrom-shaped (should be flat).
cd "$(dirname "$0")/.." || exit 1

row() {  # $1=logfile $2=row-name
    grep -o "PBENCH $2 n=[0-9]* med=[0-9.]*" "$1" 2>/dev/null | head -1 | sed 's/.*med=//'
}

declare -A logs
for i in 1 2 3 4; do
  for arm in M B; do
    f="run/pa74-pb-${i}${arm}.log"
    [ -f "$f" ] && logs[${i}${arm}]="$f"
  done
done

rows="p5a_pipe p5b_dgram_send p5b_dgram_recv_peek p5c_stream_write p5c2_stream_send p5c3_stream_wrecv p5d_rt_read p5d_rt_recv p5e_stream_send p5e_child_spin p5f_stream_dwait p1_clock p2_ctxsw"

printf "%-22s" "row"
for k in "${!logs[@]}"; do printf " %8s" "$k"; done
echo "   B_vs_M%"
for r in $rows; do
  printf "%-22s" "$r"
  line=""
  for i in 1 2 3 4; do
    for arm in M B; do
      k="${i}${arm}"; f="${logs[$k]:-}"
      v=$(row "$f" "$r"); [ -z "$v" ] && v="-"
      printf " %8s" "$v"
    done
  done
  # aggregate: best-pair comparison B(3,4 avg) vs M(1,4 avg)
  m1=$(row "${logs[1M]:-}" "$r"); m4=$(row "${logs[4M]:-}" "$r")
  b2=$(row "${logs[2B]:-}" "$r"); b3=$(row "${logs[3B]:-}" "$r")
  if [ -n "$m1" ] && [ -n "$m4" ] && [ -n "$b2" ] && [ -n "$b3" ]; then
    m=$(awk "BEGIN{print ($m1+$m4)/2}"); b=$(awk "BEGIN{print ($b2+$b3)/2}")
    awk "BEGIN{printf \"   %+.1f%%\", ($b-$m)*100/$m}"
  else
    echo -n "   n/a"
  fi
  echo ""
done
