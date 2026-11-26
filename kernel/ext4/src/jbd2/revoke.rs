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

// (INV-JBD2-BH-1: see recovery.rs — the revoke-record path rides the
// same bh-pinned discipline.)
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

/// Take (drain) the pending revoke records of a transaction for commit
/// writing. Arm-2: only the COMMITTING transaction's own set is written
/// (plus what it inherited from the spill at creation); the journal-global
/// vec is never drained here — it holds recovery state or the next
/// transaction's spill.
pub fn take_revoke_records(commit_transaction: &Arc<Transaction>) -> alloc::vec::Vec<(u64, Tid)> {
    core::mem::take(&mut *commit_transaction.t_revoke_records.lock())
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
    //
    // Arm-2 routing (f4-ext4-route-map.md §6.2 R1): the record goes to
    // the ISSUING transaction's own set while that transaction is still
    // Running — so it is logged by that transaction's commit and forces
    // nothing. If the transaction is no longer Running (an SMP handle
    // that joined a committing transaction — start_this_handle does not
    // check t_state — or a stale handle on a Finished one), the record
    // spills to the journal-level vec and is claimed by the next
    // transaction at creation. The spill never feeds the stop condition;
    // it only rides forward.
    let state_ok = matches!(
        *txn.t_state.lock(),
        super::journal::TransactionState::Running
    );
    if state_ok {
        let mut set = txn.t_revoke_records.lock();
        if !set.iter().any(|&(b, t)| b == blocknr && t == txn.t_tid) {
            set.push((blocknr, txn.t_tid));
        }
    } else {
        insert_revoke_hash(&journal, blocknr, txn.t_tid)?;
    }
    crate::debug::fsw::note_j_revoke();

    Ok(())
}

/// Cancel an outstanding revoke
///
/// Called from jbd2_journal_get_write_access when a buffer is being
/// modified again in the same transaction.
///
/// Arm-2 decision (f4-ext4-route-map.md §6.2 R2): this stays a NO-OP,
/// deliberately. Linux needs revoke-cancel records because a revoke can
/// land in the log AFTER a newer descriptor for the same block,
/// suppressing the replay of the new content. That direction cannot
/// arise here: revokes are stamped with their issuing transaction's tid
/// and logged by that transaction (or a later one, via the spill), so a
/// revoke's tid is never greater than the tid of a transaction whose
/// descriptors carry post-revoke content for the block — the recovery
/// filter (`revoke.tid >= desc_tid`) does not suppress it. Same-tid
/// suppression (reallocation inside the issuing transaction) skips
/// replay of the block's own commit-time snapshot, which is redundant:
/// the commit's write-through pass has already persisted the live
/// content at the block's final location. See design-divergences.md
/// D-8 for the recorded divergence.
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
    use rux::fs::hotpath;

    let Some(device) = journal.j_bio_device else {
        return Ok(0);
    };

    let records = take_revoke_records(commit_transaction);
    if records.is_empty() {
        return Ok(0);
    }

    let block_size = journal.j_blocksize as usize;
    let entry_size = journal.revoke_entry_size();
    let header_len = core::mem::size_of::<journal_header_t>() + core::mem::size_of::<u32>();
    // CSUM_V2/V3 journals reserve the 4-byte block tail at the end of
    // every descriptor-format block (revoke blocks included) — the entry
    // capacity must not write into it (reference
    // journal_revoke_records_per_block subtracts descr_csum_size).
    let tail_len = if journal.has_csum_v2or3() {
        core::mem::size_of::<journal_block_tail_t>()
    } else {
        0
    };
    let per_block = if entry_size > 0 {
        (block_size.saturating_sub(tail_len) - header_len) / entry_size
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
        // ext4-persist P0 fix: resolve through the extent map (jmap);
        // 0 = unmapped hole — refuse rather than write onto foreign
        // metadata (the multi-extent journal layout).
        let abs_block = journal.jmap(current_journal_block);
        if abs_block == 0 {
            return Err(EIO);
        }

        let bh = match hotpath::bread(device, abs_block) {
            Some(b) => b,
            None => return Err(EIO),
        };

        let take = core::cmp::min(per_block, records.len() - idx);
        // Writes stay inside b_data (views ride the audited bio edges;
        // the struct->bytes copy rides safety::bytes).
        {
            let block = hotpath::bh_data_mut(&bh);
            for b in block.iter_mut() {
                *b = 0;
            }
            let hdr = journal_revoke_header_t {
                r_header: journal_header_t::new(JBD2_REVOKE_BLOCK, tid),
                r_count: 0,
            };
            let hdr_bytes = crate::safety::bytes::bytes_of(&hdr);
            block[..header_len.min(hdr_bytes.len())]
                .copy_from_slice(&hdr_bytes[..header_len.min(hdr_bytes.len())]);
            let mut off = header_len;
            for &(blocknr, _rtid) in &records[idx..idx + take] {
                if entry_size == 8 {
                    block[off..off + 8].copy_from_slice(&blocknr.to_be_bytes());
                } else {
                    block[off..off + 4]
                        .copy_from_slice(&(blocknr as u32).to_be_bytes());
                }
                off += entry_size;
            }
            // r_count: bytes used in this block (header + entries)
            let rc_off = core::mem::size_of::<journal_header_t>();
            block[rc_off..rc_off + 4].copy_from_slice(&(off as u32).to_be_bytes());
            // CSUM_V2/V3 tail (reference jbd2_descriptor_block_csum_set —
            // revoke blocks use the descriptor tail): crc32c(seed, whole
            // block with the tail zeroed), stored in the tail.
            if journal.has_csum_v2or3() {
                let n = block.len();
                block[n - tail_len..].fill(0);
                let bytes: &[u8] = &block[..n];
                let csum = crate::ext4::csum::crc32c(journal.csum_seed(), bytes);
                block[n - tail_len..].copy_from_slice(&csum.to_be_bytes());
            }
            hotpath::bh_set_state_bit(&bh, rux::fs::types::BufferState::BH_Dirty);
        }
        current_journal_block += 1;
        if current_journal_block >= journal.j_last {
            current_journal_block = journal.j_first;
        }
        journal.j_head.store(current_journal_block, core::sync::atomic::Ordering::SeqCst);

        let sync_res = hotpath::sync_dirty_buffer(bh);
        hotpath::brelse(bh);
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
    let rc_off = core::mem::size_of::<journal_header_t>();
    let r_count = u32::from_be_bytes(
        data[rc_off..rc_off + 4].try_into().unwrap(),
    ) as usize;
    let used = r_count.min(data.len());
    let mut off = header_len;
    while off + entry_size <= used {
        let blocknr = if entry_size == 8 {
            u64::from_be_bytes(data[off..off + 8].try_into().unwrap())
        } else {
            u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as u64
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
    // `j_superblock` is null-checked; when non-null it points to a
    // valid, initialized `journal_superblock_t` (set during mount) —
    // the view rides the framework's boxed_ref edge.
    if journal.j_superblock.is_null() {
        return false;
    }
    let sb: &journal_superblock_t = crate::syscall::r#unsafe::boxed_ref(journal.j_superblock);
    u32::from_be(sb.s_feature_incompat) & JBD2_FEATURE_INCOMPAT_REVOKE != 0
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

    // Parse a revoke block and add records to the hash table. bh is a
    // valid BufferHead from the recovery path; the byte view rides the
    // framework's kernel_slice edge.
    {
        let journal_block_size = journal.j_blocksize as usize;
        let bh_ref: &BufferHead = crate::syscall::r#unsafe::boxed_ref(bh);
        let data = rux::fs::r#unsafe::kernel_slice(
            bh_ref.b_data as *const u8,
            journal_block_size,
        );
        scan_revoke_block(journal, data, tid);
    }
    Ok(0)
}
