//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Commit logic (simplified synchronous implementation)
//!
//! JBD2 commit phase
//!
//! Journal on-disk layout per transaction:
//!   [descriptor_block: header + tags] [data_block_1] [data_block_2] ...
//!   [descriptor_block_2: header + tags] [data_block_N] ...
//!   [commit_block]

use core::mem::size_of;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::journal::{Journal, Transaction, TransactionState};
use super::types::*;

use crate::fs::bio;

// ============================================================================
// Error codes
// ============================================================================

const EIO: i32 = 5;
const ENOMEM: i32 = 12;

// ============================================================================
// Main commit function
// ============================================================================

/// Commit a transaction to the journal (simplified synchronous implementation)
pub fn jbd2_journal_commit_transaction(
    journal: &Arc<Journal>,
    commit_transaction: &Arc<Transaction>,
) -> Result<(), i32> {
    let device = journal.j_bio_device;
    if device.is_null() {
        return Ok(());
    }

    let blk_offset = journal.j_blk_offset;
    let block_size = journal.j_blocksize as usize;
    let tid = commit_transaction.t_tid;

    // Phase 1: Lock transaction
    {
        let mut state = commit_transaction.t_state.lock();
        *state = TransactionState::Locked;
    }

    // Wait for all updates (spin on single-core — should already be 0)
    while commit_transaction.t_updates.load(core::sync::atomic::Ordering::SeqCst) > 0 {
        core::hint::spin_loop();
    }

    // Collect dirty buffers
    let dirty_buffers: Vec<(u64, Vec<u8>)>;
    {
        let mut bufs = commit_transaction.t_dirty_buffers.lock();
        dirty_buffers = core::mem::take(&mut *bufs);
    }

    if dirty_buffers.is_empty() {
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Ok(());
    }

    // Phase 2: Write to journal
    {
        let mut state = commit_transaction.t_state.lock();
        *state = TransactionState::Flush;
    }

    let tag_size = journal.tag_size();
    let header_size = size_of::<journal_header_t>();
    let tail_size = size_of::<journal_block_tail_t>();
    let tags_per_block = if tag_size > 0 {
        (block_size - header_size - tail_size) / tag_size
    } else {
        0
    };
    if tags_per_block == 0 {
        journal.abort(EIO);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Err(EIO);
    }

    // Calculate total journal blocks needed:
    // For each group of tags_per_block buffers: 1 descriptor + N data blocks
    // Plus 1 commit block
    let num_buffers = dirty_buffers.len();
    let num_desc_blocks = (num_buffers + tags_per_block - 1) / tags_per_block;
    let total_journal_blocks = num_desc_blocks + num_buffers + 1;

    let journal_head = journal.j_head.load(core::sync::atomic::Ordering::SeqCst);
    let journal_first = journal.j_first;
    let journal_last = journal.j_last;

    // Check space
    let journal_free = journal.j_free.load(core::sync::atomic::Ordering::SeqCst);
    if total_journal_blocks as u64 > journal_free {
        crate::console::puts("jbd2: journal full, cannot commit\n");
        journal.abort(ENOMEM);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Err(ENOMEM);
    }

    let mut current_journal_block = journal_head;
    let mut buf_idx = 0;

    // Write descriptor blocks interleaved with data blocks
    for _desc_block in 0..num_desc_blocks {
        let tags_this_block = core::cmp::min(tags_per_block, num_buffers - buf_idx);
        let abs_block = blk_offset + current_journal_block;

        // --- Write descriptor block ---
        // SAFETY: bio::bread returns a valid BufferHead or None (handled by ?).
        let bh = unsafe {
            match bio::bread(device, abs_block) {
                Some(b) => b,
                None => {
                    journal.abort(EIO);
                    *commit_transaction.t_state.lock() = TransactionState::Finished;
                    return Err(EIO);
                }
            }
        };

        // Build descriptor: header + tags + tail
        let header = journal_header_t::new(JBD2_DESCRIPTOR_BLOCK, tid);
        // SAFETY: header is a stack-local journal_header_t; reinterpreting as bytes is safe
        // because the struct is #[repr(C)] with no padding concerns for the sizes used.
        let header_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(&header as *const _ as *const u8, header_size)
        };

        let mut desc_data = alloc::vec![0u8; block_size];
        desc_data[0..header_size].copy_from_slice(header_bytes);

        let mut offset = header_size;
        for i in 0..tags_this_block {
            let (blocknr, _) = dirty_buffers[buf_idx + i];
            let is_last = (i == tags_this_block - 1) && (buf_idx + i == num_buffers - 1);
            let tag = journal_block_tag_t {
                t_blocknr: (blocknr as u32).to_be(),
                t_flags: if is_last { (JBD2_FLAG_LAST_TAG as u16).to_be() } else { 0 },
                ..Default::default()
            };
            // SAFETY: tag is a stack-local journal_block_tag_t; #[repr(C)] struct reinterpreted as bytes.
            let tag_bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(&tag as *const _ as *const u8, tag_size)
            };
            desc_data[offset..offset + tag_size].copy_from_slice(tag_bytes);
            offset += tag_size;
        }

        // Tail
        let tail = journal_block_tail_t::default();
        // SAFETY: tail is a stack-local journal_block_tail_t; #[repr(C)] struct reinterpreted as bytes.
        let tail_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(&tail as *const _ as *const u8, tail_size)
        };
        desc_data[block_size - tail_size..].copy_from_slice(tail_bytes);

        // SAFETY: bh is a valid BufferHead from bio::bread; b_data is block_size bytes;
        // copy_nonoverlapping sizes are bounds-checked by block_size.
        unsafe {
            let bh_ref = &mut *bh;
            core::ptr::copy_nonoverlapping(desc_data.as_ptr(), bh_ref.b_data.as_mut_ptr(), block_size);
            bh_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        }
        current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

        bio::sync_dirty_buffer(bh).map_err(|e| {
            journal.abort(e);
            *commit_transaction.t_state.lock() = TransactionState::Finished;
            e
        })?;
        bio::brelse(bh);

        // --- Write data blocks referenced by this descriptor ---
        for _ in 0..tags_this_block {
            let (blocknr, ref data) = dirty_buffers[buf_idx];
            buf_idx += 1;
            if data.is_empty() {
                // No data to write (create access), skip journal data block
                // but we still consumed a tag slot
                continue;
            }

            let data_abs_block = blk_offset + current_journal_block;
            // SAFETY: bio::bread returns a valid BufferHead or None (handled by ?).
            let data_bh = unsafe {
                match bio::bread(device, data_abs_block) {
                    Some(b) => b,
                    None => {
                        journal.abort(EIO);
                        *commit_transaction.t_state.lock() = TransactionState::Finished;
                        return Err(EIO);
                    }
                }
            };

            let copy_len = core::cmp::min(data.len(), block_size);
            // SAFETY: data_bh is a valid BufferHead; b_data is block_size bytes; copy_len is min of data/block size.
            unsafe {
                let data_bh_ref = &mut *data_bh;
                core::ptr::copy_nonoverlapping(data.as_ptr(), data_bh_ref.b_data.as_mut_ptr(), copy_len);
                for b in data_bh_ref.b_data.iter_mut().skip(copy_len) {
                    *b = 0;
                }
                data_bh_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            }
            current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

            bio::sync_dirty_buffer(data_bh).map_err(|e| {
                journal.abort(e);
                *commit_transaction.t_state.lock() = TransactionState::Finished;
                e
            })?;
            bio::brelse(data_bh);
        }
    }

    // Phase 2.5: data=ordered flush
    // Each metadata buffer was already synced individually via
    // sync_dirty_buffer during the operation (write_block_from_vec,
    // write_inode_disk, etc.) and again during Phase 2 journal writes.
    // A full sync_buffers scan is redundant and causes SMP contention
    // on the block cache bucket spinlocks.

    // Phase 3: Write commit block
    {
        let mut state = commit_transaction.t_state.lock();
        *state = TransactionState::Commit;
    }

    let commit_abs_block = blk_offset + current_journal_block;
    // SAFETY: bio::bread returns a valid BufferHead or None (handled by ?).
    let commit_bh = unsafe {
        match bio::bread(device, commit_abs_block) {
            Some(b) => b,
            None => {
                journal.abort(EIO);
                *commit_transaction.t_state.lock() = TransactionState::Finished;
                return Err(EIO);
            }
        }
    };

    let commit_hdr = commit_header {
        h_magic: JBD2_MAGIC_NUMBER.to_be(),
        h_blocktype: JBD2_COMMIT_BLOCK.to_be(),
        h_sequence: tid.to_be(),
        ..Default::default()
    };
    // SAFETY: commit_hdr is a stack-local commit_header; #[repr(C)] struct reinterpreted as bytes.
    let commit_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(&commit_hdr as *const _ as *const u8, size_of::<commit_header>())
    };
    // SAFETY: commit_bh is a valid BufferHead; b_data is block_size bytes;
    // commit_bytes.len() fits within a block.
    unsafe {
        let commit_ref = &mut *commit_bh;
        for b in commit_ref.b_data.iter_mut() { *b = 0; }
        core::ptr::copy_nonoverlapping(commit_bytes.as_ptr(), commit_ref.b_data.as_mut_ptr(), commit_bytes.len());
        commit_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }
    current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

    bio::sync_dirty_buffer(commit_bh).map_err(|e| {
        journal.abort(e);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        e
    })?;
    bio::brelse(commit_bh);

    // Phase 4: Update journal state
    journal.j_head.store(current_journal_block, core::sync::atomic::Ordering::SeqCst);
    journal.j_free.fetch_sub(total_journal_blocks as u64, core::sync::atomic::Ordering::SeqCst);
    journal.j_commit_sequence.store(tid, core::sync::atomic::Ordering::SeqCst);
    journal.j_tail_sequence.store(tid, core::sync::atomic::Ordering::SeqCst);

    // Write journal superblock
    write_journal_superblock(journal, device, blk_offset, current_journal_block, tid + 1)?;

    *commit_transaction.t_state.lock() = TransactionState::Finished;
    Ok(())
}

/// Wrap journal block number within [first, last)
#[inline]
fn wrap_journal_block(mut block: u64, first: u64, last: u64) -> u64 {
    if first >= last {
        return first; // degenerate journal, cannot advance
    }
    block += 1;
    if block >= last {
        block = first;
    }
    block
}

/// Write journal superblock with updated s_start and s_sequence
fn write_journal_superblock(
    journal: &Journal,
    device: *const crate::drivers::blkdev::GenDisk,
    blk_offset: u64,
    new_start: u64,
    new_sequence: u32,
) -> Result<(), i32> {
    // SAFETY: bio::bread returns a valid BufferHead or None (handled).
    let bh = unsafe {
        match bio::bread(device, blk_offset) {
            Some(b) => b,
            None => return Err(EIO),
        }
    };

    // SAFETY: bh is a valid BufferHead; the superblock fits within a block;
    // bh_ref.b_data contains the on-disk journal_superblock_t.
    unsafe {
        let bh_ref = &mut *bh;
        let sb = &mut *(bh_ref.b_data.as_mut_ptr() as *mut journal_superblock_t);
        sb.s_start = (new_start as u32).to_be();
        sb.s_sequence = new_sequence.to_be();
        bh_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
    }

    bio::sync_dirty_buffer(bh)?;
    bio::brelse(bh);
    Ok(())
}
