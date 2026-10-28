#!/bin/bash
# probe-common.sh — shared engine for the thin suite probe wrappers.
#
# Sourced by test/<name>-probe.sh / <name>-smoke.sh wrappers; provides
# the two wrapper shapes used across the fleet:
#
#   leg_run_driver   execute a python boot driver (binder family,
#                    faultlat, ...) and translate its exit code into a
#                    PROBE-RESULT line
#   leg_boot_rootfs  boot a PRIVATE copy of test/rootfs.img with the
#                    probe injected at /test/<name> via debugfs, feed a
#                    scripted shell session, and leave the serial log
#                    in $LEG_LOG for the wrapper's verdict greps
#
# Conventions every wrapper follows (matching test/suite.sh verdict_for):
#   * the FINAL line is exactly "PROBE-RESULT: PASS|FAIL|SKIP <detail>"
#   * exit code 0 = pass, 1 = fail, 3 = skipped (prerequisite missing)
#   * a guest-side panic line in the log always fails the leg
#   * -snapshot + file.locking=off, -smp 2 -m 1G, self-timeout
#
# Environment overrides:
#   LEG_KERNEL   kernel ELF (default the built riscv64 one)
#   LEG_TIMECAP  hard cap for one QEMU boot (default 240 s)
#   LEG_TC       thread=single|multi (default single — deterministic,
#                the same choice the standing smokes make)

# shellcheck shell=bash disable=SC2317

LEG_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEG_KERNEL="${LEG_KERNEL:-$LEG_ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LEG_TIMECAP="${LEG_TIMECAP:-240}"
LEG_TC="${LEG_TC:-single}"
LEG_LOGDIR="$LEG_ROOT/run"
LEG_LOG="${LEG_LOG:-$LEG_LOGDIR/leg-serial.log}"

leg_rc=0

leg_result() {  # leg_result PASS|FAIL|SKIP detail
    echo "PROBE-RESULT: $1 $2"
    case "$1" in
        PASS) exit 0 ;;
        SKIP) exit 3 ;;
        *)    exit 1 ;;
    esac
}

leg_need_kernel() {
    [ -f "$LEG_KERNEL" ] || leg_result SKIP "kernel not built at $LEG_KERNEL (run make build)"
}

leg_qemu_riscv_flags() {  # echoed: the standard machine flags
    echo "-M virt -accel tcg,thread=$LEG_TC -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp 2 -display none -serial stdio -snapshot"
}

# ---- shape 1: python boot driver ---------------------------------------
#
# leg_run_driver <script.py> [args...] — the driver's own stdout/err
# (serial markers included) lands in the leg log; rc translation:
#   0 -> PASS, 1 -> FAIL, 2 -> FAIL (build/setup), 124 -> FAIL (timeout)
leg_run_driver() {
    local script="$1"; shift
    leg_need_kernel
    command -v python3 >/dev/null 2>&1 || leg_result SKIP "python3 not found"
    mkdir -p "$LEG_LOGDIR"
    : > "$LEG_LOG"
    timeout "$LEG_TIMECAP" python3 "$LEG_ROOT/$script" "$@" >> "$LEG_LOG" 2>&1
    local rc=$?
    tail -20 "$LEG_LOG"
    case "$rc" in
        0) leg_result PASS "driver $script completed cleanly" ;;
        124) leg_result FAIL "driver $script timed out after ${LEG_TIMECAP}s" ;;
        *) leg_result FAIL "driver $script exited rc=$rc" ;;
    esac
}

# ---- shape 2: boot the rootfs and run the probe -------------------------
#
# leg_boot_rootfs <name> <probe-binary> <boot-delay> <guest-script-file>
#
# Boots init=/bin/sh on a private image copy (never the shared
# test/rootfs.img), runs the guest script from stdin, captures serial.
# The wrapper then greps $LEG_LOG for the probe's markers.
leg_boot_rootfs() {
    local name="$1" bin="$2" delay="$3" script="$4"
    leg_need_kernel
    local img_root="$LEG_ROOT/test/rootfs.img"
    [ -f "$img_root" ] || leg_result SKIP "test/rootfs.img not built (run make rootfs)"
    command -v qemu-system-riscv64 >/dev/null 2>&1 || leg_result SKIP "qemu-system-riscv64 not found"
    command -v debugfs >/dev/null 2>&1 || leg_result SKIP "debugfs (e2fsprogs) not found"
    [ -x "$bin" ] || leg_result SKIP "probe binary not built: $bin"

    local img="$LEG_ROOT/work/$name-leg.img"
    mkdir -p "$LEG_ROOT/run" "$LEG_ROOT/work"
    rm -f "$img" "$LEG_LOG"
    cp --reflink=auto "$img_root" "$img" 2>/dev/null || cp "$img_root" "$img"

    # Inject the probe at /test/<name> (overwrites a stale bake).
    debugfs -w -R "rm /test/$name" "$img" >/dev/null 2>&1
    debugfs -w -R "write $bin $name" "$img" >/dev/null 2>&1
    debugfs -w -R "sif /test/$name mode 0100755" "$img" >/dev/null 2>&1
    if ! debugfs -R "stat /test/$name" "$img" 2>/dev/null | grep -q "Mode:.*755"; then
        rm -f "$img"
        leg_result SKIP "debugfs injection of /test/$name failed"
    fi

    {
        sleep "$delay"
        cat "$script"
        sleep 4
    } | timeout "$LEG_TIMECAP" qemu-system-riscv64 \
        $(leg_qemu_riscv_flags) \
        -drive file="$img",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$LEG_KERNEL" \
        -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
        > "$LEG_LOG" 2>&1
    local rc=$?
    rm -f "$img"
    [ "$rc" = 0 ] || [ "$rc" = 124 ] || leg_result FAIL "qemu exited rc=$rc"
    return 0
}

# leg_panic_check — a kernel panic in the log fails any leg outright.
leg_panic_check() {
    if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LEG_LOG"; then
        grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LEG_LOG" | head -3
        leg_result FAIL "kernel panic/oops in the serial log"
    fi
}

# leg_boot_ok — the shell prompt session actually started (the first
# guest echo marker arrived); guards against silent early-boot hangs.
leg_boot_ok() {
    local marker="$1"
    if ! grep -aq "$marker" "$LEG_LOG"; then
        leg_result FAIL "guest session never started (no '$marker' in serial)"
    fi
}
