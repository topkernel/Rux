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
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

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

/// debug gauges (read by the fsw PC2 line, same shape as neghit/negstale).
/// (perf-gap-hunt) inline fast-gate discipline (the irq_note precedent):
/// the counters only tick when fsw instrumentation has armed them —
/// steady-state lookups pay one Relaxed load instead of an RMW pair.
pub static HIT: AtomicU64 = AtomicU64::new(0);
pub static MISS: AtomicU64 = AtomicU64::new(0);
pub static INSERT: AtomicU64 = AtomicU64::new(0);
pub static INVAL: AtomicU64 = AtomicU64::new(0);
static GAUGES_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Arm the debug gauges (fsw boot).
pub fn arm_gauges() {
    GAUGES_ON.store(true, Ordering::Relaxed);
}

#[inline]
fn note_hit() {
    if GAUGES_ON.load(Ordering::Relaxed) {
        HIT.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
fn note_miss() {
    if GAUGES_ON.load(Ordering::Relaxed) {
        MISS.fetch_add(1, Ordering::Relaxed);
    }
}

// (perf-gap-hunt) miss-storm backoff: a workload sweeping MANY distinct
// paths with no reuse (build trees, pbench's 1000-file sweep) thrashes a
// direct-mapped table — every lookup pays the miss probe and every walk
// pays the insert (String alloc + slot overwrite). When a measurement
// window comes back >75% miss, the memo goes pass-through (one Relaxed
// load + decrement per lookup, inserts skipped) for BACKOFF_LOOKUPS
// calls, then probes again. Hit-dominated shapes never arm it; the
// extra hot-path cost is one fetch_add outside any lock.
const WINDOW_LOOKUPS: u64 = 512;
const WINDOW_MISS_FRAC: u64 = 3; // misses/window > 3/4 => storm
const BACKOFF_LOOKUPS: i64 = 8192;
static LOOKUPS: AtomicU64 = AtomicU64::new(0);
static WINDOW_MISSES: AtomicU64 = AtomicU64::new(0);
static BACKOFF: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

/// True while the memo is pass-through (miss storm in effect).
#[inline]
fn backing_off() -> bool {
    let b = BACKOFF.load(Ordering::Relaxed);
    if b > 0 {
        // Consume one pass-through slot; multiple CPUs may overshoot
        // below zero — the load above treats that as inactive.
        BACKOFF.fetch_sub(1, Ordering::Relaxed);
        return true;
    }
    false
}

#[inline]
fn account(is_miss: bool) {
    if is_miss {
        WINDOW_MISSES.fetch_add(1, Ordering::Relaxed);
    }
    let n = LOOKUPS.fetch_add(1, Ordering::Relaxed) + 1;
    if n % WINDOW_LOOKUPS == 0 {
        let m = WINDOW_MISSES.swap(0, Ordering::Relaxed);
        if m * 4 > WINDOW_LOOKUPS * WINDOW_MISS_FRAC {
            BACKOFF.store(BACKOFF_LOOKUPS, Ordering::Relaxed);
        }
    }
}

/// (perf-gap-hunt v2) Plain (non-atomic) slot payload, read through the
/// slot's seqlock — the sync::seqlock / ChildMemoSlot UnsafeCell
/// discipline. All-inline: no heap dereference on the compare (the v1
/// String indirection is gone), no per-slot lock on the read side.
struct SlotData {
    /// Post-follow root dentry identity (compare-only raw). The owning
    /// Arc in the slot's `owner` pins it while any slot content still
    /// references the pointer, so ptr-reuse false positives are
    /// impossible (the v1 Entry.root pinning argument, unchanged).
    root_key: *const Dentry,
    /// Normalized path bytes, `path_len` valid.
    path_len: u32,
    path: [u8; MAX_PATH],
    /// (ancestor inode, dir_mut_gen at walk time), root first.
    chain: [(*const Inode, u64); MAX_CHAIN],
    chain_n: u32,
    /// Global mount generation at insert time.
    mount_gen: u64,
    /// Final dentry — cloned by readers under a WalkClaim; the owning
    /// Arc in `owner` (and, after an overwrite, the retire-park path)
    /// keeps it alive for claim holders.
    final_raw: *const Dentry,
}

/// The owning strong references for one slot. Only writers touch it
/// (under `wlock`); readers work from the raw pointers in `data`.
struct SlotOwner {
    root: Option<Arc<Dentry>>,
    final_dentry: Option<Arc<Dentry>>,
}

/// One memo slot. Readers: seqlock pair + plain field reads, no lock.
/// Writers: `wlock` serializes (inserts are the cold path), then the
/// RawSeqLock odd/even protocol publishes the payload.
#[repr(align(64))]
struct Slot {
    seq: crate::sync::seqlock::RawSeqLock,
    /// Serializes writers only; also owns the slot's strong references
    /// between overwrites.
    wlock: crate::sync::spinlock::Spinlock<SlotOwner>,
    data: core::cell::UnsafeCell<SlotData>,
}

// SAFETY: `data` is accessed only through the seqlock protocol (readers
// validate with read_begin/read_retry; writers hold `wlock` and flip the
// sequence odd/even around plain writes). `owner` is only touched under
// `wlock`. The raw pointers inside SlotData are dereferenced under the
// dentry reader-bank claim with the sequential root->leaf validation
// discipline (module doc).
unsafe impl Sync for Slot {}
unsafe impl Send for Slot {}

static TABLE: [Slot; SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const SLOT: Slot = Slot {
        seq: crate::sync::seqlock::RawSeqLock::new(),
        wlock: crate::sync::spinlock::Spinlock::new(SlotOwner {
            root: None,
            final_dentry: None,
        }),
        data: core::cell::UnsafeCell::new(SlotData {
            root_key: core::ptr::null(),
            path_len: 0,
            path: [0; MAX_PATH],
            chain: [(core::ptr::null(), 0); MAX_CHAIN],
            chain_n: 0,
            mount_gen: 0,
            final_raw: core::ptr::null(),
        }),
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
/// (perf-gap-hunt v2) Lock-free read side: one reader-bank claim + one
/// seqlock pair + plain field reads. The v1 per-slot irqsave lock is
/// gone; a racing insert just turns this lookup into a miss (the caller
/// walks and the walk re-inserts).
///
/// `cred` re-validates search permission per ancestor with the
/// caller's credentials — the walk's own per-component check, repeated
/// (hoisted once here; the walk re-fetches it per component). `None`
/// mirrors inode_permission's no-current-task rule (allowed).
#[inline]
pub fn lookup(
    root: &Arc<Dentry>,
    path: &str,
    cred: Option<&crate::process::task::Cred>,
) -> Option<(Arc<Dentry>, Arc<Inode>)> {
    if backing_off() {
        return None;
    }
    if path.len() > MAX_PATH {
        note_miss();
        account(true);
        return None;
    }
    let root_key: *const Dentry = Arc::as_ptr(root);
    let slot = &TABLE[hash(root_key, path)];
    // Reader claim: the validation derefs ancestor inodes and clones the
    // final dentry from its raw pointer; the claim keeps a
    // concurrently-retired object from draining mid-read (the dentry
    // reader-bank protocol the walk itself relies on).
    let claim = crate::fs::dentry::WalkClaim::acquire();
    let path_bytes = path.as_bytes();
    // Two attempts; a racing insert is the only source of a seq retry
    // and inserts are rare — a persistent mismatch becomes a miss and
    // the walk re-inserts.
    let mut attempt = 0;
    let hit = loop {
        let s1 = slot.seq.read_begin();
        if s1 & 1 == 0 {
            // SAFETY: seqlock-protected read (validated by the re-read
            // below; the sync::seqlock UnsafeCell discipline).
            let d = unsafe { &*slot.data.get() };
            let mut result: Option<(Arc<Dentry>, Arc<Inode>)> = None;
            if d.root_key == root_key
                && d.path_len as usize == path.len()
                && d.path[..path.len()] == *path_bytes
            {
                // Topology moved -> everything below is suspect.
                if d.mount_gen == MOUNT_GEN.load(Ordering::Acquire) {
                    // Root -> leaf: generation + search permission per
                    // ancestor.
                    let mut ok = true;
                    for &(ino_ptr, gen) in d.chain.iter().take(d.chain_n as usize) {
                        // SAFETY: sequential-validation discipline (see
                        // module doc): this inode is pinned by its
                        // parent's child map while the previous
                        // generation check passed, and the claim blocks
                        // a pending retire from freeing it under us.
                        let ino: &Inode = unsafe { &*ino_ptr };
                        if ino.dir_mut_gen_load() != gen {
                            ok = false;
                            break;
                        }
                        let search_ok = match cred {
                            Some(c) => crate::fs::permission::generic_permission(
                                ino.mode.bits() as u16,
                                ino.uid.load(Ordering::Relaxed),
                                ino.gid.load(Ordering::Relaxed),
                                crate::fs::permission::MAY_EXEC,
                                c,
                            ),
                            None => true,
                        };
                        if !search_ok {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        // Final dentry: clone under the claim AFTER the
                        // seq re-read below passes (an overwrite that
                        // raced us retires the old Arc through the
                        // claim-aware park path, so the raw pointer
                        // cannot be freed while we hold the claim).
                        let raw = d.final_raw;
                        if !raw.is_null()
                            && slot.seq.read_retry(s1) == false
                        {
                            // SAFETY: loaded from a memo slot under a
                            // held claim; see dentry_arc_from_raw.
                            let dentry =
                                unsafe { crate::fs::dentry::dentry_arc_from_raw(raw) };
                            if dentry.is_negative() {
                                // A negative final can only be a torn
                                // view (insert stores positives).
                                result = None;
                            } else {
                                let ino_ptr = dentry.inode_ptr_walk(&claim);
                                if ino_ptr.is_null() {
                                    result = None;
                                } else {
                                    // SAFETY: protected by the claim
                                    // (strong-count increment before
                                    // the claim drops).
                                    let inode = unsafe {
                                        crate::fs::dentry::inode_arc_from_raw(ino_ptr)
                                    };
                                    result = Some((dentry, inode));
                                }
                            }
                        }
                    }
                }
            }
            if result.is_some() || slot.seq.read_retry(s1) == false {
                break result;
            }
        }
        attempt += 1;
        if attempt >= 2 {
            break None;
        }
        core::hint::spin_loop();
    };
    drop(claim);
    if hit.is_some() {
        note_hit();
        account(false);
    } else {
        note_miss();
        account(true);
    }
    hit
}

/// Record a successful follow-shape walk. `chain` is the ancestor
/// (inode, dir_mut_gen) sequence in walk order, gens loaded under the
/// walk's own claim.
///
/// (perf-gap-hunt v2) Writer side: the per-slot `wlock` serializes
/// writers (inserts are the cold path — misses only), the RawSeqLock
/// odd/even pair publishes the payload to lock-free readers, and the
/// replaced final dentry's Arc retires through the claim-aware park
/// path so a reader mid-clone can never see it freed.
pub fn insert(
    root: &Arc<Dentry>,
    path: &str,
    final_dentry: Arc<Dentry>,
    chain: &[(*const Inode, u64)],
) {
    if path.len() > MAX_PATH || chain.is_empty() || chain.len() > MAX_CHAIN {
        return;
    }
    // Miss-storm backoff: populating the table during a sweep is the
    // expensive half of the thrash (alloc + overwrite per walk).
    if BACKOFF.load(Ordering::Relaxed) > 0 {
        return;
    }
    let root_key: *const Dentry = Arc::as_ptr(root);
    let slot = &TABLE[hash(root_key, path)];
    let old;
    {
        let mut owner = slot.wlock.lock();
        slot.seq.write_lock();
        // SAFETY: wlock held — writers serialize here; racing readers
        // are guarded by the seqlock pair.
        let d = unsafe { &mut *slot.data.get() };
        d.root_key = root_key;
        d.path_len = path.len() as u32;
        d.path[..path.len()].copy_from_slice(path.as_bytes());
        d.chain = [(core::ptr::null(), 0); MAX_CHAIN];
        d.chain[..chain.len()].copy_from_slice(chain);
        d.chain_n = chain.len() as u32;
        d.mount_gen = MOUNT_GEN.load(Ordering::Acquire);
        d.final_raw = Arc::as_ptr(&final_dentry);
        old = core::mem::replace(
            &mut *owner,
            SlotOwner {
                root: Some(root.clone()),
                final_dentry: Some(final_dentry),
            },
        );
        slot.seq.write_unlock();
    }
    // The replaced final dentry's Arc drops here. A concurrent reader
    // may have loaded its raw pointer from the previous slot content and
    // still hold the claim — park it through the drain-aware retire path
    // instead of a bare drop (the child-map mutator discipline).
    if let Some(oldf) = old.final_dentry {
        crate::fs::dentry::retire_arc_after_claim_readers(oldf);
    }
    // The replaced root Arc can drop plainly: readers never dereference
    // the stored root (they compare raw pointers against their own
    // pinned Arc), and any slot still keyed by that pointer holds its
    // own pin via its owner Arc.
    drop(old.root);
    INSERT.fetch_add(1, Ordering::Relaxed);
}
