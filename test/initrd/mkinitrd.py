#!/usr/bin/env python3
"""Build the Rux initrd boot-test image (OH Phase 1 prereq).

Stages a tiny initramfs (freestanding /init + /bin/hello + symlink +
hard link + dirs), packs it as cpio newc and gzips it — the same shape
as distro/OH ramdisks (`find | cpio -H newc | gzip`).

Usage: python3 test/initrd/mkinitrd.py [out.cpio.gz]
"""
import gzip
import os
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CC = os.environ.get("CC", "riscv64-linux-gnu-gcc")


def build_init():
    src = os.path.join(HERE, "init.c")
    out = os.path.join(HERE, "init")
    # Explicit -march: Ubuntu gcc 15's default ISA string includes zcb
    # (C.LBU) and friends, which the test QEMU's cpu string
    # (rv64,zbb,zba,zbs) does not implement — the freestanding init
    # SIGILLs at .text+0 on a bare C.LBU (observed: insn16=0x811c at
    # epc=.text start). Same lesson as probe6s: freestanding probes pin
    # the march to what the repo's QEMU drivers actually provide.
    # -fno-builtin: without it gcc self-folds the memset/memcpy shims in
    # init.c into recursive calls to themselves (observed: memset's fill
    # loop compiled to `jal memset`), which walks sp to the stack floor
    # and faults ~8MB below USER_STACK_TOP — looks exactly like a
    # stack-growth bug but is a build artifact.
    MARCH = os.environ.get(
        "MARCH", "-march=rv64imac -mabi=lp64 -fno-builtin")
    subprocess.run(
        [CC, "-static", "-nostdlib", "-O2"] + MARCH.split() + ["-o", out, src],
        check=True,
    )
    hello = os.path.join(HERE, "hello")
    subprocess.run(
        [CC, "-static", "-nostdlib", "-O2"] + MARCH.split() + ["-DHELLO", "-o", hello, src],
        check=True,
    )
    return out, hello


def field(v):
    return f"{v:08X}".encode()


def entry(name, mode, filesize, data, ino, nlink=1):
    hdr = b"070701"
    vals = [ino, mode, 0, 0, nlink, 0x5F000000, filesize, 0, 0, 0, 0,
            len(name) + 1, 0]
    for v in vals:
        hdr += field(v)
    rec = hdr + name.encode() + b"\x00"
    rec += b"\x00" * ((4 - len(rec) % 4) % 4)
    rec += data + b"\x00" * ((4 - len(data) % 4) % 4)
    return rec


def main():
    init, hello = build_init()
    init_data = open(init, "rb").read()
    hello_data = open(hello, "rb").read()

    out = b""
    out += entry(".", 0o040755, 0, b"", 1)
    out += entry("init", 0o100755, len(init_data), init_data, 100, 1)
    out += entry("bin", 0o040755, 0, b"", 101)
    # nlink=2 on BOTH entries: real cpio marks hard links this way.
    out += entry("bin/hello", 0o100755, len(hello_data), hello_data, 102, 2)
    out += entry("bin/lnk", 0o120777, 5, b"hello", 103)
    out += entry("bin/hard", 0o100644, 0, b"", 102, 2)
    out += entry("etc", 0o040755, 0, b"", 104)
    out += entry("etc/motd", 0o100644, 6, b"rux!\n\x00", 105)
    # A device node: must be skipped by the unpacker (rootfs has no
    # device nodes) — boot log reports it as "special skipped".
    out += entry("dev/console", 0o020600, 0, b"", 106)
    # heapwatch probe (static glibc + pthreads) when prebuilt next to the
    # source — boots as init=/heapwatch for the heap-corruption hunt
    # (see docs/development/heap-corruption-timeline.md).
    hw = os.path.join(HERE, "..", "heapwatch", "heapwatch")
    if os.path.isfile(hw):
        hw_data = open(hw, "rb").read()
        out += entry("heapwatch", 0o100755, len(hw_data), hw_data, 107)
        hw_note = f" heapwatch={len(hw_data)}B"
    else:
        hw_note = ""
    out += entry("TRAILER!!!", 0, 0, b"", 0)

    cpio = out
    blob = gzip.compress(cpio, compresslevel=9)

    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        HERE, "initrd.cpio.gz")
    with open(out_path, "wb") as f:
        f.write(blob)
    print(
        f"initrd: {out_path} cpio={len(cpio)}B gzip={len(blob)}B "
        f"init={len(init_data)}B hello={len(hello_data)}B{hw_note}"
    )


if __name__ == "__main__":
    main()
