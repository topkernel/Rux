//! ext4 htree (hashed directory index) lookup — read-only parity port.
//!
//! Reference: the kernel's fs/ext4/namei.c dx_probe / ext4_dx_find_entry /
//! ext4_htree_next_block and fs/ext4/hash.c (__ext4fs_dirhash), ported
//! bit-for-bit (same binary search, same limit formulas, same hash
//! transforms) so an image written by e2fsprogs hashes identically.
//!
//! Why: our directory lookup was a FULL linear scan — every miss paid a
//! get_data_blocks Vec + one extent walk per block + every dirent parsed
//! (the 21st measured 130µs/miss, 44s of boot walk-loop wall). An indexed
//! directory resolves in 2-3 block reads: dx_root -> (node) -> leaf.
//!
//! Safety of read-only use: our directory writer refuses EXT4_INDEX_FL
//! parents (add_dir_entry / ext4_add_entry_locked return EOPNOTSUPP), so
//! an index can never go stale through us. Any parse anomaly returns
//! None and the caller falls back to the linear scan (the reference's
//! ERR_BAD_DX_DIR contract: never reflected to userspace).
//!
//! NOT implemented (with the same refusal on the write side): index
//! maintenance, dx_add_entry, casefolded/SIPHASH dirs (hash_version 6).

use super::dir::Ext4DirEntry;
use super::Ext4FileSystem;
use crate::fs::bio;

/// dx hash versions (the reference's ext4.h enum). On-disk roots carry
/// the SIGNED base (0-2); the sb's unsigned-hash flag shifts them +3 at
/// probe time (dx_probe: `hash_version <= DX_HASH_TEA → += s_hash_unsigned`).
const DX_HASH_LEGACY: u8 = 0;
const DX_HASH_HALF_MD4: u8 = 1;
const DX_HASH_TEA: u8 = 2;

const DX_HASH_SIPHASH: u8 = 6;

/// EXT4_HTREE_EOF_32BIT (hash.c): hashes clamp below it.
const HTREE_EOF_32BIT: u32 = 0x7fff_ffff;

// ---------------------------------------------------------------------
// half_md4 / TEA / legacy — verbatim ports of hash.c
// ---------------------------------------------------------------------

const DELTA: u32 = 0x9E37_79B9; // TEA delta (hash.c)

/// hash.c TEA_transform.
fn tea_transform(buf: &mut [u32; 4], input: &[u32; 4]) {
    let mut sum: u32 = 0;
    let mut b0 = buf[0];
    let mut b1 = buf[1];
    let (a, b, c, d) = (input[0], input[1], input[2], input[3]);
    let mut n = 16;
    loop {
        sum = sum.wrapping_add(DELTA);
        b0 = b0.wrapping_add(
            b1
                .wrapping_shl(4)
                .wrapping_add(a)
                ^ b1.wrapping_add(sum)
                ^ b1.wrapping_shr(5).wrapping_add(b),
        );
        b1 = b1.wrapping_add(
            b0
                .wrapping_shl(4)
                .wrapping_add(c)
                ^ b0.wrapping_add(sum)
                ^ b0.wrapping_shr(5).wrapping_add(d),
        );
        n -= 1;
        if n == 0 {
            break;
        }
    }
    buf[0] = buf[0].wrapping_add(b0);
    buf[1] = buf[1].wrapping_add(b1);
}

/// hash.c ROUND helper (a += f(b,c,d) + x; a = rol32(a, s)).
#[inline(always)]
fn md4_round(f: fn(u32, u32, u32) -> u32, a: &mut u32, b: u32, c: u32, d: u32, x: u32, s: u32) {
    *a = a.wrapping_add(f(b, c, d)).wrapping_add(x);
    *a = a.rotate_left(s);
}

#[inline(always)]
fn md4_f(x: u32, y: u32, z: u32) -> u32 {
    z ^ (x & (y ^ z))
}
#[inline(always)]
fn md4_g(x: u32, y: u32, z: u32) -> u32 {
    (x & y).wrapping_add((x ^ y) & z)
}
#[inline(always)]
fn md4_h(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}

/// hash.c half_md4_transform — returns buf[1] ("most hashed" word).
fn half_md4_transform(buf: &mut [u32; 4], input: &[u32; 8]) -> u32 {
    let mut a = buf[0];
    let mut b = buf[1];
    let mut c = buf[2];
    let mut d = buf[3];

    const K1: u32 = 0;
    const K2: u32 = 0o13240474631;
    const K3: u32 = 0o15666365641;

    // Round 1
    md4_round(md4_f, &mut a, b, c, d, input[0].wrapping_add(K1), 3);
    md4_round(md4_f, &mut d, a, b, c, input[1].wrapping_add(K1), 7);
    md4_round(md4_f, &mut c, d, a, b, input[2].wrapping_add(K1), 11);
    md4_round(md4_f, &mut b, c, d, a, input[3].wrapping_add(K1), 19);
    md4_round(md4_f, &mut a, b, c, d, input[4].wrapping_add(K1), 3);
    md4_round(md4_f, &mut d, a, b, c, input[5].wrapping_add(K1), 7);
    md4_round(md4_f, &mut c, d, a, b, input[6].wrapping_add(K1), 11);
    md4_round(md4_f, &mut b, c, d, a, input[7].wrapping_add(K1), 19);

    // Round 2
    md4_round(md4_g, &mut a, b, c, d, input[1].wrapping_add(K2), 3);
    md4_round(md4_g, &mut d, a, b, c, input[3].wrapping_add(K2), 5);
    md4_round(md4_g, &mut c, d, a, b, input[5].wrapping_add(K2), 9);
    md4_round(md4_g, &mut b, c, d, a, input[7].wrapping_add(K2), 13);
    md4_round(md4_g, &mut a, b, c, d, input[0].wrapping_add(K2), 3);
    md4_round(md4_g, &mut d, a, b, c, input[2].wrapping_add(K2), 5);
    md4_round(md4_g, &mut c, d, a, b, input[4].wrapping_add(K2), 9);
    md4_round(md4_g, &mut b, c, d, a, input[6].wrapping_add(K2), 13);

    // Round 3
    md4_round(md4_h, &mut a, b, c, d, input[3].wrapping_add(K3), 3);
    md4_round(md4_h, &mut d, a, b, c, input[7].wrapping_add(K3), 9);
    md4_round(md4_h, &mut c, d, a, b, input[2].wrapping_add(K3), 11);
    md4_round(md4_h, &mut b, c, d, a, input[6].wrapping_add(K3), 15);
    md4_round(md4_h, &mut a, b, c, d, input[1].wrapping_add(K3), 3);
    md4_round(md4_h, &mut d, a, b, c, input[5].wrapping_add(K3), 9);
    md4_round(md4_h, &mut c, d, a, b, input[0].wrapping_add(K3), 11);
    md4_round(md4_h, &mut b, c, d, a, input[4].wrapping_add(K3), 15);

    buf[0] = buf[0].wrapping_add(a);
    buf[1] = buf[1].wrapping_add(b);
    buf[2] = buf[2].wrapping_add(c);
    buf[3] = buf[3].wrapping_add(d);

    buf[1]
}

/// hash.c dx_hack_hash_{unsigned,signed} (the legacy hash).
fn dx_hack_hash(name: &[u8], unsigned: bool) -> u32 {
    let mut hash0: u32 = 0x12a3_fe2d;
    let mut hash1: u32 = 0x37ab_e8f9;
    for &byte in name {
        let sv: i32 = if unsigned { byte as i32 } else { byte as i8 as i32 };
        let mut hash = hash1.wrapping_add((hash0 ^ sv.wrapping_mul(7152373) as u32) as u32);
        // The reference does signed arithmetic: if (hash & 0x80000000)
        // hash -= 0x7fffffff.
        if hash & 0x8000_0000 != 0 {
            hash = hash.wrapping_sub(0x7fff_ffff);
        }
        hash1 = hash0;
        hash0 = hash;
    }
    hash0 << 1
}

/// hash.c str2hashbuf_{signed,unsigned}.
fn str2hashbuf(msg: &[u8], buf: &mut [u32], unsigned: bool) {
    let len = msg.len();
    let num = buf.len();
    let mut pad = (len as u32) | ((len as u32) << 8);
    pad |= pad << 16;

    let mut val: u32 = pad;
    let mut bi = 0usize;
    let mut i = 0usize;
    let mut lim = len;
    if lim > num * 4 {
        lim = num * 4;
    }
    while i < lim {
        let b = if unsigned {
            msg[i] as u32
        } else {
            msg[i] as i8 as i32 as u32
        };
        val = b.wrapping_add(val << 8);
        if i % 4 == 3 {
            buf[bi] = val;
            bi += 1;
            val = pad;
        }
        i += 1;
    }
    // (--num >= 0) *buf++ = val; then while (--num >= 0) *buf++ = pad.
    if num > 0 {
        if bi < num {
            buf[bi] = val;
            bi += 1;
        }
        while bi < num {
            buf[bi] = pad;
            bi += 1;
        }
    }
}

/// hash.c __ext4fs_dirhash — returns (hash, minor_hash), or None when the
/// version is unsupported (SIPHASH needs keys; the caller falls back).
pub fn dirhash(name: &[u8], version: u8, seed: &[u32; 4]) -> Option<(u32, u32)> {
    let mut buf: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    // Seed: use the sb seed only when it is not all zeros (hash.c).
    if seed.iter().any(|&s| s != 0) {
        buf.copy_from_slice(seed);
    }
    let mut minor_hash: u32 = 0;
    let hash: u32;

    match version {
        DX_HASH_LEGACY => hash = dx_hack_hash(name, false),
        3 => hash = dx_hack_hash(name, true), // DX_HASH_LEGACY_UNSIGNED
        DX_HASH_HALF_MD4 | 4 => {
            let mut p = name;
            let mut len = name.len() as isize;
            let mut input = [0u32; 8];
            while len > 0 {
                str2hashbuf(p, &mut input, version == 4);
                half_md4_transform(&mut buf, &input);
                len -= 32;
                p = if p.len() >= 32 { &p[32..] } else { &[][..] };
            }
            minor_hash = buf[2];
            hash = buf[1];
        }
        DX_HASH_TEA | 5 => {
            let mut p = name;
            let mut len = name.len() as isize;
            let mut input = [0u32; 4];
            while len > 0 {
                str2hashbuf(p, &mut input, version == 5);
                tea_transform(&mut buf, &input);
                len -= 16;
                p = if p.len() >= 16 { &p[16..] } else { &[][..] };
            }
            hash = buf[0];
            minor_hash = buf[1];
        }
        DX_HASH_SIPHASH => return None,
        _ => return None,
    }
    let mut hash = hash & !1u32;
    if hash == (HTREE_EOF_32BIT << 1) {
        hash = (HTREE_EOF_32BIT - 1) << 1;
    }
    Some((hash, minor_hash))
}

// ---------------------------------------------------------------------
// dx_probe + leaf search — the namei.c lookup port
// ---------------------------------------------------------------------

/// One frame of the dx descent: a parsed index block.
struct DxFrame {
    bh: *mut bio::BufferHead,
    /// Offset of the entries[] array inside b_data. entry[0] carries the
    /// countlimit overlay in its HASH half (u32 at entries_off: low u16
    /// limit, high u16 count) — exactly the reference's entries pointer.
    entries_off: usize,
    count: usize,
    /// Index of the currently selected entry (>= 0; 0 = the sentinel).
    at: usize,
    /// The lookup's target hash (every frame carries it; the leaf loop
    /// reads it from the innermost frame — dx_probe threads `hash` the
    /// same way).
    hash: u32,
}

#[inline]
fn entry_hash(data: &[u8], entries_off: usize, i: usize) -> u32 {
    let o = entries_off + i * 8;
    u32::from_le_bytes(data[o..o + 4].try_into().unwrap_or([0; 4]))
}

#[inline]
fn entry_block(data: &[u8], entries_off: usize, i: usize) -> u32 {
    let o = entries_off + i * 8 + 4;
    u32::from_le_bytes(data[o..o + 4].try_into().unwrap_or([0; 4]))
}

/// Read one logical directory block (dx blocks are logical dir blocks).
fn read_dir_logical_block(
    fs: &Ext4FileSystem,
    dir: &super::inode::Ext4Inode,
    index: u64,
) -> Option<*mut bio::BufferHead> {
    let phys = dir.get_data_block(fs, index).ok()?;
    if phys == 0 {
        return None;
    }
    bio::bread(fs.device, phys)
}

/// Parse an index block: countlimit position + limit/count values.
/// Roots: "." rec_len 12, ".." rec_len blocksize-12, info at 24, countlimit
/// at 32. Nodes: fake dirent rec_len == blocksize, countlimit at 8.
/// Returns (countlimit_off, entries_base, count, limit); entries_base ==
/// countlimit_off (entry[0].hash IS the countlimit, entry[0].block is the
/// sentinel's leaf pointer — reference layout).
fn parse_index_block(
    data: &[u8],
    block_size: usize,
    metadata_csum: bool,
) -> Option<(usize, usize, usize, usize)> {
    let tail = if metadata_csum { 8usize } else { 0usize };

    let rlen0 = u16::from_le_bytes(data[4..6].try_into().unwrap_or([0; 2])) as usize;
    let cl;
    let dx_limit;
    if rlen0 == 12 {
        let rlen1 = u16::from_le_bytes(data[16..18].try_into().unwrap_or([0; 2])) as usize;
        if rlen1 != block_size - 12 {
            return None;
        }
        let reserved_zero = u32::from_le_bytes(data[24..28].try_into().unwrap_or([0; 4]));
        let info_length = data[29];
        if reserved_zero != 0 || info_length != 8 {
            return None;
        }
        cl = 32usize;
        dx_limit = (block_size - 12 - 12 - 8 - tail) / 8;
    } else if rlen0 == block_size {
        cl = 8usize;
        dx_limit = (block_size - 8 - tail) / 8;
    } else {
        return None;
    }

    if data.len() < cl + 4 {
        return None;
    }
    let limit = u16::from_le_bytes(data[cl..cl + 2].try_into().unwrap_or([0; 2])) as usize;
    let count = u16::from_le_bytes(data[cl + 2..cl + 4].try_into().unwrap_or([0; 2])) as usize;
    if limit != dx_limit {
        return None;
    }
    // Reference parity (dx_probe: count > limit is ERR_BAD_DX_DIR). Also a
    // memory-safety bound: the binary search touches entries[count-1], and
    // without this check a corrupt count made it read past the block.
    if count == 0 || count > limit {
        return None;
    }
    Some((cl, cl, count, limit))
}

/// The dx lookup. Returns:
/// - None: not indexed or corrupt index (caller falls back to linear);
/// - Some(Ok(entry)): found;
/// - Some(Err(-ENOENT)): htree-clean miss;
/// - Some(Err(e)): block IO error.
pub fn dx_find_entry(
    fs: &Ext4FileSystem,
    dir: &super::inode::Ext4Inode,
    name: &str,
) -> Option<Result<Ext4DirEntry, i32>> {
    if dir.flags & super::features::EXT4_INDEX_FL == 0 {
        return None;
    }
    let block_size = fs.block_size as usize;
    let mut frames: alloc::vec::Vec<DxFrame> = alloc::vec::Vec::with_capacity(2);

    // --- root block -----------------------------------------------------
    let root_bh = read_dir_logical_block(fs, dir, 0)?;
    let (hash, root_at, indirect_levels, root_eoff, root_count);
    {
        // SAFETY: bh from bio::bread; read-only access below.
        let data = unsafe { &(*root_bh).b_data };
        if data.len() < block_size {
            bio::brelse(root_bh);
            return None;
        }
        // dx_probe's validation order: hash version, unused flags,
        // indirect levels, root limit, count.
        let hash_version = data[28];
        let levels = data[30];
        let unused_flags = data[31];
        if !(hash_version == DX_HASH_LEGACY
            || hash_version == DX_HASH_HALF_MD4
            || hash_version == DX_HASH_TEA)
        {
            bio::brelse(root_bh);
            return None;
        }
        if unused_flags & 1 != 0 || levels > 1 {
            bio::brelse(root_bh);
            return None;
        }
        let Some((_, entries_off, count, _)) =
            parse_index_block(data, block_size, fs.metadata_csum)
        else {
            bio::brelse(root_bh);
            return None;
        };
        if count == 0 || count > (block_size / 8) {
            bio::brelse(root_bh);
            return None;
        }
        // Final hash version: signed base + the sb unsigned flag
        // (dx_probe: `hash_version <= DX_HASH_TEA → += s_hash_unsigned`).
        let version = hash_version + if fs.hash_unsigned { 3 } else { 0 };
        let Some((h, _minor)) = dirhash(name.as_bytes(), version, &fs.hash_seed) else {
            bio::brelse(root_bh);
            return None;
        };
        hash = h;

        // Binary search (dx_probe): p = entries+1 .. q = count-1, first
        // entry with hash > target; the chosen is p-1.
        let mut p: isize = 1;
        let mut q: isize = count as isize - 1;
        while p <= q {
            let m = (p + q) / 2;
            if entry_hash(data, entries_off, m as usize) > hash {
                q = m - 1;
            } else {
                p = m + 1;
            }
        }
        root_at = (p - 1) as usize;
        indirect_levels = levels as usize;
        root_eoff = entries_off;
        root_count = count;
    }
    frames.push(DxFrame {
        bh: root_bh,
        entries_off: root_eoff,
        count: root_count,
        at: root_at,
        hash,
    });

    // --- descend indirect levels ----------------------------------------
    {
        let mut level = 0usize;
        let mut visited: [u32; 3] = [u32::MAX; 3];
        visited[0] = 0;
        while level < indirect_levels {
            let (block, target_hash) = {
                let fr = &frames[level];
                // SAFETY: bh alive in the frame; read-only access.
                (
                    unsafe { entry_block(&(*fr.bh).b_data, fr.entries_off, fr.at) },
                    fr.hash,
                )
            };
            // Cycle check (dx_probe's blocks[] loop).
            for i in 0..=level {
                if visited[i] == block {
                    dx_release(&mut frames);
                    return None;
                }
            }
            level += 1;
            visited[level] = block;
            let Some(nbh) = read_dir_logical_block(fs, dir, block as u64) else {
                dx_release(&mut frames);
                return Some(Err(-(crate::errno::Errno::IOError.as_neg_i32())));
            };
            let parsed = {
                // SAFETY: nbh just read; read-only access.
                let data = unsafe { &(*nbh).b_data };
                if data.len() < block_size {
                    None
                } else {
                    parse_index_block(data, block_size, fs.metadata_csum).and_then(
                        |(_, eoff, count, _)| {
                            if count == 0 || count > block_size / 8 {
                                None
                            } else {
                                // Node binary search (dx_probe's loop body).
                                let mut p: isize = 1;
                                let mut q: isize = count as isize - 1;
                                while p <= q {
                                    let m = (p + q) / 2;
                                    if entry_hash(data, eoff, m as usize) > target_hash {
                                        q = m - 1;
                                    } else {
                                        p = m + 1;
                                    }
                                }
                                Some((eoff, count, p - 1))
                            }
                        },
                    )
                }
            };
            let Some((eoff, count, at)) = parsed else {
                bio::brelse(nbh);
                dx_release(&mut frames);
                return None;
            };
            frames.push(DxFrame { bh: nbh, entries_off: eoff, count, at: at as usize, hash: target_hash });
        }
    }

    // --- leaf search loop (ext4_dx_find_entry + ext4_htree_next_block) ---
    let enoent = -(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
    let result: Result<Ext4DirEntry, i32> = loop {
        let (leaf_block, target_hash) = {
            let top = &frames[frames.len() - 1];
            // SAFETY: bh alive in the frame; read-only access.
            (
                unsafe { entry_block(&(*top.bh).b_data, top.entries_off, top.at) },
                top.hash,
            )
        };
        let Some(lbh) = read_dir_logical_block(fs, dir, leaf_block as u64) else {
            break Err(-(crate::errno::Errno::IOError.as_neg_i32()));
        };
        let mut found: Option<Ext4DirEntry> = None;
        {
            // SAFETY: lbh just read; read-only access.
            let data = unsafe { &(*lbh).b_data };
            let mut offset = 0usize;
            while offset < block_size {
                let entry = unsafe { Ext4DirEntry::from_bytes(&data[offset..], block_size) };
                if entry.rec_len == 0 {
                    break;
                }
                if entry.inode != 0 {
                    if let Ok(s) = core::str::from_utf8(&entry.name[..entry.name_len as usize]) {
                        if s == name {
                            found = Some(entry);
                            break;
                        }
                    }
                }
                offset += entry.rec_len as usize;
            }
        }
        bio::brelse(lbh);
        if let Some(e) = found {
            break Ok(e);
        }
        // ext4_htree_next_block: advance the innermost frame, popping
        // exhausted parents; continue only while the next boundary hash
        // matches ((bhash & ~1) == hash for LSB-0 lookup hashes).
        let mut level = frames.len() - 1;
        loop {
            frames[level].at += 1;
            if frames[level].at < frames[level].count {
                break;
            }
            if level == 0 {
                dx_release(&mut frames);
                return Some(Err(enoent));
            }
            level -= 1;
        }
        let bhash = {
            let fr = &frames[level];
            // SAFETY: bh alive in the frame; read-only access.
            unsafe { entry_hash(&(*fr.bh).b_data, fr.entries_off, fr.at) }
        };
        if bhash & !1 != target_hash {
            dx_release(&mut frames);
            return Some(Err(enoent));
        }
        // Re-read the descended node frames (next_block's num_frames loop).
        while level < frames.len() - 1 {
            let block = {
                let fr = &frames[level];
                // SAFETY: bh alive in the frame; read-only access.
                unsafe { entry_block(&(*fr.bh).b_data, fr.entries_off, fr.at) }
            };
            level += 1;
            let Some(nbh) = read_dir_logical_block(fs, dir, block as u64) else {
                dx_release(&mut frames);
                return Some(Err(-(crate::errno::Errno::IOError.as_neg_i32())));
            };
            let parsed = {
                // SAFETY: nbh just read; read-only access.
                let data = unsafe { &(*nbh).b_data };
                parse_index_block(data, block_size, fs.metadata_csum)
                    .map(|(_, eoff, count, _)| (eoff, count))
            };
            let Some((eoff, count)) = parsed else {
                bio::brelse(nbh);
                dx_release(&mut frames);
                return None;
            };
            if count == 0 {
                bio::brelse(nbh);
                dx_release(&mut frames);
                return None;
            }
            let old = &mut frames[level];
            bio::brelse(old.bh);
            old.bh = nbh;
            old.entries_off = eoff;
            old.count = count;
            old.at = 0; // fresh node entry: the sentinel (next_block's reset)
        }
    };
    dx_release(&mut frames);
    Some(result)
}

fn dx_release(frames: &mut alloc::vec::Vec<DxFrame>) {
    for f in frames.iter_mut() {
        bio::brelse(f.bh);
    }
    frames.clear();
}
