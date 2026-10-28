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
# No LEG_TIMECAP default here: each wrapper pins its own budget with
# ': "${LEG_TIMECAP:=<seconds>}"' right after sourcing (env overrides
# still win).
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

# ---- probe builds -------------------------------------------------------
#
# Two recipes, both no-V safe (the gcc-15.2 static-glibc vector-ifunc
# SIGILL documented in mkrootfs.sh; see test/musl_build.py for the long
# story). leg_build_musl needs the project musl SDK, falling back to
# the shared main-repo copy exactly like mkrootfs.sh does.

leg_musl_sdk() {
    local d
    for d in "$LEG_ROOT/toolchain/riscv64-rux-linux-musl" \
             "/home/william/workspace/Rux/toolchain/riscv64-rux-linux-musl"; do
        [ -f "$d/lib/crt1.o" ] && { echo "$d"; return 0; }
    done
    return 1
}

# leg_build_musl <src.c> <out> [extra flags...]
leg_build_musl() {
    local src="$1" out="$2"; shift 2
    local sdk
    sdk="$(leg_musl_sdk)" || leg_result SKIP "riscv64 musl SDK missing (make sdk)"
    command -v riscv64-linux-gnu-gcc >/dev/null 2>&1 || leg_result SKIP "riscv64-linux-gnu-gcc not found"
    mkdir -p "$(dirname "$out")"
    riscv64-linux-gnu-gcc -march=rv64gc_zba_zbb_zbs -static -O2 -Wall \
        -nostdinc -isystem "$sdk/include" -nostdlib \
        -o "$out" "$LEG_ROOT/$src" \
        "$sdk/lib/crt1.o" "$sdk/lib/crti.o" "$sdk/lib/crtn.o" \
        -L"$sdk/lib" -lc \
        "$(riscv64-linux-gnu-gcc -march=rv64gc_zba_zbb_zbs -print-libgcc-file-name)" \
        "$@" || leg_result FAIL "musl build of $src failed"
}

# leg_build_free <src.c> <out> [extra flags...] — freestanding, no libc
leg_build_free() {
    local src="$1" out="$2"; shift 2
    command -v riscv64-linux-gnu-gcc >/dev/null 2>&1 || leg_result SKIP "riscv64-linux-gnu-gcc not found"
    mkdir -p "$(dirname "$out")"
    riscv64-linux-gnu-gcc -march=rv64gc_zba_zbb_zbs -nostdlib -nostartfiles \
        -static -O2 -fno-builtin -o "$out" "$LEG_ROOT/$src" "$@" \
        || leg_result FAIL "freestanding build of $src failed"
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

# ---- shape 1b: x86 python boot driver -----------------------------------
#
# leg_run_x86_driver <script.py> [args...] — same contract, prereq is
# the x86 bzImage instead of the riscv ELF (the x86 drivers default to
# the worktree bzImage path, which is exactly LEG_X86_KERNEL).
LEG_X86_KERNEL="${LEG_X86_KERNEL:-$LEG_ROOT/target/x86_64-unknown-none/debug/bzImage}"

leg_need_x86_kernel() {
    [ -f "$LEG_X86_KERNEL" ] || leg_result SKIP "x86 bzImage not built at $LEG_X86_KERNEL (PLATFORM=x86_64 make build)"
}

leg_run_x86_driver() {
    local script="$1"; shift
    leg_need_x86_kernel
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

# leg_x86_musl_loader — materialize the dynamic musl loader the older
# x86 drivers expect at fixed /tmp paths (/tmp/ohroot/lib64,
# /tmp/ww-loader). musl's libc.so IS the loader; the canonical copy in
# this tree is run/oh-x/rd/lib/ld-musl-x86_64.so.1 (untracked OH
# bring-up artifact). Prints nothing; caller handles absence.
leg_x86_musl_loader() {
    local src="$LEG_ROOT/run/oh-x/rd/lib/ld-musl-x86_64.so.1"
    [ -f "$src" ] || return 1
    mkdir -p /tmp/ohroot/lib64 /tmp/ww-loader
    [ -f /tmp/ohroot/lib64/ld-musl-x86_64.so.1 ] || cp "$src" /tmp/ohroot/lib64/ld-musl-x86_64.so.1
    [ -f /tmp/ohroot/lib64/libc.so ] || cp "$src" /tmp/ohroot/lib64/libc.so
    [ -f /tmp/ww-loader/ld-musl.so ] || cp "$src" /tmp/ww-loader/ld-musl.so
    return 0
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

    # Inject the probe at /test/<name> (overwrites a stale bake). The
    # write must run in the same debugfs session as the `cd` — each
    # -R invocation is a separate process with CWD /.
    local cmdfile="$LEG_ROOT/work/leg-inject.cmd"
    printf 'rm /test/%s\ncd /test\nwrite %s %s\nsif /test/%s mode 0100755\n' \
        "$name" "$bin" "$name" "$name" > "$cmdfile"
    debugfs -w -f "$cmdfile" "$img" >/dev/null 2>&1
    if ! debugfs -R "stat /test/$name" "$img" 2>/dev/null | grep -q "Mode:.*755"; then
        rm -f "$img"
        leg_result SKIP "debugfs injection of /test/$name failed"
    fi

    # Feed the guest session through a fifo and watch for the done
    # marker: an idle serial console does not exit on stdin EOF, so a
    # plain pipeline would idle until the time cap on every boot. When
    # the marker lands (or the cap expires) the qemu WE started is
    # terminated by pid — never another process.
    local fifo="$LEG_ROOT/work/leg-$name.fifo"
    rm -f "$fifo"; mkfifo "$fifo"
    qemu-system-riscv64 $(leg_qemu_riscv_flags) \
        -drive file="$img",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$LEG_KERNEL" \
        -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
        < "$fifo" > "$LEG_LOG" 2>&1 &
    local qpid=$!
    exec 3> "$fifo"
    sleep "$delay"
    cat "$script" >&3
    sleep 4
    exec 3>&-
    local waited=0
    while kill -0 "$qpid" 2>/dev/null && [ "$waited" -lt "$LEG_TIMECAP" ]; do
        if grep -aq "===LEG: done" "$LEG_LOG" 2>/dev/null; then
            sleep 3   # let the trailing serial bytes drain
            break
        fi
        sleep 2
        waited=$((waited + 2))
    done
    if kill -0 "$qpid" 2>/dev/null; then
        kill -TERM "$qpid" 2>/dev/null
        sleep 2
        kill -KILL "$qpid" 2>/dev/null
    fi
    wait "$qpid" 2>/dev/null
    rm -f "$fifo" "$img"
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
