#!/bin/bash
# Restore guest test-case sources from the archive (user rule 2026-11-02:
# test cases are archived under userspace/ as tarball + extraction script
# only — no loose source trees in git). Safe to re-run.
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../.." && pwd)"
tar xzf "$DIR/probes.tar.gz" -C "$ROOT"
echo "test-case sources restored to test/ (and run/ where present)"
