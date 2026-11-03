#!/bin/bash
# run-refkvm.sh — the fair-rig reference column: the host's generic
# distro kernel (the SAME kernel binary the native reference column
# runs on) booted under the SAME QEMU KVM rig as the Rux column
# (run-rux.sh), running the same static lmbench binaries off the same
# Ubuntu rootfs shape. One case per boot, serial to file, in-guest
# case budget 35 s (lmbench_run default), outer boot cap 45 s.
#
# The ONLY deltas vs the Rux column's boot: -kernel points at the
# distro kernel, init=/test/init_ref (the wrapper that mounts
# proc/sys/devtmpfs — the reference kernel does not pre-mount /proc),
# and loglevel=4 (the distro kernel is chatty on serial; Rux needs no
# loglevel). Same machine, accel, cpu, memory, smp, drive, NIC,
# snapshot, serial and timeout.
#
# Usage: run-refkvm.sh [out.txt] [kernel] [img]
# Env:  OUTER=<secs>  outer boot cap   (default 45)
#       LOAD_MAX=<x>  skip ahead of a 1-min loadavg >= this (default 7;
#                     the perf-lane validity rule) — the script waits
#                     up to LOAD_WAIT minutes for a quieter window
#       LOAD_WAIT=<m> max minutes to wait for a load window (default 40)
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$WT/run/lmbench-refkvm.txt}"
KERNEL="${2:-$WT/run/refkvm/vmlinuz-7.0.0-38-generic}"
IMG="${3:-$WT/run/refkvm/lmbench-ref.img}"
OUTER="${OUTER:-45}"
LOAD_MAX="${LOAD_MAX:-7}"
LOAD_WAIT="${LOAD_WAIT:-40}"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT";; esac

[ -f "$KERNEL" ] || { echo "kernel missing: $KERNEL" >&2; exit 1; }
[ -f "$IMG" ] || { echo "rootfs missing: $IMG" >&2; exit 1; }

load1() { cut -d' ' -f1 /proc/loadavg; }

wait_for_window() {
    local waited=0
    while [ "$(echo "$(load1) >= $LOAD_MAX" | bc)" = "1" ]; do
        if [ "$waited" -ge $((LOAD_WAIT * 60)) ]; then
            echo "refkvm: WARNING load window never opened (load1=$(load1)) — proceeding" >&2
            return 1
        fi
        sleep 30
        waited=$((waited + 30))
    done
    return 0
}

: > "$OUT"
while read -r spec; do
    [ -n "$spec" ] || continue
    name="${spec%% *}"

    wait_for_window
    l=$(load1)
    echo "refkvm: [$name] load1=$l $(date +%H:%M:%S)" >&2

    # lat_ctx's auto-calibration outlives the 35 s in-guest budget under
    # KVM (same as the native column's noted 90-150 s windows); give it
    # budget=120 and a 150 s outer cap. Everything else fits the
    # standard 45 s cap.
    outer_i="$OUTER"
    extra_append=""
    if [ "$name" = "lat_ctx" ]; then
        outer_i=150
        extra_append=" budget=120"
    fi

    log="/tmp/refkvm-$name-$$.log"
    timeout "$outer_i" qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/init_ref lmbench=$(echo "$spec" | tr ' ' ',')$extra_append console=ttyS0 loglevel=4" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    rc=$?
    l2=$(load1)
    echo "===CASE $spec" >> "$OUT"
    tr -d '\r' < "$log" 2>/dev/null \
        | grep -aE "[0-9]+\.[0-9]+|^[0-9]+k|LMBENCH (TIMEOUT|EXEC-FAIL|NO-CASE)" >> "$OUT"
    echo "refkvm: [$name] qemu rc=$rc load1_end=$l2 $(grep -ac 'LMBENCH DONE' "$log" 2>/dev/null || true) done-marker(s)" >&2
    rm -f "$log"
done <<'CASES'
lat_syscall null
lat_syscall read
lat_syscall write
lat_syscall stat
lat_syscall fstat
lat_syscall open
lat_proc fork
lat_proc exec
lat_pipe
lat_ctx -s 64 2
CASES
echo "run-refkvm: results -> $OUT"
