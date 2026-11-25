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
use crate::fs::dentry::Dentry;
use alloc::sync::Arc;

// The type definitions this file declared live in fs/types.rs; every impl
// and function that uses them stays in this file. Re-exported so the
// existing `fs::fs_struct::…` paths are unchanged.
pub use super::types::FsStruct;

impl FsStruct {
    /// Create a new FsStruct with default values
    pub fn new() -> Self {
        Self {
            cwd: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            root: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            cwd_dentry: RwSpinlock::new(None),
            root_dentry: RwSpinlock::new(None),
            pivot_put_old: RwSpinlock::new(None),
            root_is_pivot: RwSpinlock::new(false),
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

    /// Get current working directory (global-namespace path).
    ///
    /// When a cwd dentry is recorded it is the source of truth: the
    /// string is rebuilt from the dentry tree so it stays correct
    /// across mount moves (MS_MOVE/pivot-style flows rewrite
    /// mountpoint back-edges, and the rebuild picks the new location
    /// up — Linux derives paths the same way, from (vfsmnt, dentry)).
    pub fn get_cwd(&self) -> alloc::vec::Vec<u8> {
        // preempt-hunt: bind the clone BEFORE the if-let. The scrutinee's
        // read guard is a temporary that lives through the whole block,
        // so build_path() (a walk to the dentry root) used to run with
        // the rwlock guard — and its preempt-disable — held. fs_struct
        // sites were the #1 >2s incumbent family on the ph2 OH boot
        // (46 tick-heal origins at this file's sites).
        let d = self.cwd_dentry.read().clone();
        if let Some(d) = d {
            return d.build_path().into_bytes();
        }
        self.cwd.read().clone()
    }

    /// Set current working directory (string only — clears any
    /// recorded cwd dentry, the string becomes the truth again).
    pub fn set_cwd(&self, path: &[u8]) {
        *self.cwd_dentry.write() = None;
        *self.cwd.write() = alloc::vec::Vec::from(path);
    }

    /// Set the cwd from a resolved lookup: records BOTH the physical
    /// (mount-crossing) path string and the dentry it was built from.
    /// `dentry` None falls back to string-only.
    pub fn set_cwd_with_dentry(&self, path: &[u8], dentry: Option<Arc<Dentry>>) {
        *self.cwd_dentry.write() = dentry;
        *self.cwd.write() = alloc::vec::Vec::from(path);
    }

    /// The recorded cwd dentry, if any (mount-aware: the dentry after
    /// follow_mount crossings).
    pub fn cwd_dentry(&self) -> Option<Arc<Dentry>> {
        self.cwd_dentry.read().clone()
    }

    /// Get cwd as slice (for compatibility)
    pub fn cwd_slice(&self) -> alloc::boxed::Box<[u8]> {
        self.get_cwd().into_boxed_slice()
    }

    // ==================== Root Operations ====================

    /// Get root directory path (real-namespace normalized path; "/"
    /// when not chrooted). Self-heals from the root dentry when one
    /// is recorded (same rationale as get_cwd()).
    pub fn get_root(&self) -> alloc::vec::Vec<u8> {
        // Same if-let temporary-extension fix as get_cwd (preempt-hunt):
        // build_path() runs without the read guard.
        let d = self.root_dentry.read().clone();
        if let Some(d) = d {
            return d.build_path().into_bytes();
        }
        self.root.read().clone()
    }

    /// Whether the task is chrooted somewhere other than the global
    /// root — the ALLOCATION-FREE check path_lookup's fast path needs
    /// (get_root() allocates and, with a recorded root dentry, walks
    /// the parent chain; calling it per lookup cost ~5 heap objects
    /// per open). No allocs: read-lock the two slots and compare.
    pub fn root_is_slash(&self) -> bool {
        if self.root_dentry.read().is_some() {
            return false;
        }
        let r = self.root.read();
        r.as_slice() == b"/"
    }

    /// Set root directory path (chroot), string only — clears any
    /// recorded root dentry.
    pub fn set_root(&self, path: &[u8]) {
        *self.root_dentry.write() = None;
        *self.root.write() = alloc::vec::Vec::from(path);
    }

    /// Set the chroot root from a resolved lookup: records the jail
    /// root dentry (used by getcwd to compute root-relative paths and
    /// to detect a cwd outside the jail) alongside the prefix string
    /// that path_lookup interprets absolute paths against.
    pub fn set_root_with_dentry(&self, path: &[u8], dentry: Option<Arc<Dentry>>) {
        *self.root_dentry.write() = dentry;
        *self.root.write() = alloc::vec::Vec::from(path);
    }

    /// The recorded chroot root dentry, if any.
    pub fn root_dentry(&self) -> Option<Arc<Dentry>> {
        self.root_dentry.read().clone()
    }

    /// Record the put_old of this process's last pivot_root (consumed by
    /// the matching umount2 — see the field doc).
    pub fn set_pivot_put_old(&self, path: &[u8]) {
        *self.pivot_put_old.write() = Some(alloc::vec::Vec::from(path));
    }

    /// The recorded pivot put_old, if any.
    pub fn pivot_put_old(&self) -> Option<alloc::vec::Vec<u8>> {
        self.pivot_put_old.read().clone()
    }

    /// Clear the pivot marker (consumed / context reset).
    pub fn clear_pivot_put_old(&self) {
        *self.pivot_put_old.write() = None;
    }

    /// Whether the current root came from pivot_root(2)/setns (see the
    /// field doc) — drives setns(CLONE_NEWNS) root re-anchoring.
    pub fn root_is_pivot(&self) -> bool {
        *self.root_is_pivot.read()
    }

    /// Set/clear the pivot-root marker (sys_pivot_root, ns::setns).
    pub fn set_root_is_pivot(&self, v: bool) {
        *self.root_is_pivot.write() = v;
    }

    /// Copy the full filesystem context (cwd/root strings, cwd/root
    /// dentries) from `other` — fork()'s non-CLONE_FS duplicate. The
    /// old fork path copied only the cwd STRING, so a forked child of
    /// a chrooted task silently escaped the jail, and children lost
    /// the mount-aware cwd.
    pub fn copy_fs_from(&self, other: &FsStruct) {
        let cwd = other.cwd.read().clone();
        *self.cwd.write() = cwd;
        let root = other.root.read().clone();
        *self.root.write() = root;
        let cd = other.cwd_dentry.read().clone();
        *self.cwd_dentry.write() = cd;
        let rd = other.root_dentry.read().clone();
        *self.root_dentry.write() = rd;
        let rp = *other.root_is_pivot.read();
        *self.root_is_pivot.write() = rp;
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
