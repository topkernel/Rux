#!/bin/bash
# tr1-kvm-proof.sh — dfx-trace round 1 serial proof on x86_64+KVM.
#
# The reference-style flow, on the CANONICAL tracefs path:
#   P1  layout: /sys/kernel/tracing lists the standard file set (options/
#       and per_cpu included); the debugfs alias shows the same instance.
#   P2  event grammar: bare-name set_event (the reference's group match),
#       per-event enable files, format/id files.
#   P3  record proof: enable the sched family + irq + printk + syscalls,
#       tracing_on, trace_marker brackets, fork workload, then the trace
#       read shows interleaved events with correct fields; trace_pipe
#       consumes; per-CPU stats report entries/overrun/read.
#   P4  options: context-info off drops the header; sym-addr/sym-offset
#       read the reference defaults.
#   P5  enabled-vs-disabled differential: a pure-syscall workload
#       (zero-reads) timed via /proc/uptime with every event off vs
#       raw_syscalls armed, alternating reps.
#
# Usage: run/tr1-kvm-proof.sh [bzImage] [logfile]
# Env:   BOOT_DELAY (default 45), TIMEOUT (default 420)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
LOG="${2:-$ROOT/run/tr1-kvm-proof-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-45}"
TIMEOUT="${TIMEOUT:-420}"
# The x86_64 proof image: a static C init (run/tr1-proof-init.c) on a
# small ext4 — the whole P1..P5 flow runs as PID 1 (no shell needed).
ROOTFS="$ROOT/run/tr1-rootfs.img"

mkdir -p "$ROOT/run"
rm -f "$LOG"

{
    # The static init (tr1-proof-init.c) drives the whole flow as PID 1;
    # stdin stays open until the timeout ends the VM.
    sleep "$TIMEOUT"
} | timeout "$TIMEOUT" qemu-system-x86_64 \
    -M q35 \
    -accel kvm \
    -cpu max \
    -m 2G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/init console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"
grep -n "===P[0-9]\|===PROOF-DONE\|KERNPANIC\|panic" "$LOG" | head -20
