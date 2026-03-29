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
                    crate::console::puts("ext4: failed to get journal extent, journaling disabled\n");
                    return Ok(());
                }
            }
        } else {
            // Direct block pointer
            journal_inode.i_block[0] as u64
        };

        if journal_start_block == 0 {
            crate::console::puts("ext4: journal inode has no data blocks, journaling disabled\n");
            return Ok(());
        }

        // Read journal superblock (first block of the journal)
        let j_sb_bh = unsafe {
            match bio::bread(self.device, journal_start_block) {
                Some(b) => b,
                None => {
                    crate::console::puts("ext4: failed to read journal superblock\n");
                    return Err(EIO);
                }
            }
        };

        let j_sb = unsafe {
            &*((*j_sb_bh).b_data.as_ptr() as *const jbd2::journal_superblock_t)
        };

        // Validate magic number
        if u32::from_be(j_sb.s_header.h_magic) != JBD2_MAGIC_NUMBER {
            crate::console::puts("ext4: journal superblock magic mismatch, journaling disabled\n");
            bio::brelse(j_sb_bh);
            return Ok(());
        }

        // Validate block size matches filesystem
        let j_blocksize = u32::from_be(j_sb.s_blocksize);
        if j_blocksize != self.block_size {
            crate::console::puts("ext4: journal block size mismatch, journaling disabled\n");
            bio::brelse(j_sb_bh);
            return Ok(());
        }

        let j_maxlen = u32::from_be(j_sb.s_maxlen);
        let j_first = u32::from_be(j_sb.s_first);
        let j_sequence = u32::from_be(j_sb.s_sequence);
        let j_start = u32::from_be(j_sb.s_start);

        // Create Journal instance
        let mut journal = jbd2::Journal::new(j_blocksize, j_maxlen);
        journal.j_blk_offset = journal_start_block as u64;
        journal.j_bio_device = self.device;
        journal.j_first = j_first as u64;
        journal.j_last = j_maxlen as u64;
        journal.j_head.store(j_start as u64, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail.store(j_start as u64, core::sync::atomic::Ordering::SeqCst);
        journal.j_tail_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_transaction_sequence.store(j_sequence, core::sync::atomic::Ordering::SeqCst);
        journal.j_commit_sequence.store(j_sequence.saturating_sub(1), core::sync::atomic::Ordering::SeqCst);

        // Calculate free space
        let total_journal_blocks = j_maxlen as u64;
        journal.j_free.store(total_journal_blocks, core::sync::atomic::Ordering::SeqCst);

        let journal_arc = Arc::new(journal);

        // Run recovery if journal has uncommitted transactions
        if j_start != 0 {
            crate::console::puts("ext4: journal needs recovery, running replay...\n");
            match jbd2::jbd2_journal_recover(&journal_arc) {
                Ok(info) => {
                    if info.nr_replays > 0 {
                        crate::console::puts("ext4: journal recovery complete\n");
                    }
                }
                Err(_) => {
                    crate::console::puts("ext4: journal recovery failed, continuing without journal\n");
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

/// Start a journal transaction on the ext4 filesystem
pub fn ext4_journal_start(fs: &Ext4FileSystem, nblocks: i32) -> Result<jbd2::Handle, i32> {
    let journal_arc = match &fs.journal {
        Some(j) => j.clone(),
        None => return Err(EINVAL),
    };

    jbd2::jbd2_journal_start(&journal_arc, nblocks)
}

/// Stop a journal transaction (commits synchronously)
pub fn ext4_journal_stop(handle: &mut jbd2::Handle) -> Result<(), i32> {
    jbd2::jbd2_journal_stop(handle)
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
