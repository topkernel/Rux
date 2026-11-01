#!/bin/bash
# extract-probes.sh — restore the C probe sources and the x86_64 musl
# sysroot from their tracked tarballs.
#
# The repository deliberately does NOT track C source files (language-
# statistics policy, 2026-10-31): the x86_64 toolchain sysroot and the
# guest-side probe sources live in tarballs and are extracted on
# demand by the build. This script is idempotent — it extracts only
# what is missing.
#
# Hooked from the Makefile (user/test targets) and test/suite.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# x86_64 musl sysroot (headers + libc.a + musl-gcc wrapper)
if [ ! -x toolchain/x86_64-rux-linux-musl/bin/musl-gcc ]; then
    echo "extract-probes: restoring toolchain/x86_64-rux-linux-musl ..."
    tar -xzf toolchain/x86_64-rux-linux-musl.tar.gz
fi

# Guest-side probe sources: test/*.c, run/*.c, userspace smoke tests
if [ ! -f test/pbench.c ]; then
    echo "extract-probes: restoring C probe sources ..."
    tar -xzf test/probes.tar.gz
fi
