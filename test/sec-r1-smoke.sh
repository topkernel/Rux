#!/bin/bash
# sec-r1-smoke.sh — security round sec-r1 A/B smoke:
#   1. capability gates (test/caps_probe.c) — root and nobody phases
#   2. W^X execstack A/B (test/wx_probe.c built twice: default + -z
#      execstack) and READ_IMPLIES_EXEC personality
# on the riscv64 minimal rootfs (private copy, -snapshot).
#
# Boots the kernel on a private copy of test/rootfs.img with the probes
# compiled in (musl-static; the shared image is never written) and greps
# the probes' PASS/FAIL lines plus the serial-clean mandate.
#
# Usage: test/sec-r1-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 420)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/sec-r1-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-420}"

IMG="$ROOT/work/sec-r1-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"

MUSL="$ROOT/toolchain/riscv64-rux-linux-musl"
GCCINC="$(riscv64-linux-gnu-gcc -print-file-name=include)"
CC() {
    riscv64-linux-gnu-gcc -static -nostdlib -nostdinc \
        -isystem "$MUSL/include" -isystem "$GCCINC" \
        -Os -march=rv64gc -D_FORTIFY_SOURCE=0 -fno-stack-protector "$@" \
        -L"$MUSL/lib" -lc -lgcc
}

echo "sec-r1-smoke: building probes..."
CC -o "$ROOT/work/caps_probe" "$MUSL/lib/crt1.o" "$ROOT/test/caps_probe.c" \
    || { echo "caps_probe build failed"; exit 1; }
CC -o "$ROOT/work/wx_probe_nx" "$MUSL/lib/crt1.o" "$ROOT/test/wx_probe.c" \
    || { echo "wx_probe (nx) build failed"; exit 1; }
CC -z execstack -DWX_EXECSTACK -o "$ROOT/work/wx_probe_xs" "$MUSL/lib/crt1.o" \
    "$ROOT/test/wx_probe.c" \
    || { echo "wx_probe (execstack) build failed"; exit 1; }

# Sanity: the two wx variants must carry different PT_GNU_STACK flags.
for v in nx xs; do
    riscv64-linux-gnu-readelf -W -l "$ROOT/work/wx_probe_$v" \
        | grep -q GNU_STACK || { echo "wx_probe_$v: no GNU_STACK phdr"; exit 1; }
done
XS_FLAGS="$(riscv64-linux-gnu-readelf -W -l "$ROOT/work/wx_probe_xs" | awk '/GNU_STACK/ {print $7}')"
NX_FLAGS="$(riscv64-linux-gnu-readelf -W -l "$ROOT/work/wx_probe_nx" | awk '/GNU_STACK/ {print $7}')"
[ "$XS_FLAGS" = "RWE" ] || { echo "execstack variant flags: '$XS_FLAGS' (want RWE)"; exit 1; }
[ "$NX_FLAGS" = "RW" ]  || { echo "nx variant flags: '$NX_FLAGS' (want RW)"; exit 1; }
echo "sec-r1-smoke: GNU_STACK flags nx='$NX_FLAGS' xs='$XS_FLAGS' (as expected)"

inject() { # inject <src> <name>
    local src="$1" name="$2"
    debugfs -w -R "rm /test/$name" "$IMG" >/dev/null 2>&1
    debugfs -w -R "rm /$name" "$IMG" >/dev/null 2>&1
    debugfs -w -R "write $src /$name" "$IMG" >/dev/null 2>&1
    local ino
    ino=$(debugfs -R "stat /$name" "$IMG" 2>/dev/null | sed -n 's/^Inode: \([0-9]*\).*/\1/p' | head -1)
    if [ -n "$ino" ]; then
        debugfs -w -R "ln <${ino}> /test/$name" "$IMG" >/dev/null 2>&1
        debugfs -w -R "unlink /$name" "$IMG" >/dev/null 2>&1
    fi
}

cp "$ROOT/test/rootfs.img" "$IMG"
inject "$ROOT/work/caps_probe" caps_probe
inject "$ROOT/work/wx_probe_nx" wx_probe_nx
inject "$ROOT/work/wx_probe_xs" wx_probe_xs

{
    sleep "$BOOT_DELAY"
    echo 'mkdir -p /tmp'
    echo '/test/caps_probe'
    sleep 15
    echo '/test/wx_probe_nx'
    sleep 10
    echo '/test/wx_probe_xs'
    sleep 10
    echo 'echo ===SEC-R1-DONE==='
    sleep 3
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

# The serial console emits CRLF; strip CR so anchored greps behave.
LOGC="$LOG.crless"
tr -d '\r' < "$LOG" > "$LOGC"
LOG="$LOGC"

fail=0
check() {
    local what="$1" pat="$2"
    if LC_ALL=C grep -aEq "$pat" "$LOG"; then
        echo "sec-r1-smoke: PASS $what"
    else
        echo "sec-r1-smoke: FAIL $what (pattern: $pat)"
        fail=1
    fi
}

if grep -aEq "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG"; then
    grep -aE "KERNPANIC|Kernel panic|panic!|Oops:" "$LOG" | head -3
    echo "sec-r1-smoke: FAIL kernel panic/oops in the serial log"
    fail=1
fi

# Capability wiring (both phases must be all-PASS).
for t in root-afpacket-ok root-rawicmp-ok root-raw-proto0-EPROTONOSUPPORT \
         root-acct-ok root-setfcap-ok root-setfcap-remove-ok \
         root-ifwrite-past-cap-ENODEV \
         drop-to-nobody nobody-afpacket-EPERM nobody-rawicmp-EPERM \
         nobody-raw-proto0-EPROTONOSUPPORT nobody-acct-EPERM \
         nobody-setfcap-EPERM nobody-ifwrite-EPERM \
         nobody-perf-nokernel-EACCES nobody-perf-exckernel-not-EACCES; do
    check "caps: $t" "SEC-R1-CAPS: PASS $t\$"
done
if grep -aq "SEC-R1-CAPS: FAIL" "$LOG"; then
    echo "sec-r1-smoke: FAIL caps: FAIL lines present"
    fail=1
else
    echo "sec-r1-smoke: PASS caps: no FAIL lines"
fi
check "caps: done summary" "SEC-R1-CAPS: done pass=16 fail=0"

# W^X: NX variant faults the stack fetch; execstack variant runs it;
# READ_IMPLIES_EXEC makes the PROT_READ probe SIGILL (fetch allowed).
for t in stack-nx-faulted mprotect-rx-jit rie-baseline-nx file-rx-fetch \
         personality-set-RIE rie-read-map-exec; do
    check "wx(nx): $t" "SEC-R1-WX: PASS $t\$"
done
for t in stack-exec-enabled-ran mprotect-rx-jit rie-baseline-nx file-rx-fetch \
         personality-set-RIE rie-read-map-exec; do
    check "wx(xs): $t" "SEC-R1-WX: PASS $t\$"
done
if grep -aq "SEC-R1-WX: FAIL" "$LOG"; then
    echo "sec-r1-smoke: FAIL wx: FAIL lines present"
    fail=1
else
    echo "sec-r1-smoke: PASS wx: no FAIL lines"
fi
check "wx(nx): done summary" "SEC-R1-WX: done pass=6 fail=0"
check "wx(xs): done summary" "SEC-R1-WX: done pass=6 fail=0"

check "probe sequence completed" "===SEC-R1-DONE==="

if [ "$fail" = 0 ]; then
    echo "sec-r1-smoke: ALL GREEN"
else
    echo "sec-r1-smoke: FAILURES PRESENT"
fi
exit "$fail"
