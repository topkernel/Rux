#!/usr/bin/env python3
"""musl_build.py — the shared musl-SDK probe build recipe for the boot
drivers.

Debian/Ubuntu static glibc ships riscv vector stringops whose ifunc
default SIGILLs on the no-V QEMU boot CPU REGARDLESS of -march (see the
note in test/mkrootfs.sh; observed again on gcc 15.2 when the binder
drivers' plain `-static -O2` builds started trapping at insn16=0x7057).
Every probe that links a libc must therefore be built against the
project's riscv64 musl SDK, which this module does in one call:

    from musl_build import build_musl
    ok = build_musl("binder_probe.c", out_path)

The SDK is looked up in this worktree first and the shared main repo
second (the same fallback mkrootfs.sh uses), so the drivers work from
lane worktrees that never built `make sdk` themselves.
"""

import os
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
WT = os.path.dirname(HERE)
CC = "riscv64-linux-gnu-gcc"
MARCH = "rv64gc_zba_zbb_zbs"

_SDK_CANDIDATES = [
    os.path.join(WT, "toolchain", "riscv64-rux-linux-musl"),
    "/home/william/workspace/Rux/toolchain/riscv64-rux-linux-musl",
]


def sdk():
    for cand in _SDK_CANDIDATES:
        if os.path.isfile(os.path.join(cand, "lib", "crt1.o")):
            return cand
    return None


def build_musl(src_name, out, extra_args=(), sdk_path=None):
    """Build test/<src_name> against the musl SDK; True on success."""
    m = sdk_path or sdk()
    if m is None:
        print("[build] no riscv64 musl SDK (make sdk) in %s"
              % ":".join(_SDK_CANDIDATES))
        return False
    libgcc = subprocess.run(
        [CC, "-march=" + MARCH, "-print-libgcc-file-name"],
        capture_output=True, text=True).stdout.strip()
    cmd = [CC, "-march=" + MARCH, "-static", "-O2", "-Wall",
           "-nostdinc", "-isystem", os.path.join(m, "include"),
           "-nostdlib",
           "-o", out, os.path.join(HERE, src_name),
           os.path.join(m, "lib", "crt1.o"),
           os.path.join(m, "lib", "crti.o"),
           os.path.join(m, "lib", "crtn.o"),
           "-L", os.path.join(m, "lib"), "-lc", libgcc] + list(extra_args)
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("[build] compile failed: %s" % r.stderr)
        return False
    return True
