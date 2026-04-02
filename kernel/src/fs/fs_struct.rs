//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Filesystem information structure (fs_struct)
//!
//! This module implements the fs_struct abstraction for storing
//! per-process filesystem context (cwd, root, umask).
//!
//! When CLONE_FS is used, multiple threads share the same fs_struct.

extern crate alloc;

use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::rwlock::RwSpinlock;

/// Filesystem information structure
///
/// Contains per-process filesystem context that can be shared
/// between threads when CLONE_FS is used.
pub struct FsStruct {
    /// Current working directory path
    cwd: RwSpinlock<alloc::vec::Vec<u8>>,

    /// Root directory path (for chroot)
    root: RwSpinlock<alloc::vec::Vec<u8>>,

    /// File creation mask
    umask: AtomicU32,
}

impl FsStruct {
    /// Create a new FsStruct with default values
    pub fn new() -> Self {
        Self {
            cwd: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            root: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            umask: AtomicU32::new(0o022),  // Default umask
        }
    }

    /// Create a new FsStruct with specified cwd
    pub fn with_cwd(cwd: &[u8]) -> Self {
        let mut fs = Self::new();
        *fs.cwd.write() = alloc::vec::Vec::from(cwd);
        fs
    }

    // ==================== CWD Operations ====================

    /// Get current working directory
    pub fn get_cwd(&self) -> alloc::vec::Vec<u8> {
        self.cwd.read().clone()
    }

    /// Set current working directory
    pub fn set_cwd(&self, path: &[u8]) {
        *self.cwd.write() = alloc::vec::Vec::from(path);
    }

    /// Get cwd as slice (for compatibility)
    pub fn cwd_slice(&self) -> alloc::boxed::Box<[u8]> {
        let guard = self.cwd.read();
        guard.clone().into_boxed_slice()
    }

    // ==================== Root Operations ====================

    /// Get root directory
    pub fn get_root(&self) -> alloc::vec::Vec<u8> {
        self.root.read().clone()
    }

    /// Set root directory (chroot)
    pub fn set_root(&self, path: &[u8]) {
        *self.root.write() = alloc::vec::Vec::from(path);
    }

    // ==================== Umask Operations ====================

    /// Get current umask
    pub fn get_umask(&self) -> u32 {
        self.umask.load(Ordering::Acquire)
    }

    /// Set umask and return old value
    pub fn set_umask(&self, mask: u32) -> u32 {
        self.umask.swap(mask & 0o777, Ordering::AcqRel)
    }

    /// Apply umask to mode
    pub fn apply_umask(&self, mode: u32) -> u32 {
        mode & !self.umask.load(Ordering::Acquire)
    }
}

impl Default for FsStruct {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for FsStruct {}
unsafe impl Sync for FsStruct {}
