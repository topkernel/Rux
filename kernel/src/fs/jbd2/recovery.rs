//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Journal Recovery — crash recovery
//!
//! Performs a two-pass scan+replay:
//! 1. PASS_SCAN: Scan journal to find the last valid commit block.
//! 2. PASS_REPLAY: Replay only committed metadata blocks.
//!
//! This prevents replaying data from incomplete transactions (those
//! without a commit block), which would corrupt the filesystem.

use core::mem::size_of;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::journal::{Journal, Tid};
use super::types::*;

use crate::fs::bio;

// ============================================================================
// Error codes
// ============================================================================

const EIO: i32 = 5;
const EFSCORRUPTED: i32 = 117;

// ============================================================================
// Recovery info
// ============================================================================

/// Recovery information
pub struct RecoveryInfo {
    /// Number of transactions replayed
    pub nr_replays: u32,
    /// Sequence number of the first transaction to replay
    pub start_transaction: u32,
    /// Sequence number of the last committed transaction
    pub end_transaction: u32,
}

impl Default for RecoveryInfo {
    fn default() -> Self {
        Self { nr_replays: 0, start_transaction: 0, end_transaction: 0 }
    }
}

// ============================================================================
// Main recovery function
// ============================================================================

/// Recover the journal using two-pass algorithm.
pub fn jbd2_journal_recover(journal: &Arc<Journal>) -> Result<RecoveryInfo, i32> {
    let device = journal.j_bio_device;
    if device.is_null() {
        return Ok(RecoveryInfo::default());
    }

    let blk_offset = journal.j_blk_offset;
    let block_size = journal.j_blocksize as usize;
    let journal_first = journal.j_first;
    let journal_last = journal.j_last;

    // Read journal superblock to get s_start and s_sequence
    // SAFETY: bio::bread returns a valid BufferHead or None (handled).
    let sb_bh = unsafe {
        match bio::bread(device, blk_offset) {
            Some(b) => b,
            None => return Err(EIO),
        }
    };

    // SAFETY: sb_bh is a valid BufferHead; the superblock is at the start of the block.
    let (start_block, start_seq) = unsafe {
        let sb = &*((*sb_bh).b_data.as_ptr() as *const journal_superblock_t);
        let start = u32::from_be(sb.s_start);
        let seq = u32::from_be(sb.s_sequence);
        bio::brelse(sb_bh);
        (start, seq)
    };

    // If s_start == 0, journal is clean
    if start_block == 0 {
        return Ok(RecoveryInfo::default());
    }

    let mut info = RecoveryInfo::default();
    info.start_transaction = start_seq;

    let tag_size = journal.tag_size();
    let header_size = size_of::<journal_header_t>();
    let tail_size = size_of::<journal_block_tail_t>();

    let tags_per_block = if tag_size > 0 && header_size + tail_size < block_size {
        (block_size - header_size - tail_size) / tag_size
    } else {
        return Err(EFSCORRUPTED);
    };

    let max_scan = journal.j_total_len.saturating_mul(2);

    // ========================================================================
    // PASS_SCAN: find last commit block
    // ========================================================================
    let mut next_block: u64 = start_block as u64;
    let mut last_found_seq: u32 = 0;
    let mut replay_start: u64 = 0;
    let mut scanned: u32 = 0;

    while scanned < max_scan {
        scanned += 1;
        let abs_block = blk_offset + next_block;
        // SAFETY: bio::bread returns a valid BufferHead or None (break on None).
        let bh = unsafe {
            match bio::bread(device, abs_block) {
                Some(b) => b,
                None => break,
            }
        };

        // SAFETY: bh is a valid BufferHead; b_data starts with a journal_header_t;
        // magic check below validates the contents before use.
        let (magic, blocktype, sequence) = unsafe {
            let hdr = &*((*bh).b_data.as_ptr() as *const journal_header_t);
            (u32::from_be(hdr.h_magic), u32::from_be(hdr.h_blocktype), u32::from_be(hdr.h_sequence))
        };

        if magic != JBD2_MAGIC_NUMBER { bio::brelse(bh); break; }

        match blocktype {
            JBD2_DESCRIPTOR_BLOCK => {
                // CSUM_V2/V3: a descriptor whose tail checksum fails ends
                // the trustworthy log region (reference recovery.c PASS_SCAN
                // treats it as stale/unverifiable and stops extending).
                let desc_ok = unsafe {
                    let data = core::slice::from_raw_parts(
                        (*bh).b_data.as_ptr(),
                        (*bh).b_data.len(),
                    );
                    descriptor_block_csum_verify(journal, data)
                };
                if !desc_ok {
                    bio::brelse(bh);
                    break;
                }
                if replay_start == 0 { replay_start = next_block; }
                let nr = count_tags(bh, tag_size, header_size, tail_size, block_size);
                bio::brelse(bh);
                for _ in 0..nr { next_block = wrap_block(next_block, journal_first, journal_last); scanned += 1; }
                continue;
            }
            JBD2_COMMIT_BLOCK => {
                // CSUM_V2/V3: only a commit block with a valid checksum
                // completes a transaction (reference
                // jbd2_commit_block_csum_verify).
                let commit_ok = unsafe {
                    let data = core::slice::from_raw_parts(
                        (*bh).b_data.as_ptr(),
                        (*bh).b_data.len(),
                    );
                    commit_block_csum_verify(journal, data)
                };
                if commit_ok {
                    last_found_seq = sequence;
                }
                bio::brelse(bh);
                next_block = wrap_block(next_block, journal_first, journal_last);
                continue;
            }
            JBD2_REVOKE_BLOCK => {
                // Collect revoke records for the replay filter (review 5.6:
                // recovery must not replay old metadata over blocks that
                // were freed then reallocated).
                // SAFETY: bh is a valid bio::BufferHead; b_data spans the block.
                unsafe {
                    let data = core::slice::from_raw_parts(
                        (*bh).b_data.as_ptr(),
                        (*bh).b_data.len(),
                    );
                    super::revoke::scan_revoke_block(journal, data, sequence);
                }
                bio::brelse(bh);
                next_block = wrap_block(next_block, journal_first, journal_last);
                continue;
            }
            _ => { bio::brelse(bh); }
        }
        next_block = wrap_block(next_block, journal_first, journal_last);
    }

    if last_found_seq == 0 {
        write_clean_sb(journal, start_seq)?;
        return Ok(info);
    }

    info.end_transaction = last_found_seq;

    // ========================================================================
    // PASS_REPLAY: replay committed blocks
    // ========================================================================
    let mut cur = replay_start;
    let mut scanned: u32 = 0;

    while scanned < max_scan {
        scanned += 1;
        let abs_block = blk_offset + cur;
        // SAFETY: bio::bread returns a valid BufferHead or None (break on None).
        let bh = unsafe {
            match bio::bread(device, abs_block) {
                Some(b) => b,
                None => break,
            }
        };

        // SAFETY: bh is valid; b_data starts with journal_header_t; magic check validates.
        let (magic, blocktype, sequence) = unsafe {
            let hdr = &*((*bh).b_data.as_ptr() as *const journal_header_t);
            (u32::from_be(hdr.h_magic), u32::from_be(hdr.h_blocktype), u32::from_be(hdr.h_sequence))
        };

        if magic != JBD2_MAGIC_NUMBER { bio::brelse(bh); break; }

        match blocktype {
            JBD2_DESCRIPTOR_BLOCK => {
                if sequence >= info.start_transaction && sequence <= info.end_transaction {
                    // CSUM_V2/V3: an unverifiable descriptor ends replay
                    // (reference recovery.c: "Invalid checksum recovering
                    // block" -> -EFSBADCRC).
                    let desc_ok = unsafe {
                        let data = core::slice::from_raw_parts(
                            (*bh).b_data.as_ptr(),
                            (*bh).b_data.len(),
                        );
                        descriptor_block_csum_verify(journal, data)
                    };
                    if !desc_ok {
                        bio::brelse(bh);
                        break;
                    }
                    let tags = unsafe { parse_tag_entries(bh, tag_size, header_size, tail_size, block_size) };
                    bio::brelse(bh);
                    for tag in tags {
                        cur = wrap_block(cur, journal_first, journal_last);
                        scanned += 1;
                        // Revoke filter: skip blocks revoked at tid >= this
                        // transaction's sequence (a newer delete must win
                        // over an older write in the log).
                        if super::revoke::block_is_revoked_for(journal, tag.blocknr, sequence) {
                            continue;
                        }
                        replay_data(device, blk_offset, cur, tag.blocknr, journal, tag.csum, sequence);
                    }
                } else {
                    let nr = count_tags(bh, tag_size, header_size, tail_size, block_size);
                    bio::brelse(bh);
                    for _ in 0..nr { cur = wrap_block(cur, journal_first, journal_last); scanned += 1; }
                }
                continue;
            }
            JBD2_COMMIT_BLOCK => {
                let commit_ok = unsafe {
                    let data = core::slice::from_raw_parts(
                        (*bh).b_data.as_ptr(),
                        (*bh).b_data.len(),
                    );
                    commit_block_csum_verify(journal, data)
                };
                bio::brelse(bh);
                if commit_ok && sequence >= info.start_transaction && sequence <= info.end_transaction {
                    info.nr_replays += 1;
                }
            }
            _ => { bio::brelse(bh); }
        }
        cur = wrap_block(cur, journal_first, journal_last);
    }

    write_clean_sb(journal, info.end_transaction + 1)?;
    // Recovery is done — the revoke set was replay-scoped.
    journal.revoke_records.lock().clear();
    Ok(info)
}

// ============================================================================
// Helper functions
// ============================================================================

fn write_clean_sb(journal: &Arc<Journal>, next_seq: u32) -> Result<(), i32> {
    let device = journal.j_bio_device;
    let blk_offset = journal.j_blk_offset;
    // SAFETY: bio::bread returns a valid BufferHead or None (handled).
    let sb_bh = unsafe {
        match bio::bread(device, blk_offset) {
            Some(b) => b,
            None => return Err(EIO),
        }
    };
    // SAFETY: sb_bh is a valid BufferHead; the superblock is at the start of the block.
    unsafe {
        let r = &mut *sb_bh;
        let sb = &mut *(r.b_data.as_mut_ptr() as *mut journal_superblock_t);
        sb.s_start = 0u32.to_be();
        sb.s_sequence = next_seq.to_be();
        // CSUM_V2/V3: keep the superblock checksum consistent with the
        // rewritten s_start/s_sequence (reference jbd2_superblock_csum).
        if journal.has_csum_v2or3() {
            sb.s_checksum = 0;
            let bytes: &[u8] = core::slice::from_raw_parts(
                r.b_data.as_ptr(),
                size_of::<journal_superblock_t>(),
            );
            let csum = crate::fs::ext4::csum::crc32c(0xFFFF_FFFF, bytes);
            sb.s_checksum = csum.to_be();
        }
        r.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }
    let sync_res = bio::sync_dirty_buffer(sb_bh);
    bio::brelse(sb_bh);
    sync_res?;
    journal.j_tail_sequence.store(next_seq, core::sync::atomic::Ordering::SeqCst);
    journal.j_transaction_sequence.store(next_seq, core::sync::atomic::Ordering::SeqCst);
    Ok(())
}

fn count_tags(bh: *mut bio::BufferHead, tag_size: usize, hdr_size: usize, tail_size: usize, blk_size: usize) -> usize {
    let mut c = 0usize;
    let mut off = hdr_size;
    while off + tag_size <= blk_size - tail_size {
        // SAFETY: off is bounds-checked against blk_size; b_data contains valid tag data.
        let flags = unsafe { tag_flags((*bh).b_data.as_ptr().add(off), tag_size) };
        c += 1;
        if flags & JBD2_FLAG_LAST_TAG != 0 { break; }
        off += tag_size;
    }
    c
}

/// Read a tag's flag word from its on-disk bytes. V3 tags carry a 32-bit
/// flag word at +4; V1/V2 tags carry a 16-bit flag word at +6 (after the
/// truncated checksum).
/// SAFETY: `p` must point at `tag_size` readable bytes (bounds-checked by
/// the callers against the block tail).
unsafe fn tag_flags(p: *const u8, tag_size: usize) -> u32 {
    if tag_size == size_of::<journal_block_tag3_t>() {
        u32::from_be(core::ptr::read_unaligned(p.add(4) as *const u32))
    } else {
        u16::from_be(core::ptr::read_unaligned(p.add(6) as *const u16)) as u32
    }
}

/// One descriptor tag's replay inputs: target fs block and the tag
/// checksum when the journal carries CSUM_V2/V3 (V3: full u32 at +12;
/// V2: truncated u16 at +4).
struct TagEntry {
    blocknr: u64,
    csum: Option<u32>,
}

/// SAFETY: `p` must point at `tag_size` readable bytes.
unsafe fn parse_tag(p: *const u8, tag_size: usize) -> TagEntry {
    let blocknr =
        u32::from_be(core::ptr::read_unaligned(p as *const u32)) as u64;
    let (blocknr, csum) = if tag_size == size_of::<journal_block_tag3_t>() {
        let high =
            u32::from_be(core::ptr::read_unaligned(p.add(8) as *const u32));
        let csum =
            u32::from_be(core::ptr::read_unaligned(p.add(12) as *const u32));
        (blocknr | ((high as u64) << 32), Some(csum))
    } else if tag_size == size_of::<journal_block_tag_t>() {
        let high =
            u32::from_be(core::ptr::read_unaligned(p.add(8) as *const u32));
        let trunc =
            u16::from_be(core::ptr::read_unaligned(p.add(4) as *const u16)) as u32;
        (blocknr | ((high as u64) << 32), Some(trunc))
    } else {
        let trunc =
            u16::from_be(core::ptr::read_unaligned(p.add(4) as *const u16)) as u32;
        (blocknr, Some(trunc))
    };
    TagEntry { blocknr, csum }
}

/// Parse all tags of a descriptor block into replay entries.
/// SAFETY: bh is a valid BufferHead spanning blk_size bytes.
unsafe fn parse_tag_entries(bh: *mut bio::BufferHead, tag_size: usize, hdr_size: usize, tail_size: usize, blk_size: usize) -> Vec<TagEntry> {
    let mut tags = Vec::new();
    let mut off = hdr_size;
    while off + tag_size <= blk_size - tail_size {
        let p = (*bh).b_data.as_ptr().add(off);
        let e = parse_tag(p, tag_size);
        let flags = tag_flags(p, tag_size);
        tags.push(e);
        if flags & JBD2_FLAG_LAST_TAG != 0 { break; }
        off += tag_size;
    }
    tags
}

// ============================================================================
// CSUM_V2/V3 verification (reference fs/jbd2/recovery.c)
// ============================================================================

/// Verify a descriptor/revoke block's tail checksum.
/// SAFETY: `data` must span the whole journal block.
unsafe fn descriptor_block_csum_verify(journal: &Arc<Journal>, data: &[u8]) -> bool {
    if !journal.has_csum_v2or3() {
        return true;
    }
    let n = data.len();
    let tail = size_of::<journal_block_tail_t>();
    let provided_bytes = [data[n - tail], data[n - tail + 1], data[n - tail + 2], data[n - tail + 3]];
    let provided = u32::from_be_bytes(provided_bytes);
    let mut tmp = alloc::vec![0u8; n];
    tmp.copy_from_slice(data);
    tmp[n - tail..].fill(0);
    let calc = crate::fs::ext4::csum::crc32c(journal.csum_seed(), &tmp);
    calc == provided
}

/// Verify a commit block's checksum (h_chksum[0]; type/size/chksum zeroed
/// in the input).
/// SAFETY: `data` must span the whole journal block.
unsafe fn commit_block_csum_verify(journal: &Arc<Journal>, data: &[u8]) -> bool {
    if !journal.has_csum_v2or3() {
        return true;
    }
    let h = &*(data.as_ptr() as *const commit_header);
    let provided = u32::from_be(h.h_chksum[0]);
    let mut tmp = alloc::vec![0u8; data.len()];
    tmp.copy_from_slice(data);
    let th = &mut *(tmp.as_mut_ptr() as *mut commit_header);
    th.h_chksum_type = 0;
    th.h_chksum_size = 0;
    th.h_chksum[0] = 0;
    let calc = crate::fs::ext4::csum::crc32c(journal.csum_seed(), &tmp);
    calc == provided
}

/// Verify one tag's data-block checksum (reference
/// jbd2_block_tag_csum_verify: crc32c(crc32c(seed, be32(seq)), block);
/// V2 compares the truncated low 16 bits).
fn block_tag_csum_verify(
    journal: &Arc<Journal>,
    tag_csum: u32,
    sequence: u32,
    data: &[u8],
    truncated: bool,
) -> bool {
    if !journal.has_csum_v2or3() {
        return true;
    }
    let csum = crate::fs::ext4::csum::crc32c(
        journal.csum_seed(),
        &sequence.to_be_bytes(),
    );
    let calc = crate::fs::ext4::csum::crc32c(csum, data);
    if truncated {
        (calc as u16) as u32 == tag_csum
    } else {
        calc == tag_csum
    }
}

/// Replay one data block from the journal to its fs location. When the
/// journal carries CSUM_V2/V3, the tag checksum is verified against the
/// block bytes first (reference jbd2_block_tag_csum_verify — a mismatch
/// skips the block: "Invalid checksum recovering data block").
fn replay_data(
    device: *const crate::drivers::blkdev::GenDisk,
    blk_offset: u64,
    journal_block: u64,
    fs_block: u64,
    journal: &Arc<Journal>,
    tag_csum: Option<u32>,
    sequence: u32,
) {
    let data_abs = blk_offset + journal_block;
    // SAFETY: bio::bread returns a valid BufferHead or None (handled by returning).
    let data_bh = unsafe { match bio::bread(device, data_abs) { Some(b) => b, None => return } };
    if let Some(csum) = tag_csum {
        // SAFETY: data_bh spans the journal block; the slice covers exactly it.
        let ok = unsafe {
            let data = core::slice::from_raw_parts(
                (*data_bh).b_data.as_ptr(),
                (*data_bh).b_data.len(),
            );
            block_tag_csum_verify(journal, csum, sequence, data, tag_csum_truncated(journal))
        };
        if !ok {
            crate::pr_warn!("JBD2: invalid checksum recovering data block {}", fs_block);
            bio::brelse(data_bh);
            return;
        }
    }
    // SAFETY: bio::bread returns a valid BufferHead or None; data_bh released on None.
    let fs_bh = unsafe { match bio::bread(device, fs_block) { Some(b) => b, None => { bio::brelse(data_bh); return } } };
    // SAFETY: both bh pointers are valid BufferHeads from bio::bread above.
    let len = unsafe { core::cmp::min((*data_bh).b_data.len(), (*fs_bh).b_data.len()) };
    // SAFETY: both bh pointers are valid; len is the minimum of both buffer sizes;
    // copy_nonoverlapping copies exactly len bytes.
    unsafe {
        let f = &mut *fs_bh;
        let d = &*data_bh;
        core::ptr::copy_nonoverlapping(d.b_data.as_ptr(), f.b_data.as_mut_ptr(), len);
        f.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }
    bio::sync_dirty_buffer(fs_bh).ok();
    bio::brelse(fs_bh);
    bio::brelse(data_bh);
}

/// Whether this journal's tags carry truncated (V2) checksums.
fn tag_csum_truncated(journal: &Arc<Journal>) -> bool {
    journal.has_csum_v2() && !journal.has_csum_v3()
}

#[inline]
fn wrap_block(block: u64, first: u64, last: u64) -> u64 {
    let n = block + 1;
    if n >= last { first } else { n }
}
