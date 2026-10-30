//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 metadata_csum verification (read path).
//!
//! The OH system/vendor images are mkfs'd with `metadata_csum`
//! (ro_compat 0x400): every superblock, group descriptor and inode
//! carries a crc32c (Castagnoli) checksum over the fs UUID seed, the
//! object identity and the raw bytes with the checksum fields zeroed.
//! Mounting such an image read-only requires VERIFYING them (Linux
//! refuses a fs whose metadata_csum it does not implement; silently
//! trusting unverified metadata is worse).
//!
//! Algorithms byte-for-byte verified against the OH system.img with
//! e2fsprogs 1.47.2 / e2fsck -fn (clean) and a host-side reference
//! implementation:
//!
//! - seed    = crc32c(0xFFFF_FFFF, s_uuid)
//!   (e2fsprogs also stores this in s_checksum_seed when the CSUM_SEED
//!   incompat bit is set; the stored value, when non-zero, wins)
//! - sb      = crc32c(0xFFFF_FFFF, sb_bytes[0..0x3FC]) & 0xFFFF
//!             vs u16 at 0x3FC
//! - gdt[g]  = crc32c(seed, le32(g), desc[0..30], zeros(2),
//!                    desc[32..desc_size]) & 0xFFFF vs u16 at desc+30
//! - inode   = crc32c(seed, le32(ino), raw[100..104] /*generation*/,
//!                    raw[0..124], zeros(2), raw[126..130], zeros(2),
//!                    raw[132..inode_size])
//!             compared as FULL 32 bits: lo | (hi << 16) when the inode
//!             size exceeds 128, else the low 16 bits.
//!
//! The crc32c here is the RAW running-state update (no init folding, no
//! final XOR) — exactly crypto crc32c's crc32c(seed, data) as used by
//! the kernel's ext4_chksum(); the stored values are the running state
//! truncated to their field widths.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

/// crc32c (Castagnoli, reflected 0x82F63B78) table.
const CRC32C_TABLE: [u32; 256] = make_table();

const fn make_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut b = 0;
        while b < 8 {
            c = if c & 1 != 0 {
                0x82F6_3B78 ^ (c >> 1)
            } else {
                c >> 1
            };
            b += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// Raw crc32c update: `seed` is the running CRC state (start with
/// 0xFFFF_FFFF for a fresh checksum).
pub fn crc32c(seed: u32, data: &[u8]) -> u32 {
    let mut c = seed;
    for &b in data {
        c = CRC32C_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

/// Filesystem checksum seed from the superblock UUID (ext4_chksum(~0,
/// s_uuid)); the on-disk s_checksum_seed (CSUM_SEED feature) holds the
/// same value — prefer it when present and non-zero.
pub fn csum_seed(uuid: &[u8; 16], disk_seed: u32) -> u32 {
    if disk_seed != 0 {
        disk_seed
    } else {
        crc32c(0xFFFF_FFFF, uuid)
    }
}

/// Offset of s_checksum in the on-disk superblock (1020).
const SB_CSUM_OFF: usize = 0x3FC;
/// Offset of bg_checksum in an on-disk group descriptor.
const GD_CSUM_OFF: usize = 30;
/// Offset of i_checksum_lo in an on-disk inode.
const INO_CSUM_LO_OFF: usize = 124;
/// Offset of i_generation in an on-disk inode.
const INO_GEN_OFF: usize = 100;
/// Offset of i_checksum_hi in an on-disk inode (valid when inode size
/// exceeds 128).
const INO_CSUM_HI_OFF: usize = 130;
/// Legacy ("good old") inode size boundary.
const GOOD_OLD_INODE_SIZE: usize = 128;

/// Verify the superblock checksum. `sb` is the full 1024-byte on-disk
/// superblock.
pub fn superblock_csum_ok(sb: &[u8]) -> bool {
    if sb.len() < SB_CSUM_OFF + 2 {
        return false;
    }
    let stored = u16::from_le_bytes([sb[SB_CSUM_OFF], sb[SB_CSUM_OFF + 1]]);
    stored == (crc32c(0xFFFF_FFFF, &sb[..SB_CSUM_OFF]) & 0xFFFF) as u16
}

/// Compute the group descriptor checksum. `desc` is `desc_size` raw
/// bytes of the on-disk descriptor; `group` is the block group number.
pub fn group_desc_csum(seed: u32, group: u32, desc: &[u8]) -> u16 {
    if desc.len() <= GD_CSUM_OFF {
        return 0;
    }
    let mut c = crc32c(seed, &group.to_le_bytes());
    c = crc32c(c, &desc[..GD_CSUM_OFF]);
    c = crc32c(c, &[0u8; 2]); // bg_checksum zeroed
    if desc.len() > GD_CSUM_OFF + 2 {
        c = crc32c(c, &desc[GD_CSUM_OFF + 2..]);
    }
    (c & 0xFFFF) as u16
}

/// Verify one group descriptor's stored checksum.
pub fn group_desc_csum_ok(seed: u32, group: u32, desc: &[u8]) -> bool {
    if desc.len() <= GD_CSUM_OFF {
        return false;
    }
    let stored = u16::from_le_bytes([desc[GD_CSUM_OFF], desc[GD_CSUM_OFF + 1]]);
    stored == group_desc_csum(seed, group, desc)
}

/// Compute the full 32-bit inode checksum. `raw` is the FULL
/// inode_size bytes of the on-disk inode.
pub fn inode_csum(seed: u32, ino: u32, raw: &[u8]) -> u32 {
    if raw.len() < INO_CSUM_LO_OFF + 2 {
        return 0;
    }
    let mut c = crc32c(seed, &ino.to_le_bytes());
    if raw.len() >= INO_GEN_OFF + 4 {
        c = crc32c(c, &raw[INO_GEN_OFF..INO_GEN_OFF + 4]); // i_generation
    }
    c = crc32c(c, &raw[..INO_CSUM_LO_OFF]);
    c = crc32c(c, &[0u8; 2]); // i_checksum_lo zeroed
    if raw.len() > GOOD_OLD_INODE_SIZE {
        // 126..130: l_i_reserved + i_extra_isize (checksummed as stored);
        // 130..132: i_checksum_hi zeroed; then the rest of the inode.
        c = crc32c(c, &raw[126..INO_CSUM_HI_OFF]);
        c = crc32c(c, &[0u8; 2]);
        if raw.len() > INO_CSUM_HI_OFF + 2 {
            c = crc32c(c, &raw[INO_CSUM_HI_OFF + 2..]);
        }
    }
    c
}

/// Verify an inode's stored checksum (full 32-bit compare when the
/// inode carries a hi half, low-16 otherwise — ext4_inode_csum_verify).
pub fn inode_csum_ok(seed: u32, ino: u32, raw: &[u8]) -> bool {
    if raw.len() < INO_CSUM_LO_OFF + 2 {
        return false;
    }
    let lo = u16::from_le_bytes([raw[INO_CSUM_LO_OFF], raw[INO_CSUM_LO_OFF + 1]]);
    let calc = inode_csum(seed, ino, raw);
    if raw.len() > INO_CSUM_HI_OFF + 2 {
        let hi = u16::from_le_bytes([raw[INO_CSUM_HI_OFF], raw[INO_CSUM_HI_OFF + 1]]);
        (lo as u32 | (hi as u32) << 16) == calc
    } else {
        lo as u32 == (calc & 0xFFFF)
    }
}

/// Write-path counterpart of `inode_csum_ok`: store the checksum into
/// the raw on-disk inode bytes. `raw` is the FULL `inode_size` slice of
/// the inode slot with the final content already in place; the function
/// zeroes both checksum halves, recomputes over the full slot and
/// writes lo (+ hi when the slot is larger than the good-old 128).
///
/// Scope note (ext4.rwcsum bring-up mode): the kernel READ path
/// verifies exactly { superblock, group descriptor, inode } checksums;
/// these three `*_csum_set` helpers maintain those on the write path so
/// a forced-rw metadata_csum mount stays self-consistent for reads.
/// Block/inode-bitmap csums (bg_*_bitmap_csum in the descriptor),
/// extent-tree blocks and directory tails are NOT maintained — e2fsck
/// will flag those on a real device; the bring-up boots run under QEMU
/// -snapshot, where nothing persists.
pub fn inode_csum_set(seed: u32, ino: u32, raw: &mut [u8]) {
    if raw.len() < INO_CSUM_LO_OFF + 2 {
        return;
    }
    raw[INO_CSUM_LO_OFF] = 0;
    raw[INO_CSUM_LO_OFF + 1] = 0;
    if raw.len() > INO_CSUM_HI_OFF + 2 {
        raw[INO_CSUM_HI_OFF] = 0;
        raw[INO_CSUM_HI_OFF + 1] = 0;
    }
    let calc = inode_csum(seed, ino, raw);
    let lo = (calc & 0xFFFF) as u16;
    raw[INO_CSUM_LO_OFF..INO_CSUM_LO_OFF + 2].copy_from_slice(&lo.to_le_bytes());
    if raw.len() > INO_CSUM_HI_OFF + 2 {
        let hi = (calc >> 16) as u16;
        raw[INO_CSUM_HI_OFF..INO_CSUM_HI_OFF + 2].copy_from_slice(&hi.to_le_bytes());
    }
}

/// Write-path counterpart of `group_desc_csum_ok`: store bg_checksum
/// into the raw `desc_size` bytes of one group descriptor.
pub fn group_desc_csum_set(seed: u32, group: u32, desc: &mut [u8]) {
    if desc.len() <= GD_CSUM_OFF + 1 {
        return;
    }
    desc[GD_CSUM_OFF] = 0;
    desc[GD_CSUM_OFF + 1] = 0;
    let c = group_desc_csum(seed, group, desc);
    desc[GD_CSUM_OFF..GD_CSUM_OFF + 2].copy_from_slice(&c.to_le_bytes());
}

/// Write-path counterpart of `superblock_csum_ok`: store s_checksum
/// into the full 1024-byte on-disk superblock.
pub fn superblock_csum_set(sb: &mut [u8]) {
    if sb.len() < SB_CSUM_OFF + 2 {
        return;
    }
    sb[SB_CSUM_OFF] = 0;
    sb[SB_CSUM_OFF + 1] = 0;
    let c = (crc32c(0xFFFF_FFFF, &sb[..SB_CSUM_OFF]) & 0xFFFF) as u16;
    sb[SB_CSUM_OFF..SB_CSUM_OFF + 2].copy_from_slice(&c.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table's first entries are the standard Castagnoli values.
    #[test]
    fn test_table_prefix() {
        assert_eq!(CRC32C_TABLE[0], 0);
        assert_eq!(CRC32C_TABLE[1], 0x82F6_3B78);
        assert_eq!(CRC32C_TABLE[255], 0x7937_7907);
    }

    /// The write helpers round-trip through the verify helpers: set
    /// then ok on the same bytes must hold for every object size class
    /// the mount can encounter.
    #[test]
    fn test_csum_set_round_trip() {
        // Inode: good-old 128 and a 256-byte slot (checksum hi half used).
        for size in [128usize, 256] {
            let mut raw = vec![0u8; size];
            for i in 0..size {
                raw[i] = (i * 7 + 3) as u8;
            }
            inode_csum_set(0x1234_5678, 42, &mut raw);
            assert!(inode_csum_ok(0x1234_5678, 42, &raw), "inode size {size}");
            // Any content change must break the checksum.
            raw[8] ^= 0xFF;
            assert!(!inode_csum_ok(0x1234_5678, 42, &raw), "inode size {size}");
        }
        // Group descriptor: 32-byte (64-bit desc hi half beyond 32 is
        // covered by the desc_size>32 case below).
        for dsize in [32usize, 64] {
            let mut desc = vec![0u8; dsize];
            for i in 0..dsize {
                desc[i] = (i * 13 + 5) as u8;
            }
            group_desc_csum_set(0x9ABC_DEF0, 7, &mut desc);
            assert!(group_desc_csum_ok(0x9ABC_DEF0, 7, &desc), "desc size {dsize}");
            desc[4] ^= 0x01;
            assert!(!group_desc_csum_ok(0x9ABC_DEF0, 7, &desc), "desc size {dsize}");
        }
        // Superblock: 1024 bytes, checksum lives at 0x3FC.
        let mut sb = vec![0u8; 1024];
        for i in 0..0x3FC {
            sb[i] = (i * 31 + 11) as u8;
        }
        superblock_csum_set(&mut sb);
        assert!(superblock_csum_ok(&sb));
        sb[0] ^= 0x80;
        assert!(!superblock_csum_ok(&sb));
    }

    /// crc32c("123456789") with init ~0 and final XOR is 0xE306_9283
    /// (RFC 3720 / iSCSI test vector); the raw update returns the
    /// pre-final state.
    #[test]
    fn test_iscsi_vector() {
        let raw = crc32c(0xFFFF_FFFF, b"123456789");
        assert_eq!(raw ^ 0xFFFF_FFFF, 0xE306_9283);
    }
}
