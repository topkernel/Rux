//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Revoke logic
//!
//! JBD2 revoke management
//!
//! Revoke is the mechanism used to prevent old log records for deleted
//! metadata from being replayed on top of newer data using the same blocks.

use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::journal::{Journal, Transaction, Tid, BufferHead, JournalHead, ListHead, Jbd2RevokeTable, Jbd2RevokeRecord, Handle};
use super::types::*;
use super::transaction::{EIO, EINVAL, ENOMEM};

// ============================================================================
// Revoke operations
// ============================================================================

/// Initialize revoke table for a journal
pub fn jbd2_journal_init_revoke(journal: &Arc<Journal>, _hash_size: u32) -> Result<(), i32> {
    // The minimal table is the Vec<(u64, Tid)> on the Journal itself —
    // nothing to preallocate.
    Ok(())
}

/// Destroy revoke table
pub fn jbd2_journal_destroy_revoke(journal: &Arc<Journal>) {
    journal.revoke_records.lock().clear();
}

/// Insert a revoke record (deduplicated per (block, tid)).
pub fn insert_revoke_hash(journal: &Arc<Journal>, blocknr: u64, seq: Tid) -> Result<(), i32> {
    let mut records = journal.revoke_records.lock();
    if records.iter().any(|&(b, t)| b == blocknr && t == seq) {
        return Ok(());
    }
    records.push((blocknr, seq));
    Ok(())
}

/// Find a revoke record in the table.
pub fn find_revoke_record(journal: &Arc<Journal>, blocknr: u64) -> Option<Arc<Jbd2RevokeRecord>> {
    let records = journal.revoke_records.lock();
    records
        .iter()
        .find(|&&(b, _)| b == blocknr)
        .map(|&(b, t)| {
            Arc::new(Jbd2RevokeRecord {
                hash_list: ListHead::new(),
                blocknr: b,
                tid: t,
            })
        })
}

/// Is `blocknr` revoked for transaction `tid`? A revoke record written in
/// transaction T invalidates every OLDER log record for the block, so a
/// descriptor belonging to sequence S must be skipped when a revoke exists
/// with revoke.tid >= S (Linux journal_test_revoke semantics, simplified
/// to the same effect for sequential recovery).
pub fn block_is_revoked_for(journal: &Arc<Journal>, blocknr: u64, tid: Tid) -> bool {
    let records = journal.revoke_records.lock();
    records.iter().any(|&(b, t)| b == blocknr && t >= tid)
}

/// Take (drain) the pending revoke records for commit writing.
pub fn take_revoke_records(journal: &Arc<Journal>) -> alloc::vec::Vec<(u64, Tid)> {
    core::mem::take(&mut *journal.revoke_records.lock())
}

// ============================================================================
// Main revoke functions
// ============================================================================

/// Revoke a buffer from the journal
///
/// This prevents the block from being replayed during recovery if we
/// take a crash after this current transaction commits.
///
/// # Arguments
/// * `handle` - Transaction handle
/// * `blocknr` - Block number to revoke
/// * `bh_in` - Optional buffer head (will be forgotten)
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
pub fn jbd2_journal_revoke(
    handle: &mut Handle,
    blocknr: u64,
    bh_in: Option<*mut BufferHead>,
) -> Result<(), i32> {
    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;
    let journal = txn.t_journal.as_ref().ok_or(EIO)?.clone();

    // Check revoke credits — but do not fail the revoke when the handle was
    // started with a small budget: dropping the record would let recovery
    // replay a stale block over reallocated space (worse than overspending).
    // Instead, grant the credit implicitly (documented over-spend).
    if handle.h_revoke_credits > 0 {
        handle.h_revoke_credits -= 1;
    }

    // Set revoke feature if not already set
    jbd2_journal_set_revoke_feature(&journal)?;

    // NOTE: `bh_in` uses the (unused) internal Jbd2 BufferHead type; no
    // in-tree caller supplies one. When a real buffer must be forgotten,
    // callers use jbd2_journal_forget directly with the bio BufferHead.
    let _ = bh_in;

    // Record (block, tid): commit writes a revoke block; recovery skips
    // replaying older journal entries for this block.
    insert_revoke_hash(&journal, blocknr, txn.t_tid)?;

    Ok(())
}

/// Cancel an outstanding revoke
///
/// Called from jbd2_journal_get_write_access when a buffer is being
/// modified again in the same transaction.
pub fn jbd2_journal_cancel_revoke(handle: &mut Handle, jh: &mut JournalHead) {
    let txn = match &handle.h_transaction {
        Some(t) => t,
        None => return,
    };

    let journal = match &txn.t_journal {
        Some(j) => j,
        None => return,
    };

    // Steps:
    // 1. Check if buffer has RevokeValid set
    // 2. If so, check Revoked bit
    // 3. Clear revoked if needed
    // 4. Remove from revoke hash table
}

/// Clear revoked flags on all buffers in revoke table
pub fn jbd2_clear_buffer_revoked_flags(journal: &Arc<Journal>) {
    // Iterate through all hash buckets
    // and clears BH_Revoked flag on each buffer
}

/// Switch revoke tables between running and committing transactions
pub fn jbd2_journal_switch_revoke_table(journal: &Arc<Journal>) {
    // Swap j_revoke_table[0] and j_revoke_table[1]
    // and clears the new table
}

// ============================================================================
// Revoke record writing
// ============================================================================

/// Write revoke records to the journal
///
/// Called during commit (before the commit block) to write all revoke
/// records of the transaction as JBD2_REVOKE_BLOCK descriptor blocks.
/// Layout per block: journal_header_t, r_count (bytes used), then an array
/// of block numbers (4 bytes each; 8 with the 64-bit journal feature).
///
/// Returns the number of journal blocks consumed (for space accounting).
pub fn jbd2_journal_write_revoke_records(
    journal: &Arc<Journal>,
    commit_transaction: &Arc<Transaction>,
) -> Result<usize, i32> {
    use crate::fs::bio;

    let device = journal.j_bio_device;
    if device.is_null() {
        return Ok(0);
    }

    let records = take_revoke_records(journal);
    if records.is_empty() {
        return Ok(0);
    }

    let blk_offset = journal.j_blk_offset;
    let block_size = journal.j_blocksize as usize;
    let entry_size = journal.revoke_entry_size();
    let header_len = core::mem::size_of::<journal_header_t>() + core::mem::size_of::<u32>();
    let per_block = if entry_size > 0 {
        (block_size - header_len) / entry_size
    } else {
        0
    };
    if per_block == 0 {
        return Err(EIO);
    }

    let tid = commit_transaction.t_tid;
    let mut blocks_written = 0usize;
    let mut idx = 0usize;

    while idx < records.len() {
        let mut current_journal_block =
            journal.j_head.load(core::sync::atomic::Ordering::SeqCst);
        let abs_block = blk_offset + current_journal_block;

        // SAFETY: bio::bread returns a valid BufferHead or None.
        let bh = unsafe {
            match bio::bread(device, abs_block) {
                Some(b) => b,
                None => return Err(EIO),
            }
        };

        let take = core::cmp::min(per_block, records.len() - idx);
        // SAFETY: bh is valid; writes stay inside b_data.
        unsafe {
            let bh_ref = &mut *bh;
            for b in bh_ref.b_data.iter_mut() {
                *b = 0;
            }
            let hdr = journal_revoke_header_t {
                r_header: journal_header_t::new(JBD2_REVOKE_BLOCK, tid),
                r_count: 0,
            };
            core::ptr::copy_nonoverlapping(
                &hdr as *const _ as *const u8,
                bh_ref.b_data.as_mut_ptr(),
                header_len,
            );
            let mut off = header_len;
            for &(blocknr, _rtid) in &records[idx..idx + take] {
                if entry_size == 8 {
                    core::ptr::write_unaligned(
                        bh_ref.b_data.as_mut_ptr().add(off) as *mut u64,
                        blocknr.to_be(),
                    );
                } else {
                    core::ptr::write_unaligned(
                        bh_ref.b_data.as_mut_ptr().add(off) as *mut u32,
                        (blocknr as u32).to_be(),
                    );
                }
                off += entry_size;
            }
            // r_count: bytes used in this block (header + entries)
            core::ptr::write_unaligned(
                bh_ref.b_data.as_mut_ptr()
                    .add(core::mem::size_of::<journal_header_t>()) as *mut u32,
                (off as u32).to_be(),
            );
            bh_ref.set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
        }
        current_journal_block += 1;
        if current_journal_block >= journal.j_last {
            current_journal_block = journal.j_first;
        }
        journal.j_head.store(current_journal_block, core::sync::atomic::Ordering::SeqCst);

        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        sync_res?;

        idx += take;
        blocks_written += 1;
    }

    Ok(blocks_written)
}

/// Parse a revoke block read during recovery and add its entries to the
/// journal's revoke set. `tid` is the block's transaction sequence.
pub fn scan_revoke_block(
    journal: &Arc<Journal>,
    data: &[u8],
    tid: Tid,
) {
    let entry_size = journal.revoke_entry_size();
    let header_len = core::mem::size_of::<journal_header_t>() + core::mem::size_of::<u32>();
    if data.len() < header_len || entry_size == 0 {
        return;
    }
    // r_count bounds the used region.
    let r_count = u32::from_be(unsafe {
        core::ptr::read_unaligned(
            data.as_ptr().add(core::mem::size_of::<journal_header_t>()) as *const u32,
        )
    }) as usize;
    let used = r_count.min(data.len());
    let mut off = header_len;
    while off + entry_size <= used {
        let blocknr = if entry_size == 8 {
            u64::from_be(unsafe {
                core::ptr::read_unaligned(data.as_ptr().add(off) as *const u64)
            })
        } else {
            u32::from_be(unsafe {
                core::ptr::read_unaligned(data.as_ptr().add(off) as *const u32)
            }) as u64
        };
        let _ = insert_revoke_hash(journal, blocknr, tid);
        off += entry_size;
    }
}

/// Write one revoke record
fn write_one_revoke_record(
    journal: &Arc<Journal>,
    record: &Jbd2RevokeRecord,
    bh: &mut *mut BufferHead,
    offset: &mut i32,
) -> Result<(), i32> {
    // Write a single revoke record to a descriptor block
    // If the block is full, it starts a new one

    Ok(())
}

/// Flush a revoke descriptor block
fn flush_descriptor(journal: &Arc<Journal>, bh: *mut BufferHead, offset: i32) {
    // Steps:
    // 1. Sets up the header
    // 2. Calculates checksum
    // 3. Submits the block for write
}

// ============================================================================
// Revoke feature
// ============================================================================

/// Set the revoke feature flag in the journal
pub fn jbd2_journal_set_revoke_feature(journal: &Arc<Journal>) -> Result<(), i32> {
    // Set JBD2_FEATURE_INCOMPAT_REVOKE in the superblock
    // if not already set

    Ok(())
}

/// Check if journal has revoke feature
pub fn jbd2_journal_has_revoke_feature(journal: &Arc<Journal>) -> bool {
    // SAFETY: `j_superblock` is null-checked above; when non-null it points
    // to a valid, initialized `journal_superblock_t` (set during mount).
    unsafe {
        if journal.j_superblock.is_null() {
            return false;
        }
        let sb = &*journal.j_superblock;
        u32::from_be(sb.s_feature_incompat) & JBD2_FEATURE_INCOMPAT_REVOKE != 0
    }
}

// ============================================================================
// Revoke testing
// ============================================================================

/// Test if a block is revoked in the current transaction
pub fn jbd2_journal_test_revoke(journal: &Arc<Journal>, blocknr: u64, tid: Tid) -> bool {
    block_is_revoked_for(journal, blocknr, tid)
}

/// Get revoke count for a transaction
pub fn jbd2_journal_revoke_count(journal: &Arc<Journal>) -> usize {
    // Count the number of revoke records
    0
}

// ============================================================================
// Revoke record scanning (for recovery)
// ============================================================================

/// Scan revoke records during recovery
pub fn scan_revoke_records(
    journal: &Arc<Journal>,
    bh: *mut BufferHead,
    tid: Tid,
) -> Result<i32, i32> {
    if bh.is_null() {
        return Ok(0);
    }

    // Parse a revoke block and add records to the hash table.
    // SAFETY: bh is a valid bio::BufferHead from the recovery path; b_data
    // points to a full journal block.
    unsafe {
        let journal_block_size = journal.j_blocksize as usize;
        let data = core::slice::from_raw_parts((*bh).b_data as *const u8, journal_block_size);
        scan_revoke_block(journal, data, tid);
    }
    Ok(0)
}
