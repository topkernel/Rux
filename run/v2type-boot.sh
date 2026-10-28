#!/bin/bash
# v2type-boot.sh — README v2 conversation boot (user feedback 2026-10-28).
#
# Types REAL commands into the live xterm on the GNOME desktop:
#   echo rux      -> shell prints rux
#   uname -r      -> kernel version line (6.8.0-rux)
# instead of the v1 'a' probes ("command not found" read as an error
# by outsiders). Spacing follows the r21type-proven shape: click-to-
# focus 15 s before the first key, one key every 2 s.
#
# BASE_T (2nd arg, default 1415) shifts the whole schedule: the r26
# merged tree collapses its session at ~1440-1470 s (v2a/v2b), so
# BASE_T=815 types at 815..831 (echo rux) and 865..881 (uname -r),
# finishing ~600 s before the observed collapse window. Shots every
# 30 s; SECS defaults to BASE_T+150.
#   800   click-to-focus the xterm at (480,380)
#   815..831    e c h o spc r u x ret        (echo rux)
#   865..881    u n a m e spc minus r ret    (uname -r)
# Verify with run/convcheck-v2.py (--shot for a specific frame);
# compose the README asset with run/readme-v2-compose.py.
#
# Usage: run/v2type-boot.sh TAG [BASE_T] [SECS]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:?usage: v2type-boot.sh TAG [BASE_T] [SECS]}"
BASE="${2:-815}"
SECS="${3:-$((BASE + 150))}"
CLICK="$((BASE - 15))"
b="$BASE"; c=$((BASE + 50))   # second command starts +50 s
KEYS="${b}:e,$((b+2)):c,$((b+4)):h,$((b+6)):o,$((b+8)):spc,$((b+10)):r,$((b+12)):u,$((b+14)):x,$((b+16)):ret,"
KEYS+="${c}:u,$((c+2)):n,$((c+4)):a,$((c+6)):m,$((c+8)):e,$((c+10)):spc,$((c+12)):minus,$((c+14)):r,$((c+16)):ret"
python3 "$ROOT/test/ubuntu-gnome/fire.py" \
  --tag "$TAG" --seconds "$SECS" --dump-every 30 \
  --dfx watchdog,inputtrace \
  --jiggle 200,30,"$SECS" \
  --clickprobe "$CLICK" \
  --keyprobe "$KEYS" \
  > "$ROOT/runs/${TAG#gnome-}-driver.log" 2>&1
