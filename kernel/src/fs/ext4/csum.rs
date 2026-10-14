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

    /// crc32c("123456789") with init ~0 and final XOR is 0xE306_9283
    /// (RFC 3720 / iSCSI test vector); the raw update returns the
    /// pre-final state.
    #[test]
    fn test_iscsi_vector() {
        let raw = crc32c(0xFFFF_FFFF, b"123456789");
        assert_eq!(raw ^ 0xFFFF_FFFF, 0xE306_9283);
    }
}
