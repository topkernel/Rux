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

// (INV-JBD2-BH-1: see recovery.rs — the commit path's bh-pinned
// journal block IO and its byte views ride the same contract.)
use core::mem::size_of;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::journal::{Journal, Tid, Transaction, TransactionState};
use super::types::*;

use rux::fs::bio;

// ============================================================================
// Error codes
// ============================================================================

// Negative errno (see transaction.rs's convention note).
const EIO: i32 = -5;

// ============================================================================
// Main commit function
// ============================================================================

/// Commit a transaction to the journal (simplified synchronous implementation)
pub fn jbd2_journal_commit_transaction(
    journal: &Arc<Journal>,
    commit_transaction: &Arc<Transaction>,
) -> Result<(), i32> {
    // Ordered-publication ordering is enforced PER BLOCK, not by draining
    // the world's pending publications here: every filesystem block this
    // commit writes (the write-through fast path below, the slow-path
    // data blocks, the journal superblock) goes through
    // bio::sync_dirty_buffer, whose guard satisfies exactly that block's
    // pre/entry/post obligations first (bio::flush_publication_constraints).
    // The old whole-queue drain at this entry made every forced commit —
    // and therefore every fsync/fdatasync, which force a commit — pay the
    // global publication backlog, looping for as long as creators kept
    // appending (the sc310 43-min hiview fdatasync family).
    let device = match journal.j_bio_device {
        Some(d) => d,
        None => return Ok(()), // no bio device installed (never mounted)
    };

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

    // Collect dirty buffers (keyed map — block order; see the field doc
    // on Transaction::t_dirty_buffers for why order carries no weight).
    let dirty_map: alloc::collections::BTreeMap<u64, super::journal::TxnBufferKind>;
    {
        let mut bufs = commit_transaction.t_dirty_buffers.lock();
        dirty_map = core::mem::take(&mut *bufs);
    }

    // R20-FS6 (F4): create-access entries are tracked with the Created
    // kind (the real bytes arrive via dirty_metadata, which flips the
    // entry to Live). A Created entry must NOT consume a tag slot: the
    // old code still emitted its tag while skipping the data block,
    // shifting every subsequent tag↔journal-block pairing by one —
    // replay then wrote buffer N's data over the block named by tag N+1.
    // Drop them up front so tags and data blocks stay 1:1.
    let dirty_buffers: Vec<(u64, super::journal::TxnBufferKind)> = dirty_map
        .into_iter()
        .filter(|(_, kind)| *kind == super::journal::TxnBufferKind::Live)
        .collect();

    // A transaction with NEITHER data NOR revoke records commits nothing.
    // (A revoke-only transaction — e.g. a pure block-freeing operation —
    // MUST still write its revoke block + commit block, or recovery would
    // replay the stale records the revoke was meant to suppress.)
    // Arm-2: the gate reads the COMMITTING transaction's own set, not the
    // journal-global vec.
    if dirty_buffers.is_empty() && commit_transaction.t_revoke_records.lock().is_empty() {
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Ok(());
    }

    // Fast path (write-through commit, no revoke records): every buffer
    // registered via jbd2_journal_dirty_metadata is ALSO modified in place
    // through its buffer-cache BufferHead (write_inode / write_block all
    // edit the cached block before registering). Syncing the cached buffer
    // to its FINAL on-disk location persists the update directly — the
    // journal-area copy (descriptor + data + commit block + superblock,
    // four synchronous I/Os) is pure overhead under this write-through
    // design because Phase 4 below advances the log tail past the
    // transaction immediately, so recovery never replays it anyway.
    // One synchronous write per dirty buffer instead of four per commit
    // takes a small ext4 write from ~60ms (five-plus waits, the LTP
    // post-exec "hang" family: every test writing a few KB of setup data
    // in <32B chunks blew the 30s wall clock) back to ~one wait.
    if commit_transaction.t_revoke_records.lock().is_empty() {
        crate::debug::fsw::note_j_commit_fast(dirty_buffers.len());
        // BATCHED WRITE-THROUGH (paired-test item 3): the old loop paid
        // ONE SYNCHRONOUS device round trip per registered buffer — the
        // measured 3.8 ms/fsync class (five-plus serialized 4 KiB writes
        // per commit). Same write-through semantics as before (see the
        // slow-path comment below for the live-content discipline), but
        // all buffers submit first and ONE drain device-acks the window.
        // The big lock matches SpanFlusher::drain's settle discipline
        // (task-recursive — callers already holding it re-enter freely).
        let blocks: alloc::vec::Vec<u64> =
            dirty_buffers.iter().map(|(b, _)| *b).collect();
        let _ext4_guard = crate::ext4::EXT4_BIG_LOCK.lock();
        let first_err = bio::sync_dirty_blocks_batched(device, &blocks, true);
        // Sequences advance; the log itself consumed no space (j_head,
        // j_tail, j_free unchanged).
        journal
            .j_commit_sequence
            .store(tid, core::sync::atomic::Ordering::SeqCst);
        journal
            .j_tail_sequence
            .store(tid, core::sync::atomic::Ordering::SeqCst);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        if first_err != 0 {
            journal.abort(first_err);
            return Err(first_err);
        }
        return Ok(());
    }

    // Arm-2 (f4-ext4-route-map.md §6.2 R2'): the slow path now runs the
    // fast path's write-through loop FIRST. With revokes riding the
    // transaction (lazy batching), mixed batches — writes plus any
    // pending free — take this path where they used to take the fast
    // one; without write-through that would silently drop fast-path
    // persistence for them (the old slow path left every registered
    // buffer dirty in RAM). With the loop, slow ≡ fast + revoke/commit/
    // superblock bookkeeping: the registered buffers reach their final
    // locations identically on both paths, and the journal-area copies
    // below stay format-compatible belt-and-suspenders. The syncs go
    // through bio::sync_dirty_buffer's per-block publication-constraint
    // guard, exactly like the fast path.
    {
        // BATCHED WRITE-THROUGH (paired-test item 3) — same shape and
        // rationale as the fast path above: one drain window instead of
        // one serialized round trip per registered buffer.
        let blocks: alloc::vec::Vec<u64> =
            dirty_buffers.iter().map(|(b, _)| *b).collect();
        let _ext4_guard = crate::ext4::EXT4_BIG_LOCK.lock();
        let first_err = bio::sync_dirty_blocks_batched(device, &blocks, true);
        if first_err != 0 {
            journal.abort(first_err);
            *commit_transaction.t_state.lock() = TransactionState::Finished;
            return Err(first_err);
        }
    }

    // Phase 2: Write to journal
    {
        crate::debug::fsw::note_j_commit_slow();
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
    // Plus revoke descriptor blocks, plus 1 commit block
    let num_buffers = dirty_buffers.len();
    let num_desc_blocks = (num_buffers + tags_per_block - 1) / tags_per_block;
    // Arm-2: size the revoke blocks from the COMMITTING transaction's own
    // set. This runs before Phase 2.6's write_revoke_records takes the
    // set, so the count is exact at sizing time (nothing else drains it
    // in between — inserts after this point spill, per the R1 routing).
    let revoke_entries = commit_transaction.t_revoke_records.lock().len();
    let revoke_entry_size = journal.revoke_entry_size();
    let per_revoke_block = if revoke_entry_size > 0 {
        (block_size - size_of::<journal_header_t>() - size_of::<u32>()) / revoke_entry_size
    } else {
        usize::MAX
    };
    let num_revoke_blocks = if revoke_entries == 0 || per_revoke_block == 0 {
        0
    } else {
        (revoke_entries + per_revoke_block - 1) / per_revoke_block
    };
    let total_journal_blocks = num_desc_blocks + num_buffers + num_revoke_blocks + 1;

    let journal_head = journal.j_head.load(core::sync::atomic::Ordering::SeqCst);
    let journal_first = journal.j_first;
    let journal_last = journal.j_last;

    // Space check — reference shape: the reference never aborts a journal
    // for running out of its own log; jbd2_journal_commit_transaction
    // callers block in __jbd2_log_wait_for_space, which checkpoints old
    // transactions until the span fits. In THIS write-through design the
    // wait would be a no-op with nothing to free: every completed commit
    // checkpoints everything (Phase 4: j_tail == j_head, j_free ==
    // capacity), so between commits the log is EMPTY and a span larger
    // than the whole log area can never fit, now or later. The check is
    // therefore against the LOG CAPACITY (not the possibly-stale j_free
    // counter), and the response is the graceful one: the write-through
    // pass at the head of this slow path has ALREADY persisted every
    // registered buffer to its final location, and recovery only ever
    // replays the crash-window transaction (sequence >= s_sequence of the
    // last superblock write), so skipping the journal-area copy for an
    // oversized transaction loses nothing — complete it fast-path shaped
    // (sequences advance, log untouched) and WARN once instead of killing
    // the filesystem. The lazy-batch high-water in jbd2_journal_stop is
    // capacity-derived, so this arm is a belt-and-suspenders guard.
    let log_capacity = journal.j_last.saturating_sub(journal.j_first);
    if total_journal_blocks as u64 > log_capacity {
        static OVERSIZE_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if OVERSIZE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_warn!(
                "jbd2: transaction span {} exceeds log area {}; committed write-through only (tid {})",
                total_journal_blocks,
                log_capacity,
                tid
            );
        }
        // Self-heal the free counter while we are here (head==tail ring).
        journal.j_free.store(log_capacity, core::sync::atomic::Ordering::SeqCst);
        journal
            .j_commit_sequence
            .store(tid, core::sync::atomic::Ordering::SeqCst);
        journal
            .j_tail_sequence
            .store(tid, core::sync::atomic::Ordering::SeqCst);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Ok(());
    }

    let mut current_journal_block = journal_head;
    let mut buf_idx = 0;

    // Write descriptor blocks interleaved with data blocks.
    //
    // CSUM_V2/V3 (reference parity): the descriptor block carries a
    // crc32c tail over the whole block and each tag carries a crc32c of
    // the referenced data block — so the DATA blocks are copied into the
    // journal area FIRST and each tag checksum is computed over the exact
    // bytes that landed there (the reference checksums the frozen buffer
    // image; our journal-area copy IS that frozen image). The on-disk
    // write order (data before its descriptor) is recovery-neutral: the
    // commit block remains the atomic completion point for both our
    // recovery and the reference's.
    let journal_has_csum3 = journal.has_csum_v3();
    let journal_has_64bit = journal.has_64bit();

    for _desc_block in 0..num_desc_blocks {
        let tags_this_block = core::cmp::min(tags_per_block, num_buffers - buf_idx);

        // The descriptor block takes the current head position; the data
        // blocks it describes follow it in the log.
        let desc_log_block = current_journal_block;
        current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

        // --- Copy the data blocks into the journal area, checksumming
        //     the copied bytes for the tags ---
        let mut tag_csums = alloc::vec![0u32; tags_this_block];
        for i in 0..tags_this_block {
            let (blocknr, _kind) = dirty_buffers[buf_idx + i];

            // Source: the live buffer-cache copy of the metadata block.
            // R3FIX: the journal-area image is copied from the LIVE buffer
            // cache at commit time, not from a registration-time clone —
            // the live content is always at least as new as any
            // registration image (same R20 argument as the write-through
            // fast path above).
            // bread on a valid device returns a cached (or
            // freshly-read) buffer head for any valid block number.
            let src_bh = match bio::bread(device, blocknr) {
                Some(b) => b,
                None => {
                    journal.abort(EIO);
                    *commit_transaction.t_state.lock() = TransactionState::Finished;
                    return Err(EIO);
                }
            };

            // ext4-persist P0 fix: journal blocks resolve through the
            // extent map (Journal::jmap), never the contiguous
            // arithmetic — e2fsprogs journals are multi-extent with
            // holes, and offset+logical wrote journal data onto the
            // inode bitmap / inode tables. 0 = unmapped hole: abort.
            let data_abs_block = journal.jmap(current_journal_block);
            if data_abs_block == 0 {
                bio::brelse(src_bh);
                journal.abort(EIO);
                *commit_transaction.t_state.lock() = TransactionState::Finished;
                return Err(EIO);
            }
            let data_bh = match bio::bread(device, data_abs_block) {
                Some(b) => b,
                None => {
                    bio::brelse(src_bh);
                    journal.abort(EIO);
                    *commit_transaction.t_state.lock() = TransactionState::Finished;
                    return Err(EIO);
                }
            };

            // b_data is block_size bytes on both heads; the copy length
            // is the journal block size. Views ride the audited bio
            // edges (bh_data / bh_data_mut).
            {
                let src_bytes = bio::bh_data(&src_bh);
                let dst_bytes = bio::bh_data_mut(&data_bh);
                let n = core::cmp::min(src_bytes.len(), dst_bytes.len());
                dst_bytes[..n].copy_from_slice(&src_bytes[..n]);
                // Tag checksum over the frozen image (reference
                // jbd2_block_tag_csum_set: crc32c(crc32c(seed, be32(seq)),
                // block bytes)).
                let data_bytes: &[u8] = &dst_bytes[..block_size.min(dst_bytes.len())];
                tag_csums[i] = block_tag_csum(journal, tid, data_bytes);
                bio::bh_set_state_bit(&data_bh, rux::fs::bio::BufferState::BH_Dirty);
            }
            bio::brelse(src_bh);
            current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

            let sync_res = bio::sync_dirty_buffer(data_bh);
            bio::brelse(data_bh);
            if let Err(e) = sync_res {
                journal.abort(e);
                *commit_transaction.t_state.lock() = TransactionState::Finished;
                return Err(e);
            }
        }

        // --- Build and write the descriptor block at desc_log_block ---
        let abs_block = journal.jmap(desc_log_block);
        if abs_block == 0 {
            journal.abort(EIO);
            *commit_transaction.t_state.lock() = TransactionState::Finished;
            return Err(EIO);
        }
        let bh = match bio::bread(device, abs_block) {
            Some(b) => b,
            None => {
                journal.abort(EIO);
                *commit_transaction.t_state.lock() = TransactionState::Finished;
                return Err(EIO);
            }
        };

        // Build descriptor: header + tags + tail
        let header = journal_header_t::new(JBD2_DESCRIPTOR_BLOCK, tid);
        // The struct->bytes view rides safety::bytes (the audited edge).
        let header_bytes: &[u8] = &crate::safety::bytes::bytes_of(&header)[..header_size];

        let mut desc_data = alloc::vec![0u8; block_size];
        desc_data[0..header_size].copy_from_slice(header_bytes);

        let mut offset = header_size;
        for i in 0..tags_this_block {
            let (blocknr, _) = dirty_buffers[buf_idx + i];
            let is_last = (i == tags_this_block - 1) && (buf_idx + i == num_buffers - 1);
            let flags: u32 = if is_last { JBD2_FLAG_LAST_TAG } else { 0 };
            if journal_has_csum3 {
                // V3 tag: { blocknr, flags(u32), blocknr_high, csum(u32) }
                // (reference jbd2_write_metadata_block tag layout).
                let tag = journal_block_tag3_t {
                    t_blocknr: (blocknr as u32).to_be(),
                    t_flags: flags.to_be(),
                    t_blocknr_high: (((blocknr >> 31) >> 1) as u32).to_be(),
                    t_checksum: tag_csums[i].to_be(),
                };
                // #[repr(C, packed)]; size == tag_size for V3
                // (the view rides safety::bytes).
                let tag_bytes: &[u8] = &crate::safety::bytes::bytes_of(&tag)[..tag_size];
                desc_data[offset..offset + tag_size].copy_from_slice(tag_bytes);
            } else if journal_has_64bit {
                // V1/V2 tag with the high block half.
                let tag = journal_block_tag_t {
                    t_blocknr: (blocknr as u32).to_be(),
                    t_checksum: (tag_csums[i] as u16).to_be(),
                    t_flags: (flags as u16).to_be(),
                    t_blocknr_high: (((blocknr >> 31) >> 1) as u32).to_be(),
                };
                // #[repr(C, packed)]; size == tag_size for 64-bit V2
                // (the view rides safety::bytes).
                let tag_bytes: &[u8] = &crate::safety::bytes::bytes_of(&tag)[..tag_size];
                desc_data[offset..offset + tag_size].copy_from_slice(tag_bytes);
            } else {
                // 32-bit V1/V2 tag: blocknr, truncated csum, flags.
                let tag = journal_block_tag_t {
                    t_blocknr: (blocknr as u32).to_be(),
                    t_checksum: (tag_csums[i] as u16).to_be(),
                    t_flags: (flags as u16).to_be(),
                    t_blocknr_high: 0,
                };
                // 8-byte truncated view of a 12-byte tag — in bounds
                // (the view rides safety::bytes).
                let tag_bytes: &[u8] = &crate::safety::bytes::bytes_of(&tag)[..tag_size];
                desc_data[offset..offset + tag_size].copy_from_slice(tag_bytes);
            }
            offset += tag_size;
        }

        // Tail: zeroed in desc_data already; set the crc32c tail when the
        // journal carries V2/V3 checksums.
        descriptor_block_csum_set(journal, &mut desc_data);

        // b_data is block_size bytes; views ride the bio edges.
        {
            let dst = bio::bh_data_mut(&bh);
            let n = core::cmp::min(dst.len(), desc_data.len());
            dst[..n].copy_from_slice(&desc_data[..n]);
            bio::bh_set_state_bit(&bh, rux::fs::bio::BufferState::BH_Dirty);
        }

        // R20-FS5 (F3): release the bh BEFORE propagating a sync error — the
        // old `map_err(..)?` early-return leaked one bh reference per error,
        // permanently pinning the cache entry.
        let sync_res = bio::sync_dirty_buffer(bh);
        bio::brelse(bh);
        if let Err(e) = sync_res {
            journal.abort(e);
            *commit_transaction.t_state.lock() = TransactionState::Finished;
            return Err(e);
        }

        buf_idx += tags_this_block;
    }

    // Phase 2.5: data=ordered flush
    // The write-through pass at the head of the slow path has already
    // synced every registered buffer to its final location (Arm-2 R2';
    // the old comment here claimed the ops themselves had synced inline —
    // that predates bio::sync_dirty_buffer's under-handle deferral, which
    // leaves op-time metadata dirty in the buffer cache). A full
    // sync_buffers scan would still be redundant and would add SMP
    // contention on the block-cache bucket spinlocks.

    // Phase 2.6: revoke records (review 5.6 high: revoke 全空壳). Written
    // BEFORE the commit block: a crash between data blocks and here simply
    // replays everything (old behaviour); a crash after the commit block
    // sees the revokes and skips the freed blocks.
    let revoke_blocks = super::revoke::jbd2_journal_write_revoke_records(
        journal,
        commit_transaction,
    )?;
    let _ = revoke_blocks;

    // Phase 3: Write commit block
    {
        let mut state = commit_transaction.t_state.lock();
        *state = TransactionState::Commit;
    }

    let commit_abs_block = journal.jmap(current_journal_block);
    if commit_abs_block == 0 {
        journal.abort(EIO);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Err(EIO);
    }
    let commit_bh = match bio::bread(device, commit_abs_block) {
        Some(b) => b,
        None => {
            journal.abort(EIO);
            *commit_transaction.t_state.lock() = TransactionState::Finished;
            return Err(EIO);
        }
    };

    let commit_hdr = commit_header {
        h_magic: JBD2_MAGIC_NUMBER.to_be(),
        h_blocktype: JBD2_COMMIT_BLOCK.to_be(),
        h_sequence: tid.to_be(),
        ..Default::default()
    };
    // The struct->bytes view rides safety::bytes (the audited edge);
    // the block window rides the bio edges.
    let commit_bytes: &[u8] = crate::safety::bytes::bytes_of(&commit_hdr);
    {
        let block = bio::bh_data_mut(&commit_bh);
        for b in block.iter_mut() { *b = 0; }
        let n = core::cmp::min(block.len(), commit_bytes.len());
        block[..n].copy_from_slice(&commit_bytes[..n]);
        // CSUM_V2/V3 (reference jbd2_commit_block_csum_set): h_chksum_type,
        // h_chksum_size and h_chksum[0] are all zero at this point
        // (commit_header::Default + the block wipe), so the checksum runs
        // straight over the block and lands in h_chksum[0].
        if journal.has_csum_v2or3() {
            let csum = crate::ext4::csum::crc32c(
                journal.csum_seed(),
                &block[..block_size.min(block.len())],
            );
            let hdr = rux::fs::r#unsafe::typed_ref_mut::<commit_header>(block)
                .expect("commit header view");
            hdr.h_chksum[0] = csum.to_be();
        }
        bio::bh_set_state_bit(&commit_bh, rux::fs::bio::BufferState::BH_Dirty);
    }
    current_journal_block = wrap_journal_block(current_journal_block, journal_first, journal_last);

    let sync_res = bio::sync_dirty_buffer(commit_bh);
    bio::brelse(commit_bh);
    if let Err(e) = sync_res {
        journal.abort(e);
        *commit_transaction.t_state.lock() = TransactionState::Finished;
        return Err(e);
    }

    // Phase 4: Update journal state
    journal.j_head.store(current_journal_block, core::sync::atomic::Ordering::SeqCst);
    // j_free recompute (review 5.6: j_free 失真): this implementation
    // writes every journaled buffer THROUGH to its final location before
    // the commit block lands (write-through + ordered data), i.e. the
    // transaction is fully checkpointed the moment it commits. The log
    // region [tail, head) is therefore immediately reusable: advance the
    // tail to the head and recompute free space from the geometry instead
    // of the never-restored reservation counter.
    journal.j_tail.store(current_journal_block, core::sync::atomic::Ordering::SeqCst);
    if journal.j_last > journal.j_first {
        journal.j_free.store(
            journal.j_last - journal.j_first,
            core::sync::atomic::Ordering::SeqCst,
        );
    }
    journal.j_commit_sequence.store(tid, core::sync::atomic::Ordering::SeqCst);
    journal.j_tail_sequence.store(tid, core::sync::atomic::Ordering::SeqCst);

    // Write journal superblock (logical block 0, resolved through jmap)
    write_journal_superblock(journal, device, journal.jmap(0), current_journal_block, tid + 1)?;

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

// ============================================================================
// Journal checksums (JBD2_FEATURE_INCOMPAT_CSUM_V2/V3 parity)
//
// The reference (fs/jbd2 + include/linux/jbd2.h) checksums every journal
// metadata block with crc32c when the journal carries CSUM_V2/V3:
//   - journal superblock: crc32c(~0, jsb[0..1024] with s_checksum zeroed)
//     — stored in s_checksum (offset 252) — jbd2_superblock_csum()
//   - descriptor/revoke block: crc32c(j_csum_seed, whole block with the
//     4-byte tail zeroed) — stored in the tail —
//     jbd2_descriptor_block_csum_set()
//   - commit block: crc32c(j_csum_seed, whole block with h_chksum_type,
//     h_chksum_size and h_chksum[0] zeroed) — stored in h_chksum[0] —
//     jbd2_commit_block_csum_set()
//   - each tag: crc32c(crc32c(j_csum_seed, be32(t_sequence)),
//     referenced block bytes) — jbd2_block_tag_csum_set() (full u32 for
//     V3, truncated be16 for V2)
// j_csum_seed = crc32c(~0, jsb.s_uuid) is computed at journal init.
// ============================================================================

/// Set (in place) the tail checksum of a descriptor/revoke block image.
fn descriptor_block_csum_set(journal: &Journal, block: &mut [u8]) {
    if !journal.has_csum_v2or3() {
        return;
    }
    let n = block.len();
    block[n - size_of::<journal_block_tail_t>()..].fill(0);
    let csum = crate::ext4::csum::crc32c(journal.csum_seed(), block);
    block[n - size_of::<journal_block_tail_t>()..]
        .copy_from_slice(&csum.to_be_bytes());
}

/// Per-tag checksum over the referenced data block image.
fn block_tag_csum(journal: &Journal, sequence: Tid, data: &[u8]) -> u32 {
    let csum = crate::ext4::csum::crc32c(
        journal.csum_seed(),
        &sequence.to_be_bytes(),
    );
    crate::ext4::csum::crc32c(csum, data)
}


/// Write journal superblock with updated s_start and s_sequence.
/// `sb_block` is the PHYSICAL block of the journal superblock (journal
/// logical 0, resolved by the caller through Journal::jmap).
fn write_journal_superblock(
    journal: &Journal,
    device: &'static crate::drivers::types::GenDisk,
    sb_block: u64,
    new_start: u64,
    new_sequence: u32,
) -> Result<(), i32> {
    // SAFETY: bio::bread returns a valid BufferHead or None (handled).
    let bh = match bio::bread(device, sb_block) {
        Some(b) => b,
        None => return Err(EIO),
    };

    // The superblock fits within a block; the typed view rides the
    // framework's ext4_edges (packed struct — alignment check is
    // trivially satisfied).
    {
        let block = bio::bh_data_mut(&bh);
        let sb = rux::fs::r#unsafe::typed_ref_mut::<journal_superblock_t>(block)
            .expect("journal superblock view");
        sb.s_start = (new_start as u32).to_be();
        sb.s_sequence = new_sequence.to_be();
        // CSUM_V2/V3 (reference jbd2_superblock_csum + jbd2_journal_update_sb_*
        // write path): the superblock checksum covers the 1024-byte
        // journal_superblock_t region with s_checksum zeroed, seeded ~0 —
        // NOT j_csum_seed, NOT the full block.
        if journal.has_csum_v2or3() {
            sb.s_checksum = 0;
            let block = bio::bh_data(&bh);
            let bytes: &[u8] = &block[..size_of::<journal_superblock_t>()];
            let csum = crate::ext4::csum::crc32c(0xFFFF_FFFF, bytes);
            sb.s_checksum = csum.to_be();
        }
        bio::bh_set_state_bit(&bh, rux::fs::bio::BufferState::BH_Dirty);
    }

    let sync_res = bio::sync_dirty_buffer(bh);
    bio::brelse(bh);
    sync_res
}
