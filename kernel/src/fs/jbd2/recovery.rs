//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Journal Recovery (simplified implementation)
//!
//! JBD2 journal recovery
//!
//! Performs a single-pass scan+replay:
//! 1. Read journal superblock to get s_start and s_sequence
//! 2. Scan journal blocks starting from s_start
//! 3. For descriptor blocks, collect (blocknr) references
//! 4. Read the following data blocks from journal
//! 5. On commit block, replay all collected data blocks to filesystem
//! 6. On invalid/unexpected block, stop (incomplete transaction)
//! 7. Update journal superblock: s_start = 0 (clean)

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
}

impl Default for RecoveryInfo {
    fn default() -> Self {
        Self { nr_replays: 0 }
    }
}

// ============================================================================
// Main recovery function
// ============================================================================

/// Recover the journal
///
/// Scans the journal for committed but uncheckpointed transactions
/// and replays their metadata blocks to the filesystem.
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
    let sb_bh = unsafe {
        match bio::bread(device, blk_offset) {
            Some(b) => b,
            None => return Err(EIO),
        }
    };

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
    let tag_size = journal.tag_size();
    let header_size = size_of::<journal_header_t>();
    let tail_size = size_of::<journal_block_tail_t>();

    // How many tags per descriptor block
    let tags_per_block = if tag_size > 0 && header_size + tail_size < block_size {
        (block_size - header_size - tail_size) / tag_size
    } else {
        return Err(EFSCORRUPTED);
    };

    // Scan journal starting from start_block
    let mut current_block: u64 = start_block as u64;
    let mut current_seq: u32 = start_seq;
    let mut total_replayed: u32 = 0;

    loop {
        // Read next journal block
        let abs_block = blk_offset + current_block;
        let bh = unsafe {
            match bio::bread(device, abs_block) {
                Some(b) => b,
                None => break, // I/O error, stop recovery
            }
        };

        // Parse header
        let (magic, blocktype, sequence) = unsafe {
            let hdr = &*((*bh).b_data.as_ptr() as *const journal_header_t);
            (
                u32::from_be(hdr.h_magic),
                u32::from_be(hdr.h_blocktype),
                u32::from_be(hdr.h_sequence),
            )
        };

        // Validate magic
        if magic != JBD2_MAGIC_NUMBER {
            bio::brelse(bh);
            break; // Invalid block, stop
        }

        match blocktype {
            JBD2_DESCRIPTOR_BLOCK => {
                // Parse tags to collect blocknr references
                let mut blocknrs: Vec<u64> = Vec::new();
                let mut offset = header_size;

                for _ in 0..tags_per_block {
                    if offset + tag_size > block_size - tail_size {
                        break;
                    }

                    let tag = unsafe {
                        &*((*bh).b_data.as_ptr().add(offset) as *const journal_block_tag_t)
                    };
                    let blocknr = u32::from_be(tag.t_blocknr) as u64;
                    let flags = u16::from_be(tag.t_flags);

                    blocknrs.push(blocknr);

                    if (flags & (JBD2_FLAG_LAST_TAG as u16)) != 0 {
                        break;
                    }

                    offset += tag_size;
                }

                bio::brelse(bh);

                // Read data blocks that follow the descriptor
                for blocknr in &blocknrs {
                    current_block = wrap_journal_block(current_block, journal_first, journal_last);
                    let data_abs = blk_offset + current_block;

                    let data_bh = unsafe {
                        match bio::bread(device, data_abs) {
                            Some(b) => b,
                            None => break,
                        }
                    };

                    // Write data block to its filesystem location
                    let fs_bh = unsafe {
                        match bio::bread(device, *blocknr) {
                            Some(b) => b,
                            None => {
                                bio::brelse(data_bh);
                                break;
                            }
                        }
                    };

                    let copy_len = unsafe {
                        core::cmp::min((*data_bh).b_data.len(), (*fs_bh).b_data.len())
                    };
                    unsafe {
                        let fs_ref = &mut *fs_bh;
                        let data_ref = &*data_bh;
                        core::ptr::copy_nonoverlapping(
                            data_ref.b_data.as_ptr(),
                            fs_ref.b_data.as_mut_ptr(),
                            copy_len,
                        );
                        fs_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    }

                    bio::sync_dirty_buffer(fs_bh).ok();
                    bio::brelse(fs_bh);
                    bio::brelse(data_bh);
                }

                total_replayed += blocknrs.len() as u32;
            }
            JBD2_COMMIT_BLOCK => {
                bio::brelse(bh);
                // Transaction fully committed — advance sequence
                current_seq = sequence + 1;
                info.nr_replays += 1;
            }
            _ => {
                // Unknown or revoke block — skip
                bio::brelse(bh);
            }
        }

        current_block = wrap_journal_block(current_block, journal_first, journal_last);

        // Safety: don't scan more than journal size
        if total_replayed > journal.j_total_len {
            break;
        }
    }

    // Mark journal as clean: write s_start = 0
    let clean_bh = unsafe {
        match bio::bread(device, blk_offset) {
            Some(b) => b,
            None => return Err(EIO),
        }
    };

    unsafe {
        let clean_ref = &mut *clean_bh;
        let sb = &mut *(clean_ref.b_data.as_mut_ptr() as *mut journal_superblock_t);
        sb.s_start = 0u32.to_be();
        sb.s_sequence = current_seq.to_be();
        clean_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }

    bio::sync_dirty_buffer(clean_bh)?;
    bio::brelse(clean_bh);

    // Update in-memory journal state
    journal.j_head.store(current_block, core::sync::atomic::Ordering::SeqCst);
    journal.j_tail.store(current_block, core::sync::atomic::Ordering::SeqCst);
    journal.j_tail_sequence.store(current_seq, core::sync::atomic::Ordering::SeqCst);
    journal.j_transaction_sequence.store(current_seq, core::sync::atomic::Ordering::SeqCst);

    Ok(info)
}

/// Wrap journal block number within [first, last)
#[inline]
fn wrap_journal_block(mut block: u64, first: u64, last: u64) -> u64 {
    block += 1;
    if block >= last {
        block = first;
    }
    block
}
