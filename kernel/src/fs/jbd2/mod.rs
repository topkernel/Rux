//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! JBD2 (Journal Block Device 2) implementation
//!
//! Based on Linux kernel fs/jbd2/
//!
//! The JBD2 layer provides journaling for block devices. It is used by
//! ext4 filesystem for data integrity and crash recovery.

pub mod types;
pub mod journal;
pub mod transaction;
pub mod commit;
pub mod recovery;
pub mod checkpoint;
pub mod revoke;

// Re-export main types
pub use types::*;
pub use journal::{
    Journal, Handle, Transaction, TransactionState,
    Jbd2Inode, JournalHead, BufferHead, ListHead,
    Jbd2RevokeTable, Jbd2RevokeRecord,
    TransactionStats, TransactionRunStats, TransactionChpStats,
    Tid,
    // Constants
    JBD2_ABORT, JBD2_ACK_ERR, JBD2_FLUSHED, JBD2_LOADED,
    JBD2_UPDATE_SYNC, JBD2_SYNC, JBD2_BROKEN,
    BJ_None, BJ_SyncData, BJ_Metadata, BJ_Forget, BJ_IO,
    BJ_Shadow, BJ_LogCtl, BJ_Reserved, BJ_Locked,
    JI_COMMIT_RUNNING, JI_WRITE_DATA, JI_WAIT_DATA,
    // Functions
    journal_start, journal_stop, journal_extend,
};
pub use transaction::{
    jbd2_journal_start, jbd2_journal_stop, jbd2_journal_extend,
    jbd2_journal_get_write_access, jbd2_journal_get_create_access,
    jbd2_journal_dirty_metadata, jbd2_journal_forget,
    is_handle_aborted,
};
