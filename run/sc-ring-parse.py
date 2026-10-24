#!/usr/bin/env python3
"""sc-ring-parse.py — extract the Rux printk ring from a plain ELF
guest physical memory dump (QEMU dump-guest-memory, no flags).

Usage: sc-ring-parse.py <dump.elf> <phys_hex> <len_hex>
Prints ring records (seq, level, pid, text) sorted by seq.
"""
import struct
import sys

PAGE = 4096


def segments(f):
    f.seek(0)
    hdr = f.read(64)
    assert hdr[:5] == b'\x7fELF\x02', 'not ELF64 LE'
    e_phoff = struct.unpack_from('<Q', hdr, 32)[0]
    e_phentsize, e_phnum = struct.unpack_from('<HH', hdr, 54)
    out = []
    for i in range(e_phnum):
        f.seek(e_phoff + i * e_phentsize)
        ph = f.read(56)
        t, _fl, off, _va, pa, fsz, _msz, _al = struct.unpack('<IIQQQQQQ', ph)
        if t == 1:
            out.append((pa, off, fsz))
    return out


def read_phys(f, segs, pa, n):
    out = bytearray()
    while n > 0:
        for spa, off, fsz in segs:
            if spa <= pa < spa + fsz:
                take = min(n, fsz - (pa - spa))
                f.seek(off + (pa - spa))
                out += f.read(take)
                pa += take
                n -= take
                break
        else:
            # not covered
            out += b'\x00' * min(n, PAGE)
            pa += min(n, PAGE)
            n -= min(n, PAGE)
    return bytes(out)


def main():
    dump, pa_hex, len_hex = sys.argv[1], sys.argv[2], sys.argv[3]
    pa, ln = int(pa_hex, 16), int(len_hex, 16)
    with open(dump, 'rb') as f:
        segs = segments(f)
        blob = read_phys(f, segs, pa, ln)

    # LogRecord (repr(C)): level@0 cpu_id@2 pad@4 pid@8 text_len@12
    # (pad to 16) seq@16 timestamp@24 text@32 — stride 288 (u64 align
    # pads seq past text_len; RECORD_TOTAL_SIZE 280 in the source is the
    # capacity constant's arithmetic, not the real stride).
    RECBYTES = 288
    TEXTOFF = 32
    n = (ln - 8) // RECBYTES
    recs = []
    for i in range(n):
        r = blob[8 + i * RECBYTES: 8 + (i + 1) * RECBYTES]
        if len(r) < RECBYTES:
            break
        pid, text_len = struct.unpack_from('<IH', r, 8)
        seq, ts = struct.unpack_from('<QQ', r, 16)
        if text_len == 0 or text_len > 256 or seq == 0:
            continue
        recs.append((seq, r[0], pid, r[TEXTOFF:TEXTOFF + text_len]))
    recs.sort()
    sys.stderr.write('# %d live records\n' % len(recs))
    for seq, level, pid, text in recs:
        s = text.decode('utf-8', 'replace').rstrip('\n')
        print('%d L%d p%d | %s' % (seq, level, pid, s))


if __name__ == '__main__':
    main()
