#!/bin/bash
# pop-hunt.sh — golden-population hunt on a PRISTINE userdata copy.
#
# The golden population = CreateNewUser(100) running AFTER the first-boot
# BMS scan has populated the pre-install KVS (lb65's shape: bundle size:34)
# and PERSISTING it (NOSNAP). The known losing order (lb59): creation runs
# BEFORE bundles.ready, reads an empty store, and persists user 100 with
# ZERO per-user records — that copy can never produce the launcher
# (CheckUserParam passes forever, creation never re-runs).
#
# This watcher enforces the order at runtime:
#   - "bootevent.bms.main.bundles.ready" set  -> READY=1 (creation is safe)
#   - CreateNewUser START while READY=0        -> kill the VM FAST, before
#     the creation txn completes and poisons the copy with empty records
#     (creation runs 20-30s start->end; the kill window is generous)
#   - "bundle size"                            -> GOLDEN. Leave running.
#   - SwitchToUser / userId-not-exist          -> lineage dead end; kill.
# Only PIDs started by this script are killed.
cd "$(dirname "$0")/.." || exit 1
TAG=$1
SER="run/oh-${TAG}-serial.log"
# Resolve the REAL python driver: a bare "lb-boot.py $TAG" pattern also
# matches agent wrapper shells whose command line quotes the launch
# command — anchor on the python executable and require the qemu child.
DRV=$(pgrep -f "^python3 run/lb-boot.py $TAG " | head -1)
[ -z "$DRV" ] && DRV=$(pgrep -f "lb-boot.py $TAG " | while read -r p; do
  c=$(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null)
  case "$c" in python3*) echo "$p"; break;; esac
done)
[ -z "$DRV" ] && { echo "[hunt] no driver for $TAG"; exit 1; }
QEMU=$(pgrep -P "$DRV" | head -1)
echo "[hunt] $TAG driver=$DRV qemu=$QEMU watch $(date +%H:%M:%S)"
READY=0
while kill -0 "$DRV" 2>/dev/null; do
  if grep -q 'bootevent.bms.main.bundles.ready' "$SER" 2>/dev/null; then
    [ "$READY" = 0 ] && echo "[hunt] $TAG bundles.ready SET at $(date +%H:%M:%S) — creation now safe"
    READY=1
  fi
  if grep -q 'bundle size' "$SER" 2>/dev/null; then
    echo "[hunt] $TAG *** GOLDEN POPULATION — bundle size landed $(date +%H:%M:%S) — VM LEFT RUNNING ***"
    exit 0
  fi
  if grep -qE 'CreateNewUser|GenerateUserKeys' "$SER" 2>/dev/null; then
    if [ "$READY" = 1 ]; then
      echo "[hunt] $TAG creation AFTER ready — riding it $(date +%H:%M:%S)"
      # continue watching for bundle size / failure
      tail -F "$SER" 2>/dev/null | while read -r l; do
        case "$l" in
          *'bundle size'*) echo "[hunt] $TAG *** GOLDEN *** $(date +%H:%M:%S)"; exit 0;;
          *'SwitchToUser'*) echo "[hunt] $TAG creation done, SwitchToUser $(date +%H:%M:%S)"; exit 0;;
        esac
      done
      exit 0
    else
      echo "[hunt] $TAG creation STARTED BEFORE ready — KILLING to protect the copy $(date +%H:%M:%S)"
      kill "$QEMU" 2>/dev/null; kill "$DRV" 2>/dev/null
      exit 2
    fi
  fi
  if grep -q 'userId not exist' "$SER" 2>/dev/null; then
    echo "[hunt] $TAG shapeB — killing $(date +%H:%M:%S)"
    kill "$QEMU" 2>/dev/null; kill "$DRV" 2>/dev/null
    exit 3
  fi
  sleep 2
done
echo "[hunt] $TAG driver exited $(date +%H:%M:%S)"
