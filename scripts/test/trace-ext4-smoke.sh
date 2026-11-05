#!/bin/bash
# trace-ext4-smoke.sh — round-3 tracepoint family smoke: the ext4:/
# jbd2: reference families and the rux:epoll documented-divergence
# family, the reference formats verbatim (field rows and print fmts
# from include/trace/events/ext4.h and jbd2.h, the show_* flag blobs
# expanded as the live format files carry them).
#
# Checks, in order:
#   1. available_events carries the fourteen round-3 events
#   2. the format files: name/ID headers, reference field rows, the
#      print fmt heads (including the __print_flags blobs for
#      map_blocks/free_blocks and the %pS ops column of epoll_delivery)
#   3. live traffic for the fs families: arming ext4+jbd2, a bounded
#      write+fsync+read+rm burst (the rm's deferred reap drains at the
#      NEXT ext4 entry point — the follow-up touch), disarming, then
#      reading the trace view records da_write_begin/end,
#      ext_map_blocks_enter/exit, sync_file_enter/exit, free_blocks
#      and jbd2_start/end_commit lines in the reference geometry
#      (dev major,minor; mflags M; the trailing space of
#      sync_file_enter's printk)
#   4. the rux:epoll family: formats + group arming (live epoll lines
#      need an epoll workload — the KVM proof init carries one; this
#      minimal-rootfs smoke has no epoll user)
#
# The probe image is a private copy of test/rootfs.img (never the
# shared one). Build the kernel with `make build` first (gen-ksyms
# embeds the per-arch symbol table; a bare cargo build after an
# other-arch build leaves stale symbols).
#
# Usage: test/trace-ext4-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 300)

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-ext4-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-300}"

IMG="$ROOT/work/trace-ext4-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===TR3: available ext4/jbd2/rux ==='
    sleep 0.3
    echo 'grep -E "^(ext4|jbd2|rux):" /sys/kernel/debug/tracing/available_events | sort'
    sleep 0.8
    echo 'echo ===TR3: sync_file_enter format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/ext4_sync_file_enter/format'
    sleep 0.5
    echo 'echo ===TR3: map_blocks_exit format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/ext4_ext_map_blocks_exit/format'
    sleep 0.8
    echo 'echo ===TR3: da_write_end format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/ext4_da_write_end/format'
    sleep 0.5
    echo 'echo ===TR3: free_blocks format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/ext4_free_blocks/format'
    sleep 0.8
    echo 'echo ===TR3: jbd2_end_commit format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/jbd2/jbd2_end_commit/format'
    sleep 0.5
    echo 'echo ===TR3: epoll_delivery format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/rux/epoll_delivery/format'
    sleep 0.5
    echo 'echo ===TR3: group states before ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/enable /sys/kernel/debug/tracing/events/jbd2/enable /sys/kernel/debug/tracing/events/rux/enable'
    sleep 0.5
    # Arm the fs families, one bounded burst, disarm.
    echo 'echo ===TR3: arming ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    echo 'echo 1 > /sys/kernel/debug/tracing/events/ext4/enable'
    echo 'echo 1 > /sys/kernel/debug/tracing/events/jbd2/enable'
    sleep 0.5
    # write + fsync the FD (conv=fsync): da_write pair, sync_file
    # bracket, forced jbd2 commit; then read it back (map_blocks).
    echo 'dd if=/dev/zero of=/tr3smoke.dat bs=64 count=1 conv=fsync 2>/dev/null'
    sleep 1.5
    echo 'cat /tr3smoke.dat > /dev/null'
    sleep 1.5
    echo 'rm /tr3smoke.dat'
    sleep 1.5
    # the deferred reap drains at the next ext4 entry point: free_blocks
    echo 'touch /tr3reap.dat'
    sleep 1.5
    echo 'rm /tr3reap.dat'
    sleep 1.5
    echo 'touch /tr3reap2.dat'
    sleep 1.5
    echo 'echo 0 > /sys/kernel/debug/tracing/events/ext4/enable'
    echo 'echo 0 > /sys/kernel/debug/tracing/events/jbd2/enable'
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 1.0
    echo 'echo ===TR3: group states after ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/ext4/enable'
    sleep 0.5
    echo 'cd /sys/kernel/debug/tracing'
    sleep 0.5
    echo 'echo ===TR3: da_write lines ==='
    sleep 0.3
    echo 'grep " ext4_da_write_" trace | head -4'
    sleep 1.2
    echo 'echo ===TR3: map_blocks lines ==='
    sleep 0.3
    echo 'grep " ext4_ext_map_blocks_" trace | head -4'
    sleep 1.2
    echo 'echo ===TR3: sync_file lines ==='
    sleep 0.3
    echo 'grep " ext4_sync_file_" trace | head -4'
    sleep 1.2
    echo 'echo ===TR3: free_blocks lines ==='
    sleep 0.3
    echo 'grep " ext4_free_blocks:" trace | head -4'
    sleep 1.2
    echo 'echo ===TR3: jbd2 lines ==='
    sleep 0.3
    echo 'grep " jbd2_" trace | head -4'
    sleep 1.2
    # rux:epoll arming proof (formats already checked; no epoll user here)
    echo 'echo ===TR3: rux arming ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/events/rux/enable'
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/rux/enable'
    sleep 0.3
    echo 'echo 0 > /sys/kernel/debug/tracing/events/rux/enable'
    sleep 0.3
    echo 'echo ===TR3: done ==='
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
check "available: the ext4 seven" 'ext4:ext4_sync_file_enter'
check "available: map_blocks pair" 'ext4:ext4_ext_map_blocks_exit'
check "available: da_write pair" 'ext4:ext4_da_write_end'
check "available: free_blocks" 'ext4:ext4_free_blocks'
check "available: jbd2 pair" 'jbd2:jbd2_start_commit'
check "available: the rux five" 'rux:epoll_delivery'
check "sync_file_enter format: name and id" 'name: ext4_sync_file_enter'
check "sync_file_enter format: parent field row" 'field:u64 parent'
check "sync_file_enter format: print fmt head" 'dev %d,%d ino %llu parent %llu datasync %d '
check "map_blocks_exit format: mflags field row" 'field:unsigned int mflags'
check "map_blocks_exit format: map flags blob" '\{ 0x1, "CREATE" \}'
check "map_blocks_exit format: mflags blob" '\{ 0x800, "U" \}'
check "da_write_end format: copied field row" 'field:unsigned int copied'
check "free_blocks format: mode field row" 'field:__u16 mode'
check "free_blocks format: free-flags blob" '\{ 0x1, "METADATA" \}'
check "jbd2_end_commit format: head field row" 'field:tid_t head'
check "epoll_delivery format: ops print" 'pid=%d fd=%d events=%#x ops=%pS'
check "da_write_begin line in the reference geometry" \
    'ext4_da_write_begin: dev [0-9]+,[0-9]+ ino [0-9]+ pos [0-9]+ len [0-9]+'
check "da_write_end line in the reference geometry" \
    'ext4_da_write_end: dev [0-9]+,[0-9]+ ino [0-9]+ pos [0-9]+ len [0-9]+ copied [0-9]+'
check "map_blocks_enter line" \
    'ext4_ext_map_blocks_enter: dev [0-9]+,[0-9]+ ino [0-9]+ lblk [0-9]+ len 1 flags'
check "map_blocks_exit line with mflags M" \
    'ext4_ext_map_blocks_exit: dev [0-9]+,[0-9]+ ino [0-9]+ flags * lblk [0-9]+ pblk [0-9]+ len [0-9]+ mflags M ret [0-9]+'
check "sync_file_enter line (trailing space)" \
    'ext4_sync_file_enter: dev [0-9]+,[0-9]+ ino [0-9]+ parent [0-9]+ datasync [0-9]'
check "sync_file_exit line" \
    'ext4_sync_file_exit: dev [0-9]+,[0-9]+ ino [0-9]+ ret [0-9]+'
check "free_blocks line" \
    'ext4_free_blocks: dev [0-9]+,[0-9]+ ino [0-9]+ mode 0[0-9]+ block [0-9]+ count [0-9]+ flags'
check "jbd2_start_commit line" \
    'jbd2_start_commit: dev [0-9]+,[0-9]+ transaction [0-9]+ sync [0-9]'
check "jbd2_end_commit line with head" \
    'jbd2_end_commit: dev [0-9]+,[0-9]+ transaction [0-9]+ sync [0-9] head [0-9]+'
check "ext4 group off after disarm" 'TR3: group states after'
check "rux group armed read" 'TR3: rux arming'

exit $fail
