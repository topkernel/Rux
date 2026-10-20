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

/// Filesystem information structure
///
/// Contains per-process filesystem context that can be shared
/// between threads when CLONE_FS is used.
pub struct FsStruct {
    /// Current working directory path
    cwd: RwSpinlock<alloc::vec::Vec<u8>>,

    /// Root directory path (for chroot)
    root: RwSpinlock<alloc::vec::Vec<u8>>,

    /// The cwd as a dentry (mount-aware — the dentry reached after
    /// follow_mount crossings). Linux keeps fs->pwd as a (vfsmount,
    /// dentry) pair and derives every path string from it on demand;
    /// the string above is Rux's lookup-prefix equivalent. Strings go
    /// STALE when mounts move (MS_MOVE): a cwd string captured as
    /// "/usr" keeps naming the empty dir after the /usr mount moved
    /// to "/" (the OH switch_root failure). The dentry follows the
    /// mount (its mountpoint back-edge is updated by the move), so it
    /// stays valid — getters below REGENERATE the string from the
    /// dentry when one is recorded.
    cwd_dentry: RwSpinlock<Option<Arc<Dentry>>>,

    /// The chroot jail root as a dentry (None = not chrooted / the
    /// global root). Same self-healing rationale as cwd_dentry.
    root_dentry: RwSpinlock<Option<Arc<Dentry>>>,

    /// put_old recorded by this process's last pivot_root(2) (jail-style
    /// pivot stacks no real mount — see sys_pivot_root). The post-pivot
    /// cleanup umount2(put_old | ".", MNT_DETACH) consumes it: Linux
    /// leaves the OLD root mounted at put_old and that umount drops it;
    /// here it just reports success for the matching path once.
    pivot_put_old: RwSpinlock<Option<alloc::vec::Vec<u8>>>,

    /// File creation mask
    umask: AtomicU32,
}

impl FsStruct {
    /// Create a new FsStruct with default values
    pub fn new() -> Self {
        Self {
            cwd: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            root: RwSpinlock::new(alloc::vec::Vec::from(&b"/"[..])),
            cwd_dentry: RwSpinlock::new(None),
            root_dentry: RwSpinlock::new(None),
            pivot_put_old: RwSpinlock::new(None),
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
        if let Some(d) = self.cwd_dentry.read().clone() {
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
        if let Some(d) = self.root_dentry.read().clone() {
            return d.build_path().into_bytes();
        }
        self.root.read().clone()
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
