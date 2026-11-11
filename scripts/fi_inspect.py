#!/usr/bin/env python3
"""fi_inspect.py — host-side ext4 inspector for the data-integrity lane.

Why this exists: the OH userdata image carries the metadata_csum feature
and the kernel's inode writes do not keep the checksum valid, so e2fsprogs
(debugfs/e2fsck) REFUSE to read the very inodes we need to check (they
abort with "Inode checksum does not match inode"). The crash-window
acceptance criterion is a byte-exact read-back (md5) of a file that the
kernel synced — so the measurement tool must not depend on the broken
checksums.

It reads the ext4 structures directly:
  superblock -> group descriptors -> inode table -> extent tree -> data
and prints the file's on-disk size, its data blocks, the file's md5 and
whether the inode's metadata_csum verifies (the second-order
data-destruction path: a host e2fsck DELETES kernel-written dirs).

Usage:
  fi_inspect.py <image> <path>            # stat + md5 of <path>
  fi_inspect.py <image> --csum            # fs-wide inode checksum audit
"""
import hashlib
import struct
import sys

# --- crc32c (Castagnoli, reflected 0x82F63B78) ------------------------------
_TAB = []
for _i in range(256):
    _c = _i
    for _ in range(8):
        _c = (0x82F63B78 ^ (_c >> 1)) if (_c & 1) else (_c >> 1)
    _TAB.append(_c)


def crc32c(seed, data):
    c = seed & 0xFFFFFFFF
    for b in data:
        c = _TAB[(c ^ b) & 0xFF] ^ (c >> 8)
    return c & 0xFFFFFFFF


class Ext4:
    def __init__(self, path):
        self.f = open(path, "rb")
        self.sb = self._read(1024, 1024)
        (self.inodes_count, self.blocks_lo, self.rb, self.free_b, self.free_i,
         self.first_data_block, self.log_bs, self.log_cs, self.bpg, self.cpg,
         self.ipg) = struct.unpack_from("<11I", self.sb, 0)
        self.magic = struct.unpack_from("<H", self.sb, 0x38)[0]
        assert self.magic == 0xEF53, "not ext4 (magic %x)" % self.magic
        self.inode_size = struct.unpack_from("<H", self.sb, 0x58)[0]
        self.feature_incompat = struct.unpack_from("<I", self.sb, 0x60)[0]
        self.feature_ro_compat = struct.unpack_from("<I", self.sb, 0x64)[0]
        self.uuid = self.sb[0x68:0x78]
        self.desc_size = struct.unpack_from("<H", self.sb, 0xFE)[0]
        if self.desc_size == 0:
            self.desc_size = 32
        self.checksum_seed_disk = struct.unpack_from("<I", self.sb, 0x270)[0]
        self.block_size = 1024 << self.log_bs
        self.csum_seed = (self.checksum_seed_disk if self.checksum_seed_disk
                          else crc32c(0xFFFFFFFF, self.uuid))
        self.metadata_csum = bool(self.feature_ro_compat & 0x400)
        self.gd_block = self.first_data_block + 1
        self._gd = None

    # --- block io ---
    def _read(self, off, n):
        self.f.seek(off)
        return self.f.read(n)

    def block(self, bno):
        return self._read(bno * self.block_size, self.block_size)

    def blocks(self, bno, n):
        return self._read(bno * self.block_size, n * self.block_size)

    # --- group descriptors ---
    def gdesc(self, group):
        if self._gd is None:
            # The GDT spans ceil(group_count * desc_size / block_size) blocks.
            ngroups = max(1, self.blocks_lo // self.bpg)
            nbytes = ngroups * self.desc_size
            nblocks = (nbytes + self.block_size - 1) // self.block_size
            self._gd = self.blocks(self.gd_block, nblocks)
        off = group * self.desc_size
        lo = struct.unpack_from("<I", self._gd, off + 0x08)[0]
        hi = struct.unpack_from("<I", self._gd, off + 0x2C)[0]
        return lo | (hi << 32)

    def inode_table(self, group):
        return self.gdesc(group)

    def group_of(self, ino):
        return (ino - 1) // self.ipg

    # --- inodes ---
    def raw_inode(self, ino):
        group = self.group_of(ino)
        index = (ino - 1) % self.ipg
        per_block = self.block_size // self.inode_size
        blk = self.inode_table(group) + index // per_block
        off = (index % per_block) * self.inode_size
        return self.block(blk)[off:off + self.inode_size]

    def inode_csum_ok(self, ino, raw):
        lo = struct.unpack_from("<H", raw, 0x7C)[0]
        c = crc32c(self.csum_seed, struct.pack("<I", ino))
        c = crc32c(c, raw[100:104])
        c = crc32c(c, raw[0:124])
        c = crc32c(c, b"\0\0")
        c = crc32c(c, raw[126:130])
        c = crc32c(c, b"\0\0")
        c = crc32c(c, raw[132:self.inode_size])
        if len(raw) > 132:
            return (lo | (struct.unpack_from("<H", raw, 0x82)[0] << 16)) == c
        return lo == (c & 0xFFFF)

    def inode_size_of(self, raw):
        # i_size_lo at 0x04; i_size_high at 0x6C (NOT 0x08 — that is
        # i_atime).
        lo = struct.unpack_from("<I", raw, 4)[0]
        hi = struct.unpack_from("<I", raw, 0x6C)[0]
        return lo | (hi << 32)

    # --- extents ---
    def extents(self, ino, raw):
        """Return a list of (logical_block, physical_block, count)."""
        magic = struct.unpack_from("<H", raw, 0x28)[0]
        if magic != 0xF30A:
            return []  # not an extent file (indirect blocks)
        out = []
        self._walk_extent(raw[0x28:0x64], 0, out)
        out.sort()
        return out

    def _walk_extent(self, node, base, out):
        eh_magic, eh_entries, eh_max, eh_depth = struct.unpack_from("<HHHH", node, 0)
        if eh_magic != 0xF30A:
            return
        if eh_depth == 0:
            for i in range(eh_entries):
                off = 12 + i * 12
                ee_block, ee_len, ee_start_hi, ee_start_lo = struct.unpack_from(
                    "<IHHI", node, off)
                length = ee_len if ee_len <= 32768 else ee_len - 32768
                phys = ee_start_lo | (ee_start_hi << 32)
                out.append((base + ee_block, phys, length))
        else:
            for i in range(eh_entries):
                off = 12 + i * 12
                ei_block, leaf_lo, leaf_hi = struct.unpack_from("<IIH", node, off)
                leaf = leaf_lo | (leaf_hi << 32)
                self._walk_extent(self.block(leaf), ei_block, out)

    def file_blocks(self, ino, raw):
        """Physical data blocks of a regular file (holes skipped)."""
        out = []
        for (lblk, phys, count) in self.extents(ino, raw):
            for k in range(count):
                out.append((lblk + k, phys + k))
        return out

    def file_bytes(self, ino, raw):
        size = self.inode_size_of(raw)
        blocks = self.file_blocks(ino, raw)
        buf = bytearray()
        for (lblk, phys) in blocks:
            start = lblk * self.block_size
            if start >= size:
                break
            chunk = self.block(phys)
            take = min(len(chunk), size - start)
            buf += chunk[:take]
        return bytes(buf[:size])

    # --- directories ---
    def dir_entries(self, ino, raw):
        """Linear scan of every data block of a directory."""
        entries = []
        for (lblk, phys) in self.file_blocks(ino, raw):
            data = self.block(phys)
            pos = 0
            while pos + 8 <= len(data):
                child, rec_len, name_len, ftype = struct.unpack_from("<IHBB", data, pos)
                if rec_len < 8 or pos + rec_len > len(data):
                    break
                name = data[pos + 8:pos + 8 + name_len]
                if child:
                    entries.append((name.decode("utf-8", "replace"), child, ftype))
                pos += rec_len
        return entries

    def lookup(self, path):
        ino = 2  # root
        for part in [p for p in path.split("/") if p]:
            raw = self.raw_inode(ino)
            hit = None
            for (name, child, ftype) in self.dir_entries(ino, raw):
                if name == part:
                    hit = child
                    break
            if hit is None:
                return None
            ino = hit
        return ino


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    img = sys.argv[1]
    fs = Ext4(img)
    if "--csum" in sys.argv:
        bad = 0
        total = 0
        for ino in range(11, fs.inodes_count + 1):
            raw = fs.raw_inode(ino)
            mode = struct.unpack_from("<H", raw, 0)[0]
            if mode == 0:  # unused
                continue
            total += 1
            if not fs.inode_csum_ok(ino, raw):
                bad += 1
                if bad <= 10:
                    print("  BAD csum ino=%d mode=%o size=%d" %
                          (ino, mode, fs.inode_size_of(raw)))
        print("FS %s: metadata_csum=%s inodes_used=%d bad_csum=%d" %
              (img, fs.metadata_csum, total, bad))
        return 0
    path = sys.argv[2]
    ino = fs.lookup(path)
    if ino is None:
        print("MISS %s (not found)" % path)
        return 1
    raw = fs.raw_inode(ino)
    size = fs.inode_size_of(raw)
    data = fs.file_bytes(ino, raw)
    print("PATH %s ino=%d size=%d md5=%s csum_ok=%s blocks=%d" %
          (path, ino, size, hashlib.md5(data).hexdigest(),
           fs.inode_csum_ok(ino, raw), len(fs.file_blocks(ino, raw))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
