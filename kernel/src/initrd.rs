//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Boot initrd support (OH Phase 1 prereq): gzip decompression and
//! cpio (newc) unpack into the rootfs.
//!
//! Boot flow (mirrors Linux's initramfs path, `root=/dev/ram0` semantics):
//!
//! 1. Early boot (before the zone allocator exists) the FDT /chosen node is
//!    parsed for `linux,initrd-start` / `linux,initrd-end` and the range is
//!    `memblock_reserve`d so the buddy allocator never hands those pages
//!    out (`set_region`).
//! 2. After the ramfs rootfs is mounted at `/`, `load()` decompresses the
//!    image when it carries the gzip magic (QEMU `-initrd` images of
//!    Ubuntu/OpenHarmony are gzip cpio) and unpacks the cpio archive into
//!    the rootfs — files, directories, symlinks, and hard links; device
//!    nodes are counted and skipped (rootfs is a memory fs without device
//!    node support; userspace inits create their own nodes on /dev).
//! 3. `root=/dev/ram0` in main.rs then skips the ext4 root mount: the
//!    initrd content IS the root filesystem.
//!
//! The decompressor is a compact RFC 1951 DEFLATE decoder (stored, fixed
//! and dynamic Huffman blocks, LZ77 back-references) with the RFC 1952
//! gzip wrapper (magic/header parsing, CRC32 + ISIZE verification) —
//! small, allocation-light, and sufficient for the one-shot boot unpack.
//! (zstd/xz/lz4 initrds are not supported yet.)

use crate::fs::rootfs::RootFSSuperBlock;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Physical start address of the boot initrd (0 = none).
static INITRD_START: AtomicUsize = AtomicUsize::new(0);
/// Physical end address (exclusive) of the boot initrd.
static INITRD_END: AtomicUsize = AtomicUsize::new(0);
/// Set once the image was successfully unpacked into the rootfs.
static INITRD_LOADED: AtomicBool = AtomicBool::new(false);

/// Maximum decompressed size we are willing to materialize on the kernel
/// heap (128MB): the heap is 128MB, so an unbounded (or lying) ISIZE must
/// not take the whole kernel down before the rootfs gets a chance to hold
/// the entries.
const MAX_DECOMPRESSED_SIZE: usize = 64 * 1024 * 1024;

/// Record the initrd's physical range (early boot, from /chosen).
///
/// `end` must be > `start`; both must be inside RAM (the caller parsed
/// them from the FDT). Calling this twice keeps the last range.
pub fn set_region(start: usize, end: usize) {
    INITRD_START.store(start, Ordering::Release);
    INITRD_END.store(end, Ordering::Release);
}

/// The recorded initrd physical range, if any.
pub fn region() -> Option<(usize, usize)> {
    let start = INITRD_START.load(Ordering::Acquire);
    let end = INITRD_END.load(Ordering::Acquire);
    if start != 0 && end > start {
        Some((start, end))
    } else {
        None
    }
}

/// Whether an initrd was already unpacked into the rootfs.
pub fn loaded() -> bool {
    INITRD_LOADED.load(Ordering::Acquire)
}

/// Result of a successful cpio unpack.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpioStats {
    pub files: usize,
    pub dirs: usize,
    pub symlinks: usize,
    pub hardlinks: usize,
    /// Device nodes / fifos / sockets present but not representable in
    /// rootfs (skipped; userspace creates its own /dev nodes).
    pub skipped_special: usize,
    pub total_bytes: usize,
}

/// Decompress (when gzipped) and unpack the initrd into the rootfs.
///
/// Must run after `fs::rootfs::init_rootfs()` — the ramfs superblock is
/// the unpack target. Safe to call only once; subsequent calls are no-ops
/// that report the already-loaded state.
pub fn load() -> Result<CpioStats, &'static str> {
    if loaded() {
        return Err("initrd already loaded");
    }
    let (start, end) = region().ok_or("no initrd image")?;

    // The early-boot FDT parse ran on the identity mapping; here the
    // permanent linear mapping is live, so go through phys_to_virt.
    let virt = crate::arch::mm::phys_to_virt(
        crate::arch::mm::PhysAddr::new(start as u64),
    ).bits();
    // SAFETY: [start, end) is reserved RAM owned by the initrd; the range
    // was validated (end > start, inside RAM) by the FDT parse.
    let raw: &[u8] = unsafe {
        core::slice::from_raw_parts(virt as *const u8, end - start)
    };

    let cpio: CpioSource = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        CpioSource::Owned(gunzip(raw)?)
    } else {
        CpioSource::Borrowed(raw)
    };

    let sb = crate::fs::rootfs::get_rootfs_sb()
        .ok_or("rootfs not initialized")?;
    // SAFETY: the superblock pointer is published by init_rootfs and
    // lives for the rest of the boot.
    let sb: &RootFSSuperBlock = unsafe { &*sb };

    let stats = unpack_newc(cpio.as_slice(), sb)?;
    INITRD_LOADED.store(true, Ordering::Release);
    Ok(stats)
}

/// Either a borrowed (uncompressed) or owned (decompressed) cpio image.
enum CpioSource<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl CpioSource<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            CpioSource::Borrowed(s) => s,
            CpioSource::Owned(v) => v.as_slice(),
        }
    }
}

// ============================================================================
// DEFLATE / gzip (RFC 1951 / RFC 1952)
// ============================================================================

/// LSB-first bit reader over a byte slice.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, bitbuf: 0, bitcnt: 0 }
    }

    /// Read `n` bits (n <= 16), LSB first.
    ///
    /// Reads past the end of the buffer deliver ZERO bits, not an error:
    /// a DEFLATE stream may legally end inside the final symbol's code
    /// (zlib drops trailing all-zero pad bits — e.g. a 7-bit EOB code
    /// whose remaining zeros never appear in the file). Stream validity is
    /// enforced by the block logic (EOB arrival, window bounds, output
    /// cap) and by gunzip's CRC32/ISIZE check, not by the bit reader.
    fn bits(&mut self, n: u32) -> Result<u32, ()> {
        debug_assert!(n <= 16);
        while self.bitcnt < n {
            let b = *self.data.get(self.pos).unwrap_or(&0) as u32;
            self.pos += 1;
            self.bitbuf |= b << self.bitcnt;
            self.bitcnt += 8;
        }
        let mask = if n == 32 { u32::MAX } else { (1u32 << n) - 1 };
        let v = self.bitbuf & mask;
        self.bitbuf >>= n;
        self.bitcnt -= n;
        Ok(v)
    }

    /// Discard partial bits so the next read is byte-aligned (stored blocks).
    fn align_byte(&mut self) {
        self.bitbuf = 0;
        self.bitcnt = 0;
    }
}

/// Canonical Huffman decoding table (count/symbol form — "puff" style).
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build from per-symbol code lengths (0 = unused).
    /// Fails on over-subscribed length sets (more codes than the tree can
    /// hold — corrupt stream). Incomplete sets are accepted; decoding a
    /// missing code fails at `decode` time.
    fn new(lengths: &[u8]) -> Result<Self, ()> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        let mut left: i32 = 1;
        for len in 1..16 {
            left <<= 1;
            left -= counts[len] as i32;
            if left < 0 {
                return Err(());
            }
        }
        // First symbol index per code length (offsets).
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = alloc::vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol, MSB of the canonical code arriving bit by bit.
    fn decode(&self, br: &mut BitReader) -> Result<u16, ()> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16usize {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                let idx = (index + (code - first)) as usize;
                return self.symbols.get(idx).copied().ok_or(());
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(())
    }
}

/// Length-symbol bases/extras for codes 257..285 (RFC 1951 §3.2.5).
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51,
    59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4,
    4, 5, 5, 5, 5, 0,
];
/// Distance-symbol bases for codes 0..29 (RFC 1951 §3.2.5).
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385,
    513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385,
    24577,
];
/// Distance-symbol extra-bit counts — they grow every TWO codes
/// (0,0,0,0,1,1,2,2,...), not per code; verified against zlib with a
/// 300-stream differential fuzz.
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10,
    10, 11, 11, 12, 12, 13, 13,
];
/// Order in which code-length-code lengths are stored (RFC 1951 §3.2.7).
const CLC_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4,
    12, 3, 13, 2, 14, 1, 15];

/// Inflate a raw DEFLATE stream. `cap` bounds the output size.
fn inflate_raw(data: &[u8], cap: usize) -> Result<Vec<u8>, ()> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;

        match btype {
            // Stored (uncompressed) block.
            0 => {
                br.align_byte();
                if br.pos + 4 > data.len() {
                    return Err(());
                }
                let len = u16::from_le_bytes(
                    [data[br.pos], data[br.pos + 1]]) as usize;
                let nlen = u16::from_le_bytes(
                    [data[br.pos + 2], data[br.pos + 3]]) as usize;
                br.pos += 4;
                if len != (!nlen & 0xFFFF) {
                    return Err(());
                }
                if br.pos + len > data.len() || out.len() + len > cap {
                    return Err(());
                }
                out.extend_from_slice(&data[br.pos..br.pos + len]);
                br.pos += len;
            }
            // Huffman block (fixed or dynamic tables).
            _ => {
                let (litlen, dist) = if btype == 1 {
                    (fixed_litlen_table(), fixed_dist_table())
                } else if btype == 2 {
                    read_dynamic_tables(&mut br)?
                } else {
                    return Err(()); // BTYPE=11 reserved
                };
                inflate_block(&mut br, &litlen, &dist, &mut out, cap)?;
            }
        }

        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

/// Fixed literal/length code (RFC 1951 §3.2.6).
fn fixed_litlen_table() -> Huffman {
    let mut lengths = [0u8; 288];
    for (sym, l) in lengths.iter_mut().enumerate() {
        *l = match sym {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    Huffman::new(&lengths).expect("fixed litlen table is well-formed")
}

/// Fixed distance code: 30 (32 defined, 30/31 never emitted) codes of 5 bits.
fn fixed_dist_table() -> Huffman {
    let lengths = [5u8; 30];
    Huffman::new(&lengths).expect("fixed dist table is well-formed")
}

/// Read dynamic-block Huffman table definitions (RFC 1951 §3.2.7).
fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), ()> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(()); // more codes than the alphabets define
    }

    // Read the code-length-code lengths in the special order.
    let mut clc_lengths = [0u8; 19];
    for i in 0..hclen {
        clc_lengths[CLC_ORDER[i]] = br.bits(3)? as u8;
    }
    let clc = Huffman::new(&clc_lengths)?;

    // Decode hlit + hdist code lengths with run codes 16/17/18.
    let total = hlit + hdist;
    let mut lengths = alloc::vec![0u8; total];
    let mut i = 0usize;
    while i < total {
        let sym = clc.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(()); // no previous length to repeat
                }
                let prev = lengths[i - 1];
                let rep = 3 + br.bits(2)? as usize;
                if i + rep > total {
                    return Err(());
                }
                for _ in 0..rep {
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let rep = 3 + br.bits(3)? as usize;
                if i + rep > total {
                    return Err(());
                }
                i += rep; // zeros already initialized
            }
            18 => {
                let rep = 11 + br.bits(7)? as usize;
                if i + rep > total {
                    return Err(());
                }
                i += rep;
            }
            _ => return Err(()),
        }
    }
    if lengths[256] == 0 {
        return Err(()); // no end-of-block code
    }
    let litlen = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..])?;
    Ok((litlen, dist))
}

/// Decode one Huffman-compressed block into `out`.
fn inflate_block(
    br: &mut BitReader,
    litlen: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
    cap: usize,
) -> Result<(), ()> {
    loop {
        let sym = litlen.decode(br)?;
        match sym {
            0..=255 => {
                if out.len() >= cap {
                    return Err(());
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let li = (sym - 257) as usize;
                let len = LENGTH_BASE[li] as usize
                    + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym > 29 {
                    return Err(());
                }
                let distance = DIST_BASE[dsym] as usize
                    + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
                if distance == 0 || distance > out.len() {
                    return Err(()); // reference before start of output
                }
                if out.len() + len > cap {
                    return Err(());
                }
                // Overlapping copies repeat the pattern — copy byte-wise.
                let mut src = out.len() - distance;
                for _ in 0..len {
                    let b = out[src];
                    out.push(b);
                    src += 1;
                }
            }
            _ => return Err(()), // 286/287 invalid
        }
    }
}

/// CRC-32 (IEEE 802.3, reflected, as used by gzip).
fn crc32(data: &[u8]) -> u32 {
    // Compute the table on the stack — 256 entries, one-shot boot path.
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Decompress a gzip (RFC 1952) member; verifies CRC32 and ISIZE.
/// Public for the boot-time unit test suite.
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    if data.len() < 18 {
        return Err("initrd gzip: truncated");
    }
    if data[0] != 0x1f || data[1] != 0x8b {
        return Err("initrd gzip: bad magic");
    }
    if data[2] != 8 {
        return Err("initrd gzip: not deflate");
    }
    let flg = data[3];
    if flg & 0xE0 != 0 {
        return Err("initrd gzip: reserved flag set");
    }
    let mut pos = 10usize; // magic(2) CM(1) FLG(1) MTIME(4) XFL(1) OS(1)
    if flg & 0x04 != 0 {
        // FEXTRA: 16-bit length + payload
        if pos + 2 > data.len() {
            return Err("initrd gzip: truncated FEXTRA");
        }
        let xlen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + xlen;
        if pos > data.len() {
            return Err("initrd gzip: truncated FEXTRA");
        }
    }
    // FNAME then FCOMMENT (each NUL-terminated)
    for flag in [0x08u8, 0x10u8] {
        if flg & flag != 0 {
            while pos < data.len() && data[pos] != 0 {
                pos += 1;
            }
            pos += 1; // NUL
            if pos > data.len() {
                return Err("initrd gzip: truncated name");
            }
        }
    }
    if flg & 0x02 != 0 {
        pos += 2; // FHCRC (not verified)
        if pos > data.len() {
            return Err("initrd gzip: truncated FHCRC");
        }
    }

    // Trailer is the last 8 bytes: CRC32(4) ISIZE(4) after the deflate
    // stream — the deflate length is only known by decoding, so decode
    // from `pos` and verify against the trailer at the fixed end. (This
    // assumes a single-member gzip with no trailing padding, which is what
    // `find | cpio -H newc | gzip` and every distro mkinitrd produce.)
    let want_crc = u32::from_le_bytes([
        data[data.len() - 8], data[data.len() - 7],
        data[data.len() - 6], data[data.len() - 5],
    ]);
    let want_isize = u32::from_le_bytes([
        data[data.len() - 4], data[data.len() - 3],
        data[data.len() - 2], data[data.len() - 1],
    ]);
    if want_isize as usize > MAX_DECOMPRESSED_SIZE {
        return Err("initrd gzip: decompressed size exceeds kernel limit");
    }
    let cap = want_isize as usize;

    let out = inflate_raw(&data[pos..data.len() - 8], cap)
        .map_err(|_| "initrd gzip: corrupt deflate stream")?;
    if out.len() != want_isize as usize {
        return Err("initrd gzip: size mismatch");
    }
    if crc32(&out) != want_crc {
        return Err("initrd gzip: CRC mismatch");
    }
    Ok(out)
}

// ============================================================================
// cpio (newc) unpack — see cpio(5) / Linux initramfs format
// ============================================================================

/// File-type bits of the cpio `mode` field.
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

/// Parse an 8-character big-endian hex field of a newc header.
fn parse_hex8(field: &[u8]) -> Option<u32> {
    if field.len() != 8 {
        return None;
    }
    let mut v: u32 = 0;
    for &c in field {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        v = v.wrapping_mul(16).wrapping_add(d as u32);
    }
    Some(v)
}

/// Align a newc record offset to 4 bytes.
fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Maximum number of (ino → path) hard-link memories we keep.
const MAX_LINK_MEMO: usize = 8192;

/// Unpack a newc cpio archive into a rootfs superblock.
///
/// Handles the subset Linux's initramfs uses: regular files, directories,
/// symlinks, hard links (same c_ino with filesize=0 after the first).
/// Device nodes, fifos and sockets are counted but skipped. Public for
/// the boot-time unit test suite (fresh RootFSSuperBlock as target).
pub fn unpack_newc(data: &[u8], sb: &RootFSSuperBlock) -> Result<CpioStats, &'static str> {
    let mut stats = CpioStats::default();
    let mut link_memo: Vec<(u32, alloc::string::String)> = Vec::new();
    let mut pos = 0usize;

    loop {
        if pos + 110 > data.len() {
            return Err("cpio: truncated header");
        }
        let hdr = &data[pos..pos + 110];
        if &hdr[0..6] != b"070701" && &hdr[0..6] != b"070702" {
            return Err("cpio: bad magic");
        }
        // Field layout (110 bytes): magic(6), ino(8), mode(8), uid(8),
        // gid(8), nlink(8), mtime(8), filesize(8), devmajor(8), devminor(8),
        // rdevmajor(8), rdevminor(8), namesize(8), check(8).
        let ino = parse_hex8(&hdr[6..14]).ok_or("cpio: bad ino")?;
        let mode = parse_hex8(&hdr[14..22]).ok_or("cpio: bad mode")?;
        let _uid = parse_hex8(&hdr[22..30]).ok_or("cpio: bad uid")?;
        let _gid = parse_hex8(&hdr[30..38]).ok_or("cpio: bad gid")?;
        let nlink = parse_hex8(&hdr[38..46]).ok_or("cpio: bad nlink")?;
        let mtime = parse_hex8(&hdr[46..54]).ok_or("cpio: bad mtime")? as u64;
        let filesize = parse_hex8(&hdr[54..62]).ok_or("cpio: bad filesize")? as usize;
        let namesize = parse_hex8(&hdr[94..102]).ok_or("cpio: bad namesize")? as usize;

        let name_off = pos + 110;
        if name_off + namesize > data.len() || namesize == 0 {
            return Err("cpio: truncated name");
        }
        let name_bytes = &data[name_off..name_off + namesize];
        let nul = name_bytes.iter().position(|&b| b == 0)
            .ok_or("cpio: unterminated name")?;
        let name = core::str::from_utf8(&name_bytes[..nul])
            .map_err(|_| "cpio: non-utf8 name")?;

        let data_off = align4(name_off + namesize);
        if data_off + filesize > data.len() {
            return Err("cpio: truncated file data");
        }
        pos = align4(data_off + filesize);

        if name == "TRAILER!!!" {
            break;
        }
        if name == "." {
            continue;
        }
        // Normalize: drop leading "./" and "/" (cpio names are root-relative
        // and may be prefixed either way), then re-prefix "/" — the rootfs
        // helpers require absolute paths (lookup() rejects relative ones).
        let rel = name
            .trim_start_matches("./")
            .trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        // Reject traversal — the archive is unpacked at the filesystem root.
        if rel.split('/').any(|c| c == "..") {
            continue;
        }
        let path = alloc::format!("/{}", rel);

        let ftype = mode & S_IFMT;
        match ftype {
            S_IFDIR => {
                if mkdir_p(sb, &path).is_ok() {
                    stats.dirs += 1;
                }
                set_node_meta(sb, &path, mode, mtime);
            }
            S_IFREG => {
                // Hard link: a later entry with the same ino and no data
                // links to the first occurrence.
                if filesize == 0 && nlink > 1 {
                    if let Some((_, first)) = link_memo.iter().find(|(i, _)| *i == ino) {
                        if sb.link(first, &path).is_ok() {
                            stats.hardlinks += 1;
                        }
                        continue;
                    }
                }
                let file_data = data[data_off..data_off + filesize].to_vec();
                if create_file(sb, &path, file_data).is_ok() {
                    stats.files += 1;
                    stats.total_bytes += filesize;
                }
                set_node_meta(sb, &path, mode, mtime);
                if nlink > 1 && link_memo.len() < MAX_LINK_MEMO {
                    link_memo.push((ino, path.clone()));
                }
            }
            S_IFLNK => {
                let target_bytes = &data[data_off..data_off + filesize];
                if let Ok(target) = core::str::from_utf8(target_bytes) {
                    if mkdir_p_parents(sb, &path).is_ok()
                        && sb.symlink(target, &path).is_ok()
                    {
                        stats.symlinks += 1;
                        set_node_meta(sb, &path, mode, mtime);
                    }
                }
            }
            _ => {
                // char/block devices, fifos, sockets: rootfs cannot hold
                // them; userspace init builds its own /dev (OH /init and
                // busybox both mknod after mounting tmpfs on /dev).
                stats.skipped_special += 1;
            }
        }
    }

    // Guarantee the standard mountpoints exist even when the archive did
    // not carry them — the kernel's own later mounts (/proc, /sys, /dev,
    // /run, /tmp) need them in an initrd-rooted boot.
    for dir in ["/dev", "/proc", "/sys", "/run", "/tmp"] {
        let _ = mkdir_p(sb, dir);
    }

    Ok(stats)
}

/// mkdir -p semantics over the rootfs superblock.
fn mkdir_p(sb: &RootFSSuperBlock, path: &str) -> Result<(), i32> {
    mkdir_p_parents(sb, path)?;
    match sb.mkdir(path) {
        Ok(()) => Ok(()),
        Err(e) if e == -crate::errno::Errno::FileExists.as_neg_i32() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Create every parent directory of `path` (not path itself).
fn mkdir_p_parents(sb: &RootFSSuperBlock, path: &str) -> Result<(), i32> {
    let mut prefix = alloc::string::String::new();
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    for c in &comps[..comps.len().saturating_sub(1)] {
        prefix.push('/');
        prefix.push_str(c);
        match sb.mkdir(&prefix) {
            Ok(()) => {}
            Err(e) if e == -crate::errno::Errno::FileExists.as_neg_i32() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Create (or replace) a regular file with `data` at `path`.
fn create_file(sb: &RootFSSuperBlock, path: &str, data: Vec<u8>) -> Result<(), i32> {
    mkdir_p_parents(sb, path)?;
    // Replace an existing entry (later archive entries win, matching
    // Linux initramfs overwrite semantics).
    if let Some(parent) = path.rfind('/').map(|i| if i == 0 { "/" } else { &path[..i] }) {
        if let Some(dir) = sb.lookup(parent) {
            let base = path.rsplit('/').next().unwrap_or("");
            dir.remove_child(base.as_bytes());
        }
    }
    sb.create_file(path, data)
}

/// Apply cpio mode (permission bits) and mtime to a freshly created node.
fn set_node_meta(sb: &RootFSSuperBlock, path: &str, mode: u32, mtime: u64) {
    if let Some(node) = sb.lookup(path) {
        // Keep the node's own type bits; apply only the permission bits.
        let mut m = node.mode.lock();
        *m = (*m & !0o7777) | (mode & 0o7777);
        drop(m);
        node.mtime.store(mtime, Ordering::Release);
    }
}
