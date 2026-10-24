#!/bin/bash
# ww-pafix1.sh — smoke boot of the pa15-incident fix (WW IRQ-OFF-WINDOW
# probe: never walk a user-frame rsp; pt_regs_rip_for_cpu high-half guard
# on both arches). Worktree bzImage at HEAD+fix; short budget, own port,
# chain-style dfx flags. Validates boot health only — the user-frame
# guard itself is behavior-neutral on healthy paths (pure added guards).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export TAG=ww-pafix1
export GDBPORT=12377
export SMP=2
export SECS=330
export KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
export EXTRA_APPEND="dfx=wildwatch"
exec "$ROOT/run/ww-boot.sh"
