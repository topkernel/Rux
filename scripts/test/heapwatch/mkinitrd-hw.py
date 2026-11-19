#!/usr/bin/env python3
"""Pack a minimal initrd containing ONLY the heapwatch probe.

Avoids mkinitrd.py's freestanding /init build (host gcc emits libcalls
under -nostdlib); the kernel unpacks the cpio and runs init=/heapwatch
straight from the ramfs root.
"""
import gzip
import os
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CC = os.environ.get("CC", "riscv64-linux-gnu-gcc")


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
    src = os.path.join(HERE, "heapwatch.c")
    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        HERE, "initrd-hw.cpio.gz")
    out_bin = (os.path.splitext(out_path)[0] + ".bin")
    subprocess.run(
        [CC, "-static", "-pthread", "-O2", "-Wall", "-o", out_bin, src],
        check=True,
    )
    data = open(out_bin, "rb").read()

    out = b""
    out += entry(".", 0o040755, 0, b"", 1)
    out += entry("proc", 0o040755, 0, b"", 2)
    out += entry("heapwatch", 0o100755, len(data), data, 100)
    out += entry("TRAILER!!!", 0, 0, b"", 0)

    blob = gzip.compress(out, compresslevel=9)
    with open(out_path, "wb") as f:
        f.write(blob)
    print(f"heapwatch initrd: {out_path} cpio={len(out)}B gzip={len(blob)}B "
          f"heapwatch={len(data)}B")


if __name__ == "__main__":
    main()
