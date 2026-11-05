#!/bin/bash
# oh-mon.sh — send one QEMU monitor command to a running oh-boot instance.
# Usage: oh-mon.sh <tag> <command...>   e.g. oh-mon.sh b1 "screendump /tmp/x.ppm"
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TAG="${1:?usage: oh-mon.sh <tag> <command>}"
shift
{ sleep 0.1; printf '%s\n' "$*"; sleep 0.5; } | nc -U -q 1 "$ROOT/run/oh-$TAG-mon.sock" || true
