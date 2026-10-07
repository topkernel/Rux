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
    subprocess.run(
        [CC, "-static", "-nostdlib", "-O2", "-o", out, src],
        check=True,
    )
    hello = os.path.join(HERE, "hello")
    subprocess.run(
        [CC, "-static", "-nostdlib", "-O2", "-DHELLO", "-o", hello, src],
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
    out += entry("TRAILER!!!", 0, 0, b"", 0)

    cpio = out
    blob = gzip.compress(cpio, compresslevel=9)

    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        HERE, "initrd.cpio.gz")
    with open(out_path, "wb") as f:
        f.write(blob)
    print(
        f"initrd: {out_path} cpio={len(cpio)}B gzip={len(blob)}B "
        f"init={len(init_data)}B hello={len(hello_data)}B"
    )


if __name__ == "__main__":
    main()
