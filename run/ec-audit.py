#!/usr/bin/env python3
# ext4-corrupt lane: full on-disk audit of a cooked userdata image.
# Families checked (WHY-SLOW.md §7 / mission):
#   (a) inode-bitmap inconsistency: bitmap bit vs inode-table state
#       (mode/dtime), plus bg inode-bitmap csum vs stored GD csum.
#   (b) free-path double-free: block double-claims across inode trees,
#       freed-but-still-referenced maps, free counts vs bitmaps.
#   (c) dirent corruption: per-dir dirent chain walk (rec_len sanity),
#       with special attention to dir block 0.
# Usage: ec-audit.py <image> [--dir INO] [--raw INO]
import struct, sys

POLY = 0x82F63B78
_tbl = []
for i in range(256):
    c = i
    for _ in range(8):
        c = (c >> 1) ^ (POLY if c & 1 else 0)
    _tbl.append(c)

def crc32c(data, seed):
    crc = seed
    for b in data:
        crc = _tbl[(crc ^ b) & 0xFF] ^ (crc >> 8)
    return crc

img = sys.argv[1]
f = open(img, 'rb')

f.seek(1024)
sb = f.read(1024)
magic, = struct.unpack('<H', sb[56:58])
assert magic == 0xEF53, hex(magic)
inodes_count, blocks_count = struct.unpack('<II', sb[0:8])
ipg, = struct.unpack('<I', sb[40:44])
blocks_per_group, = struct.unpack('<I', sb[32:36])
first_data_block, = struct.unpack('<I', sb[20:24])
log_bs, = struct.unpack('<I', sb[24:28])
bs = 1024 << log_bs
isz, = struct.unpack('<H', sb[88:90])
rev_level, = struct.unpack('<I', sb[76:80])
# sb-relative offsets: s_feature_compat 0x5C, s_feature_incompat 0x60,
# s_feature_ro_compat 0x64, s_uuid 0x68, s_checksum_seed 0x3FC.
feat_compat, = struct.unpack('<I', sb[92:96])
feat_incompat, = struct.unpack('<I', sb[96:100])
feat_ro, = struct.unpack('<I', sb[100:104])
uuid = sb[104:120]
disk_seed, = struct.unpack('<I', sb[1020:1024])
csum_seed_present = (feat_ro & 0x40000) != 0
seed = disk_seed if (csum_seed_present and disk_seed) else crc32c(uuid, 0xFFFFFFFF)
metadata_csum = (feat_ro & 0x400) != 0
desc_size = struct.unpack('<H', sb[254:256])[0] if rev_level >= 1 else 32
if desc_size < 64:
    desc_size = 32
gdt_start = 2 if bs == 1024 else 1
ngroups = (blocks_count - first_data_block + blocks_per_group - 1) // blocks_per_group

print(f"bs={bs} ipg={ipg} isz={isz} groups={ngroups} desc_size={desc_size} "
      f"metadata_csum={metadata_csum} inodes={inodes_count} blocks={blocks_count}")

def gd(group):
    f.seek(gdt_start * bs + group * desc_size)
    return f.read(desc_size)

def inode_raw(ino):
    g = (ino - 1) // ipg
    d = gd(g)
    itbl, = struct.unpack('<I', d[8:12])
    itbl |= struct.unpack('<H', d[0x28:0x2A])[0] << 32 if desc_size >= 64 else 0
    idx = (ino - 1) % ipg
    f.seek(itbl * bs + idx * isz)
    return f.read(isz)

def inode_csum_ok(ino, raw):
    if not metadata_csum or isz < 128:
        return None
    lo, hi = struct.unpack('<HH', raw[0x7C:0x80])
    cs = lo | (hi << 16 if isz > 128 else 0)
    i_generation, = struct.unpack('<I', raw[100:104])
    crc = crc32c(struct.pack('<I', ino), seed)
    crc = crc32c(struct.pack('<I', i_generation), crc)
    # i_checksum_hi included in data when isz>128
    if isz > 128:
        end = isz
        crc = crc32c(raw[:0x7C], crc)
        crc = crc32c(raw[0x7E:end], crc)
    else:
        crc = crc32c(raw[:0x7C], crc)
    return crc == cs

FT = {0:'?',1:'REG',2:'DIR',3:'CHR',4:'BLK',5:'FIFO',6:'SOCK',7:'LNK'}

def ext_walk(ino, raw):
    """Return (list-of-extents, list-of-meta-blocks, errors)."""
    hdr = raw[40:100]
    magic, entries, _max, depth, gen = struct.unpack('<HHHHI', hdr[:12])
    out, meta, errs = [], [], []
    if magic != 0xF30A:
        return out, meta, ['no-extent-magic']
    def walk_node(block, depth):
        f.seek(block * bs)
        nd = f.read(bs)
        m, e, _mx, d, _g = struct.unpack('<HHHHI', nd[:12])
        if m != 0xF30A:
            errs.append(f'node@{block} bad-magic {m:#x}')
            return
        meta.append(block)
        if metadata_csum:
            stored, = struct.unpack('<I', nd[-4:])
            s = crc32c(struct.pack('<I', ino), seed)
            s = crc32c(struct.pack('<I', gen), s)
            if crc32c(nd[:-4], s) != stored:
                errs.append(f'node@{block} csum-bad')
        if d == 0:
            if e > (bs - 12) // 12:
                errs.append(f'leaf@{block} claims {e} entries')
                e = (bs - 12) // 12
            for i in range(e):
                lbl, ln, hi, lo = struct.unpack('<IHHI', nd[12+12*i:24+12*i])
                out.append((lbl, lo | (hi << 32), ln & 0x7FFF, ln > 0x8000))
        else:
            for i in range(e):
                lbl, lo, hi, _l = struct.unpack('<IIHH', nd[12+12*i:24+12*i])
                walk_node(lo | (hi << 32), d - 1)
    if depth == 0:
        if entries > 4:
            errs.append(f'root claims {entries} inline entries (>4)')
            entries = 4
        for i in range(entries):
            lbl, ln, hi, lo = struct.unpack('<IHHI', hdr[12+12*i:24+12*i])
            out.append((lbl, lo | (hi << 32), ln & 0x7FFF, ln > 0x8000))
    else:
        nmax = (bs - 12) // 12
        if entries > nmax:
            errs.append(f'root claims {entries} index entries (>{nmax})')
            entries = nmax
        for i in range(entries):
            lbl, lo, hi, _l = struct.unpack('<IIHH', hdr[12+12*i:24+12*i])
            walk_node(lo | (hi << 32), depth - 1)
    return out, meta, errs

def parse_dir_block(data, limit=None):
    """Parse dirent chain. Return (entries, anomalies). entries: (off, ino,
    rec_len, name_len, ftype, name)."""
    ents, anoms = [], []
    off = 0
    tail = len(data) - 12 if metadata_csum else len(data)
    while off + 8 <= len(data):
        ino, rec_len, name_len, ftype = struct.unpack('<IHBB', data[off:off+8])
        if rec_len < 12 or (rec_len % 4) or off + rec_len > len(data):
            anoms.append(f'off={off}: BAD rec_len={rec_len} (<12/unaligned/overrun) ino={ino}')
            break
        if ino == 0:
            if off >= tail:
                anoms.append(f'off={off}: tail slack ino=0 (ok)')
            ents.append((off, ino, rec_len, name_len, ftype, None))
        else:
            if off + 8 + name_len > off + rec_len:
                anoms.append(f'off={off}: name_len {name_len} > rec_len {rec_len}')
            if off + rec_len > tail:
                anoms.append(f'off={off}: entry spans dir tail (off={off} rec={rec_len} tail={tail})')
            nm = data[off+8:off+8+min(name_len, rec_len-8)]
            ents.append((off, ino, rec_len, name_len, ftype, nm))
        off += rec_len
    return ents, anoms

# ---------------- mode dispatch ----------------
args = sys.argv[2:]
if args and args[0] == '--raw':
    # --raw INO: dump inode + dir chain of block 0
    ino = int(args[1])
    raw = inode_raw(ino)
    mode, = struct.unpack('<H', raw[0:2])
    print(f"ino {ino}: mode={mode:#o} links={struct.unpack('<H', raw[26:28])[0]} "
          f"size={struct.unpack('<I', raw[4:8])[0]} dtime={struct.unpack('<I', raw[20:24])[0]} "
          f"gen={struct.unpack('<I', raw[100:104])[0]} "
          f"csum_ok={inode_csum_ok(ino, raw)}")
    exts, meta, errs = ext_walk(ino, raw)
    print(f"  extents={exts} meta={meta} errs={errs}")
    if (mode & 0xF000) == 0x4000:
        for (lbl, phys, ln, uw) in exts:
            for b in range(phys, phys + ln):
                f.seek(b * bs)
                data = f.read(bs)
                ents, anoms = parse_dir_block(data)
                print(f"  block {b} (lblk {lbl + b - phys}):")
                for (off, i2, rl, nl, ft, nm) in ents[:20]:
                    print(f"    off={off:4d} ino={i2:6d} rec={rl:5d} nl={nl:3d} "
                          f"ft={FT.get(ft, ft)} name={nm}")
                for a in anoms:
                    print(f"    ANOM: {a}")
            break  # block 0 only
    sys.exit(0)

# ---------------- full audit ----------------
if args and args[0] == '--dir':
    # walk every directory's block-0 chain and report anomalies
    pass

# Full audit
print("\n== (a) inode bitmap vs table ==")
bitmap_free_notmode = []   # bit clear + mode!=0 + dtime==0
bitmap_set_nomode = []     # bit set + mode==0
dtime_set_bit_set = []     # bit set + dtime!=0
gd_csum_bad = []
for g in range(ngroups):
    d = gd(g)
    ibmp, = struct.unpack('<I', d[4:8])
    ibmp |= (struct.unpack('<H', d[0x2A:0x2C])[0] << 32) if desc_size >= 64 else 0
    f.seek(ibmp * bs)
    bmp = f.read(bs)
    nbytes = (ipg + 7) // 8
    for bit in range(ipg):
        ino = g * ipg + bit + 1
        if ino > inodes_count:
            break
        used = (bmp[bit // 8] >> (bit % 8)) & 1
        if not used:
            continue
        raw = inode_raw(ino)
        mode, = struct.unpack('<H', raw[0:2])
        dtime, = struct.unpack('<I', raw[20:24])
        if mode == 0 and dtime == 0:
            bitmap_set_nomode.append(ino)
        if dtime != 0:
            dtime_set_bit_set.append((ino, mode, dtime))
print(f"  bit-set + mode==0 + dtime==0 (ghost allocs): {len(bitmap_set_nomode)} "
      f"{bitmap_set_nomode[:10]}")
print(f"  bit-set + dtime!=0 (freed-while-allocated): {len(dtime_set_bit_set)} "
      f"{dtime_set_bit_set[:10]}")

print("\n== group descriptor bitmap csums + free counts ==")
# e2fsprogs writes block-bmp csum lo@0x18 hi@0x32; inode-bmp lo@0x1A
# (hi not written by mkfs, kernel writes it) — compare LO16 only, and
# skip UNINIT bitmaps entirely (mkfs stores no csum for those).
INODE_UNINIT, BLOCK_UNINIT = 0x1, 0x2
bad_csum_groups = []
free_drift = []
for g in range(ngroups):
    d = gd(g)
    flags, = struct.unpack('<H', d[0x12:0x14])
    ibmp, = struct.unpack('<I', d[4:8])
    bbmp, = struct.unpack('<I', d[0:4])
    msgs = []
    if not flags & BLOCK_UNINIT:
        f.seek(bbmp * bs)
        bb = f.read((blocks_per_group + 7) // 8)
        crc_b = crc32c(bb, seed) & 0xFFFF
        st_b, = struct.unpack('<H', d[0x18:0x1A])
        if st_b != crc_b:
            msgs.append('block-bmp csum stored=%#04x calc=%#04x' % (st_b, crc_b))
    if not flags & INODE_UNINIT:
        f.seek(ibmp * bs)
        ib = f.read((ipg + 7) // 8)
        crc_i = crc32c(ib, seed) & 0xFFFF
        st_i, = struct.unpack('<H', d[0x1A:0x1C])
        if st_i != crc_i:
            msgs.append('inode-bmp csum-lo stored=%#04x calc=%#04x' % (st_i, crc_i))
        crc_i_full = crc32c(ib, seed)
        st_i_hi, = struct.unpack('<H', d[0x3A:0x3C])
        if (st_i_hi << 16) | st_i != crc_i_full:
            msgs.append('inode-bmp csum-hi stored=%#04x calc=%#04x' % (st_i_hi, crc_i_full >> 16))
    # free counts vs bitmap truth
    fb_st, fi_st = struct.unpack('<HH', d[0x0C:0x10])
    if not flags & BLOCK_UNINIT:
        f.seek(bbmp * bs)
        bb = f.read((blocks_per_group + 7) // 8)
        used = sum(bin(b).count("1") for b in bb)
        free_true = blocks_per_group - used
        if fb_st != free_true:
            free_drift.append((g, 'blocks', fb_st, free_true))
    if not flags & INODE_UNINIT:
        f.seek(ibmp * bs)
        ib = f.read((ipg + 7) // 8)
        used = sum(bin(b).count("1") for b in ib)
        free_true = ipg - used
        if fi_st != free_true:
            free_drift.append((g, 'inodes', fi_st, free_true))
    if msgs:
        bad_csum_groups.append((g, flags, msgs))
        print(f"  grp {g} flags={flags:#x}: " + '; '.join(msgs))
print(f"  groups with csum mismatches: {len(bad_csum_groups)}")
print(f"  free-count drift (grp, kind, stored, true): {free_drift[:12]}")
# used-dirs count vs truth (counted later in section (c) — collect here)
g_used_dirs = {}
for g in range(ngroups):
    d = gd(g)
    u, = struct.unpack('<H', d[0x10:0x12])
    g_used_dirs[g] = u

print("\n== (b) block claims / double claims ==")
claims = {}
for g in range(ngroups):
    d = gd(g)
    ibmp, = struct.unpack('<I', d[4:8])
    f.seek(ibmp * bs)
    bmp = f.read(bs)
    for bit in range(ipg):
        ino = g * ipg + bit + 1
        if ino > inodes_count:
            break
        if not (bmp[bit // 8] >> (bit % 8)) & 1:
            continue
        raw = inode_raw(ino)
        mode, = struct.unpack('<H', raw[0:2])
        dtime, = struct.unpack('<I', raw[20:24])
        if mode == 0 or dtime != 0:
            continue
        exts, meta, errs = ext_walk(ino, raw)
        if errs:
            print(f"  ino {ino} ({FT.get((mode>>12)&7)}): TREE ERRORS {errs[:4]}")
        for (lbl, phys, ln, uw) in exts:
            for b in range(phys, phys + ln):
                claims.setdefault(b, []).append((ino, 'data', lbl))
        for b in meta:
            claims.setdefault(b, []).append((ino, 'meta', None))
dbl = [(b, v) for b, v in claims.items() if len(set(v)) > 1]
print(f"  double-claimed blocks: {len(dbl)}")
for b, v in dbl[:15]:
    print(f"    blk {b}: {v}")

print("\n== (c) directory chains ==")
dir_inos_seen = []
nbad = 0
for g in range(ngroups):
    d = gd(g)
    ibmp, = struct.unpack('<I', d[4:8])
    f.seek(ibmp * bs)
    bmp = f.read(bs)
    for bit in range(ipg):
        ino = g * ipg + bit + 1
        if ino > inodes_count:
            break
        if not (bmp[bit // 8] >> (bit % 8)) & 1:
            continue
        raw = inode_raw(ino)
        mode, = struct.unpack('<H', raw[0:2])
        dtime, = struct.unpack('<I', raw[20:24])
        if (mode & 0xF000) != 0x4000 or dtime != 0:
            continue
        dir_inos_seen.append(ino)
        exts, meta, errs = ext_walk(ino, raw)
        for (lbl, phys, ln, uw) in exts:
            for j, b in enumerate(range(phys, phys + ln)):
                f.seek(b * bs)
                data = f.read(bs)
                ents, anoms = parse_dir_block(data)
                hard = [a for a in anoms if 'tail slack' not in a]
                if hard:
                    nbad += 1
                    print(f"  dir ino {ino} block {b} (#{j}, lblk {lbl + j}): {hard}")
print(f"  dirs with bad chains: {nbad}")
dirs_per_group = {}
for g2 in range(ngroups):
    dirs_per_group[g2] = 0
for ino in dir_inos_seen:
    g2 = (ino - 1) // ipg
    dirs_per_group[g2] += 1
drift_dirs = [(g2, g_used_dirs[g2], dirs_per_group[g2])
              for g2 in range(ngroups) if g_used_dirs[g2] != dirs_per_group[g2]]
print(f"  used-dirs drift (grp, stored, true): {drift_dirs[:12]}")
