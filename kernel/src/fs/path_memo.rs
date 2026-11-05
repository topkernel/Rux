//! Full-path -> dentry memoization for the common lookup shape.
//!
//! The per-component dentry cache already makes each hop cheap, but a
//! 4-component walk still pays component glue (claim/refresh, iterator,
//! child-map probe) on every hop. This memo collapses the whole walk
//! into: hash + one slot lock + string compare + per-ancestor
//! generation validation — the same lazy-refresh discipline the
//! negative-entry memo uses (fs/vfs.rs, `memo_negative`), lifted from
//! one component to the whole resolved path.
//!
//! Soundness skeleton (mirrors the walk, never weakens it):
//! * Key: (post-follow root dentry identity, normalized path bytes,
//!   follow-flags shape). Chrooted/relative/dirty paths never touch it.
//! * Entry: final dentry Arc (pins the final dentry + its inode) plus,
//!   per walked ANCESTOR directory inode, the dir_mut_gen observed at
//!   walk time, plus the global mount generation.
//! * Validation (per hit, under a walk reader claim, in root->leaf
//!   order): each ancestor inode's dir_mut_gen must be unchanged. Any
//!   create/unlink/rename through an ancestor bumps its generation
//!   (VFS mutation sites, `dir_mut_gen_bump`); a rename of the final
//!   entry bumps its parent; mount/umount anywhere bumps the global
//!   mount generation. Sequential validation is use-after-free safe:
//!   the root is pinned by VFS_STATE and each ancestor stays pinned by
//!   its parent's child map until an unlink bumps the PARENT gen —
//!   which the preceding validation step observes first.
//! * Search permission (MAY_EXEC) is RE-CHECKED per hit on every
//!   ancestor with the CURRENT task's creds — permission is never
//!   memoized.
//! * Only ext4-lookup ancestors are memoable (same gate as the
//!   negative memo): dynamic filesystems (procfs & friends) change
//!   content without VFS-visible directory mutation, so their gens
//!   would never move and a stale positive could hide new files.
//! * Lookups that must not follow a final symlink (LOOKUP_NOFOLLOW /
//!   LOOKUP_NO_SYMLINKS) skip the memo entirely, and walks that
//!   followed any symlink do not insert — a stored entry always means
//!   "the follow-shape resolution of this path".
//!
//! Bounded: SLOTS direct-mapped entries, overwrite on collision (a
//! collision costs a miss, never correctness).

use crate::fs::dentry::Dentry;
use crate::fs::inode::Inode;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Max ancestors recorded (path depth cap for memoization).
pub const MAX_CHAIN: usize = 8;
/// Max memoized path length in bytes.
const MAX_PATH: usize = 128;
/// Direct-mapped slot count (power of two).
const SLOTS: usize = 256;
const SLOT_MASK: usize = SLOTS - 1;

/// Global VFS topology generation: bumped by every mount attach /
/// detach / flag flip (Dentry::set_mount / clear_mount / set_mount_flag).
pub static MOUNT_GEN: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn mount_gen_bump() {
    MOUNT_GEN.fetch_add(1, Ordering::AcqRel);
}

/// DFX gauges (read by the fsw PC2 line, same shape as neghit/negstale).
pub static HIT: AtomicUsize = AtomicUsize::new(0);
pub static MISS: AtomicUsize = AtomicUsize::new(0);
pub static INSERT: AtomicUsize = AtomicUsize::new(0);
pub static INVAL: AtomicUsize = AtomicUsize::new(0);

struct Entry {
    /// Pins the root identity (ptr-reuse false positives impossible).
    root: Arc<Dentry>,
    path: String,
    final_dentry: Arc<Dentry>,
    /// (ancestor inode, dir_mut_gen at walk time), root first.
    chain: [(*const Inode, u64); MAX_CHAIN],
    chain_n: usize,
    mount_gen: u64,
}

// SAFETY: Entry is only reachable through its slot's spinlock; the raw
// ancestor pointers are dereferenced exclusively under that lock and
// the dentry reader-bank claim (see lookup), with the sequential
// root->leaf validation discipline keeping them alive (module doc).
unsafe impl Send for Entry {}
unsafe impl Sync for Entry {}

struct Slot {
    lock: crate::sync::spinlock::Spinlock<Option<Entry>>,
}

static TABLE: [Slot; SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const SLOT: Slot = Slot {
        lock: crate::sync::spinlock::Spinlock::new(None),
    };
    [SLOT; SLOTS]
};

#[inline]
fn hash(root: *const Dentry, path: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325 ^ (root as u64);
    for b in path.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h as usize) & SLOT_MASK
}

/// Try the memo. Returns the resolved (dentry, inode) for the
/// follow-shape lookup of `path` from `root`, or None (caller walks).
///
/// `may_exec_of` re-validates search permission per ancestor with the
/// caller's credentials — the walk's own per-component check, repeated.
#[inline]
pub fn lookup(
    root: &Arc<Dentry>,
    path: &str,
    may_exec_of: impl Fn(&Inode) -> bool,
) -> Option<(Arc<Dentry>, Arc<Inode>)> {
    if path.len() > MAX_PATH {
        MISS.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let root_key: *const Dentry = Arc::as_ptr(root);
    let slot = &TABLE[hash(root_key, path)];
    // Reader claim: the validation derefs ancestor inodes; the claim
    // keeps a concurrently-retired ancestor from draining mid-read
    // (the dentry reader-bank protocol the walk itself relies on).
    let claim = crate::fs::dentry::WalkClaim::acquire();
    let guard = slot.lock.lock_irqsave();
    let hit = (|| -> Option<(Arc<Dentry>, Arc<Inode>)> {
        let entry = guard.as_ref()?;
        if Arc::as_ptr(&entry.root) != root_key || entry.path != path {
            return None;
        }
        // Topology moved -> everything below is suspect.
        if entry.mount_gen != MOUNT_GEN.load(Ordering::Acquire) {
            return None;
        }
        // Root -> leaf: generation + search permission per ancestor.
        for &(ino_ptr, gen) in entry.chain.iter().take(entry.chain_n) {
            // SAFETY: sequential-validation discipline (see module
            // doc): this inode is pinned by its parent's child map
            // while the previous generation check passed, and the
            // claim blocks a pending retire from freeing it under us.
            let ino: &Inode = unsafe { &*ino_ptr };
            if ino.dir_mut_gen_load() != gen || !may_exec_of(ino) {
                return None;
            }
        }
        // Final dentry: pinned by the entry's own Arc; fetch its inode
        // under the claim (same protocol as the walk tail).
        let d = &entry.final_dentry;
        if d.is_negative() {
            return None;
        }
        let ino_ptr = d.inode_ptr_walk(&claim);
        if ino_ptr.is_null() {
            return None;
        }
        // SAFETY: protected by the claim (strong-count increment
        // before the claim drops).
        let inode = unsafe { crate::fs::dentry::inode_arc_from_raw(ino_ptr) };
        Some((d.clone(), inode))
    })();
    drop(guard);
    drop(claim);
    if hit.is_some() {
        HIT.fetch_add(1, Ordering::Relaxed);
    } else {
        MISS.fetch_add(1, Ordering::Relaxed);
    }
    hit
}

/// Record a successful follow-shape walk. `chain` is the ancestor
/// (inode, dir_mut_gen) sequence in walk order, gens loaded under the
/// walk's own claim.
pub fn insert(
    root: &Arc<Dentry>,
    path: &str,
    final_dentry: Arc<Dentry>,
    chain: &[(*const Inode, u64)],
) {
    if path.len() > MAX_PATH || chain.is_empty() || chain.len() > MAX_CHAIN {
        return;
    }
    let root_key: *const Dentry = Arc::as_ptr(root);
    let slot = &TABLE[hash(root_key, path)];
    let mut e = Entry {
        root: root.clone(),
        path: String::from(path),
        final_dentry,
        chain: [(core::ptr::null(), 0); MAX_CHAIN],
        chain_n: chain.len(),
        mount_gen: MOUNT_GEN.load(Ordering::Acquire),
    };
    e.chain[..chain.len()].copy_from_slice(chain);
    let mut guard = slot.lock.lock_irqsave();
    *guard = Some(e);
    drop(guard);
    INSERT.fetch_add(1, Ordering::Relaxed);
}
