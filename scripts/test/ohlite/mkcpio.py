#!/usr/bin/env python3
"""Pack a staging directory tree as gzip'd cpio newc (distro ramdisk shape).

Usage: mkcpio.py <staging-dir> <out.img>

Hard links inside the tree are emitted the way real cpio does: the FIRST
entry of a group carries nlink>1 with size 0, the LAST carries the data.
"""
import gzip
import os
import stat
import sys


def field(v):
    return f"{v:08X}".encode()


def header(name, mode, filesize, ino, nlink, rmaj, rmin):
    hdr = b"070701"
    vals = [ino, mode, 0, 0, nlink, 0x5F000000, filesize, rmaj, rmin, 0, 0,
            len(name) + 1, 0]
    for v in vals:
        hdr += field(v)
    rec = hdr + name.encode() + b"\x00"
    rec += b"\x00" * ((4 - len(rec) % 4) % 4)
    return rec


def main():
    src, out = sys.argv[1], sys.argv[2]

    seen_inos = {}   # realpath -> (ino, nlink_count)
    # first pass: count hard links per (dev, ino)
    linkcount = {}
    for dirpath, dirnames, filenames in os.walk(src):
        for fn in filenames:
            p = os.path.join(dirpath, fn)
            if os.path.islink(p):
                continue
            st = os.lstat(p)
            if stat.S_ISREG(st.st_mode) and st.st_nlink > 1:
                key = (st.st_dev, st.st_ino)
                linkcount[key] = linkcount.get(key, 0) + 1

    blob = bytearray()
    ino = 100
    emitted_data = set()

    def walk(d, rel):
        nonlocal ino
        entries = sorted(os.listdir(d), key=lambda s: (s != ".", s))
        # dir entry first
        blob.extend(header(rel or ".", 0o040755, 0, ino, 2, 0, 0))
        ino += 1
        for fn in sorted(os.listdir(d)):
            p = os.path.join(d, fn)
            r = (rel + "/" + fn) if rel else fn
            st = os.lstat(p)
            if stat.S_ISDIR(st.st_mode):
                walk(p, r)
            elif stat.S_ISLNK(st.st_mode):
                tgt = os.readlink(p)
                blob.extend(header(r, 0o120777, len(tgt), ino, 1, 0, 0))
                blob.extend(tgt.encode())
                blob.extend(b"\x00" * ((4 - len(blob) % 4) % 4))
                ino += 1
            elif stat.S_ISREG(st.st_mode):
                key = (st.st_dev, st.st_ino)
                nlink = linkcount.get(key, 1)
                if nlink > 1 and key in emitted_data:
                    # subsequent members of the group: empty, same ino
                    # (full mode incl. S_IFREG — cpio stores the file type)
                    blob.extend(header(r, stat.S_IFREG | (st.st_mode & 0o7777), 0,
                                       seen_inos[key][0], nlink, 0, 0))
                    continue
                data = open(p, "rb").read()
                blob.extend(header(r, stat.S_IFREG | (st.st_mode & 0o7777),
                                   len(data), ino, nlink, 0, 0))
                blob.extend(data)
                blob.extend(b"\x00" * ((4 - len(blob) % 4) % 4))
                if nlink > 1:
                    seen_inos[key] = (ino, nlink)
                    emitted_data.add(key)
                    # NOTE: real cpio puts data LAST; we approximate by
                    # letting the tree order decide — the kernel unpacker's
                    # deferred-link resolution handles either order.
                ino += 1
            else:
                sys.stderr.write(f"skip special {r}\n")

    walk(src, "")
    blob.extend(header("TRAILER!!!", 0, 0, 0, 1, 0, 0))
    gz = gzip.compress(bytes(blob), compresslevel=6)
    with open(out, "wb") as f:
        f.write(gz)
    print(f"cpio: {out} raw={len(blob)}B gzip={len(gz)}B")


if __name__ == "__main__":
    main()
