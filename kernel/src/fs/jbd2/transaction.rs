//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Transaction management
//!
//! JBD2 transaction management

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::journal::{Journal, Handle, Transaction, TransactionState, Tid};
use super::types::*;

// ============================================================================
// Error codes
// ============================================================================

pub const EROFS: i32 = 30;   // Read-only filesystem
pub const ENOMEM: i32 = 12;  // Out of memory
pub const EIO: i32 = 5;      // I/O error
pub const EINVAL: i32 = 22;  // Invalid argument

// ============================================================================
// Transaction helper functions
// ============================================================================

/// Check if handle is aborted
#[inline]
pub fn is_handle_aborted(handle: &Handle) -> bool {
    handle.h_aborted ||
    handle.h_transaction.as_ref().map_or(true, |txn| {
        txn.t_journal.as_ref().map_or(true, |j| j.is_aborted())
    })
}

/// Get current handle from journal
pub fn journal_current_handle(journal: &Arc<Journal>) -> Option<Arc<Handle>> {
    // Get the handle from current task
    // For now, return None as we don't have per-task storage
    None
}

// ============================================================================
// Transaction creation
// ============================================================================

/// Create a new transaction for the journal
pub fn jbd2_get_transaction(journal: &Arc<Journal>) -> Transaction {
    let tid = journal.j_transaction_sequence.fetch_add(1, Ordering::SeqCst);

    Transaction {
        t_journal: Some(journal.clone()),
        t_tid: tid,
        t_state: Spinlock::new(TransactionState::Running),
        t_log_start: 0,
        t_nr_buffers: 0,
        t_reserved_list: core::ptr::null_mut(),
        t_buffers: core::ptr::null_mut(),
        t_forget: core::ptr::null_mut(),
        t_checkpoint_list: core::ptr::null_mut(),
        t_shadow_list: core::ptr::null_mut(),
        t_inode_list: super::journal::ListHead::new(),
        t_max_wait: 0,
        t_start: 0, // get_jiffies()
        t_requested: 0,
        t_chp_stats: super::journal::TransactionChpStats::default(),
        t_updates: AtomicI32::new(0),
        t_outstanding_credits: AtomicI32::new(
            journal.j_transaction_overhead_buffers +
            journal.j_reserved_credits.load(Ordering::SeqCst)
        ),
        t_outstanding_revokes: AtomicI32::new(0),
        t_handle_count: AtomicI32::new(0),
        t_cpnext: None,
        t_cpprev: None,
        t_expires: 0, // jiffies + journal.j_commit_interval
        t_start_time: 0,
        t_synchronous_commit: false,
        t_need_data_flush: 0,
        t_dirty_buffers: Spinlock::new(alloc::vec::Vec::new()),
    }
}

// ============================================================================
// Handle allocation
// ============================================================================

/// Allocate a new handle
pub fn new_handle(nblocks: i32) -> Handle {
    Handle {
        h_total_credits: nblocks,
        h_ref: 1,
        ..Handle::default()
    }
}

/// Start a handle on a journal
pub fn start_this_handle(journal: &Arc<Journal>, handle: &mut Handle) -> Result<(), i32> {
    // Get or create running transaction
    let mut running_txn = journal.j_running_transaction.lock();

    // If no running transaction, create one
    if running_txn.is_none() {
        let txn = jbd2_get_transaction(journal);
        *running_txn = Some(Arc::new(txn));
    }

    // Get the transaction
    let txn = running_txn.as_ref().ok_or(ENOMEM)?;

    // Update handle
    handle.h_transaction = Some(txn.clone());
    handle.h_requested_credits = handle.h_total_credits as u32;
    handle.h_revoke_credits_requested = handle.h_revoke_credits;
    // handle.h_start_jiffies = get_jiffies();

    // Increment transaction counters
    txn.t_updates.fetch_add(1, Ordering::SeqCst);
    txn.t_handle_count.fetch_add(1, Ordering::SeqCst);

    Ok(())
}

// ============================================================================
// Journal start/stop operations
// ============================================================================

/// Start a new transaction handle
///
/// This is the main entry point for starting a journal transaction.
/// The handle represents a single atomic update to the filesystem.
pub fn jbd2_journal_start(journal: &Arc<Journal>, nblocks: i32) -> Result<Handle, i32> {
    // Check if journal exists
    if journal.is_aborted() {
        return Err(EROFS);
    }

    // Check for nested handle
    // Check current journal info

    // Create new handle
    let mut handle = new_handle(nblocks);

    // Start the handle
    start_this_handle(journal, &mut handle)?;

    Ok(handle)
}

/// Start a handle with extended options
pub fn jbd2__journal_start(
    journal: &Arc<Journal>,
    nblocks: i32,
    rsv_blocks: i32,
    revoke_records: i32,
) -> Result<Handle, i32> {
    // Adjust nblocks for revoke records
    let adjusted_nblocks = nblocks +
        (revoke_records + journal.j_revoke_records_per_block - 1) /
        journal.j_revoke_records_per_block;

    let mut handle = new_handle(adjusted_nblocks);
    handle.h_revoke_credits = revoke_records;

    // Create reserved handle if requested
    if rsv_blocks > 0 {
        // For now, we skip the reserved handle implementation
        // Create a second handle with h_reserved = 1
    }

    start_this_handle(journal, &mut handle)?;

    Ok(handle)
}

/// Stop a transaction handle
///
/// This completes a transaction handle and returns any remaining
/// buffer credits to the transaction. On single-core, we always
/// commit synchronously when the last handle stops.
pub fn jbd2_journal_stop(handle: &mut Handle) -> Result<(), i32> {
    // Decrement reference count
    if handle.h_ref > 1 {
        handle.h_ref -= 1;
        if is_handle_aborted(handle) {
            return Err(EIO);
        }
        return Ok(());
    }

    // Get transaction and journal
    let txn = match &handle.h_transaction {
        Some(t) => t.clone(),
        None => return Ok(()),
    };
    let journal = match &txn.t_journal {
        Some(j) => j.clone(),
        None => {
            handle.h_transaction = None;
            return Ok(());
        }
    };

    // Check for abort
    let mut err = 0;
    if is_handle_aborted(handle) {
        err = EIO;
    }

    // Decrement transaction updates
    txn.t_updates.fetch_sub(1, Ordering::SeqCst);

    // On single-core: always commit synchronously when last handle stops
    // Spin-wait for any other handles (shouldn't happen on single-core)
    while txn.t_updates.load(Ordering::SeqCst) > 0 {
        core::hint::spin_loop();
    }

    // Commit the transaction
    if err == 0 {
        err = match super::commit::jbd2_journal_commit_transaction(&journal, &txn) {
            Ok(()) => 0,
            Err(e) => e,
        };
    }

    // Clear running transaction
    {
        let mut running = journal.j_running_transaction.lock();
        // Only clear if it's our transaction
        if let Some(ref r) = *running {
            if r.t_tid == txn.t_tid {
                *running = None;
            }
        }
    }

    // Clear transaction reference
    handle.h_transaction = None;

    if err != 0 {
        Err(err)
    } else {
        Ok(())
    }
}

/// Extend a handle's credits
pub fn jbd2_journal_extend(handle: &mut Handle, nblocks: i32) -> Result<(), i32> {
    let txn = handle.h_transaction.as_ref().ok_or(EROFS)?;

    // Check if we can extend
    let current = txn.t_outstanding_credits.load(Ordering::SeqCst);
    let journal = txn.t_journal.as_ref().ok_or(EROFS)?;

    // Check journal space
    let max_bufs = journal.j_max_transaction_buffers;
    if current + nblocks > max_bufs {
        return Err(ENOMEM);
    }

    // Add credits
    txn.t_outstanding_credits.fetch_add(nblocks, Ordering::SeqCst);
    handle.h_total_credits += nblocks;

    Ok(())
}

// ============================================================================
// Buffer operations (bio::BufferHead based)
// ============================================================================

/// Get write access to a bio buffer
///
/// This must be called before modifying a buffer that will be journaled.
/// It freezes a copy of the buffer's current data for the journal.
pub fn jbd2_journal_get_write_access(handle: &mut Handle, bio_bh: *mut crate::fs::bio::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;

    unsafe {
        let blocknr = (*bio_bh).b_blocknr;
        let data = (*bio_bh).b_data.clone();

        let mut dirty_bufs = txn.t_dirty_buffers.lock();
        // Check if already tracked
        for (existing_nr, _) in dirty_bufs.iter() {
            if *existing_nr == blocknr {
                return Ok(()); // Already tracked
            }
        }
        dirty_bufs.push((blocknr, data));
    }

    Ok(())
}

/// Get create access to a buffer (for newly allocated blocks)
///
/// Similar to get_write_access but the buffer doesn't have meaningful
/// prior content, so we don't need to preserve old data.
pub fn jbd2_journal_get_create_access(handle: &mut Handle, bio_bh: *mut crate::fs::bio::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;

    unsafe {
        let blocknr = (*bio_bh).b_blocknr;
        let mut dirty_bufs = txn.t_dirty_buffers.lock();
        for (existing_nr, _) in dirty_bufs.iter() {
            if *existing_nr == blocknr {
                return Ok(()); // Already tracked
            }
        }
        // For create access, we store the new data (will be updated by dirty_metadata)
        dirty_bufs.push((blocknr, alloc::vec::Vec::new()));
    }

    Ok(())
}

/// Mark a buffer as dirty metadata
///
/// This updates the frozen copy of the buffer data to the current state.
/// The journal will write this data during commit.
pub fn jbd2_journal_dirty_metadata(handle: &mut Handle, bio_bh: *mut crate::fs::bio::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;

    unsafe {
        let blocknr = (*bio_bh).b_blocknr;
        let data = (*bio_bh).b_data.clone();

        let mut dirty_bufs = txn.t_dirty_buffers.lock();
        for (existing_nr, existing_data) in dirty_bufs.iter_mut() {
            if *existing_nr == blocknr {
                *existing_data = data;
                return Ok(());
            }
        }
        // Not previously registered via get_write_access — register now
        dirty_bufs.push((blocknr, data));
    }

    Ok(())
}

/// Forget a buffer (release it from the transaction)
pub fn jbd2_journal_forget(handle: &mut Handle, bio_bh: *mut crate::fs::bio::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;

    unsafe {
        let blocknr = (*bio_bh).b_blocknr;
        let mut dirty_bufs = txn.t_dirty_buffers.lock();
        dirty_bufs.retain(|(nr, _)| *nr != blocknr);
    }

    Ok(())
}

// ============================================================================
// Reserved handle operations
// ============================================================================

/// Free a reserved handle
pub fn jbd2_journal_free_reserved(handle: &mut Handle) {
    if !handle.h_reserved {
        return;
    }

    // Return reserved credits to journal
    if let Some(ref journal) = handle.h_journal {
        journal.j_reserved_credits.fetch_sub(
            handle.h_total_credits,
            Ordering::SeqCst
        );
    }
}

/// Start a previously reserved handle
pub fn jbd2_journal_start_reserved(handle: &mut Handle) -> Result<(), i32> {
    if !handle.h_reserved {
        // Not a reserved handle
        jbd2_journal_stop(handle)?;
        return Err(EIO);
    }

    handle.h_reserved = false;

    // Start the handle - clone journal first to avoid borrow issues
    let journal_clone = handle.h_journal.clone();
    if let Some(journal) = journal_clone {
        start_this_handle(&journal, handle)?;
    }

    Ok(())
}

// ============================================================================
// Credit management
// ============================================================================

/// Add credits to a running transaction
pub fn add_transaction_credits(journal: &Arc<Journal>, blocks: i32, rsv_blocks: i32) -> bool {
    // Check if we need to wait
    let running = journal.j_running_transaction.lock();
    if running.is_none() {
        return false;
    }

    let txn = running.as_ref().unwrap();
    let current_credits = txn.t_outstanding_credits.load(Ordering::SeqCst);
    let max_bufs = journal.j_max_transaction_buffers;

    // Check if transaction is too large
    if current_credits + blocks > max_bufs {
        // Need to start commit
        return true;
    }

    // Check journal space
    let free_space = journal.j_free.load(Ordering::SeqCst) as i32;
    if blocks + rsv_blocks > free_space {
        // Need to wait for checkpoint
        return true;
    }

    // Add credits
    txn.t_outstanding_credits.fetch_add(blocks, Ordering::SeqCst);
    journal.j_free.fetch_sub(blocks as u64, Ordering::SeqCst);

    false
}

/// Subtract reserved credits
pub fn sub_reserved_credits(journal: &Arc<Journal>, blocks: i32) {
    journal.j_reserved_credits.fetch_sub(blocks, Ordering::SeqCst);
}

// ============================================================================
// Transaction state management
// ============================================================================

/// Check if transaction can be closed
pub fn jbd2_transaction_can_close(journal: &Arc<Journal>) -> bool {
    let running = journal.j_running_transaction.lock();
    if let Some(ref txn) = *running {
        let updates = txn.t_updates.load(Ordering::SeqCst);
        return updates == 0;
    }
    false
}

/// Close a transaction (transition to commit state)
pub fn jbd2_close_transaction(journal: &Arc<Journal>) -> Result<(), i32> {
    let mut running = journal.j_running_transaction.lock();

    let txn = running.as_ref().ok_or(EIO)?;

    // Check no updates pending
    if txn.t_updates.load(Ordering::SeqCst) > 0 {
        return Err(EINVAL);
    }

    // Move to committing state
    *txn.t_state.lock() = TransactionState::Locked;

    // Also:
    // - Set up commit timer
    // - Wake up commit thread

    Ok(())
}
