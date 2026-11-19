//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ext4 journal initialization and wrapper functions
//!
//! Bridges ext4 filesystem with JBD2 journaling layer.

use alloc::sync::Arc;

use crate::fs::bio;
use crate::fs::jbd2;
use crate::fs::jbd2::types::*;

use super::Ext4FileSystem;

// ============================================================================
// Error codes
// ============================================================================

const EIO: i32 = 5;
const EINVAL: i32 = 22;

// ============================================================================
// Journal initialization
// ============================================================================

impl Ext4FileSystem {
    /// Initialize the journal from the journal inode
    ///
    /// Reads journal inode (typically inode 8), gets its data blocks,
    /// reads journal superblock, validates, and optionally runs recovery.
    pub fn init_journal(&mut self) -> Result<(), i32> {
        // No journal inode — skip (filesystem without journal)
        if self.journal_ino == 0 {
            return Ok(());
        }

        // Read journal inode
        let journal_inode = match super::inode::read_inode(self, self.journal_ino) {
            Ok(inode) => inode,
            Err(_) => {
                // Journal inode not found — skip journaling
                return Ok(());
            }
        };

        // Get journal start block from inode's first data block
        // For simple journals, the journal data starts at i_block[0]
        let journal_extent_based = (journal_inode.i_flags & 0x80000) != 0;
        let journal_start_block = if journal_extent_based {
            // Extents-based journal inode — read first extent
            use crate::fs::ext4::extent::ext4_ext_get_block;
            match ext4_ext_get_block(self, &journal_inode.i_block, 0) {
                Ok(block) => block,
                Err(_) => {
                    crate::print_status("ext4", "journal extent not found, disabled", false);
                    return Ok(());
                }
            }
        } else {
            // Direct block pointer
            journal_inode.i_block[0] as u64
        };

        if journal_start_block == 0 {
            crate::print_status("ext4", "journal inode empty, disabled", false);
            return Ok(());
        }

        // Read journal superblock (first block of the journal)
        // SAFETY: bio::bread returns a valid raw BufferHead pointer or None;
        // the None case returns early above.
        let j_sb_bh = unsafe {
            match bio::bread(self.device, journal_start_block) {
                Some(b) => b,
                None => {
                    crate::print_status("ext4", "journal superblock read failed", false);
                    return Err(EIO);
                }
            }
        };

        // SAFETY: j_sb_bh is a valid BufferHead from bread(); b_data is a full
        // block-sized byte array, large enough to hold journal_superblock_t;
        // the pointer is valid for the lifetime of j_sb_bh.
        let j_sb = unsafe {
            &*((*j_sb_bh).b_data.as_ptr() as *const jbd2::journal_superblock_t)
        };

        // Validate magic number
        if u32::from_be(j_sb.s_header.h_magic) != JBD2_MAGIC_NUMBER {
            crate::print_status("ext4", "journal magic mismatch, disabled", false);
            bio::brelse(j_sb_bh);
            return Ok(());
        }

        // Validate block size matches filesystem
        let j_blocksize = u32::from_be(j_sb.s_blocksize);
        if j_blocksize != self.block_size {
            crate::print_status("ext4", "journal blocksize mismatch, disabled", false);
            bio::brelse(j_sb_bh);
            return Ok(());
        }

        let j_maxlen = u32::from_be(j_sb.s_maxlen);
        let j_first = u32::from_be(j_sb.s_first);
        let j_sequence = u32::from_be(j_sb.s_sequence);
        let j_start = u32::from_be(j_sb.s_start);
        // Feature negotiation for the JOURNAL itself (review 5.6): capture
        // s_feature_incompat so tag/record layouts match the on-disk format
        // the image was created with (64-bit journals need the wide tags).
        let j_features = u32::from_be(j_sb.s_feature_incompat);
        if j_features & !(jbd2::types::JBD2_KNOWN_INCOMPAT_FEATURES) != 0 {
            crate::pr_warn!("ext4: journal has unknown incompat features {:#x}", j_features);
        }
        // CSUM_V2/V3 (reference jbd2_journal_load): verify the superblock
        // checksum and derive the checksum seed from the journal UUID.
        // Divergence note: the reference fails the whole mount on a jsb
        // checksum error; we keep this mount's permissive journal-disable
        // shape (same as the magic-mismatch case) so images written by
        // older kernels remain mountable — record in TASK.md.
        let has_csum_v2or3 = j_features
            & (jbd2::types::JBD2_FEATURE_INCOMPAT_CSUM_V2
                | jbd2::types::JBD2_FEATURE_INCOMPAT_CSUM_V3)
            != 0;
        let mut csum_seed = 0u32;
        if has_csum_v2or3 {
            csum_seed = crate::fs::ext4::csum::crc32c(0xFFFF_FFFF, &j_sb.s_uuid);
            let old = j_sb.s_checksum;
            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(
                    (*j_sb_bh).b_data.as_ptr() as *const u8,
                    core::mem::size_of::<jbd2::types::journal_superblock_t>(),
                )
            };
            // SAFETY: j_sb is a & reference into the buffer; the checksum
            // field must be zeroed in the checksum input — clone-free
            // verify via a temporary copy of the 1024-byte region.
            let mut tmp = alloc::vec![0u8; bytes.len()];
            tmp.copy_from_slice(bytes);
            let off = core::mem::offset_of!(
                jbd2::types::journal_superblock_t,
                s_checksum
            );
            tmp[off..off + 4].fill(0);
            let calc = crate::fs::ext4::csum::crc32c(0xFFFF_FFFF, &tmp);
            if calc != u32::from_be(old) {
                crate::print_status("ext4", "journal checksum error, disabled", false);
                bio::brelse(j_sb_bh);
                return Ok(());
            }
        }

        // Create Journal instance
        //
        // LOG AREA GEOMETRY (journal-life fix, 2026-11-19): the usable log
        // area is [s_first, s_maxlen) — the reference reads the two fields
        // side by side (fs/jbd2/journal.c journal_reset:
        // `journal->j_first = be32_to_cpu(sb->s_first);
        //   journal->j_last = be32_to_cpu(sb->s_maxlen);`).
        // s_maxlen counts the journal inode's TOTAL blocks, superblock
        // block included: a 4 MiB journal inode has logical 0..1023 with
        // logical 0 the superblock, so the log area is [1, 1024) = 1023
        // blocks. The old `j_last = j_first + j_maxlen` (= 1025 here)
        // claimed logical block 1024 — one block BEYOND the journal inode
        // — as usable log: as j_head marched forward under churn (write-
        // through commits keep tail==head, so the ring never rewinds
        // early), the first commit whose span crossed 1024 resolved
        // logical 1024 through Journal::jmap, which has no extent for it
        // (the map covers exactly the inode), returned 0 (hole) and the
        // commit aborted the journal with EIO (~83 files x ~12 journal
        // blocks ~= 1023: the deterministic "stops persisting after ~83
        // files" face; every later op failed EROFS while error-code sign
        // bugs made userspace see garbage non-error returns).
        let j_last = j_maxlen as u64;
        if j_first as u64 >= j_last {
            // Degenerate on-disk geometry (s_first >= s_maxlen): the log
            // area would be empty. Disable journaling rather than write
            // through a zero-length ring.
            crate::print_status("ext4", "journal log area empty, disabled", false);
            bio::brelse(j_sb_bh);
            return Ok(());
        }
        let mut journal = jbd2::Journal::new(j_blocksize, j_maxlen);
        journal.j_feature_incompat
            .store(j_features, core::sync::atomic::Ordering::SeqCst);
        journal.j_blk_offset = journal_start_block as u64;
        journal.j_bio_device = self.device;
        journal.j_first = j_first as u64;
        journal.j_last = j_last;

        // When journal is clean (s_start == 0), head/tail start at j_first,
        // not 0 — block 0 is the journal superblock and must not be overwritten.
        // A stale s_start beyond the (now correctly sized) log area clamps
        // to j_first: recovery treats it as an empty log.
        let log_head = if j_start == 0 || j_start as u64 >= j_last {
            j_first as u64
        } else {
            j_start as u64
        };
        journal.j_head.store(log_head, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail.store(log_head, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_transaction_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_commit_sequence.store(j_sequence.saturating_sub(1), core::sync::atomic::Ordering::SeqCst);

        // Free space: head == tail at mount (the whole ring is free no
        // matter WHERE in the ring the head sits — the old
        // `j_last - log_head` understated free space on a mid-ring crash
        // recovery, where s_start lands mid-ring with an empty log).
        journal.j_free.store(j_last - j_first as u64, core::sync::atomic::Ordering::SeqCst);

        // JOURNAL BLOCK MAP (ext4-persist P0 fix): e2fsprogs lays the
        // journal inode out as MULTIPLE extents with physical holes
        // between them (the 96M battery image: logical 0-9 -> 19-28,
        // 10-24 -> 30-44, 25-1023 -> 1582-2580; physical 29 is the
        // inode BITMAP and 45..1581 the inode TABLES). The old
        // `j_blk_offset + logical` arithmetic assumed one contiguous
        // range and wrote journal DATA blocks onto live metadata:
        // dir-block journal copies over the inode bitmap (reserved
        // inodes handed out to mkdir), journal copies over the inode
        // table — catastrophic on-disk corruption within seconds of
        // any revoke-carrying commit (rmdir churn) under load. Capture
        // the true logical->physical map here, BEFORE recovery runs
        // (replay reads through the same map). Holes inside the journal
        // length stay 0: Journal::jmap returns 0 and commits into them
        // abort instead of corrupting. Legacy direct-block journals
        // keep the empty map (jmap's contiguous fallback — exact for
        // the Rux-made images).
        if journal_extent_based {
            use crate::fs::ext4::extent::ext4_ext_get_block;
            let mut map = alloc::vec::Vec::with_capacity(j_maxlen as usize);
            let mut holes = 0u32;
            for logical in 0..j_maxlen as u64 {
                match ext4_ext_get_block(self, &journal_inode.i_block, logical) {
                    Ok(b) if b != 0 => map.push(b),
                    _ => {
                        map.push(0);
                        holes += 1;
                    }
                }
            }
            if holes != 0 {
                crate::pr_warn!(
                    "ext4: journal inode has {}/{} unmapped logical blocks (multi-extent layout); writes into them abort",
                    holes,
                    j_maxlen
                );
            }
            *journal.j_block_map.lock() = map;
        }

        let journal_arc = Arc::new(journal);
        // Seed for CSUM_V2/V3 block/tag checksums (reference journal.c:
        // j_csum_seed = crc32c(~0, sb->s_uuid)); applied below via a
        // one-shot interior store because Journal is Arc-shared.
        journal_arc.set_csum_seed(csum_seed);

        // Run recovery if journal has uncommitted transactions
        if j_start != 0 {
            crate::print_status_ex("ext4", "journal recovery running", None);
            match jbd2::jbd2_journal_recover(&journal_arc) {
                Ok(info) => {
                    if info.nr_replays > 0 {
                        crate::print_status("ext4", "journal recovery complete", true);
                    }
                }
                Err(_) => {
                    crate::print_status("ext4", "journal recovery failed", false);
                    bio::brelse(j_sb_bh);
                    return Ok(());
                }
            }
        }

        self.journal = Some(journal_arc);

        bio::brelse(j_sb_bh);
        Ok(())
    }
}

// ============================================================================
// ext4 journal wrapper functions
// ============================================================================

/// Start a journal transaction on the ext4 filesystem.
///
/// Route-1 (F4, the no-journal lazy write path): when the filesystem has
/// no journal, this returns a NULL-JOURNAL handle instead of EINVAL. The
/// handle carries no transaction and no journal, so `jbd2_journal_stop`
/// returns early without committing — but while it is armed on the task
/// slot it does everything the deferral machinery needs:
/// - `bio::sync_dirty_buffer` defers (the buffer stays BH_Dirty), so the
///   3-8 inline synchronous metadata writes per mkdir/create/unlink on
///   the no-journal path stop happening; persistence moves to the batch
///   points (fsync/sync, buffer-cache eviction), exactly like Linux's
///   no-journal ext4 write-back mode;
/// - the PRE/POST publication capture windows arm, so every deferred
///   write keeps the crash ordering the ghost-file fix established
///   (pre blocks before the entry block, post frees after it) — the
///   scoped publication drains enforce it (per-task records at the
///   fsync/mkdir points, per-block obligations at every out-of-band
///   buffer write; see bio.rs).
/// Crash consistency class: a crash mid-batch loses recent metadata to
/// the same degree a write-back mount of the reference kernel does;
/// recovery is e2fsck (documented F4-R2 stance).
pub fn ext4_journal_start(fs: &Ext4FileSystem, nblocks: i32) -> Result<jbd2::Handle, i32> {
    let journal_arc = match &fs.journal {
        Some(j) => j.clone(),
        // Route-1 null-journal handle (see comment above). new_handle()
        // leaves h_transaction/h_journal None; stop() takes its early
        // return for that shape.
        None => return Ok(jbd2::new_handle(nblocks)),
    };

    jbd2::jbd2_journal_start(&journal_arc, nblocks)
}

/// Stop a journal transaction (commits synchronously)
pub fn ext4_journal_stop(handle: &mut jbd2::Handle) -> Result<(), i32> {
    let r = jbd2::jbd2_journal_stop(handle);
    // Op-boundary publication batch drain (the fab0e029 prescription):
    // every entry mutation (mkdir/create/mknod/rmdir/unlink/rename) closes
    // its bracket here, so this is the "transaction window" boundary where
    // deferred publication records batch. O(1) unless the queue crossed
    // PUB_BATCH, in which case this op pays ONE ordered drain — amortized
    // creator-paid write-back instead of either per-op scoped drains
    // (mkdir 15-30ms) or unbounded deferral (unlink constrained 100x).
    bio::publication_batch_drain();
    r
}

/// Get write access to a buffer within a transaction
pub fn ext4_journal_get_write_access(
    handle: &mut jbd2::Handle,
    bio_bh: *mut bio::BufferHead,
) -> Result<(), i32> {
    jbd2::jbd2_journal_get_write_access(handle, bio_bh)
}

/// Get create access to a buffer within a transaction (for new blocks)
pub fn ext4_journal_get_create_access(
    handle: &mut jbd2::Handle,
    bio_bh: *mut bio::BufferHead,
) -> Result<(), i32> {
    jbd2::jbd2_journal_get_create_access(handle, bio_bh)
}

/// Mark a buffer as dirty metadata within a transaction
pub fn ext4_journal_dirty_metadata(
    handle: &mut jbd2::Handle,
    bio_bh: *mut bio::BufferHead,
) -> Result<(), i32> {
    jbd2::jbd2_journal_dirty_metadata(handle, bio_bh)
}

/// Forget a buffer (remove from transaction tracking)
pub fn ext4_journal_forget(
    handle: &mut jbd2::Handle,
    bio_bh: *mut bio::BufferHead,
) -> Result<(), i32> {
    jbd2::jbd2_journal_forget(handle, bio_bh)
}
