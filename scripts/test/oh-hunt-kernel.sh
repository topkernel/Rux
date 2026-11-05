#!/bin/bash
# oh-hunt-kernel.sh — build the "every wedge names its syscall" kernel.
#
# Sprint recipe (ui-launcher timer/dfx lane): one ready-made kernel build
# where each taskdump line carries, for EVERY task (D-state included):
#
#   uepc=/usp=      the user instruction/stack a sleeper is blocked at
#   nr=a0=a1=a2=    the LAST SYSCALL + first three args — a D-state task
#                   names its blocking syscall on the spot (nr=25 pselect,
#                   nr=202 futex, nr=232 epoll_wait, ...)
#   sp= <words>     kernel stack words (return addrs; symbolize with
#                   riscv64-linux-gnu-addr2line / addr2line -e rux)
#
# plus, with the runtime switches below:
#   dfx=periodic    a full task snapshot every 5 s — silent wedges land on
#                   serial even when nothing trips a watchdog
#   dfx=taskdump    the "DFX!" UART magic takes an on-demand snapshot
#   dfx=monowatch   ~1 Hz CLOCK_MONOTONIC jump tripwire (clocksource named)
#   dfx=ledger      "LEDGER!" UART magic + OOM-moment page accounting
#   dfx=watchdog    spinlock-stuck prints append the task snapshot
#
# Build features used (kernel/Cargo.toml):
#   dfx-taskdump-bt   the per-task bt/nr/args lines (taskdump.rs)
#   dfx-futex-trace   futex sleeper word + waiter-chain dump in taskdump,
#                     and (with dfx=futextrace at runtime) per-op traces
#   dfx-lock-owner    deadlock prints carry holder=<cpu> (optional; adds
#                     two atomic stores per spinlock pair — include when
#                     lock ordering is in scope)
#
# Usage:
#   test/oh-hunt-kernel.sh [--with-lock-owner]
#
# Output: target/x86_64-unknown-none/debug/bzImage-hunt
#         (a separate name — never clobbers the plain bzImage other
#          flows boot). Boot it with the OH driver and:
#
#   EXTRA_APPEND="dfx=periodic,taskdump,monowatch" \
#       SMP=2 MEM=4G ACCEL=kvm ./run/oh-boot.sh hunt <bzImage-hunt path>
#
# Notes:
# - The extra code SHIFTS BINARY LAYOUT and can widen heap-lock races:
#   this is a hunt kernel, not a production gate. Reproduce first on the
#   plain build, then attribute with this one.
# - Cost at rest: taskdump-bt only prints on dumps; periodic snapshots
#   are ~1 page of serial per 5 s — mind 57k-line logs on long soaks
#   (hilogd.rd_kmsg copies kmsg to /data; see SyscallTrace history).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

FEATURES="x86_64,dfx-taskdump-bt,dfx-futex-trace"
if [ "${1:-}" = "--with-lock-owner" ]; then
    FEATURES="$FEATURES,dfx-lock-owner"
fi

echo "[hunt-kernel] building with features: $FEATURES"
cargo build --target x86_64-unknown-none --no-default-features --features "$FEATURES"
python3 build/mkbzimage.py \
    target/x86_64-unknown-none/debug/rux \
    target/x86_64-unknown-none/debug/bzImage-hunt

echo "[hunt-kernel] done: target/x86_64-unknown-none/debug/bzImage-hunt"
echo "[hunt-kernel] boot with EXTRA_APPEND='dfx=periodic,taskdump,monowatch'"
