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
                if replay_start == 0 { replay_start = next_block; }
                let nr = count_tags(bh, tag_size, header_size, tail_size, block_size);
                bio::brelse(bh);
                for _ in 0..nr { next_block = wrap_block(next_block, journal_first, journal_last); scanned += 1; }
                continue;
            }
            JBD2_COMMIT_BLOCK => {
                last_found_seq = sequence;
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
                    let tags = parse_tags(bh, tag_size, header_size, tail_size, block_size);
                    bio::brelse(bh);
                    for blocknr in tags {
                        cur = wrap_block(cur, journal_first, journal_last);
                        scanned += 1;
                        replay_data(device, blk_offset, cur, blocknr);
                    }
                } else {
                    let nr = count_tags(bh, tag_size, header_size, tail_size, block_size);
                    bio::brelse(bh);
                    for _ in 0..nr { cur = wrap_block(cur, journal_first, journal_last); scanned += 1; }
                }
                continue;
            }
            JBD2_COMMIT_BLOCK => {
                bio::brelse(bh);
                if sequence >= info.start_transaction && sequence <= info.end_transaction {
                    info.nr_replays += 1;
                }
            }
            _ => { bio::brelse(bh); }
        }
        cur = wrap_block(cur, journal_first, journal_last);
    }

    write_clean_sb(journal, info.end_transaction + 1)?;
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
        r.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }
    bio::sync_dirty_buffer(sb_bh)?;
    bio::brelse(sb_bh);
    journal.j_tail_sequence.store(next_seq, core::sync::atomic::Ordering::SeqCst);
    journal.j_transaction_sequence.store(next_seq, core::sync::atomic::Ordering::SeqCst);
    Ok(())
}

fn count_tags(bh: *mut bio::BufferHead, tag_size: usize, hdr_size: usize, tail_size: usize, blk_size: usize) -> usize {
    let mut c = 0usize;
    let mut off = hdr_size;
    while off + tag_size <= blk_size - tail_size {
        // SAFETY: off is bounds-checked against blk_size; b_data contains valid tag data.
        let flags = unsafe { u16::from_be((*((*bh).b_data.as_ptr().add(off) as *const journal_block_tag_t)).t_flags) };
        c += 1;
        if (flags & (JBD2_FLAG_LAST_TAG as u16)) != 0 { break; }
        off += tag_size;
    }
    c
}

fn parse_tags(bh: *mut bio::BufferHead, tag_size: usize, hdr_size: usize, tail_size: usize, blk_size: usize) -> Vec<u64> {
    let mut tags = Vec::new();
    let mut off = hdr_size;
    while off + tag_size <= blk_size - tail_size {
        // SAFETY: off is bounds-checked against blk_size; b_data contains valid tag data.
        let (nr, flags) = unsafe {
            let t = &*((*bh).b_data.as_ptr().add(off) as *const journal_block_tag_t);
            (u32::from_be(t.t_blocknr) as u64, u16::from_be(t.t_flags))
        };
        tags.push(nr);
        if (flags & (JBD2_FLAG_LAST_TAG as u16)) != 0 { break; }
        off += tag_size;
    }
    tags
}

fn replay_data(device: *const crate::drivers::blkdev::GenDisk, blk_offset: u64, journal_block: u64, fs_block: u64) {
    let data_abs = blk_offset + journal_block;
    // SAFETY: bio::bread returns a valid BufferHead or None (handled by returning).
    let data_bh = unsafe { match bio::bread(device, data_abs) { Some(b) => b, None => return } };
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

#[inline]
fn wrap_block(block: u64, first: u64, last: u64) -> u64 {
    let n = block + 1;
    if n >= last { first } else { n }
}
