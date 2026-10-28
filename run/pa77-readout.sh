#!/bin/bash
# pa77-readout.sh — extract p5 (and p1/p2 sanity) medians from the four
# pa77 board logs plus the pin-window rows from the two pin logs; print
# the M/B comparison with % delta. Gate (IPC domain): P5 rows within
# +-20% of control or improved; pin_r64/pin_r4k should hold their
# improved class; pin_recv64/spread are untouched-class sanity.
cd "$(dirname "$0")/.." || exit 1

row() {  # $1=logfile $2=row-name
    grep -o "PBENCH $2 n=[0-9]* med=[0-9.]*" "$1" 2>/dev/null | head -1 | sed 's/.*med=//'
}

declare -A logs
for i in 1 2 3 4; do
  for arm in M B; do
    f="run/pa77-pb-${i}${arm}.log"
    [ -f "$f" ] && logs[${i}${arm}]="$f"
  done
done

rows="p5a_pipe p5b_dgram_send p5b_dgram_recv_peek p5c_stream_write p5c2_stream_send p5c3_stream_wrecv p5d_rt_read p5d_rt_recv p5e_stream_send p5e_child_spin p5f_stream_dwait p1_clock p2_ctxsw"

echo "== board rows (med us) =="
printf "%-22s" "row"
for k in "${!logs[@]}"; do printf " %8s" "$k"; done
echo "   B_vs_M%"
for r in $rows; do
    printf "%-22s" "$r"
    for i in 1 2 3 4; do
        for arm in M B; do
            k="${i}${arm}"
            if [ -n "${logs[$k]}" ]; then
                v=$(row "${logs[$k]}" "$r")
                printf " %8s" "${v:--}"
                eval "${r}_${k}=\$v"
            fi
        done
    done
    # per-pair deltas
    line=""
    for pair in "1M 2B" "3B 4M"; do
        set -- $pair
        a="${r}_$1"; b="${r}_$2"
        if [ -n "${!a}" ] && [ -n "${!b}" ] && [ "${!a}" != "-" ] && [ "${!b}" != "-" ]; then
            d=$(awk -v b="${!b}" -v a="${!a}" 'BEGIN{printf "%+.1f", (b-a)/a*100}')
            line="$line $1->$2:$d%"
        fi
    done
    echo "$line"
done

echo
echo "== pin windows (med us, adjacent M then B) =="
for r in pin_r64 pin_r4k pin_recv64 spread_r64; do
    m=$(row run/pa77-pinM.log "$r")
    b=$(row run/pa77-pinB.log "$r")
    d=$(awk -v b="$b" -v m="$m" 'BEGIN{printf "%+.1f", (b-m)/m*100}')
    printf "%-14s M=%-10s B=%-10s B_vs_M: %s%%\n" "$r" "$m" "$b" "$d"
done
