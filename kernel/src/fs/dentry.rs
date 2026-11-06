//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Directory Entry (Dentry) Management
//!
//!
//! Core concepts:
//! - `struct dentry`: Directory entry, representing an entry in a directory
//! - `dcache`: Directory entry cache, speeds up path lookup
//! - `LRU`: Least Recently Used eviction policy

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicPtr, AtomicUsize, Ordering};
use crate::fs::inode::Inode;
use crate::fs::mount::MntFlags;

/// Dentry state flags
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(u32);

impl DentryFlags {
    /// Directory entry not connected to dcache
    pub const DCACHE_UNHASHED: u32 = 0x00000001;
    /// Directory entry connected to dcache
    pub const DCACHE_HASHED: u32 = 0x00000002;
    /// Directory entry in use
    pub const DCACHE_REFERENCED: u32 = 0x00000010;
    /// Directory entry deleted
    pub const DCACHE_DENTRY_KILL: u32 = 0x00000040;

    pub fn new(flags: u32) -> Self {
        Self(flags)
    }

    pub fn is_hashed(&self) -> bool {
        (self.0 & Self::DCACHE_HASHED) != 0
    }

    pub fn is_unhashed(&self) -> bool {
        (self.0 & Self::DCACHE_UNHASHED) != 0
    }

    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// Dentry state
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    /// Dentry unused
    DUnhashed,
    /// Dentry used
    DHashed,
    /// Dentry being deleted
    DKill,
}

/// VFS mount descriptor — attached to a dentry that is a mount point.
///
/// When `follow_mount()` encounters a dentry with a `VfsMountInternal`,
/// it transparently switches to `root` (the mounted filesystem's root dentry).
pub struct VfsMountInternal {
    /// Root dentry of the mounted filesystem
    pub root: Arc<Dentry>,
    /// Mount flags (interior-mutable: mount(2) with MS_REMOUNT updates them
    /// in place while concurrent lookups read them on other CPUs)
    pub flags: Spinlock<MntFlags>,
    /// sc181: global attach sequence (monotonic). The MS_REC clone scan
    /// (fs::vfs::collect_subtree_mounts) uses it to emit the clone set in
    /// attach order — identical to the old registry-order linear scan —
    /// so stacked clones always attach parents before children.
    pub seq: u64,
}

/// sc181: mount attach-sequence allocator (see VfsMountInternal::seq).
static MOUNT_SEQ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

impl VfsMountInternal {
    pub fn new(root: Arc<Dentry>, flags: MntFlags) -> Self {
        Self {
            root,
            flags: Spinlock::new(flags),
            seq: MOUNT_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        }
    }
}

unsafe impl Send for VfsMountInternal {}
unsafe impl Sync for VfsMountInternal {}

// ============================================================================
// Reader banks + deferred retirement (gap-fs lane)
// ============================================================================
//
// The path walk's per-component costs were dominated by lock pairs:
// `children` (BTreeMap under a spinlock) and `inode` (Option<Arc> under a
// spinlock) — measured ~39 ns + ~20 ns per walked component under the fsw
// pathcost probe, with the line bouncing between the two vCPUs of the
// lmbench rig. The protocol below (the FdTable leg-52 lever-B pattern,
// generalized) lets the walk read both lock-free:
//
// READER: acquire a WalkClaim (a SeqCst increment of THIS CPU's reader
// bank — or, for a cpu id outside the bank range, the DENTRY_SLOW_READ
// irqsave lock), then load a memo slot's child pointer / the inode
// pointer mirror and, when an owned reference is needed, increment the
// target's strong count BEFORE releasing the claim.
//
// WRITER: every mutation of the child map or the inode slot bumps the
// dentry's `children_ver` (memo slots stamped with an older version are
// then ignored) and parks any Arc it removes in the RETIRED lists
// instead of dropping it in place.
//
// DRAIN: `dentry_drain_if_quiescent` reclaims the retired Arcs only
// after a SeqCst fence and a scan that observes EVERY bank at zero (plus
// DENTRY_SLOW_READ held across the take). A reader can therefore only
// load a pointer whose reference is provably alive: either the map /
// inode slot still publishes it, or the removing writer's drain is
// waiting for THIS reader's bank to return to zero before its drop. The
// reader's SeqCst claim-before-load and the drain's SeqCst-fence-then-
// scan close the store-load ordering hole (same argument as FdTable's
// get_file / drain_retired pair).
//
// Unlike FdTable (whose sections are irq-off and bounded), a WalkClaim
// is PREEMPTIBLE and held for the dcache-hot span of one path walk: a
// walk never holds it across an operation that can sleep (fs lookup on
// a dcache miss, symlink read/follow, mount descriptor cloning) — those
// arms materialize owned Arcs and drop the claim first. A task
// preempted mid-claim only delays reclamation: the drain has a spin
// budget and defers to the next mutation (DENTRY_DRAIN_DEFERS counts
// the gives-up; correctness is never affected).

/// Number of per-CPU reader banks. Kernel.toml's SMP ceiling is 4; 16
/// leaves generous headroom. A cpu id at or above this bound degrades to
/// the DENTRY_SLOW_READ lock (held for the claim's span — unreachable
/// with any current config, but kept correct rather than assumed away;
/// the FdTable r8final discipline).
const DENTRY_READER_BANKS: usize = 16;

/// One per-CPU reader-nesting counter, padded to its own cacheline so
/// the CPUs' counter RMWs never share a line (FdTable discipline).
#[repr(align(64))]
struct DentryReaderBank(AtomicUsize);

static DENTRY_BANKS: [DentryReaderBank; DENTRY_READER_BANKS] = [const {
    DentryReaderBank(AtomicUsize::new(0))
}; DENTRY_READER_BANKS];

/// Slow-path exclusion for readers whose cpu id falls outside the bank
/// range; drains hold it across the drop of retired Arcs.
static DENTRY_SLOW_READ: Spinlock<()> = Spinlock::new(());

/// Dentries removed from a parent's child map (remove_child /
/// add_child displacement / clear / rehome) whose last strong reference
/// may still be racing a banked reader — parked until a drain observes
/// every reader bank at zero.
static DENTRY_RET_DENTRY: Spinlock<Vec<Arc<Dentry>>> = Spinlock::new(Vec::new());

/// Inodes swapped out of a dentry's inode slot (set_inode / clear_inode)
/// — same parking discipline.
static DENTRY_RET_INODE: Spinlock<Vec<Arc<Inode>>> = Spinlock::new(Vec::new());

/// Drain give-up counter (debug observability, FdTable discipline): a
/// drain that could not observe all banks at zero within its spin budget
/// defers the retired drop to the next writer.
pub static DENTRY_DRAIN_DEFERS: AtomicUsize = AtomicUsize::new(0);

/// Walk-scoped reader claim. `acquire()` bumps this CPU's bank (SeqCst —
/// the claim is globally visible before any subsequent pointer load
/// executes); Drop decrements it (Release). The value is Copy-able
/// evidence of holding the claim: methods taking `&WalkClaim` may load
/// raw pointers that are only guaranteed alive while some claim is held
/// on the calling CPU.
pub struct WalkClaim {
    /// Bank index claimed, or usize::MAX for the slow-path lock (whose
    /// guard we keep alive in `slow`).
    cpu: usize,
    slow: Option<crate::sync::spinlock::SpinlockIrqGuard<'static, ()>>,
}

impl WalkClaim {
    /// Claim this CPU's reader bank (or the slow-path lock for an
    /// out-of-range cpu id).
    pub fn acquire() -> Self {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < DENTRY_READER_BANKS {
            DENTRY_BANKS[cpu].0.fetch_add(1, Ordering::SeqCst);
            WalkClaim { cpu, slow: None }
        } else {
            // SAFETY of holding this lock for the claim's span: drains
            // take it across the retired drop, so a raw pointer loaded
            // under it cannot be freed mid-read. Unreachable with the
            // current SMP configs (banks sized above the ceiling).
            let g = DENTRY_SLOW_READ.lock_irqsave();
            WalkClaim { cpu: usize::MAX, slow: Some(g) }
        }
    }
}

impl Drop for WalkClaim {
    fn drop(&mut self) {
        if self.cpu != usize::MAX {
            DENTRY_BANKS[self.cpu].0.fetch_sub(1, Ordering::Release);
        } else {
            self.slow.take(); // releases the slow-path lock
        }
    }
}

/// Park a detached dentry until a reader-quiescent drain (see the
/// protocol comment above). Called by the child-map mutators with the
/// removed Arc in hand.
fn retire_dentry_arc(a: Arc<Dentry>) {
    DENTRY_RET_DENTRY.lock_irqsave().push(a);
}

/// (perf-gap-hunt) Retire an Arc that claim-holding readers may still be
/// reading through a raw pointer loaded from the full-path memo slots
/// (fs/path_memo.rs): park it and drop it once every reader bank is
/// quiescent — the same discipline the child-map mutators use for
/// removed children.
pub(crate) fn retire_arc_after_claim_readers(a: Arc<Dentry>) {
    retire_dentry_arc(a);
    dentry_drain_if_quiescent(true);
}

/// Park a swapped-out inode (set_inode / clear_inode).
fn retire_inode_arc(a: Arc<Inode>) {
    DENTRY_RET_INODE.lock_irqsave().push(a);
}

/// Drop retired dentries/inodes once no reader can be mid-read.
///
/// Quiescence: every bank reads 0 (each scan load Acquire — reading 0
/// synchronizes with that reader's Release decrement, so its
/// strong-count increment has completed) AND the slow-path lock is held
/// across the take. Readers that START after a successful scan cannot
/// load these Arcs: the map/slot publications preceded the park, the
/// parks precede this drain's SeqCst fence, and a fresh reader's pointer
/// load follows its own SeqCst bank claim.
///
/// `parked`: the caller just pushed entries (skip the emptiness
/// pre-check). Called by the mutators after every retire and (cheaply)
/// when their retired lists have grown past the drain threshold.
pub fn dentry_drain_if_quiescent(parked: bool) {
    if !parked {
        // Cheap emptiness check first: most lookups/mutations park
        // nothing and most drains find a previous drain already took
        // the vecs.
        let d = DENTRY_RET_DENTRY.lock_irqsave();
        if d.is_empty() {
            let i = DENTRY_RET_INODE.lock_irqsave();
            if i.is_empty() {
                return;
            }
        }
    }
    let mut scans: usize = 0;
    loop {
        // The fence orders the earlier retire/park stores before the
        // bank reads below (the reader side of the handshake is its
        // SeqCst bank claim before its pointer load).
        core::sync::atomic::fence(Ordering::SeqCst);
        let mut all_zero = true;
        for b in DENTRY_BANKS.iter() {
            if b.0.load(Ordering::Acquire) != 0 {
                all_zero = false;
                break;
            }
        }
        if all_zero {
            break;
        }
        scans += 1;
        if scans >= 64 {
            // Defer, never fail: the next writer retries. Walk claims
            // are preemptible, so a deferral is expected whenever a
            // hot-walk CPU is mid-path — bounded by mutation bursts.
            DENTRY_DRAIN_DEFERS.fetch_add(1, Ordering::Relaxed);
            return;
        }
        core::hint::spin_loop();
    }
    {
        let _slow = DENTRY_SLOW_READ.lock_irqsave();
        let taken_d = core::mem::take(&mut *DENTRY_RET_DENTRY.lock_irqsave());
        let taken_i = core::mem::take(&mut *DENTRY_RET_INODE.lock_irqsave());
        // Drops run inside the slow-lock scope: any cascade (a dentry's
        // child map / inode slot releasing further Arcs) cannot race a
        // banked reader, which by the scan above cannot exist now.
        drop(taken_d);
        drop(taken_i);
    }
}

/// Increment a dentry's strong count for a pointer loaded under a
/// reader claim (the claim blocks the drain that would otherwise free
/// it) and return an owned Arc.
///
/// # Safety
/// `p` must have been loaded from a dentry child memo slot / inode
/// pointer mirror while a WalkClaim is held by the caller, and `p` must
/// be non-null.
pub unsafe fn dentry_arc_from_raw(p: *const Dentry) -> Arc<Dentry> {
    Arc::increment_strong_count(p);
    Arc::from_raw(p)
}

/// Same as [`dentry_arc_from_raw`] for inodes (the inode pointer mirror).
///
/// # Safety
/// See [`dentry_arc_from_raw`]; `p` non-null, loaded under a claim.
pub unsafe fn inode_arc_from_raw(p: *const Inode) -> Arc<Inode> {
    Arc::increment_strong_count(p);
    Arc::from_raw(p)
}

/// FNV-1a over the component bytes — the memo slot key.
#[inline]
fn child_name_hash(name: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for &b in name {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Inline name capacity of a memo slot (24 bytes covers the hot
/// component shapes; longer names take the locked path).
const CHILD_MEMO_NAME_MAX: usize = 24;
/// Memo slots per dentry (index = hash & mask).
const CHILD_MEMO_SLOTS: usize = 4;
const CHILD_MEMO_MASK: usize = CHILD_MEMO_SLOTS - 1;

/// Plain (non-atomic) slot payload — read/written through the enclosing
/// slot's seqlock (the sync::seqlock UnsafeCell discipline).
struct ChildMemoData {
    /// `children_ver` at fill time — the slot is valid only while it
    /// equals the dentry's current version (any map mutation bumped it).
    ver: u64,
    /// FNV-1a of the name (fast reject before the byte compare).
    hash: u64,
    /// Name bytes, `name_len` valid.
    name: [u8; CHILD_MEMO_NAME_MAX],
    name_len: u8,
    /// The child (raw — the child map / a pending retire keeps it alive;
    /// see the reader-bank protocol).
    child: *const Dentry,
}

/// One lock-free child-memo slot, exactly one cacheline. Racing readers
/// are guarded by the per-slot seqlock (odd = a refill is writing).
#[repr(align(64))]
struct ChildMemoSlot {
    /// Per-slot seqlock (writer: odd → fields → even).
    seq: AtomicU64,
    data: core::cell::UnsafeCell<ChildMemoData>,
}

/// Directory entry
///
#[repr(C)]
pub struct Dentry {
    /// dentry name (last component, e.g. "null" not "/dev/null")
    pub name: Spinlock<String>,
    /// parent directory entry
    pub parent: Spinlock<Option<Arc<Dentry>>>,
    /// child dentries (name -> dentry mapping)
    pub children: Spinlock<BTreeMap<String, Arc<Dentry>>>,
    /// associated inode
    pub inode: Spinlock<Option<Arc<Inode>>>,
    /// If this dentry is a mount point, points to the mount descriptor.
    /// None means this dentry is not a mount point.
    pub vfsmount: Spinlock<Option<Arc<VfsMountInternal>>>,
    /// Only set on a MOUNTED FILESYSTEM'S ROOT dentry: the mountpoint
    /// dentry in the parent filesystem. Weak on purpose — the mountpoint
    /// dentry holds the mount (`vfsmount`) which holds this dentry
    /// strongly; a strong edge back would form an Arc cycle. Lets
    /// `build_path()` cross the mount boundary: a mounted fs root has no
    /// `parent` pointer, so without this hop paths of entries inside a
    /// mount lose the mountpoint prefix (chdir("/proc") stored cwd="/").
    mountpoint: Spinlock<Option<alloc::sync::Weak<Dentry>>>,
    /// dentry state
    pub state: Spinlock<DentryState>,
    /// dentry flags
    pub flags: Spinlock<DentryFlags>,
    /// reference count
    ref_count: AtomicU64,
    /// Negative dentry: lookup found no file on disk
    pub negative: AtomicBool,
    /// (perf-path lane) Mirror of `vfsmount.is_some()`: lets the walk's
    /// per-component mount test run as ONE Relaxed load instead of the
    /// vfsmount spinlock + Option<Arc> clone. Maintained by
    /// set_mount/clear_mount, which remain the serialized writers of the
    /// authoritative field.
    is_mountpoint: AtomicBool,
    /// (perf-path lane) icache identity generation at the time `inode`
    /// was last refreshed against the icache (see inode::icache_regen).
    /// When it equals the current generation, the cached Arc IS the
    /// icache's current object for (ino, fs_id) and the walk skips the
    /// global-lock re-check. Relaxed is sufficient: the load only decides
    /// whether to take the SLOW path — a stale "current" verdict is
    /// caught on the next walk after the generation bumps.
    inode_gen: AtomicU64,
    /// (21st successor, negative-lookup memo) The parent directory
    /// INODE's content generation at insert time (meaningful only while
    /// `negative` is set). The stamp lives on the shared inode —
    /// bind-mount aliases are separate dentries over one inode, so a
    /// mutation through ANY alias invalidates memos cached under every
    /// alias. 0 = unstamped.
    pub neg_gen: AtomicU64,
    /// Negative memo children currently cached under this parent
    /// (bounded — see NEG_MEMO_CAP / memo_negative).
    neg_count: AtomicUsize,
    /// (gap-fs lane) Child-map mutation version — bumped (under the
    /// children lock) by every add/remove/clear/rehome. A memo slot
    /// stamped with an older version is stale and ignored, so readers
    /// never need to serialize with the mutators.
    children_ver: AtomicU64,
    /// (gap-fs lane) Lock-free child memo — see ChildMemoSlot.
    memo: [ChildMemoSlot; CHILD_MEMO_SLOTS],
    /// (gap-fs lane) Mirror of the `inode` slot's Arc pointer (null =
    /// None), published with Release by set_inode/clear_inode. Readers
    /// load it under a WalkClaim and may increment the strong count
    /// directly (the claim blocks the drain; see the reader-bank
    /// protocol comment).
    inode_ptr: AtomicPtr<Inode>,
}

unsafe impl Send for Dentry {}
unsafe impl Sync for Dentry {}

impl Dentry {
    /// Create new dentry
    pub fn new(name: String) -> Self {
        Self {
            name: Spinlock::new(name),
            parent: Spinlock::new(None),
            children: Spinlock::new(BTreeMap::new()),
            inode: Spinlock::new(None),
            vfsmount: Spinlock::new(None),
            mountpoint: Spinlock::new(None),
            state: Spinlock::new(DentryState::DUnhashed),
            flags: Spinlock::new(DentryFlags::new(DentryFlags::DCACHE_UNHASHED)),
            ref_count: AtomicU64::new(1),
            negative: AtomicBool::new(false),
            is_mountpoint: AtomicBool::new(false),
            inode_gen: AtomicU64::new(0),
            neg_gen: AtomicU64::new(0),
            neg_count: AtomicUsize::new(0),
            children_ver: AtomicU64::new(0),
            memo: [const {
                ChildMemoSlot {
                    seq: AtomicU64::new(0),
                    data: core::cell::UnsafeCell::new(ChildMemoData {
                        ver: 0,
                        hash: 0,
                        name: [0; CHILD_MEMO_NAME_MAX],
                        name_len: 0,
                        child: core::ptr::null(),
                    }),
                }
            }; CHILD_MEMO_SLOTS],
            inode_ptr: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// (negative-lookup memo) The generation this negative child was
    /// stamped with at insert time.
    #[inline]
    pub fn neg_gen_load(&self) -> u64 {
        self.neg_gen.load(Ordering::Acquire)
    }

    /// Cap on negative memo children per parent. The OH boot's probe
    /// storm targets a small fixed name set per directory (dozens at
    /// most); 32 pins the working set while bounding the heap cost that
    /// made uncached negatives a leak (one Dentry + String per ENOENT,
    /// inode02's ALLOCTHROW family).
    pub const NEG_MEMO_CAP: usize = 32;

    /// (negative-lookup memo) Insert a NEGATIVE child for `name` — an
    /// ext4 ENOENT this walk just paid the full directory scan for.
    /// Bounded: at the cap, one existing negative child is evicted
    /// (FIFO-by-name is not needed; any eviction keeps the bound, and a
    /// later probe of the evicted name re-inserts it). A positive child
    /// (or a fresher racer's insert) is never replaced. Call only for
    /// filesystems whose content changes exclusively through VFS
    /// mutations (ext4) — dynamic filesystems (procfs/devfs/sysfs)
    /// change without them and must never memo.
    pub fn memo_negative(self: &Arc<Self>, name: &str, parent_gen: u64) {
        let mut guard = self.children.lock();
        if let Some(existing) = guard.get(name) {
            if existing.is_negative() {
                // Refresh the stamp so a mutation that happened between
                // our miss and this insert does not doom the memo.
                existing.neg_gen.store(parent_gen, Ordering::Release);
            }
            // Positive child: the file exists — nothing to memo.
            return;
        }
        if self.neg_count.load(Ordering::Acquire) >= Self::NEG_MEMO_CAP {
            // Evict one negative child (first found) to keep the bound.
            let victim = guard
                .iter()
                .find(|(_, v)| v.is_negative())
                .map(|(k, _)| k.clone());
            match victim {
                Some(v) => {
                    guard.remove(&v);
                    self.neg_count.fetch_sub(1, Ordering::AcqRel);
                    // (gap-fs lane) map mutation — retire memo slots
                    // stamped with the older version.
                    self.children_ver.fetch_add(1, Ordering::Release);
                }
                None => {
                    // Counter drift (evictions raced): cap refused this
                    // insert; the next probe re-runs the fs lookup.
                    self.neg_count
                        .store(0, Ordering::Release);
                    return;
                }
            }
        }
        let d = Arc::new(Dentry::new(alloc::string::String::from(name)));
        d.set_parent(self.clone());
        d.negative.store(true, Ordering::Release);
        d.neg_gen.store(parent_gen, Ordering::Release);
        guard.insert(alloc::string::String::from(name), d);
        self.neg_count.fetch_add(1, Ordering::AcqRel);
        // (gap-fs lane) map mutation — invalidate memo slots stamped
        // with the older version (a slot could otherwise keep serving a
        // hit for a name whose map entry changed under it).
        self.children_ver.fetch_add(1, Ordering::Release);
    }

    /// Set parent directory entry
    pub fn set_parent(&self, parent: Arc<Dentry>) {
        *self.parent.lock() = Some(parent);
    }

    /// Set inode
    pub fn set_inode(&self, inode: Arc<Inode>) {
        let old = {
            let mut slot = self.inode.lock();
            // Publish the mirror first: a concurrent banked reader sees
            // either the old pointer (kept alive by the slot until the
            // replace below, then by its retire until the drain) or the
            // new one (held by the slot) — both valid under the claim.
            self.inode_ptr
                .store(Arc::as_ptr(&inode) as *mut Inode, Ordering::Release);
            slot.replace(inode)
        };
        // (perf-path lane) stamp the icache generation this Arc was made
        // current at — see inode_gen.
        self.inode_gen
            .store(crate::fs::inode::icache_regen(), Ordering::Relaxed);
        if let Some(old) = old {
            retire_inode_arc(old);
            dentry_drain_if_quiescent(true);
        }
    }

    /// (gap-fs lane) Drop the dentry's inode (unlink/rmdir/rename kill
    /// the cached entry). Publishes the null mirror and parks the old
    /// Arc for a reader-quiescent drain — replaces the direct
    /// `*inode.lock() = None` writes, which a banked reader of the old
    /// pointer could not be protected from.
    pub fn clear_inode(&self) {
        let old = {
            let mut slot = self.inode.lock();
            self.inode_ptr.store(core::ptr::null_mut(), Ordering::Release);
            slot.take()
        };
        if let Some(old) = old {
            retire_inode_arc(old);
            dentry_drain_if_quiescent(true);
        }
    }

    /// The current inode as a raw pointer, for callers holding a
    /// WalkClaim (see the reader-bank protocol comment): the object
    /// stays alive while the claim is held (the inode slot or a pending
    /// retire owns it). Null when the dentry has no inode.
    pub fn inode_ptr_walk(&self, _claim: &WalkClaim) -> *const Inode {
        self.inode_ptr.load(Ordering::Acquire)
    }

    /// Lock-free inode read for standalone callers: claim, load the
    /// mirror, take a strong reference, release.
    pub fn get_inode_fast(&self) -> Option<Arc<Inode>> {
        let claim = WalkClaim::acquire();
        let p = self.inode_ptr.load(Ordering::Acquire);
        let out = if p.is_null() {
            None
        } else {
            // SAFETY: p is non-null and kept alive by the inode slot or
            // its pending retire; our claim blocks the drain.
            Some(unsafe { inode_arc_from_raw(p) })
        };
        drop(claim);
        out
    }

    /// The icache generation `inode` was last refreshed at (see inode_gen).
    pub fn get_inode_gen(&self) -> u64 {
        self.inode_gen.load(Ordering::Relaxed)
    }

    /// (perf-path lane) Stamp the icache generation without touching the
    /// inode slot (the walk's refreshed-with-same-Arc case).
    pub fn set_inode_gen(&self, gen: u64) {
        self.inode_gen.store(gen, Ordering::Relaxed);
    }

    /// Get inode
    pub fn get_inode(&self) -> Option<Arc<Inode>> {
        self.inode.lock().clone()
    }

    /// Get name
    pub fn get_name(&self) -> String {
        self.name.lock().clone()
    }

    /// Build full path from root by walking parent chain
    pub fn build_path(&self) -> alloc::string::String {
        // The global climb can never fail (it terminates at the global
        // root); the Option only exists for the rooted variant below.
        self.build_path_rooted(None)
            .unwrap_or_else(|| alloc::string::String::from("/"))
    }

    /// Build the path of `self` the way Linux prepend_path() does: the
    /// climb stops at `stop` — the dentry treated as the process root —
    /// instead of the global root, so the result is ROOT-RELATIVE (after
    /// chroot("/usr") the "/usr/bin" dentry reports "/bin").
    ///
    /// Returns None when the climb reaches the global root without passing
    /// through `stop` — the path is not reachable under that root. getcwd
    /// translates this to ENOENT (Linux behavior for a cwd outside the
    /// process root); display-only callers may fall back to the global
    /// path.
    ///
    /// `stop = None` is the plain global path: same walk, same
    /// mount-boundary crossing as build_path() (a mount root climbs from
    /// its mountpoint so entries inside a mount carry the mountpoint
    /// prefix — chdir("/proc") must store cwd="/proc", not "/").
    pub fn build_path_rooted(
        &self,
        stop: Option<&Arc<Dentry>>,
    ) -> Option<alloc::string::String> {
        let mut components: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        let name = self.get_name();
        // Limit depth to prevent infinite loops on corrupted dentry trees.
        // 256 (review 5.1: build_path>64 截断 — deep trees silently got
        // truncated paths, breaking /proc/self/fd links and getcwd).
        let max_depth = 256;
        // `self` IS the process root: the rooted path is "/".
        if let Some(s) = stop {
            if core::ptr::eq(self, s.as_ref()) {
                return Some(alloc::string::String::from("/"));
            }
        }
        // A mounted filesystem's root dentry has NO parent pointer; the
        // climb starts at (or continues into) the MOUNTPOINT dentry in the
        // parent filesystem instead, so paths of entries inside a mount
        // carry the mountpoint prefix. Without this hop, build_path() on
        // the procfs root returned "/" and chdir("/proc") silently stored
        // cwd="/" — cd appeared to have no effect (pwd and ls still saw /).
        let mut current: Option<Arc<Dentry>> = match self.parent.lock().clone() {
            Some(p) => Some(p),
            // `self` is a filesystem root; a mount root climbs from its
            // mountpoint, the global root contributes nothing.
            None => self.mountpoint_dentry(),
        };
        while let Some(d) = current {
            if components.len() >= max_depth {
                break;
            }
            // Arrived at the process root: stop here — everything collected
            // so far is the path BELOW the root.
            if let Some(s) = stop {
                if core::ptr::eq(d.as_ref(), s.as_ref()) {
                    break;
                }
            }
            let parent = d.parent.lock().clone();
            match parent {
                Some(p) => {
                    components.push(d.get_name());
                    current = Some(p);
                }
                None => {
                    // Reached a filesystem root. Its name is "/" — pushing
                    // it as a COMPONENT made the join below emit "//tmp/..."
                    // (a leading double slash) for every absolute path.
                    // Callers that string-compare results (LTP getcwd01,
                    // symlink01 chdir01 vs getcwd) need the canonical
                    // single-slash form, so the root contributes the leading
                    // '/', not a component. A MOUNT root instead crosses
                    // into the parent filesystem at its mountpoint (stacked
                    // mounts resolve one hop per loop iteration); the
                    // mountpoint's own name is pushed on the next iteration.
                    match d.mountpoint_dentry() {
                        Some(mp) => current = Some(mp),
                        // The global (or a detached) filesystem root: with a
                        // stop requested we never met the process root, so
                        // `self` is NOT under it (Linux: unreachable).
                        None => {
                            if stop.is_some() {
                                return None;
                            }
                            break;
                        }
                    }
                }
            }
        }
        components.reverse();
        // Build path — absolute: "/" + root-to-leaf components.
        let mut path = alloc::string::String::from("/");
        for comp in &components {
            if path != "/" {
                path.push('/');
            }
            path.push_str(comp);
        }
        if !name.is_empty() && name != "/" {
            if path != "/" {
                path.push('/');
            }
            path.push_str(&name);
        }
        Some(path)
    }

    /// Look up a child dentry by name. Returns None if not found.
    pub fn lookup_child(&self, name: &str) -> Option<Arc<Dentry>> {
        self.children.lock().get(name).cloned()
    }

    /// (gap-fs lane) Lock-free child lookup for the path walk. Returns
    /// the child as a RAW pointer valid only while `claim` is held (the
    /// child map — or the removed child's pending retire — owns it; the
    /// claim blocks the drain). Callers that need the reference beyond
    /// the claim must `dentry_arc_from_raw` it first.
    ///
    /// Memo hit path: version check + per-slot seqlock pair + one name
    /// compare — no lock, no refcount traffic. Miss path: the locked
    /// BTreeMap lookup, refilling the memo slot (serialized by the
    /// children lock) so the next walk of the same component rides the
    /// memo.
    pub fn lookup_child_walk(
        &self,
        claim: &WalkClaim,
        name: &str,
    ) -> Option<*const Dentry> {
        if name.len() > CHILD_MEMO_NAME_MAX {
            // Long names can't ride the inline memo. Locked lookup (the
            // children lock serializes with every retire); the returned
            // Arc's map reference keeps the child alive under the claim
            // after our clone drops.
            let c = self.lookup_child(name)?;
            return Some(Arc::as_ptr(&c));
        }
        let h = child_name_hash(name.as_bytes());
        let slot = &self.memo[(h as usize) & CHILD_MEMO_MASK];
        let ver = self.children_ver.load(Ordering::Relaxed);
        let mut spins: u32 = 0;
        loop {
            let s1 = slot.seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                // SAFETY: seqlock-protected read (validated by the seq
                // re-load below; the sync::seqlock UnsafeCell discipline).
                let d = unsafe { &*slot.data.get() };
                let hit = d.ver == ver
                    && d.hash == h
                    && d.name_len as usize == name.len()
                    && d.name[..name.len()] == *name.as_bytes();
                let p = d.child;
                if slot.seq.load(Ordering::Acquire) == s1 {
                    if hit {
                        debug_assert!(!p.is_null());
                        return Some(p);
                    }
                    break; // clean miss → locked fallback + refill
                }
            }
            spins += 1;
            if spins >= 128 {
                break; // persistent seq mismatch → locked fallback
            }
            core::hint::spin_loop();
        }
        let child = {
            let guard = self.children.lock();
            let c = guard.get(name).cloned();
            if let Some(ref c) = c {
                self.refill_memo_locked(name, c);
            }
            c
        };
        child.map(|c| Arc::as_ptr(&c))
    }

    /// Lock-free child read for standalone callers: claim, memo/locked
    /// lookup, take a strong reference, release.
    pub fn lookup_child_fast(&self, name: &str) -> Option<Arc<Dentry>> {
        let claim = WalkClaim::acquire();
        let out = match self.lookup_child_walk(&claim, name) {
            Some(p) => {
                // SAFETY: loaded under the claim; see the protocol.
                Some(unsafe { dentry_arc_from_raw(p) })
            }
            None => None,
        };
        drop(claim);
        out
    }

    /// Refill a memo slot — called with the CHILDREN LOCK HELD (all
    /// refills and all `children_ver` bumps are serialized by it), so
    /// the plain-field writes race only readers, which the per-slot
    /// seqlock guards.
    fn refill_memo_locked(&self, name: &str, child: &Arc<Dentry>) {
        if name.len() > CHILD_MEMO_NAME_MAX {
            return;
        }
        let ver = self.children_ver.load(Ordering::Relaxed);
        let h = child_name_hash(name.as_bytes());
        let slot = &self.memo[(h as usize) & CHILD_MEMO_MASK];
        // A fill that already matches the current version wins (another
        // CPU refilled between our miss and taking the lock).
        {
            // SAFETY: children lock held — all refills serialize here.
            let cur = unsafe { &*slot.data.get() };
            if cur.ver == ver && cur.hash == h {
                return;
            }
        }
        let s = slot.seq.load(Ordering::Relaxed);
        slot.seq.store(s | 1, Ordering::Release); // odd — enter write
        // SAFETY: children lock held (serializes writers); racing
        // readers are guarded by the seqlock.
        {
            let d = unsafe { &mut *slot.data.get() };
            d.ver = ver;
            d.hash = h;
            d.name_len = name.len() as u8;
            d.name[..name.len()].copy_from_slice(name.as_bytes());
            d.child = Arc::as_ptr(child);
        }
        slot.seq.store((s | 1) + 1, Ordering::Release); // even — leave
    }

    /// Add a child dentry. If a child with the same name exists, it is replaced.
    pub fn add_child(&self, name: String, child: Arc<Dentry>) {
        // lb-lane hunt (OH /storage invisibility): pid 1's early tmpfs
        // mounts (/mnt, /mnt/data, /storage — mounted rc=0 at ~1.2 s) are
        // invisible to pid 1 itself by 4.8 s (mkdir under /mnt -> EROFS,
        // mkdir /storage/media -> ENOENT, kernel-confirmed), while later
        // block mounts stay visible. add_child REPLACES a same-name child
        // — if anything re-creates a dentry whose predecessor carried a
        // mount, the replacement has none and every later walk loses the
        // mount. Ring exactly that case.
        {
            let guard = self.children.lock();
            if let Some(old) = guard.get(&name) {
                if old.vfsmount.lock().is_some() && child.vfsmount.lock().is_none() {
                    static DUPMNT_N: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    if DUPMNT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                        let pname = self.name.lock().clone();
                        crate::pr_warn!(
                            "DUPMNT-REPLACE parent='{}' name='{}' (pid {})",
                            pname,
                            name,
                            crate::process::current_pid()
                        );
                    }
                }
            }
        }
        let old = self.children.lock().insert(name, child);
        // (gap-fs lane) Invalidate memo slots stamped with the old
        // version, then park a displaced child for a reader-quiescent
        // drain (a banked reader may still hold its raw pointer).
        self.children_ver.fetch_add(1, Ordering::Release);
        if let Some(old) = old {
            retire_dentry_arc(old);
            dentry_drain_if_quiescent(true);
        }
    }

    /// Remove a child dentry by name.
    pub fn remove_child(&self, name: &str) {
        let old = {
            let mut guard = self.children.lock();
            let removed = guard.remove(name);
            if let Some(ref removed) = removed {
                // Keep the negative-memo bound honest: a detached
                // negative child (stale-memo discard, mutation
                // maintenance) frees its slot.
                if removed.is_negative() {
                    let was = self.neg_count.fetch_sub(1, Ordering::AcqRel);
                    if was == 0 {
                        // Counter underflow race — clamp.
                        self.neg_count.store(0, Ordering::Release);
                    }
                }
            }
            removed
        };
        // (gap-fs lane) Invalidate memo slots stamped with the old
        // version, then park the removed child for a reader-quiescent
        // drain (a banked reader may still hold its raw pointer).
        self.children_ver.fetch_add(1, Ordering::Release);
        if let Some(old) = old {
            retire_dentry_arc(old);
            dentry_drain_if_quiescent(true);
        }
    }

    /// Drop every cached child (mount(2) MS_MOVE onto "/": the old root's
    /// cached ramdisk names must not shadow the moved-in filesystem's
    /// root — lookups fall back to the new root inode's directory ops).
    pub fn clear_children(&self) {
        let old: Vec<Arc<Dentry>> = {
            let mut guard = self.children.lock();
            core::mem::take(&mut *guard).into_values().collect()
        };
        self.neg_count.store(0, Ordering::Release);
        self.children_ver.fetch_add(1, Ordering::Release);
        for c in old {
            retire_dentry_arc(c);
        }
        if !DENTRY_RET_DENTRY.lock_irqsave().is_empty() {
            dentry_drain_if_quiescent(true);
        }
    }

    /// Move every cached child of `self` into `into` (re-parenting them).
    /// mount(2) MS_MOVE onto "/": the mounts the userspace switcher
    /// attached UNDER the moved root (/proc, /dev, ... — children of the
    /// moved mount's root dentry) must survive the root dentry swap, or
    /// the new root loses its /proc and /dev entirely.
    pub fn rehome_children_into(&self, into: &Arc<Dentry>) {
        let moving: Vec<(String, Arc<Dentry>)> = self.children.lock().iter().map(|(k, v)| {
            (k.clone(), v.clone())
        }).collect();
        {
            let mut guard = self.children.lock();
            guard.clear();
        }
        self.children_ver.fetch_add(1, Ordering::Release);
        for (name, child) in moving {
            child.set_parent(into.clone());
            into.add_child(name, child.clone());
            // (gap-fs lane) Our clone drops here — if it was the last
            // live reference (no new-parent map entry, e.g. a racing
            // displacement), the free must wait out banked readers, so
            // park it like every other detached child.
            retire_dentry_arc(child);
        }
        if !DENTRY_RET_DENTRY.lock_irqsave().is_empty() {
            dentry_drain_if_quiescent(true);
        }
    }

    /// Detach the mount descriptor (mount(2) MS_MOVE source side and
    /// umount(2)). Also forgets the mounted root's mountpoint
    /// back-pointer so a detached mount's root stops claiming a
    /// mountpoint it no longer occupies.
    pub fn clear_mount(&self) {
        let mnt = self.vfsmount.lock().take();
        self.is_mountpoint.store(false, Ordering::Release);
        if let Some(mnt) = mnt {
            mnt.root.clear_mountpoint_back();
        }
        crate::fs::path_memo::mount_gen_bump();
    }

    /// (perf-path lane) Take the mount descriptor out and update the
    /// presence mirror — for teardown paths that handle the mountpoint
    /// back-reference themselves (identical to clear_mount otherwise).
    pub fn take_mount(&self) -> Option<Arc<VfsMountInternal>> {
        let mnt = self.vfsmount.lock().take();
        self.is_mountpoint.store(mnt.is_some(), Ordering::Release);
        crate::fs::path_memo::mount_gen_bump();
        mnt
    }

    /// Set the mount descriptor (marks this dentry as a mount point).
    pub fn set_mount(&self, mount: Arc<VfsMountInternal>) {
        *self.vfsmount.lock() = Some(mount);
        self.is_mountpoint.store(true, Ordering::Release);
        crate::fs::path_memo::mount_gen_bump();
    }

    /// Get the mount descriptor (if this is a mount point).
    pub fn get_mount(&self) -> Option<Arc<VfsMountInternal>> {
        self.vfsmount.lock().clone()
    }

    /// (perf-path lane) Lock-free mount-point presence test — the
    /// authoritative field stays `vfsmount` (set_mount/clear_mount keep
    /// this mirror in step; flag first, descriptor second on set;
    /// descriptor first, flag second on clear — a transient mismatch is
    /// impossible to observe worse than taking the slow lock path).
    pub fn has_mount(&self) -> bool {
        self.is_mountpoint.load(Ordering::Acquire)
    }

    /// (perf-path lane) Presence-mirror writer for teardown paths that
    /// mutate `vfsmount` under its own lock (see vfs_umount_object).
    pub fn set_mount_flag(&self, on: bool) {
        self.is_mountpoint.store(on, Ordering::Release);
        crate::fs::path_memo::mount_gen_bump();
    }

    /// Record the mountpoint dentry on a mounted filesystem's ROOT dentry
    /// (the reverse edge of `set_mount`'s mountpoint→root link, held Weak
    /// to avoid an Arc cycle). Set by vfs_mount()/vfs_move_mount().
    pub fn set_mountpoint_back(&self, mountpoint: Arc<Dentry>) {
        *self.mountpoint.lock() = Some(Arc::downgrade(&mountpoint));
    }

    /// The mountpoint this fs-root dentry is mounted on, if any (see
    /// set_mountpoint_back). Used by ancestry walks that must cross mount
    /// boundaries upward (MS_REC bind cloning).
    pub fn get_mountpoint_back(&self) -> Option<Arc<Dentry>> {
        self.mountpoint.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// Forget the mountpoint back-pointer (mount detached/moved away).
    pub fn clear_mountpoint_back(&self) {
        *self.mountpoint.lock() = None;
    }

    /// The mountpoint dentry this filesystem root is mounted at, if this
    /// is a mounted root and the mountpoint dentry is still alive.
    pub fn mountpoint_dentry(&self) -> Option<Arc<Dentry>> {
        self.mountpoint.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// Set to hashed state
    pub fn set_hashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_HASHED);
        *self.state.lock() = DentryState::DHashed;
    }

    /// Set to unhashed state
    pub fn set_unhashed(&self) {
        let mut flags = self.flags.lock();
        *flags = DentryFlags::new(flags.bits() | DentryFlags::DCACHE_UNHASHED);
        *self.state.lock() = DentryState::DUnhashed;
    }

    /// Increment reference count
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn dec_ref(&self) -> u64 {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Get reference count
    pub fn get_ref(&self) -> u64 {
        self.ref_count.load(Ordering::Acquire)
    }

    /// Mark as negative dentry (lookup found no file)
    pub fn set_negative(&self) {
        self.negative.store(true, Ordering::Release);
    }

    /// Check if this is a negative dentry
    pub fn is_negative(&self) -> bool {
        self.negative.load(Ordering::Acquire)
    }

    /// Clear negative flag (e.g. after file creation)
    pub fn clear_negative(&self) {
        self.negative.store(false, Ordering::Release);
    }
}

/// Create root directory entry
pub fn make_root_dentry() -> Option<Arc<Dentry>> {
    let dentry = Arc::new(Dentry::new("/".to_owned()));
    // Note: Arc returns &T when dereferenced
    // For now, we'll return the Arc directly - the caller can call set_hashed if needed
    Some(dentry)
}

// ============================================================================
// Dentry cache (dcache)
// ============================================================================

/// Dentry cache size - from config
const DCACHE_SIZE: usize = crate::config::DCACHE_SIZE;

/// Dentry cache statistics
#[derive(Debug)]
pub struct DentryCacheStats {
    /// Cache hit count
    pub hits: AtomicU64,
    /// Cache miss count
    pub misses: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

impl DentryCacheStats {
    pub fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            (hits as f64) / (total as f64)
        }
    }
}

/// Hash table bucket
struct DentryHashBucket {
    /// dentry pointer
    dentry: Option<Arc<Dentry>>,
    /// hash key (for quick comparison)
    key: u64,
    /// parent inode number (for hash collision disambiguation)
    parent_ino: u64,
    /// LRU timestamp (for eviction)
    access_time: AtomicU64,
}

impl Clone for DentryHashBucket {
    fn clone(&self) -> Self {
        Self {
            dentry: self.dentry.clone(),
            key: self.key,
            parent_ino: self.parent_ino,
            access_time: AtomicU64::new(self.access_time.load(Ordering::Relaxed)),
        }
    }
}

/// Dentry hash table
struct DentryCache {
    /// Hash table (heap-allocated to avoid large stack allocation)
    buckets: alloc::boxed::Box<[DentryHashBucket]>,
    /// Number of entries in cache
    count: usize,
    /// Global timestamp (for LRU)
    global_time: AtomicU64,
    /// Statistics
    stats: DentryCacheStats,
}

unsafe impl Send for DentryCache {}
unsafe impl Sync for DentryCache {}

/// Global Dentry cache
static DCACHE: Spinlock<Option<DentryCache>> = Spinlock::new(None);

/// Initialize Dentry cache
fn dcache_init() {
    let mut cache = DCACHE.lock();
    if cache.is_some() {
        return;  // Already initialized
    }

    // Heap-allocate bucket array to avoid large stack allocation
    let buckets: alloc::vec::Vec<DentryHashBucket> = (0..DCACHE_SIZE)
        .map(|_| DentryHashBucket {
            dentry: None,
            key: 0,
            parent_ino: 0,
            access_time: AtomicU64::new(0),
        })
        .collect();

    *cache = Some(DentryCache {
        buckets: buckets.into_boxed_slice(),
        count: 0,
        global_time: AtomicU64::new(1),
        stats: DentryCacheStats::new(),
    });
}

/// Calculate hash value
///
/// Uses simple FNV-1a hash algorithm
fn dentry_hash(name: &str, parent_ino: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis

    // Mix parent inode number
    hash ^= parent_ino;
    hash = hash.wrapping_mul(0x100000001b3);

    // Mix name
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}

/// Look up in Dentry cache
///
pub fn dcache_lookup(name: &str, parent_ino: u64) -> Option<Arc<Dentry>> {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let cache_inner = cache.as_mut()?;

    // Calculate hash value
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // Find matching entry
    let bucket = &cache_inner.buckets[index];

    if let Some(ref dentry) = bucket.dentry {
        // Compare hash key and parent_ino to avoid false cache hits on collision
        if bucket.key == hash && bucket.parent_ino == parent_ino {
            // Compare name
            if dentry.name.lock().as_str() == name {
                // Update access time (for LRU)
                let current_time = cache_inner.global_time.fetch_add(1, Ordering::Relaxed);
                bucket.access_time.store(current_time, Ordering::Relaxed);

                // Record hit
                cache_inner.stats.record_hit();

                return Some(dentry.clone());
            }
        }
    }

    // Record miss
    cache_inner.stats.record_miss();

    None
}

/// Add Dentry to cache
///
pub fn dcache_add(dentry: Arc<Dentry>, parent_ino: u64) {
    // Ensure cache is initialized
    dcache_init();

    // Calculate hash value (outside cache lock)
    let name = dentry.name.lock();
    let name_str = name.clone();
    drop(name);  // Release name lock
    let hash = dentry_hash(&name_str, parent_ino);

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    let index = (hash as usize) % DCACHE_SIZE;

    // Check if already exists
    if let Some(ref _existing) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            return;  // Already in cache
        }

        // Use LRU policy: find and evict least recently used entry
        dcache_evict_lru(inner);
    }

    // Get current timestamp
    let current_time = inner.global_time.fetch_add(1, Ordering::Relaxed);

    // Add to cache (only increment count when the bucket was previously empty)
    let was_empty = inner.buckets[index].dentry.is_none();
    inner.buckets[index] = DentryHashBucket {
        dentry: Some(dentry.clone()),
        key: hash,
        parent_ino,
        access_time: AtomicU64::new(current_time),
    };
    if was_empty {
        inner.count += 1;
    }

    // Mark as hashed (outside cache lock)
    drop(cache);  // Release cache lock
    dentry.set_hashed();
}

/// LRU eviction policy: evict least recently used entry
///
fn dcache_evict_lru(cache: &mut DentryCache) {
    // Find least recently used entry (minimum access time)
    let mut lru_index = 0;
    let mut lru_time = u64::MAX;
    let mut found = false;

    for (i, bucket) in cache.buckets.iter().enumerate() {
        if bucket.dentry.is_some() {
            let access_time = bucket.access_time.load(Ordering::Relaxed);
            if access_time < lru_time {
                lru_time = access_time;
                lru_index = i;
                found = true;
            }
        }
    }

    // Evict LRU entry
    if found {
        if let Some(ref dentry) = cache.buckets[lru_index].dentry {
            // Mark as unhashed
            dentry.set_unhashed();
        }

        cache.buckets[lru_index].dentry = None;
        cache.buckets[lru_index].key = 0;
        cache.buckets[lru_index].parent_ino = 0;
        cache.buckets[lru_index].access_time.store(0, Ordering::Relaxed);
        cache.count -= 1;

        // Record eviction
        cache.stats.record_eviction();
    }
}

/// Remove from Dentry cache
///
pub fn dcache_remove(name: &str, parent_ino: u64) {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // Calculate hash value
    let hash = dentry_hash(name, parent_ino);
    let index = (hash as usize) % DCACHE_SIZE;

    // Remove entry
    if let Some(ref dentry) = inner.buckets[index].dentry {
        if inner.buckets[index].key == hash {
            // Mark as unhashed
            dentry.set_unhashed();

            // Remove from cache
            inner.buckets[index].dentry = None;
            inner.buckets[index].key = 0;
            inner.buckets[index].access_time.store(0, Ordering::Relaxed);
            inner.count -= 1;
        }
    }
}

/// Get cache statistics
pub fn dcache_stats() -> (usize, usize) {
    // Ensure cache is initialized
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (cache_inner.count, DCACHE_SIZE)
}

/// Get detailed cache statistics
pub fn dcache_stats_detailed() -> (u64, u64, u64, f64) {
    // Ensure cache is initialized
    dcache_init();

    let cache = DCACHE.lock();
    let cache_inner = cache.as_ref().expect("dcache not initialized");

    (
        cache_inner.stats.hits.load(Ordering::Relaxed),
        cache_inner.stats.misses.load(Ordering::Relaxed),
        cache_inner.stats.evictions.load(Ordering::Relaxed),
        cache_inner.stats.get_hit_rate(),
    )
}

/// Clear Dentry cache
///
pub fn dcache_flush() {
    // Ensure cache is initialized
    dcache_init();

    let mut cache = DCACHE.lock();
    let inner = cache.as_mut().expect("dcache not initialized");

    // Clear all buckets
    for bucket in inner.buckets.iter_mut() {
        if let Some(ref dentry) = bucket.dentry {
            // Mark as unhashed
            dentry.set_unhashed();
        }
        bucket.dentry = None;
        bucket.key = 0;
        bucket.access_time.store(0, Ordering::Relaxed);
    }

    inner.count = 0;
}
