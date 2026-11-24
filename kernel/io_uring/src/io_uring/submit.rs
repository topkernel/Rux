//! The io_uring SUBMIT loop (moved from kernel/src/io_uring/mod.rs at
//! the cold-four policy flip — sole copy here; zero behavior change).
//!
//! Reads the shared SQ ring strictly through the framework's window
//! views and offset accessors (INV-IOURING-RING-1); completions go
//! back through the framework-side io_uring_post_cqe (the completion
//! path stays with the wakeup path).

use super::*;

/// Submit and process SQEs from the submission queue.
pub fn submit_sqes(ring: &IoUring, to_submit: u32) -> u32 {
    let mut submitted = 0u32;

    // Read head and tail ONCE (per Linux io_uring_submit_sqes).  A malicious
    // userspace could modify the shared ring between iterations (TOCTOU),
    // causing double-processing or skipping of SQEs.  We advance a local
    // counter and write back only after the loop finishes.
    let sq = ring.sq_hdr();
    let mut head = sq.read_u32(ring.sq_head_off());
    let tail = sq.read_u32(ring.sq_tail_off());

    // Cap to actual available SQEs
    let available = tail.wrapping_sub(head);
    let batch = core::cmp::min(to_submit, available);

    for _ in 0..batch {
        // Read SQE index from the array (window-checked read; the index
        // itself is validated against sq_entries right below).
        let sqe_idx = sq.read_u32(
            ring.sq_array_off() + (head & ring.sq_ring_mask()) as usize * 4,
        );

        // Validate SQE index against sq_entries (per Linux:
        // READ_ONCE(ring->array[i]) < ctx->sq_entries).
        if sqe_idx >= ring.sq_entries() {
            break; // Invalid index — stop processing
        }

        // Read the SQE through the SQE-array view.
        let sqe = ring.sqe_at(sqe_idx);

        // Advance local head
        head = head.wrapping_add(1);

        // Execute operation synchronously
        let res = ops::io_uring_dispatch_op(&sqe);

        // Post CQE
        io_uring_post_cqe(ring, sqe.user_data, res, 0);

        submitted += 1;
    }

    // Write back the advanced head only after all submissions are complete
    if submitted > 0 {
        sq.write_u32(ring.sq_head_off(), head);
    }

    submitted
}

/// Wait for at least `min_complete` CQEs.
///
/// Since operations execute synchronously, CQEs are already posted
/// before this is called. The first check will always succeed.
pub fn wait_for_cqes(ring: &IoUring, min_complete: u32) -> i32 {
    for _ in 0..1000 {
        let completed = ring.cq_completed();
        if completed >= min_complete {
            return completed as i32;
        }

        if crate::signal::signal_pending() {
            return -4; // EINTR
        }

        crate::sched::yield_cpu();
    }

    // Timeout (should not happen with sync ops)
    -110 // ETIMEDOUT
}
