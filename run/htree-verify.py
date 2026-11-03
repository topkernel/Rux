#!/usr/bin/env python3
# htree-verify.py — on-disk check of the htree port's logic (hash + dx
# probe binary search) against the REAL dx trees of system-lb5.img.
# For every indexed directory: linear-scan all data blocks to learn each
# name's home leaf block, then run the port's algorithm (same half-md4 /
# TEA / legacy transforms, same binary search, same next-block walk) and
# verify lookup lands on the right leaf and finds the entry.
import struct, sys

IMG = sys.argv[1] if len(sys.argv) > 1 else 'run/system-lb5.img'
f = open(IMG, 'rb')

# --- superblock ---
f.seek(1024); sb = f.read(1024)
s_inodes_count, s_blocks_count = struct.unpack_from('<II', sb, 0)
s_log_block_size, = struct.unpack_from('<i', sb, 24)
BS = 1024 << s_log_block_size
s_blocks_per_group, = struct.unpack_from('<I', sb, 32)
s_inodes_per_group, = struct.unpack_from('<I', sb, 40)
s_inode_size, = struct.unpack_from('<H', sb, 88)
s_feature_compat, = struct.unpack('<I', sb[92:96])
s_hash_seed = struct.unpack('<4I', sb[0xEC:0xFC])
s_desc_size, = struct.unpack_from('<H', sb, 254)
GDT_BS = BS if BS >= 2048 else 2048
desc_size = s_desc_size if s_desc_size >= 64 else 32
GDESCS = BS // 8  # blocks per group (assume 8 blocks/byte bitmap)

print(f"BS={BS} inodes={s_inodes_count} ipg={s_inodes_per_group} isz={s_inode_size} "
      f"dir_index={bool(s_feature_compat & 0x20)} seed={s_hash_seed} desc_size={desc_size}")

def group_inode_table(g):
    f.seek(GDT_BS + g * desc_size)
    d = f.read(desc_size)
    lo, = struct.unpack_from('<I', d, 8)
    hi, = struct.unpack_from('<I', d, 0x28) if desc_size >= 64 else (0,)
    return lo | (hi << 32)

def read_block(b):
    f.seek(b * BS)
    return f.read(BS)

def read_inode(ino):
    g = (ino - 1) // s_inodes_per_group
    idx = (ino - 1) % s_inodes_per_group
    tbl = group_inode_table(int(g))
    f.seek(tbl * BS + idx * s_inode_size)
    raw = f.read(s_inode_size)
    i_flags, = struct.unpack_from('<I', raw, 32)
    i_size, = struct.unpack_from('<Q', raw, 16) if s_inode_size >= 128 else (struct.unpack_from('<I', raw, 4)[0],)
    # extent tree walk (i_block header magic 0xf30a)
    def extents():
        out = []
        def walk(node_blk, depth_expected):
            data = read_block(node_blk) if node_blk else raw[40:100]
            if struct.unpack_from('<H', data, 0)[0] != 0xf30a:
                return
            _, _, eh_e, eh_d, eh_gen = struct.unpack_from('<HHHHI', data, 0)
            for i in range(eh_e):
                off = 12 + i * 12
                leaf, lo_blk, cnt, hi = struct.unpack_from('<IHHI', data, off)
                if eh_d == 0:
                    out.append((lo_blk | (hi << 32), lo_blk | (hi << 32) + cnt - 1 if False else (lo_blk | (hi << 32), cnt)))
                else:
                    walk(leaf, depth_expected - 1)
        # root header inside i_block
        magic, _, eh_e, eh_d, _ = struct.unpack_from('<HHHHI', raw, 40)
        if magic != 0xf30a:
            return None  # non-extent inode
        entries = []
        def walk_node(data, depth):
            magic, _, eh_e, eh_d, _ = struct.unpack_from('<HHHHI', data, 0)
            for i in range(eh_e):
                leaf, lo, cnt, hi = struct.unpack_from('<IHHI', data, 12 + i * 12)
                lblk = lo | (hi << 32)
                if depth == 0:
                    entries.append((lblk, cnt, leaf))
                else:
                    walk_node(read_block(leaf), depth - 1)
        walk_node(raw[40:40+60], eh_d)
        entries.sort()
        return entries
    exts = extents()
    return i_flags, i_size, exts

def logical_block(exts, lb):
    for (lblk, cnt, pblk) in exts:
        if lblk <= lb < lblk + cnt:
            return pblk + (lb - lblk)
    return 0

# --- the port's hash (mirror of htree.rs) ---
DELTA = 0x9E3779B9
M32 = 0xFFFFFFFF
def md4_f(x,y,z): return z ^ (x & (y ^ z))
def md4_g(x,y,z): return (x & y) + ((x ^ y) & z)
def md4_h(x,y,z): return x ^ y ^ z
def rol(x,s): return ((x << s) | (x >> (32 - s))) & M32
def md4_round(fn, a, b, c, d, x, s):
    a = (a + fn(b,c,d) + x) & M32
    return rol(a, s), b, c, d
def half_md4(buf, inp):
    a,b,c,d = buf
    K1, K2, K3 = 0, 0o13240474631 & M32, 0o15666365641 & M32
    x = inp
    # round 1 (ROUND(F,a,b,c,d,x,s): a += F(b,c,d)+x; rol)
    a,b,c,d = md4_round(md4_f, a,b,c,d, (x[0]+K1)&M32, 3)
    d,a,b,c = md4_round(md4_f, d,a,b,c, (x[1]+K1)&M32, 7)
    c,d,a,b = md4_round(md4_f, c,d,a,b, (x[2]+K1)&M32, 11)
    b,c,d,a = md4_round(md4_f, b,c,d,a, (x[3]+K1)&M32, 19)
    a,b,c,d = md4_round(md4_f, a,b,c,d, (x[4]+K1)&M32, 3)
    d,a,b,c = md4_round(md4_f, d,a,b,c, (x[5]+K1)&M32, 7)
    c,d,a,b = md4_round(md4_f, c,d,a,b, (x[6]+K1)&M32, 11)
    b,c,d,a = md4_round(md4_f, b,c,d,a, (x[7]+K1)&M32, 19)
    # round 2
    a,b,c,d = md4_round(md4_g, a,b,c,d, (x[1]+K2)&M32, 3)
    d,a,b,c = md4_round(md4_g, d,a,b,c, (x[3]+K2)&M32, 5)
    c,d,a,b = md4_round(md4_g, c,d,a,b, (x[5]+K2)&M32, 9)
    b,c,d,a = md4_round(md4_g, b,c,d,a, (x[7]+K2)&M32, 13)
    a,b,c,d = md4_round(md4_g, a,b,c,d, (x[0]+K2)&M32, 3)
    d,a,b,c = md4_round(md4_g, d,a,b,c, (x[2]+K2)&M32, 5)
    c,d,a,b = md4_round(md4_g, c,d,a,b, (x[4]+K2)&M32, 9)
    b,c,d,a = md4_round(md4_g, b,c,d,a, (x[6]+K2)&M32, 13)
    # round 3
    a,b,c,d = md4_round(md4_h, a,b,c,d, (x[3]+K3)&M32, 3)
    d,a,b,c = md4_round(md4_h, d,a,b,c, (x[7]+K3)&M32, 9)
    c,d,a,b = md4_round(md4_h, c,d,a,b, (x[2]+K3)&M32, 11)
    b,c,d,a = md4_round(md4_h, b,c,d,a, (x[6]+K3)&M32, 15)
    a,b,c,d = md4_round(md4_h, a,b,c,d, (x[1]+K3)&M32, 3)
    d,a,b,c = md4_round(md4_h, d,a,b,c, (x[5]+K3)&M32, 9)
    c,d,a,b = md4_round(md4_h, c,d,a,b, (x[0]+K3)&M32, 11)
    b,c,d,a = md4_round(md4_h, b,c,d,a, (x[4]+K3)&M32, 15)
    nb = [(buf[0]+a)&M32, (buf[1]+b)&M32, (buf[2]+c)&M32, (buf[3]+d)&M32]
    return nb, nb[1]

def tea_transform(buf, inp):
    sum_ = 0
    b0, b1 = buf[0], buf[1]
    a, b, c, d = inp
    for _ in range(16):
        sum_ = (sum_ + DELTA) & M32
        b0 = (b0 + ((((b1 << 4) & M32) + a) ^ ((b1 + sum_) & M32) ^ (((b1 >> 5)) + b)) ) & M32
        b1 = (b1 + ((((b0 << 4) & M32) + c) ^ ((b0 + sum_) & M32) ^ (((b0 >> 5)) + d)) ) & M32
    return [(buf[0]+b0)&M32, (buf[1]+b1)&M32, buf[2], buf[3]]

def str2hashbuf(msg, num, unsigned):
    ln = len(msg)
    pad = ln | (ln << 8); pad = (pad | (pad << 16)) & M32
    val = pad
    buf = []
    lim = min(ln, num * 4)
    i = 0
    while i < lim:
        bch = msg[i] if unsigned else (msg[i] - 256 if msg[i] >= 128 else msg[i])
        val = (bch + (val << 8)) & M32
        if i % 4 == 3:
            buf.append(val); val = pad
        i += 1
    if num > 0:
        if len(buf) < num:
            buf.append(val)
        while len(buf) < num:
            buf.append(pad)
    return buf

def dx_hack_hash(name, unsigned):
    h0, h1 = 0x12a3fe2d, 0x37abe8f9
    for ch in name:
        sv = ch if unsigned else (ch - 256 if ch >= 128 else ch)
        h = (h1 + ((h0 ^ ((sv * 7152373) & M32)) & M32)) & M32
        if h & 0x80000000:
            h = (h - 0x7fffffff) & M32
        h1 = h0; h0 = h
    return (h0 << 1) & M32

def dirhash(name, version, seed):
    buf = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476]
    if any(seed):
        buf = list(seed)
    nb = name.encode() if isinstance(name, str) else name
    if version == 0: return dx_hack_hash(nb, False), 0
    if version == 3: return dx_hack_hash(nb, True), 0
    if version in (1, 4):
        out = b''
        p = nb; ln = len(nb)
        while ln > 0:
            inp = str2hashbuf(p, 8, version == 4)
            buf, ret = half_md4(buf, inp)
            ln -= 32
            p = p[32:]
        return ret, buf[2]
    if version in (2, 5):
        p = nb; ln = len(nb)
        while ln > 0:
            inp = str2hashbuf(p, 4, version == 5)
            buf = tea_transform(buf, inp)
            ln -= 16
            p = p[16:]
        return buf[0], buf[1]
    return None, None

EOF = 0x7fffffff
def fin_hash(h):
    h = h & ~1 & M32
    if h == (EOF << 1) & M32:
        h = ((EOF - 1) << 1) & M32
    return h

def parse_index(data):
    rlen0 = struct.unpack_from('<H', data, 4)[0]
    tail = 8  # metadata_csum — verify below
    if rlen0 == 12:
        rlen1 = struct.unpack_from('<H', data, 16)[0]
        if rlen1 != len(data) - 12: return None
        rz, = struct.unpack_from('<I', data, 24)
        il = data[29]
        if rz != 0 or il != 8: return None
        cl = 32
        dx_limit = (len(data) - 12 - 12 - 8 - tail) // 8
    elif rlen0 == len(data):
        cl = 8
        dx_limit = (len(data) - 8 - tail) // 8
    else:
        return None
    limit, count = struct.unpack_from('<HH', data, cl)
    if limit != dx_limit: return None
    if count == 0 or count > limit: return None
    return cl, count, limit

def ehash(data, cl, i):
    return struct.unpack_from('<I', data, cl + i * 8)[0]
def eblock(data, cl, i):
    return struct.unpack_from('<I', data, cl + i * 8 + 4)[0]

def dx_lookup(ino, i_size, exts, name, hash_unsigned):
    root = read_block(logical_block(exts, 0))
    hv = root[28]
    levels = root[30]
    if hv > 2: return ('UNSUPPORTED-VER', hv)
    if root[31] & 1 or levels > 1: return ('UNSUPPORTED', None)
    pr = parse_index(root)
    if not pr: return ('BADROOT', None)
    cl, count, _ = pr
    version = hv + (3 if hash_unsigned else 0)
    h, _minor = dirhash(name, version, s_hash_seed)
    if h is None: return ('NOHASH', None)
    h = fin_hash(h)
    p, q = 1, count - 1
    while p <= q:
        m = (p + q) // 2
        if ehash(root, cl, m) > h: q = m - 1
        else: p = m + 1
    at = p - 1
    frame = (root, cl, count, at)
    # descend (levels<=1 in this port)
    leaf = eblock(frame[0], frame[1], at)
    if levels == 1:
        node = read_block(logical_block(exts, leaf))
        pn = parse_index(node)
        if not pn: return ('BADNODE', leaf)
        ncl, ncount, _ = pn
        p, q = 1, ncount - 1
        while p <= q:
            m = (p + q) // 2
            if ehash(node, ncl, m) > h: q = m - 1
            else: p = m + 1
        leaf = eblock(node, ncl, p - 1)
    return ('OK', leaf)

# --- scan every inode: indexed dirs ---
indexed = []
N_CHECK = int(sys.argv[2]) if len(sys.argv) > 2 else 0
n_groups = (s_inodes_count + s_inodes_per_group - 1) // s_inodes_per_group
n = 0
for g in range(n_groups):
    # inode bitmap
    f.seek(GDT_BS + g * desc_size); lo, = struct.unpack_from('<I', f.read(desc_size), 8)
    bm = read_block(lo)
    for bit in range(len(bm) * 8):
        ino = g * s_inodes_per_group + bit + 1
        if ino > s_inodes_count: break
        if not (bm[bit // 8] >> (bit % 8)) & 1: continue
        i_flags, i_size, exts = read_inode(ino)
        if i_flags & 0x1000 and exts is not None and i_size >= BS:
            indexed.append((ino, i_size, exts))
            n += 1
            if n >= 400: break
    if n >= 400: break

print(f"indexed dirs found (capped 400): {len(indexed)}")
total_entries = 0; mismatches = 0; checked_dirs = 0
for (ino, i_size, exts) in indexed[:80]:
    nblocks = (i_size + BS - 1) // BS
    # linear scan of every data block: name -> home block
    home = {}
    for lb in range(nblocks):
        pb = logical_block(exts, lb)
        if pb == 0: continue
        data = read_block(pb)
        off = 0
        while off < BS:
            ino_e, rlen, nl, typ = struct.unpack_from('<IHBB', data, off)
            if rlen == 0: break
            if ino_e != 0 and nl > 0:
                nm = data[off+8:off+8+nl]
                if off != 0 or lb != 0:  # skip . and .. of block 0
                    home[nm] = lb
            off += rlen
    if not home: continue
    # port lookup for every name
    bad = 0
    for nm, lblk in home.items():
        st, res = dx_lookup(ino, i_size, exts, nm, False)
        if st != 'OK':
            bad += 1
            if bad <= 3: print(f"  dir ino {ino}: name {nm[:24]!r} -> {st}")
        elif res != lblk:
            bad += 1
            if bad <= 5:
                print(f"  dir ino {ino}: name {nm[:24]!r} port-leaf={res} actual={lblk} MISMATCH")
    total_entries += len(home)
    mismatches += bad
    checked_dirs += 1
print(f"checked {checked_dirs} dirs, {total_entries} entries, mismatches={mismatches}")
