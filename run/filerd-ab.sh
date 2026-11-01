#!/bin/bash
# filerd-ab.sh — med-of-runs A/B for the bw_file_rd lane (perf-filerd).
#
# Boots K0 (base 09d2969c), K1 (+merged copyout), K2 (+stac/clac) with
# the committed rig shape (init=/test/lmbench_run, one case per boot),
# interleaved. A run is VOID and re-run when host loadavg(1min) >= 4 at
# boot time (lane rule). Writes tab-separated
#   kernel case run value load1 load5
# lines to the output file as it goes.
set -uo pipefail
WT=/home/william/rux-agents/perf-filerd
IMG="$WT/work/lmbench-x86.img"
OUT="$WT/run/filerd-ab-results.txt"
: > "$OUT"

boot_case() { # $1 kernel  $2 case (spec)  $3 label
    local kernel="$1" spec="$2" label="$3"
    local log="/tmp/filerd-ab-$$.log"
    local attempt=0
    while [ $attempt -lt 120 ]; do
        attempt=$((attempt + 1))
        local l1 l5
        read -r l1 l5 <<< "$(awk '{print $1, $2}' /proc/loadavg)"
        if awk "BEGIN{exit !($l1 >= 4)}"; then
            # Patient: sleep a minute and try again (up to ~2 h per boot).
            sleep 60
            continue
        fi
        timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
            -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
            -kernel "$kernel" \
            -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo "$spec" | tr ' ' ',') console=ttyS0" \
            -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
            -device virtio-blk-pci,drive=rootfs \
            -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
        local val
        val=$(tr -d '\r' < "$log" | awk '
            /LMBENCH TIMEOUT/ {print "TIMEOUT"; exit}
            /LMBENCH RUN/ {seen=1; next}
            !seen {next}
            /^[0-9]+\.[0-9]+ [0-9]+\.[0-9]+$/ {v=$2}
            /microseconds/ {for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\.[0-9]+$/){v=$i}}
            /LMBENCH DONE/ {if(v!="") {print v; exit}}
        ')
        if [ -n "$val" ] && [ "$val" != "TIMEOUT" ]; then
            echo -e "$label\t$spec\t$attempt\t$val\t$l1\t$l5" >> "$OUT"
            echo "OK $label $spec = $val (load $l1)" >&2
            rm -f "$log"
            return 0
        fi
        echo "RETRY $label $spec (no value, load $l1)" >&2
        sleep 20
    done
    echo -e "$label\t$spec\tFAIL\t-\t-\t-" >> "$OUT"
}

K0="$WT/work/k0-base.bzImage"
K1="$WT/work/k1-merge.bzImage"
K2="$WT/work/k3-holebreak.bzImage"

# Interleaved passes: bw_file_rd x5 on all three kernels (splits the two
# commits), neighbors x3 on K0 and K2.
for pass in 1 2 3 4 5; do
    boot_case "$K0" "bw_file_rd 1m io_only /tmp/lmbench.file" "K0-pass$pass"
    boot_case "$K1" "bw_file_rd 1m io_only /tmp/lmbench.file" "K1-pass$pass"
    boot_case "$K2" "bw_file_rd 1m io_only /tmp/lmbench.file" "K2-pass$pass"
done
for pass in 1 2 3; do
    boot_case "$K0" "bw_mmap_rd 1m mmap_only /tmp/lmbench.file" "K0-pass$pass"
    boot_case "$K2" "bw_mmap_rd 1m mmap_only /tmp/lmbench.file" "K2-pass$pass"
    boot_case "$K0" "lat_pagefault /tmp/lmbench.file" "K0-pass$pass"
    boot_case "$K2" "lat_pagefault /tmp/lmbench.file" "K2-pass$pass"
    boot_case "$K0" "lat_syscall stat" "K0-pass$pass"
    boot_case "$K2" "lat_syscall stat" "K2-pass$pass"
done
echo "A/B COMPLETE" >> "$OUT"
