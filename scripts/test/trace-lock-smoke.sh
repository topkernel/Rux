#!/bin/bash
# trace-lock-smoke.sh — round-4 tracepoint family smoke: the lock:
# reference family (lock_acquire/lock_acquired/lock_contended/
# lock_release + the lockdep-free contention_begin/contention_end
# pair), formats verbatim from include/trace/events/lock.h, and the
# round-4 tcp:/sock: catalog rows (live TCP lines are the KVM proof
# init's job — this minimal-rootfs smoke has no socket workload).
# Checks, in order:
#   1. available_events carries the ten round-4 events
#   2. the format files: lock_acquire's try/read flag printk,
#      the lock-class "%p %s" shape, contention_begin's LCB flag blob,
#      contention_end's ret, inet_sock_set_state's protocol/state blob
#   3. live traffic: arming the lock group while the biglock_fair
#      probe hammers the SleepMutex big locks (hammer threads + a
#      create/write/unlink churn thread; the rootfs shell cannot
#      background jobs — setpgid EPERM — so the storm runs in the
#      foreground via /test/biglock_fair). lock_acquire/lock_release
#      on every acquire; contended/acquired + contention_begin/end
#      under the collision; group on/off switches the records on/off.
# The probe image is a private copy of test/rootfs.img (never the
# shared one). Build the kernel with `make build` first (gen-ksyms
# embeds the per-arch symbol table; a bare cargo build after an
# other-arch build leaves stale symbols).
# Usage: scripts/test/trace-lock-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 480)

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-lock-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-480}"

IMG="$ROOT/work/trace-lock-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===TR4: available lock/tcp/sock rows ==='
    sleep 0.3
    echo 'grep -E "^(lock|tcp|sock):" /sys/kernel/debug/tracing/available_events | sort'
    sleep 0.8
    echo 'echo ===TR4: lock_acquire format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/lock_acquire/format'
    sleep 0.5
    echo 'echo ===TR4: lock_contended format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/lock_contended/format'
    sleep 0.5
    echo 'echo ===TR4: contention_begin format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/contention_begin/format'
    sleep 0.5
    echo 'echo ===TR4: contention_end format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/contention_end/format'
    sleep 0.5
    echo 'echo ===TR4: inet_sock_set_state format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/sock/inet_sock_set_state/format'
    sleep 0.8
    echo 'echo ===TR4: group states before ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/enable'
    sleep 0.5
    # Arm the lock family, run the big-lock contention probe, disarm.
    # (The rootfs shell cannot background jobs — setpgid EPERM spam —
    # so the storm is /test/biglock_fair: hammer threads + a
    # create/write/unlink churn thread colliding on the SleepMutex big
    # locks, in the FOREGROUND.)
    echo 'echo ===TR4: arming + biglock_fair storm ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    echo 'echo 1 > /sys/kernel/debug/tracing/events/lock/enable'
    sleep 0.5
    echo '/test/biglock_fair 6 3000'
    sleep 60
    echo 'echo 0 > /sys/kernel/debug/tracing/events/lock/enable'
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 1.0
    echo 'echo ===TR4: group state after ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/lock/enable'
    sleep 0.5
    echo 'cd /sys/kernel/debug/tracing'
    sleep 0.5
    echo 'echo ===TR4: lock_acquire lines ==='
    sleep 0.3
    echo 'grep " lock_acquire:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: lock_contended lines ==='
    sleep 0.3
    echo 'grep " lock_contended:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: lock_acquired lines ==='
    sleep 0.3
    echo 'grep " lock_acquired:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: lock_release lines ==='
    sleep 0.3
    echo 'grep " lock_release:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: contention_begin lines ==='
    sleep 0.3
    echo 'grep " contention_begin:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: contention_end lines ==='
    sleep 0.3
    echo 'grep " contention_end:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR4: done ==='
    sleep 2
} | timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

fail=0
check() {
    local what="$1" pat="$2"
    if grep -a -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "available: the lock six" 'lock:lock_acquire'
check "available: contention pair" 'lock:contention_begin'
check "available: lock_release" 'lock:lock_release'
check "available: the tcp three" 'tcp:tcp_retransmit_skb'
check "available: inet state store" 'sock:inet_sock_set_state'
check "lock_acquire format: name and id" 'name: lock_acquire'
check "lock_acquire format: flags field row" 'field:unsigned int flags'
check "lock_acquire format: try/read printk" 'try \\?" :'
check "lock_acquire format: name string field" 'field:__data_loc char\[\] name'
check "lock_contended format: name and id" 'name: lock_contended'
check "lock_contended format: %p %s printk" '%p %s'
check "contention_begin format: flags blob" '\{ 0x20, "MUTEX" \}'
check "contention_end format: ret printk" 'ret=%d'
check "inet_sock_set_state format: protocol blob" 'show_inet_protocol_name'
check "inet_sock_set_state format: state blobs" 'show_tcp_state_name\(REC->oldstate\)'
check "lock_acquire line in the reference geometry" \
    'lock_acquire: 0x[0-9a-f]+ [A-Z0-9-]+[[:space:]]*$'
check "lock_contended line (big-lock name)" \
    'lock_contended: 0x[0-9a-f]+ (VFS-MUTATION|EXT4-BIGLOCK)[[:space:]]*$'
check "lock_acquired line" 'lock_acquired: 0x[0-9a-f]+ [A-Z0-9-]+[[:space:]]*$'
check "lock_release line" 'lock_release: 0x[0-9a-f]+ [A-Z0-9-]+[[:space:]]*$'
check "contention_begin line with SPIN flag" \
    'contention_begin: 0x[0-9a-f]+ \(flags=SPIN\)[[:space:]]*$'
check "contention_end line with ret" 'contention_end: 0x[0-9a-f]+ \(ret=0\)[[:space:]]*$'
check "lock group off after disarm" 'TR4: group state after'

exit $fail
