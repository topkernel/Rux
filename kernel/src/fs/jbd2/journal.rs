//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 Journal structure
//!
//! JBD2 journal management

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::Mutex;
use spin::RwLock;

use super::types::*;

// ============================================================================
// Type aliases
// ============================================================================

/// Transaction ID type
pub type Tid = u32;

// ============================================================================
// Journal flags
// ============================================================================

/// Journal has been aborted
pub const JBD2_ABORT: u32 = 0x001;
/// Journal thread should stop
pub const JBD2_ACK_ERR: u32 = 0x002;
/// Journal has been flushed on mount
pub const JBD2_FLUSHED: u32 = 0x004;
/// Journal loaded from disk */
pub const JBD2_LOADED: u32 = 0x008;
/// Journal has been updated on disk
pub const JBD2_UPDATE_SYNC: u32 = 0x010;
/// Journal in synchronous mode
pub const JBD2_SYNC: u32 = 0x020;
/// Journal checksum type
pub const JBD2_CRC32: u32 = 0x100;
pub const JBD2_MD5: u32 = 0x200;
pub const JBD2_SHA1: u32 = 0x400;
pub const JBD2_CRC32C: u32 = 0x800;
/// Journal broken on disk
pub const JBD2_BROKEN: u32 = 0x1000;
/// Journal use async commit
pub const JBD2_ASYNC_COMMIT: u32 = 0x2000;
/// Journal fast commit in progress
pub const JBD2_FC_REPLAY: u32 = 0x4000;

// ============================================================================
// Transaction state
// ============================================================================

/// Transaction states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// Accepting new updates
    Running,
    /// Updates still running but we don't accept new ones
    Locked,
    /// Updates are tidying up but have finished requesting new buffers
    Switch,
    /// All updates complete, but we are still writing to disk
    Flush,
    /// All data on disk, writing commit record
    Commit,
    /// Data flushed to disk
    CommitDflush,
    /// Journal flushed to disk
    CommitJflush,
    /// Running callbacks
    CommitCallback,
    /// We still have to keep the transaction for checkpointing
    Finished,
}

impl Default for TransactionState {
    fn default() -> Self {
        Self::Running
    }
}

// ============================================================================
// Journal list types (b_jlist values)
// ============================================================================

pub const BJ_None: u32 = 0;
pub const BJ_SyncData: u32 = 1;
pub const BJ_Metadata: u32 = 2;
pub const BJ_Forget: u32 = 3;
pub const BJ_IO: u32 = 4;
pub const BJ_Shadow: u32 = 5;
pub const BJ_LogCtl: u32 = 6;
pub const BJ_Reserved: u32 = 7;
pub const BJ_Locked: u32 = 8;
pub const BJ_Types: u32 = 9;

// ============================================================================
// JBD2 Inode
// ============================================================================

/// JBD2 inode structure - links inodes in ordered mode
pub struct Jbd2Inode {
    /// Which transaction does this inode belong to?
    pub i_transaction: Option<Arc<Transaction>>,
    /// Pointer to the running transaction modifying inode's data
    pub i_next_transaction: Option<Arc<Transaction>>,
    /// List of inodes in the i_transaction
    pub i_list: ListHead,
    /// VFS inode this inode belongs to
    pub i_vfs_inode: *mut core::ffi::c_void,
    /// Flags of inode
    pub i_flags: AtomicU32,
    /// Offset in bytes where the dirty range starts
    pub i_dirty_start: u64,
    /// Inclusive offset in bytes where the dirty range ends
    pub i_dirty_end: u64,
}

// JBD2 inode flags
pub const JI_COMMIT_RUNNING: u32 = 1 << 0;
pub const JI_WRITE_DATA: u32 = 1 << 1;
pub const JI_WAIT_DATA: u32 = 1 << 2;

// ============================================================================
// List head (simplified)
// ============================================================================

/// Simplified list head for doubly-linked lists
pub struct ListHead {
    pub next: *mut ListHead,
    pub prev: *mut ListHead,
}

impl Default for ListHead {
    fn default() -> Self {
        Self {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        }
    }
}

impl ListHead {
    pub const fn new() -> Self {
        Self {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        }
    }

    /// Initialize list head to point to itself
    pub fn init(&mut self) {
        self.next = self as *mut _;
        self.prev = self as *mut _;
    }

    /// Check if list is empty
    pub fn is_empty(&self) -> bool {
        self.next == self as *const _ as *mut _
    }

    /// Add entry to list
    pub fn add(&mut self, new: *mut ListHead) {
        unsafe {
            (*new).next = self.next;
            (*new).prev = self as *mut _;
            if !self.next.is_null() {
                (*self.next).prev = new;
            }
            self.next = new;
        }
    }

    /// Add entry to tail of list
    pub fn add_tail(&mut self, new: *mut ListHead) {
        unsafe {
            (*new).next = self as *mut _;
            (*new).prev = self.prev;
            if !self.prev.is_null() {
                (*self.prev).next = new;
            }
            self.prev = new;
        }
    }

    /// Remove entry from list
    pub fn remove(entry: *mut ListHead) {
        unsafe {
            if !(*entry).next.is_null() && !(*entry).prev.is_null() {
                (*(*entry).next).prev = (*entry).prev;
                (*(*entry).prev).next = (*entry).next;
            }
        }
    }
}

// ============================================================================
// Journal Head
// ============================================================================

/// Journal head - attached to buffer_head for journaling
pub struct JournalHead {
    /// Points back to our buffer_head
    pub b_bh: *mut BufferHead,
    /// Reference count
    pub b_jcount: AtomicI32,
    /// Journalling list for this buffer
    pub b_jlist: u32,
    /// Buffer has been modified by the currently running transaction
    pub b_modified: bool,
    /// Copy of the buffer data frozen for writing to the log
    pub b_frozen_data: *mut u8,
    /// Saved copy of buffer containing no uncommitted deallocation references
    pub b_committed_data: *mut u8,
    /// Compound transaction which owns this buffer's metadata
    pub b_transaction: Option<Arc<Transaction>>,
    /// Running compound transaction modifying the buffer's metadata
    pub b_next_transaction: Option<Arc<Transaction>>,
    /// Doubly-linked list of buffers on a transaction
    pub b_tnext: *mut JournalHead,
    pub b_tprev: *mut JournalHead,
    /// Compound transaction against which this buffer is checkpointed
    pub b_cp_transaction: Option<Arc<Transaction>>,
    /// Doubly-linked list of buffers for checkpointing
    pub b_cpnext: *mut JournalHead,
    pub b_cpprev: *mut JournalHead,
    /// Trigger type
    pub b_triggers: *mut Jbd2BufferTriggerType,
    /// Trigger type for the committing transaction's frozen data
    pub b_frozen_triggers: *mut Jbd2BufferTriggerType,
}

/// Buffer head (simplified for JBD2)
pub struct BufferHead {
    /// Block number
    pub b_blocknr: u64,
    /// Buffer size
    pub b_size: u32,
    /// Buffer data
    pub b_data: *mut u8,
    /// Buffer state flags
    pub b_state: AtomicU32,
    /// Journal head attached to this buffer
    pub b_private: *mut JournalHead,
    /// Block device
    pub b_bdev: *mut core::ffi::c_void,
    /// Reference count
    pub b_count: AtomicI32,
}

/// Buffer trigger type (placeholder)
pub struct Jbd2BufferTriggerType {
    pub frozen_triggers: *mut core::ffi::c_void,
}

// ============================================================================
// Handle
// ============================================================================

/// Handle structure - represents a single atomic update
pub struct Handle {
    /// Which compound transaction is this update a part of?
    pub h_transaction: Option<Arc<Transaction>>,
    /// Which journal handle belongs to
    pub h_journal: Option<Arc<Journal>>,
    /// Handle reserved for finishing the logical operation
    pub h_rsv_handle: Option<*mut Handle>,
    /// Number of remaining buffers we are allowed to add to journal
    pub h_total_credits: i32,
    /// Number of remaining revoke records available
    pub h_revoke_credits: i32,
    /// Requested revoke credits
    pub h_revoke_credits_requested: i32,
    /// Reference count on this handle
    pub h_ref: i32,
    /// Field for caller's use to track errors
    pub h_err: i32,
    /// Flag for sync-on-close
    pub h_sync: bool,
    /// Flag for handle for reserved credits
    pub h_reserved: bool,
    /// Flag indicating fatal error on handle
    pub h_aborted: bool,
    /// Handle type for statistics
    pub h_type: u8,
    /// Line number for statistics
    pub h_line_no: u16,
    /// Handle start time in jiffies
    pub h_start_jiffies: u64,
    /// Requested credits
    pub h_requested_credits: u32,
}

impl Default for Handle {
    fn default() -> Self {
        Self {
            h_transaction: None,
            h_journal: None,
            h_rsv_handle: None,
            h_total_credits: 0,
            h_revoke_credits: 0,
            h_revoke_credits_requested: 0,
            h_ref: 1,
            h_err: 0,
            h_sync: false,
            h_reserved: false,
            h_aborted: false,
            h_type: 0,
            h_line_no: 0,
            h_start_jiffies: 0,
            h_requested_credits: 0,
        }
    }
}

impl Handle {
    pub fn new(nblocks: i32) -> Self {
        Self {
            h_total_credits: nblocks,
            h_ref: 1,
            h_start_jiffies: 0, // get_jiffies()
            ..Default::default()
        }
    }
}

// ============================================================================
// Transaction
// ============================================================================

/// Transaction statistics
pub struct TransactionRunStats {
    pub rs_wait: u64,
    pub rs_request_delay: u64,
    pub rs_running: u64,
    pub rs_locked: u64,
    pub rs_flushing: u64,
    pub rs_logging: u64,
    pub rs_handle_count: u32,
    pub rs_blocks: u32,
    pub rs_blocks_logged: u32,
}

impl Default for TransactionRunStats {
    fn default() -> Self {
        Self {
            rs_wait: 0,
            rs_request_delay: 0,
            rs_running: 0,
            rs_locked: 0,
            rs_flushing: 0,
            rs_logging: 0,
            rs_handle_count: 0,
            rs_blocks: 0,
            rs_blocks_logged: 0,
        }
    }
}

/// Checkpoint statistics
pub struct TransactionChpStats {
    pub cs_chp_time: u64,
    pub cs_forced_to_close: u32,
    pub cs_written: u32,
    pub cs_dropped: u32,
}

impl Default for TransactionChpStats {
    fn default() -> Self {
        Self {
            cs_chp_time: 0,
            cs_forced_to_close: 0,
            cs_written: 0,
            cs_dropped: 0,
        }
    }
}

/// Transaction statistics
pub struct TransactionStats {
    pub ts_tid: u64,
    pub ts_requested: u64,
    pub run: TransactionRunStats,
}

impl Default for TransactionStats {
    fn default() -> Self {
        Self {
            ts_tid: 0,
            ts_requested: 0,
            run: TransactionRunStats::default(),
        }
    }
}

/// Transaction structure - the guts of the journaling mechanism
pub struct Transaction {
    /// Pointer to the journal for this transaction
    pub t_journal: Option<Arc<Journal>>,
    /// Sequence number for this transaction
    pub t_tid: Tid,
    /// Transaction's current state (protected by Mutex for interior mutability)
    pub t_state: Mutex<TransactionState>,
    /// Where in the log does this transaction's commit start?
    pub t_log_start: u64,
    /// Tracked dirty metadata buffers: (filesystem blocknr, frozen data copy)
    /// Replaces raw linked list with Rust-idiomatic Vec for single-core no_std
    pub t_dirty_buffers: Mutex<alloc::vec::Vec<(u64, alloc::vec::Vec<u8>)>>,
    /// Number of buffers on the t_buffers list
    pub t_nr_buffers: i32,
    /// Doubly-linked circular list of all reserved but not yet modified buffers
    pub t_reserved_list: *mut JournalHead,
    /// Doubly-linked circular list of all metadata buffers owned by this transaction
    pub t_buffers: *mut JournalHead,
    /// Doubly-linked circular list of all forget buffers
    pub t_forget: *mut JournalHead,
    /// Doubly-linked circular list of all buffers to be flushed before checkpoint
    pub t_checkpoint_list: *mut JournalHead,
    /// Doubly-linked circular list of metadata buffers being shadowed by log IO
    pub t_shadow_list: *mut JournalHead,
    /// List of inodes associated with the transaction
    pub t_inode_list: ListHead,
    /// Longest time some handle had to wait
    pub t_max_wait: u64,
    /// When transaction started
    pub t_start: u64,
    /// When commit was requested
    pub t_requested: u64,
    /// Checkpointing stats
    pub t_chp_stats: TransactionChpStats,
    /// Number of outstanding updates running on this transaction
    pub t_updates: AtomicI32,
    /// Number of blocks reserved for this transaction
    pub t_outstanding_credits: AtomicI32,
    /// Number of revoke records for this transaction
    pub t_outstanding_revokes: AtomicI32,
    /// How many handles used this transaction
    pub t_handle_count: AtomicI32,
    /// Forward and backward links for checkpoint list
    pub t_cpnext: Option<Arc<Transaction>>,
    pub t_cpprev: Option<Arc<Transaction>>,
    /// When will the transaction expire
    pub t_expires: u64,
    /// When this transaction started (nanoseconds)
    pub t_start_time: u64,
    /// This transaction is being forced
    pub t_synchronous_commit: bool,
    /// Disk flush needs to be sent to fs partition
    pub t_need_data_flush: i32,
}

impl Transaction {
    pub fn new(journal: Arc<Journal>) -> Self {
        Self {
            t_journal: Some(journal.clone()),
            t_tid: journal.j_transaction_sequence.load(Ordering::SeqCst),
            t_state: Mutex::new(TransactionState::Running),
            t_log_start: 0,
            t_nr_buffers: 0,
            t_reserved_list: core::ptr::null_mut(),
            t_buffers: core::ptr::null_mut(),
            t_forget: core::ptr::null_mut(),
            t_checkpoint_list: core::ptr::null_mut(),
            t_shadow_list: core::ptr::null_mut(),
            t_inode_list: ListHead::new(),
            t_max_wait: 0,
            t_start: 0, // get_jiffies()
            t_requested: 0,
            t_chp_stats: TransactionChpStats::default(),
            t_updates: AtomicI32::new(0),
            t_outstanding_credits: AtomicI32::new(0),
            t_outstanding_revokes: AtomicI32::new(0),
            t_handle_count: AtomicI32::new(0),
            t_cpnext: None,
            t_cpprev: None,
            t_expires: 0,
            t_start_time: 0,
            t_synchronous_commit: false,
            t_need_data_flush: 0,
            t_dirty_buffers: Mutex::new(alloc::vec::Vec::new()),
        }
    }
}

// ============================================================================
// Revoke table
// ============================================================================

/// Revoke table structure
pub struct Jbd2RevokeTable {
    /// Hash table size
    pub hash_size: u32,
    /// Hash mask
    pub hash_mask: u32,
    /// Hash table
    pub hash_table: *mut *mut Jbd2RevokeRecord,
}

/// Revoke record
pub struct Jbd2RevokeRecord {
    /// List head for hash chain
    pub hash_list: ListHead,
    /// Block number being revoked
    pub blocknr: u64,
    /// Transaction ID
    pub tid: Tid,
}

// ============================================================================
// Journal
// ============================================================================

/// Journal statistics
pub struct JournalStats {
    pub j_stats: TransactionStats,
}

/// Journal structure - the main journal control structure
pub struct Journal {
    /// General journaling state flags
    pub j_flags: AtomicU32,
    /// Is there an outstanding uncleared error on the journal
    pub j_errno: AtomicI32,
    /// Lock the whole aborting procedure
    pub j_abort_mutex: Mutex<()>,
    /// The first part of the superblock buffer
    pub j_sb_buffer: *mut BufferHead,
    /// The second part of the superblock buffer
    pub j_superblock: *mut journal_superblock_t,
    /// Number of processes waiting to create a barrier lock
    pub j_barrier_count: AtomicI32,
    /// The barrier lock itself
    pub j_barrier: Mutex<()>,
    /// The current running transaction (protected by j_barrier mutex)
    pub j_running_transaction: Mutex<Option<Arc<Transaction>>>,
    /// The transaction we are pushing to disk (protected by j_barrier mutex)
    pub j_committing_transaction: Mutex<Option<Arc<Transaction>>>,
    /// Linked circular list of all transactions waiting for checkpointing
    pub j_checkpoint_transactions: Mutex<VecDeque<Arc<Transaction>>>,
    /// Semaphore for locking against concurrent checkpoints
    pub j_checkpoint_mutex: Mutex<()>,
    /// Journal head: identifies the first unused block in the journal
    pub j_head: AtomicU64,
    /// Journal tail: identifies the oldest still-used block in the journal
    pub j_tail: AtomicU64,
    /// Journal free: how many free blocks are there in the journal
    pub j_free: AtomicU64,
    /// The block number of the first usable block in the journal
    pub j_first: u64,
    /// The block number one beyond the last usable block
    pub j_last: u64,
    /// The block number of the first fast commit block
    pub j_fc_first: u64,
    /// Number of fast commit blocks currently allocated
    pub j_fc_off: u64,
    /// The block number one beyond the last fast commit block
    pub j_fc_last: u64,
    /// Device where we store the journal
    pub j_dev: *mut core::ffi::c_void,
    /// Block size for the location where we store the journal
    pub j_blocksize: u32,
    /// Starting block offset into the device where we store the journal
    pub j_blk_offset: u64,
    /// Filesystem block device pointer (for bio I/O during commit)
    pub j_bio_device: *const crate::drivers::blkdev::GenDisk,
    /// Journal device name
    pub j_devname: [u8; 64],
    /// Device which holds the client fs
    pub j_fs_dev: *mut core::ffi::c_void,
    /// Total maximum capacity of the journal region on disk
    pub j_total_len: u32,
    /// Number of buffers reserved from the running transaction
    pub j_reserved_credits: AtomicI32,
    /// Sequence number of the oldest transaction in the log
    pub j_tail_sequence: AtomicU32,
    /// Sequence number of the next transaction to grant
    pub j_transaction_sequence: AtomicU32,
    /// Sequence number of the most recently committed transaction
    pub j_commit_sequence: AtomicU32,
    /// Sequence number of the most recent transaction wanting commit
    pub j_commit_request: AtomicU32,
    /// Journal uuid
    pub j_uuid: [u8; 16],
    /// Pointer to the current commit thread for this journal
    pub j_task: *mut core::ffi::c_void,
    /// Maximum number of metadata buffers in a single compound commit
    pub j_max_transaction_buffers: i32,
    /// Number of revoke records that fit in one descriptor block
    pub j_revoke_records_per_block: i32,
    /// Number of blocks each transaction needs for its own bookkeeping
    pub j_transaction_overhead_buffers: i32,
    /// Maximum transaction lifetime before we begin a commit
    pub j_commit_interval: u64,
    /// The revoke table
    pub j_revoke: Mutex<Option<Arc<Jbd2RevokeTable>>>,
    /// Alternate revoke tables
    pub j_revoke_table: [Mutex<Option<Arc<Jbd2RevokeTable>>>; 2],
    /// Array of buffer heads for commit
    pub j_wbuf: Mutex<Vec<*mut BufferHead>>,
    /// Array of fast commit buffer heads
    pub j_fc_wbuf: Mutex<Vec<*mut BufferHead>>,
    /// Size of j_wbuf array
    pub j_wbufsize: i32,
    /// Size of j_fc_wbuf array
    pub j_fc_wbufsize: i32,
    /// The pid of the last person to run a synchronous operation
    pub j_last_sync_writer: i32,
    /// Average commit time in nanoseconds
    pub j_average_commit_time: AtomicU64,
    /// Minimum batch time in microseconds
    pub j_min_batch_time: u32,
    /// Maximum batch time in microseconds
    pub j_max_batch_time: u32,
    /// Commit callback
    pub j_commit_callback: Option<fn(&Journal, &Transaction)>,
    /// Submit inode data buffers callback
    pub j_submit_inode_data_buffers: Option<fn(&Jbd2Inode) -> i32>,
    /// Finish inode data buffers callback
    pub j_finish_inode_data_buffers: Option<fn(&Jbd2Inode) -> i32>,
    /// Failed journal commit ID
    pub j_failed_commit: u32,
    /// Private data for fs
    pub j_private: *mut core::ffi::c_void,
    /// Precomputed journal UUID checksum for seeding other checksums
    pub j_csum_seed: u32,
    /// Fast commit cleanup callback
    pub j_fc_cleanup_callback: Option<fn(&Journal, bool, Tid)>,
    /// Fast commit replay callback
    pub j_fc_replay_callback: Option<fn(&Journal, *mut BufferHead, i32, i32, Tid) -> i32>,
    /// Bmap function
    pub j_bmap: Option<fn(&Journal, *mut u64) -> i32>,
}

impl Journal {
    /// Create a new journal
    pub fn new(block_size: u32, total_len: u32) -> Self {
        Self {
            j_flags: AtomicU32::new(0),
            j_errno: AtomicI32::new(0),
            j_abort_mutex: Mutex::new(()),
            j_sb_buffer: core::ptr::null_mut(),
            j_superblock: core::ptr::null_mut(),
            j_barrier_count: AtomicI32::new(0),
            j_barrier: Mutex::new(()),
            j_running_transaction: Mutex::new(None),
            j_committing_transaction: Mutex::new(None),
            j_checkpoint_transactions: Mutex::new(VecDeque::new()),
            j_checkpoint_mutex: Mutex::new(()),
            j_head: AtomicU64::new(0),
            j_tail: AtomicU64::new(0),
            j_free: AtomicU64::new(total_len as u64),
            j_first: 1,
            j_last: total_len as u64,
            j_fc_first: 0,
            j_fc_off: 0,
            j_fc_last: 0,
            j_dev: core::ptr::null_mut(),
            j_blocksize: block_size,
            j_blk_offset: 0,
            j_bio_device: core::ptr::null(),
            j_devname: [0; 64],
            j_fs_dev: core::ptr::null_mut(),
            j_total_len: total_len,
            j_reserved_credits: AtomicI32::new(0),
            j_tail_sequence: AtomicU32::new(0),
            j_transaction_sequence: AtomicU32::new(1),
            j_commit_sequence: AtomicU32::new(0),
            j_commit_request: AtomicU32::new(0),
            j_uuid: [0; 16],
            j_task: core::ptr::null_mut(),
            j_max_transaction_buffers: (total_len / 4) as i32,
            j_revoke_records_per_block: (block_size / 16) as i32,
            j_transaction_overhead_buffers: 1,
            j_commit_interval: JBD2_DEFAULT_MAX_COMMIT_AGE as u64 * 100, // in jiffies
            j_revoke: Mutex::new(None),
            j_revoke_table: [Mutex::new(None), Mutex::new(None)],
            j_wbuf: Mutex::new(Vec::new()),
            j_fc_wbuf: Mutex::new(Vec::new()),
            j_wbufsize: 0,
            j_fc_wbufsize: 0,
            j_last_sync_writer: 0,
            j_average_commit_time: AtomicU64::new(0),
            j_min_batch_time: 0,
            j_max_batch_time: 15000, // 15ms in microseconds
            j_commit_callback: None,
            j_submit_inode_data_buffers: None,
            j_finish_inode_data_buffers: None,
            j_failed_commit: 0,
            j_private: core::ptr::null_mut(),
            j_csum_seed: 0,
            j_fc_cleanup_callback: None,
            j_fc_replay_callback: None,
            j_bmap: None,
        }
    }

    /// Check if journal is aborted
    pub fn is_aborted(&self) -> bool {
        self.j_flags.load(Ordering::SeqCst) & JBD2_ABORT != 0
    }

    /// Abort the journal
    pub fn abort(&self, errno: i32) {
        self.j_errno.store(errno, Ordering::SeqCst);
        self.j_flags.fetch_or(JBD2_ABORT, Ordering::SeqCst);
    }

    /// Get free space in journal
    pub fn free_space(&self) -> u64 {
        self.j_free.load(Ordering::SeqCst)
    }

    /// Calculate tag size based on journal features
    pub fn tag_size(&self) -> usize {
        let has_64bit = self.has_64bit();
        let has_csum_v3 = self.has_csum_v3();
        journal_tag_size(has_64bit, has_csum_v3)
    }

    /// Check if journal has 64-bit feature
    pub fn has_64bit(&self) -> bool {
        unsafe {
            if self.j_superblock.is_null() {
                return false;
            }
            let sb = &*self.j_superblock;
            u32::from_be(sb.s_feature_incompat) & JBD2_FEATURE_INCOMPAT_64BIT != 0
        }
    }

    /// Check if journal has checksum v3 feature
    pub fn has_csum_v3(&self) -> bool {
        unsafe {
            if self.j_superblock.is_null() {
                return false;
            }
            let sb = &*self.j_superblock;
            u32::from_be(sb.s_feature_incompat) & JBD2_FEATURE_INCOMPAT_CSUM_V3 != 0
        }
    }

    /// Check if journal has checksum v2 feature
    pub fn has_csum_v2(&self) -> bool {
        unsafe {
            if self.j_superblock.is_null() {
                return false;
            }
            let sb = &*self.j_superblock;
            u32::from_be(sb.s_feature_incompat) & JBD2_FEATURE_INCOMPAT_CSUM_V2 != 0
        }
    }
}

// ============================================================================
// Journal functions
// ============================================================================

/// Start a new handle for a transaction
pub fn journal_start(journal: &Arc<Journal>, nblocks: i32) -> Handle {
    let handle = Handle::new(nblocks);

    // Get or create running transaction
    let _journal_guard = journal.j_barrier.lock();
    let mut running_txn = journal.j_running_transaction.lock();

    if running_txn.is_none() {
        // Create new transaction
        let tid = journal.j_transaction_sequence.fetch_add(1, Ordering::SeqCst);
        let mut txn = Transaction::new(journal.clone());
        txn.t_tid = tid;
        *running_txn = Some(Arc::new(txn));
    }

    // Increment transaction updates count
    if let Some(ref txn) = *running_txn {
        txn.t_updates.fetch_add(1, Ordering::SeqCst);
        txn.t_handle_count.fetch_add(1, Ordering::SeqCst);
    }

    handle
}

/// Stop a handle
pub fn journal_stop(handle: &mut Handle) -> Result<(), i32> {
    if handle.h_ref > 0 {
        handle.h_ref -= 1;
    }

    if handle.h_ref == 0 {
        // Decrement transaction updates count
        if let Some(ref txn) = handle.h_transaction {
            txn.t_updates.fetch_sub(1, Ordering::SeqCst);
        }
    }

    Ok(())
}

/// Extend a handle's credits
pub fn journal_extend(handle: &mut Handle, nblocks: i32) -> Result<(), i32> {
    handle.h_total_credits += nblocks;
    Ok(())
}
