//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Transaction management
//!
//! Based on Linux kernel fs/jbd2/transaction.c

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
    // In Linux, this gets the handle from current->journal_info
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
        t_state: spin::Mutex::new(TransactionState::Running),
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
    // In Linux, this would check current->journal_info

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
        // In Linux, this creates a second handle with h_reserved = 1
    }

    start_this_handle(journal, &mut handle)?;

    Ok(handle)
}

/// Stop a transaction handle
///
/// This completes a transaction handle and returns any remaining
/// buffer credits to the transaction.
pub fn jbd2_journal_stop(handle: &mut Handle) -> Result<(), i32> {
    // Decrement reference count
    if handle.h_ref > 1 {
        handle.h_ref -= 1;
        if is_handle_aborted(handle) {
            return Err(EIO);
        }
        return Ok(());
    }

    // Get transaction
    let txn = match &handle.h_transaction {
        Some(t) => t.clone(),
        None => return Ok(()),
    };

    // Check for abort
    let mut err = 0;
    if is_handle_aborted(handle) {
        err = EIO;
    }

    // Mark synchronous commit if needed
    if handle.h_sync {
        // txn.t_synchronous_commit = true;
    }

    // Decrement transaction updates
    txn.t_updates.fetch_sub(1, Ordering::SeqCst);

    // If sync or transaction expired, start commit
    // In Linux, this would call jbd2_log_start_commit()

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
        return Err(ENOMEM); // ENOSPC in Linux
    }

    // Add credits
    txn.t_outstanding_credits.fetch_add(nblocks, Ordering::SeqCst);
    handle.h_total_credits += nblocks;

    Ok(())
}

// ============================================================================
// Buffer operations
// ============================================================================

/// Get write access to a buffer
///
/// This must be called before modifying a buffer that will be journaled.
pub fn jbd2_journal_get_write_access(handle: &mut Handle, bh: *mut super::journal::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    // In Linux, this:
    // 1. Gets the journal_head for the buffer
    // 2. Checks if buffer is already part of transaction
    // 3. If not, files it in the transaction's reserved list
    // 4. Returns error if no credits available

    Ok(())
}

/// Get create access to a buffer (for new blocks)
pub fn jbd2_journal_get_create_access(handle: &mut Handle, bh: *mut super::journal::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    // Similar to get_write_access but for newly created buffers

    Ok(())
}

/// Mark a buffer as dirty metadata
///
/// This tells the journal that the buffer contains metadata that
/// needs to be written to the journal.
pub fn jbd2_journal_dirty_metadata(handle: &mut Handle, bh: *mut super::journal::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    let txn = handle.h_transaction.as_ref().ok_or(EIO)?;

    // In Linux, this:
    // 1. Gets journal_head for buffer
    // 2. Verifies buffer belongs to this transaction
    // 3. Files buffer in BJ_Metadata list
    // 4. Sets b_modified flag

    Ok(())
}

/// Forget a buffer (release it from the transaction)
pub fn jbd2_journal_forget(handle: &mut Handle, bh: *mut super::journal::BufferHead) -> Result<(), i32> {
    if is_handle_aborted(handle) {
        return Err(EROFS);
    }

    // In Linux, this:
    // 1. Removes buffer from transaction
    // 2. May refile to BJ_Forget list
    // 3. Clears dirty flags

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
        return true; // Would wait in Linux
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

    // In Linux, this would also:
    // - Set up commit timer
    // - Wake up commit thread

    Ok(())
}
