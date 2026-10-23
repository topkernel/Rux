#!/bin/bash
# ww-v2soak2.sh — session-29 re-run of the 45-min OH 0-panic gate for
# 4fb5d480 (reclaim-time dirty writeback v2). Session 28's ww-v2soak was
# killed at 20/45 by the user pause; this is the completed-gate re-run.
# Kernel: worktree build = byte-identical to session-28's v2 binary
# (md5 e5038877...). Same dfx flags as the ww-fms2 gate baseline
# (taskdump,watchdog,periodic,memwatch,ledger) so memfree_pages is
# comparable. Own QEMU only; -snapshot + file.locking=off via ww-boot.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export TAG=ww-v2soak2
export GDBPORT=12375
export SMP=2
export SECS=2700
export KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
export EXTRA_APPEND="dfx=taskdump,watchdog,periodic,memwatch,ledger"
exec "$ROOT/run/ww-boot.sh"
