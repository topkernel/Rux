#!/bin/bash
# lostwake-matrix.sh — run the lostwake reproducer matrix on the minimal
# rootfs rig (wake-fastrepro lane).
#
# Each leg boots the x86_64 kernel (KVM, q35, SMP, serial to file) with
# the lostwake probe as init and a surface subset via lw.surfs= cmdline
# tokens. ~40 s per leg including boot.
#
# usage: run/lostwake-matrix.sh [secs]      (default 30)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
SECS="${1:-30}"

run() { # tag surfs [extra-append]
    local tag="$1" surfs="$2" extra="${3:-}"
    local log="$WT/run/lostwake-$tag.log"
    python3 "$WT/run/lostwake_boot.py" --secs "$SECS" --mem 2G \
        --surfs "$surfs" --tag "$tag" --append "loglevel=4 $extra" \
        > /dev/null 2>&1
    local rc=$?
    local lost kdiag
    lost=$(grep -ac "LW: LOSTWAKE" "$log" 2>/dev/null || true)
    kdiag=$(grep -acE "WAKE-|W2-|W3-|STALE-" "$log" 2>/dev/null || true)
    printf "%-12s surfs=%-6s rc=%d lostwakes=%s kernel_diag=%s log=%s\n" \
        "$tag" "${surfs:-all}" "$rc" "$lost" "$kdiag" "$log"
}

echo "== lostwake matrix (${SECS}s legs, KVM q35 smp8 2G) =="
run all      ""     # full mix (futex epoll nano svc fork itimer)
run noitimer fpnsk  # - setitimer
run nofork   fpns   # - fork storm
run nosvc    fpn    # - kill-cycle (expected clean: kill is necessary)
run fplusn   fn     # futex + nano only (expected clean)
run minimal  fpns   # the smallest reliable failing set (re-run)
run svconly  s      # kill-cycle alone (low rate: ~1 hit / 30 s)
echo "== done =="
