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
        let journal_start_block = if (journal_inode.i_flags & 0x80000) != 0 {
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

        // Journal geometry (reference fs/jbd2/journal.c,
        // journal_check_superblock + journal_load_superblock): s_maxlen is
        // the TOTAL journal length in blocks, journal superblock included,
        // so the log ring is [s_first, s_maxlen) and j_last = s_maxlen is
        // already "one past the last usable block". mke2fs writes
        // s_first = 1 and s_maxlen = the journal inode's block count, so
        // j_first + j_maxlen pointed one block PAST the journal inode: the
        // ring wrap in commit.rs / revoke.rs then wrote log blocks onto the
        // filesystem block that follows the journal. Reject geometry the
        // reference rejects (it fails the mount; here the journal is
        // disabled, like the other superblock checks above).
        let j_inode_blocks = ((journal_inode.i_size as u64)
            | ((journal_inode.i_size_high as u64) << 32))
            / (self.block_size as u64);
        let geometry_ok = j_first != 0
            && j_first < j_maxlen
            && (j_maxlen as u64) <= j_inode_blocks
            && (j_start == 0 || (j_start >= j_first && j_start < j_maxlen));
        if !geometry_ok {
            crate::print_status("ext4", "journal geometry invalid, disabled", false);
            bio::brelse(j_sb_bh);
            return Ok(());
        }
        let mut j_last = j_maxlen as u64;
        // Fast-commit area: the reference carves the last s_num_fc_blks
        // (default JBD2_DEFAULT_FAST_COMMIT_BLOCKS) out of the regular log
        // when the feature is set, provided the remaining log keeps
        // JBD2_MIN_JOURNAL_BLOCKS.
        if j_features & jbd2::types::JBD2_FEATURE_INCOMPAT_FAST_COMMIT != 0 {
            let fc = match u32::from_be(j_sb.s_num_fc_blks) {
                0 => jbd2::types::JBD2_DEFAULT_FAST_COMMIT_BLOCKS,
                n => n,
            } as u64;
            if j_last.saturating_sub(fc) >= jbd2::types::JBD2_MIN_JOURNAL_BLOCKS as u64 {
                j_last -= fc;
            }
        }
        if (j_first as u64) >= j_last || (j_start != 0 && (j_start as u64) >= j_last) {
            crate::print_status("ext4", "journal geometry invalid, disabled", false);
            bio::brelse(j_sb_bh);
            return Ok(());
        }

        // Create Journal instance
        let mut journal = jbd2::Journal::new(j_blocksize, j_maxlen);
        journal.j_feature_incompat
            .store(j_features, core::sync::atomic::Ordering::SeqCst);
        journal.j_blk_offset = journal_start_block as u64;
        journal.j_bio_device = self.device;
        journal.j_first = j_first as u64;
        journal.j_last = j_last;

        // When journal is clean (s_start == 0), head/tail start at j_first,
        // not 0 — block 0 is the journal superblock and must not be overwritten.
        let log_head = if j_start == 0 { j_first as u64 } else { j_start as u64 };
        journal.j_head.store(log_head, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail.store(log_head, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_transaction_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_commit_sequence.store(j_sequence.saturating_sub(1), core::sync::atomic::Ordering::SeqCst);

        // Free space = total log area minus used
        journal.j_free.store(j_last - log_head, core::sync::atomic::Ordering::SeqCst);

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
