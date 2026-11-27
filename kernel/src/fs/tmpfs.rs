//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! tmpfs — page-granular in-memory filesystem (P0-6)
//!
//! Backing store for `/dev/shm` (POSIX `shm_open` via musl maps it to
//! `/dev/shm/<name>`) and general RAM-backed mounts (`mount -t tmpfs`).
//!
//! Storage model:
//! - File contents live in a `BTreeMap<usize, Box<[u8; 4096]>>` — pages are
//!   allocated ONLY when written; reads of never-written pages return zeros
//!   (sparse semantics, like Linux shmem). A file's logical size is tracked
//!   separately from its page set, so `truncate` + re-extend yields zeros.
//! - Pages are kernel-heap allocations. They are NOT refcounted into RSS and
//!   are NEVER reclaimed by kswapd (no rmap / no swap backing). They are
//!   freed on unlink/last-close-of-truncated-size/truncate. Documented
//!   limitation of this wave; a shmem inode cache + reclaim path is future
//!   work.
//! - Hard links share the SAME `Arc<Spinlock<BTreeMap>>` (POSIX
//!   shared-visibility semantics, same design as rootfs).
//!
//! Instances:
//! - Every `mount -t tmpfs` creates an INDEPENDENT `TmpfsSuperBlock` (own
//!   inode number space and own icache `fs_id` — low bits of the id are the
//!   instance counter, so (ino, fs_id) never collide across mounts).
//! - Superblocks are leaked on mount (mount-lifetime objects, like the ext4
//!   global instance); umount detaches the dentry but does not free the
//!   tree (open inodes keep their `Arc<TmpfsNode>`s alive regardless).
//!
//! Unsupported (documented): mmap of tmpfs files (files are not backed by
//! refcounted user pages — mapping falls back to the generic inode path),
//! memory limits/statfs accounting is static.

// (INV-TMPFS-SB-1: the tmpfs mount-lifetime family — the superblock
// is leaked into the registry at mount (INV-FS-GLOBALSB-1 provenance);
// the name cells are parent-children-lock serialized (set_name's
// contract); read_at/reserve_page rows ride the page's own lock.)
use crate::errno;
use crate::fs::inode::{file_type, setattr_attr, Inode, InodeMode, Ino, INodeOps, VfsDirEntry};
use crate::fs::superblock::{FileSystemType, FsContext, SuperBlock, SuperBlockFlags};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;

/// Linux TMPFS_MAGIC.
pub const TMPFS_MAGIC: u32 = 0x0102_1994;

/// tmpfs page size (matches the kernel page size / stat blksize).
pub(crate) const PAGE_SIZE: usize = 4096;

/// icache identity prefix ("TPFS"); the low bits carry the per-mount
/// instance id so (ino, fs_id) pairs from different tmpfs mounts never
/// collide (VFS-H8 discipline — see FS_ID_ROOTFS in inode.rs).
const FS_ID_TMPFS_BASE: u64 = 0x5450_4653_0000_0000;

/// Wall-clock seconds since epoch (RTC-armed boot offset + monotonic);
/// tmpfs atime/mtime are Unix epoch time, as on Linux.
fn wall_secs() -> u64 {
    crate::drivers::rtc::wall_secs()
}

/// One 4 KiB data page of a tmpfs file, plus the owning superblock.
///
/// Drop releases the page back to the mount's page budget
/// (`TmpfsSuperBlock::used_pages`), so EVERY removal path — truncate,
/// unlink, hard-link-drop, whole-mount teardown — decrements exactly
/// once, by construction. The size= budget is what keeps an
/// unprivileged `mount -t tmpfs -o size=32M` from eating the whole
/// kernel heap: tmpfs pages are kernel-heap boxes here (no swap, no
/// page-cache reclaim), so without the limit a 118 MB write pattern
/// (LTP preadv203) exhausts the 128 MB heap and the ALLOCTHROW path
/// kills every later fork in the boot (x86-r3 chunk c8: 91 forkfail
/// cascade).
pub(crate) struct TmpfsPage {
    data: Box<[u8; PAGE_SIZE]>,
    sb: *const TmpfsSuperBlock,
}

impl TmpfsPage {
    fn new(sb: *const TmpfsSuperBlock) -> Self {
        // The caller must have reserved budget (used_pages already
        // incremented) — see TmpfsNode::write_at.
        Self {
            data: Box::new([0u8; PAGE_SIZE]),
            sb,
        }
    }
}

impl Drop for TmpfsPage {
    fn drop(&mut self) {
        if !self.sb.is_null() {
            unsafe {
                (*self.sb).used_pages.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

// ============================================================================
// Node
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TmpfsType {
    Directory,
    RegularFile,
    SymbolicLink,
}

/// One tmpfs object (file / directory / symlink).
///
/// All mutable state is behind Spinlocks; `name` uses interior mutability
/// under the parent's children lock (identical discipline to RootFSNode).
#[repr(C)]
pub struct TmpfsNode {
    /// Node name (mutated only via set_name with the parent lock held).
    pub(crate) name: UnsafeCell<Vec<u8>>,
    pub node_type: TmpfsType,
    /// Page-granular contents. Shared `Arc` between hard links so writes
    /// through one link are visible through the others (POSIX semantics).
    pages: Arc<Spinlock<BTreeMap<usize, TmpfsPage>>>,
    /// Logical file size in bytes (independent of the allocated page set).
    size: AtomicU64,
    /// File-type + permission bits (chmod-visible).
    pub mode: Spinlock<u32>,
    /// atime / mtime in wall-clock (epoch) seconds.
    atime: AtomicU64,
    mtime: AtomicU64,
    /// Symlink target (None for non-symlinks).
    link_target: Option<Vec<u8>>,
    /// Child nodes (directories only).
    children: Spinlock<Vec<Arc<TmpfsNode>>>,
    /// Inode number within this tmpfs instance.
    pub ino: u64,
    /// Instance identity (icache key low bits are in the node so iget can
    /// stamp VFS inodes without touching the superblock).
    pub fs_id: u64,
    /// Owning superblock (leaked at mount; valid for the kernel's lifetime).
    sb: *const TmpfsSuperBlock,
    /// Device number for device-typed nodes (S_IFCHR/S_IFBLK), USERSPACE
    /// dev_t encoding (new_encode_dev — what mknod(2) passed and stat(2)
    /// reports). 0 for non-device nodes. OH Phase 1b: OH's init mounts a
    /// tmpfs over /dev and mknods /dev/{null,random,urandom,kmsg}; open()
    /// binds those nodes to CharDev registry ops through this number.
    rdev: AtomicU64,
    /// Owner uid/gid, chown-visible. Initialized from the creating task's
    /// fsuid/fsgid (Linux inode_init_owner()); updated by chown/setattr
    /// (LTP creat09 tmpfs round: chown(dir, uid, gid) then stat must
    /// report the new gid).
    uid: AtomicU32,
    gid: AtomicU32,
}

impl TmpfsNode {
    /// Build a node of the given type with default modes (root-owned).
    fn new(name: Vec<u8>, node_type: TmpfsType, ino: u64, fs_id: u64, sb: *const TmpfsSuperBlock) -> Self {
        let mode = match node_type {
            TmpfsType::Directory => InodeMode::S_IFDIR | 0o777,
            TmpfsType::RegularFile => InodeMode::S_IFREG | 0o666,
            TmpfsType::SymbolicLink => InodeMode::S_IFLNK | 0o777,
        };
        // Linux inode_init_owner(): creator's EFFECTIVE filesystem ids, not
        // the real ones (same rule as ext4_new_inode).
        let (uid, gid) = match crate::sched::current() {
            Some(task) => {
                // SAFETY: sched::current() yields a valid task reference.
                let cred =  (*task).cred() ;
                (cred.fsuid, cred.fsgid)
            }
            None => (0, 0),
        };
        Self {
            name: UnsafeCell::new(name),
            node_type,
            pages: Arc::new(Spinlock::new(BTreeMap::new())),
            size: AtomicU64::new(0),
            mode: Spinlock::new(mode),
            atime: AtomicU64::new(wall_secs()),
            mtime: AtomicU64::new(wall_secs()),
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ino,
            fs_id,
            sb,
            rdev: AtomicU64::new(0),
            uid: AtomicU32::new(uid),
            gid: AtomicU32::new(gid),
        }
    }

    /// Node name as a byte slice.
    pub fn name(&self) -> &[u8] {
        // SAFETY: name is only mutated via set_name(), which requires the
        // parent's children lock (exclusive access).
        unsafe { &*self.name.get() }
    }

    /// Rename in place. Caller must hold the parent's children lock.
    pub fn set_name(&self, new_name: Vec<u8>) {
        // SAFETY: caller guarantees exclusive access (parent lock held).
        unsafe { *self.name.get() = new_name; }
    }

    pub fn is_dir(&self) -> bool {
        self.node_type == TmpfsType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.node_type == TmpfsType::RegularFile
    }

    pub fn is_symlink(&self) -> bool {
        self.node_type == TmpfsType::SymbolicLink
    }

    pub fn add_child(&self, child: Arc<TmpfsNode>) {
        self.children.lock().push(child);
    }

    pub fn remove_child(&self, name: &[u8]) -> bool {
        let mut children = self.children.lock();
        if let Some(pos) = children.iter().position(|c| c.name() == name) {
            children.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn find_child(&self, name: &[u8]) -> Option<Arc<TmpfsNode>> {
        let children = self.children.lock();
        children.iter().find(|c| c.name() == name).cloned()
    }

    pub fn list_children(&self) -> Vec<Arc<TmpfsNode>> {
        self.children.lock().clone()
    }

    pub fn file_size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// Number of allocated pages (stat blocks / memory accounting).
    pub fn page_count(&self) -> u64 {
        self.pages.lock().len() as u64
    }

    /// Read up to `buf.len()` bytes at `offset`. Never-written pages read as
    /// zeros (sparse); reads past EOF return short.
    ///
    /// Cache-frame-first, PER PAGE (fsx-linux MAP_SHARED coherence): a
    /// SHARED mapper's stores land directly in PAGE-CACHE alias frames
    /// (PCACHE_SHARED — the fault arm maps the cache frame writable), so
    /// a cached frame is the FRESHEST face of that page; the node copy
    /// only mirrors what the fd path wrote. Each page is served from its
    /// pinned frame when one is cached, else from the node pages — a
    /// missing page does NOT stop later pages from using their frames
    /// (a prefix-only pass served whole-node data past the first gap:
    /// the FXW "exp='o' got='O'" family). This cannot recurse into the
    /// fault path: `shared_file_fault_page` reaches read_at only on a
    /// cache MISS, and every get_stale here that HITS means no bounce
    /// read happens for that page.
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let size = self.size.load(Ordering::Acquire) as usize;
        if offset >= size || buf.is_empty() {
            return 0;
        }
        let avail = size - offset;
        let total = buf.len().min(avail);

        let cache = crate::fs::page_cache::get_page_cache();
        let mut done = 0usize;
        while done < total {
            let off = offset + done;
            let page_idx = off / PAGE_SIZE;
            let page_off = off % PAGE_SIZE;
            let chunk = (total - done).min(PAGE_SIZE - page_off);
            // Frame face first. get_stale: an invalidated-but-mapped
            // frame is still a live face for tmpfs (mapper stores live
            // in it and never reach the node pages) — refusing it reads
            // stale NODE bytes (FXW FD-MISMATCH family).
            if let Some(frame) = cache.get_stale(self.fs_id, self.ino, page_idx as u64) {
                // SAFETY: `frame` is the pinned cache page's kernel VA —
                // PAGE_SIZE readable bytes for the pin's lifetime;
                // page_off + chunk <= PAGE_SIZE and done + chunk <=
                // total by construction.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        frame.add(page_off),
                        buf.as_mut_ptr().add(done),
                        chunk,
                    );
                }
                cache.put(self.fs_id, self.ino, page_idx as u64);
                done += chunk;
                continue;
            }
            // Node face for this page (zero-fill holes). The pages lock
            // is taken per page and released before the next cache
            // lookup — never nested with a pcache lock.
            let pages = self.pages.lock();
            if let Some(page) = pages.get(&page_idx) {
                buf[done..done + chunk].copy_from_slice(&page.data[page_off..page_off + chunk]);
            } else {
                // Hole: zero-fill.
                buf[done..done + chunk].fill(0);
            }
            drop(pages);
            done += chunk;
        }

        self.atime.store(wall_secs(), Ordering::Release);
        done
    }

    /// Write `data` at `offset`, allocating pages on demand. Extends the
    /// logical size to cover the stored bytes.
    ///
    /// Returns the number of bytes stored. When the mount's page budget
    /// (mount option `size=`/`nr_blocks=`) runs out mid-write, the stored
    /// prefix is returned — a SHORT count; 0 when not even one new page
    /// could be allocated. The file-op caller maps a zero return to
    /// ENOSPC (POSIX: short write when some bytes fit, ENOSPC when none).
    pub fn write_at(&self, offset: usize, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }

        // r-tmpfs wedge fix (2026-11-19, lat_fs official-rig residual):
        // the page object is a 4 KiB KERNEL-HEAP Box. Allocating it
        // inside the pages-spinlock section chained this node's lock
        // INTO the allocator (small_alloc slab refill -> buddy ->
        // under memory pressure the direct-reclaim slowpath ->
        // PageCache::shrink's wlock) — a concurrent path the reclaimer
        // takes back toward tmpfs then deadlocks the pair, wedging the
        // cpu at IF=0 inside tmpfs_file_write (the mkdir-EIO chain's
        // head). The reference's shmem allocates the page OUTSIDE the
        // info lock (shmem_getpage: page-cache alloc first, the locked
        // section only installs) — same shape here: speculate the page
        // BEFORE taking the lock, install it under the lock, drop the
        // unused speculation (its Drop returns the budget slot).
        let first_idx = offset / PAGE_SIZE;
        let mut spec: Option<TmpfsPage> = if self.reserve_page() {
            Some(TmpfsPage::new(self.sb))
        } else {
            None // budget exhausted; the loop breaks at first need
        };

        let mut done = 0usize;
        {
            let mut pages = self.pages.lock();
            while done < data.len() {
                let off = offset + done;
                let page_idx = off / PAGE_SIZE;
                let page_off = off % PAGE_SIZE;
                let chunk = (data.len() - done).min(PAGE_SIZE - page_off);
                let needs_page = !pages.contains_key(&page_idx);
                if needs_page {
                    // Fresh page: consume the speculation (only valid for
                    // the FIRST first-touch index — a multi-page write's
                    // later first-touches fall back to an in-lock alloc,
                    // which is acceptable: the common tmpfs write class
                    // (lat_fs directories, small logs) touches one page;
                    // the wedge head is the pressure slowpath, reached
                    // once per refill, not per page).
                    let candidate = match spec.take() {
                        Some(p) if page_idx == first_idx => p,
                        _ => {
                            if !self.reserve_page() {
                                break; // budget exhausted
                            }
                            TmpfsPage::new(self.sb)
                        }
                    };
                    pages.insert(page_idx, candidate);
                }
                let page = pages.get_mut(&page_idx).unwrap();
                page.data[page_off..page_off + chunk]
                    .copy_from_slice(&data[done..done + chunk]);
                done += chunk;
            }
        }
        // Unused speculation (page existed, or budget broke the loop
        // before first touch): Drop returns the reserved slot.

        // size = max(size, offset + done) — only bytes that actually
        // landed extend the logical size (no fetch_max on AtomicU64,
        // CAS loop).
        let end = (offset + done) as u64;
        let mut cur = self.size.load(Ordering::Acquire);
        while end > cur {
            match self
                .size
                .compare_exchange_weak(cur, end, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }

        // MAP_SHARED alias bridge (fsx-linux coherence): the node copy
        // above is the fd-read face, but SHARED mappers alias PAGE-CACHE
        // frames — push the same bytes into every live frame so both
        // faces stay one file (read_at prefers the frames; a frame not
        // yet cached simply doesn't exist, and the next fault populates
        // it from the fresh node state written above). Runs AFTER the
        // pages lock: pcache locks and the node pages lock never nest.
        if done > 0 {
            // _stale (force through invalidated entries): a mapper's
            // still-live alias of an invalidated frame keeps serving its
            // PTE — skipping it here made fd writes invisible to old
            // mappers (FXW MAP-MISMATCH family).
            crate::fs::page_cache::get_page_cache().writeback_update_range_stale(
                self.fs_id,
                self.ino,
                offset as u64,
                &data[..done],
            );
        }

        self.mtime.store(wall_secs(), Ordering::Release);
        done
    }

    /// fallocate(2) allocation semantics (falloc-wedge lane,
    /// 2026-11-16): materialize ZERO pages for every missing page of
    /// [offset, offset+len) under the mount's page budget — shmem's
    /// mode=0 behavior (the reference allocates the range; Rux used to
    /// only grow the logical size and leave the pages demand-allocated,
    /// so a write into the "preallocated" range of a FULL tmpfs failed
    /// ENOSPC — LTP fallocate05/06's tmpfs iterations, run/f05i). On a
    /// budget shortfall returns ENOSPC with the pages allocated so far
    /// kept (shmem keeps partial progress).
    pub fn fallocate_alloc(&self, offset: usize, len: usize) -> Result<(), i32> {
        if len == 0 {
            return Ok(());
        }
        let end = offset
            .checked_add(len)
            .ok_or(errno::Errno::InvalidArgument.as_neg_i32())?;
        let first = offset / PAGE_SIZE;
        let last = (end - 1) / PAGE_SIZE;
        {
            let mut pages = self.pages.lock();
            for idx in first..=last {
                if pages.contains_key(&idx) {
                    continue;
                }
                if !self.reserve_page() {
                    return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
                }
                pages.insert(idx, TmpfsPage::new(self.sb));
            }
        }
        self.mtime.store(wall_secs(), Ordering::Release);
        Ok(())
    }

    /// fallocate(2) PUNCH_HOLE|FALLOC_FL_KEEP_SIZE semantics (falloc-wedge
    /// lane, 2026-11-16): the range reads as zeros afterward, the logical
    /// size is unchanged. Whole pages inside the range are dropped (their
    /// Drop returns the mount-budget slot — the freed space is
    /// fallocate-able again, LTP fallocate05's dealloc step); partial
    /// pages at the edges keep their frames with the punched slice zeroed.
    /// Byte-granular (shmem semantics; no block alignment requirement).
    pub fn punch_hole(&self, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        let end = offset.saturating_add(len);
        {
            let mut pages = self.pages.lock();
            let first = offset / PAGE_SIZE;
            let last = if end == 0 { 0 } else { (end - 1) / PAGE_SIZE };
            for idx in first..=last {
                let page_start = idx * PAGE_SIZE;
                let page_end = page_start + PAGE_SIZE;
                if page_start >= offset && page_end <= end {
                    pages.remove(&idx); // Drop returns the budget slot
                } else if let Some(page) = pages.get_mut(&idx) {
                    let s = offset.saturating_sub(page_start);
                    let e = core::cmp::min(end - page_start, PAGE_SIZE);
                    if s < e {
                        page.data[s..e].fill(0);
                    }
                }
            }
        }
        // Cache side (the truncate family's discipline): zero every live
        // frame's bytes in the range — read prefers the frames, and a
        // stale-but-mapped alias must serve zeros too. Frames of fully
        // dropped pages are NOT invalidated here (invalidate_inode_range
        // is a tail-floor, not a mid-file range); they hold zeros, which
        // is the required content, and die by eviction or final put.
        {
            const ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
            let cache = crate::fs::page_cache::get_page_cache();
            let mut off = offset as u64;
            let end64 = end as u64;
            while off < end64 {
                let n = (end64 - off).min(PAGE_SIZE as u64) as usize;
                cache.writeback_update_range_stale(self.fs_id, self.ino, off, &ZERO_PAGE[..n]);
                off += n as u64;
            }
        }
        self.mtime.store(wall_secs(), Ordering::Release);
    }

    /// Truncate to `new_size`: drop whole pages beyond the new size and
    /// zero the tail of the (kept) partial last page so a later re-extend
    /// exposes zeros, not stale data. Dropping a `TmpfsPage` returns it
    /// to the mount's page budget automatically.
    ///
    /// MAP_SHARED alias coherence (execl01 / fsx-linux family), node side
    /// FIRST then cache side: a racing fault between the two stages
    /// re-populates a frame from the NEW node state (zero tail, dropped
    /// pages), which the cache-side pass then either re-zeroes or
    /// invalidates — the reverse order would let such a fault capture a
    /// stale tail that nothing afterwards corrects. Cache side:
    /// (a) shrink — zero the partial last page's tail IN every live frame
    /// (`writeback_update_range_stale` reaches stale-but-mapped aliases
    /// too: mapped PTEs keep pointing at those frames), then drop every
    /// frame fully beyond the new size (`invalidate_inode_range`;
    /// mapped frames die on their final put — documented deviation,
    /// ext4-class); (b) grow — zero every frame covering
    /// [old_size, new_size): frames dropped by an earlier shrink (but
    /// still mapped) would otherwise resurrect pre-truncate bytes inside
    /// the now-valid range on re-extend.
    pub fn truncate(&self, new_size: usize) {
        let old_size = self.size.load(Ordering::Acquire) as usize;
        // Node side (unchanged semantics).
        {
            let mut pages = self.pages.lock();
            if new_size == 0 {
                // A 0-size file keeps NO pages. The old `idx <= new_size /
                // PAGE_SIZE` retained page 0 for new_size == 0 — a leaked page
                // (and, with the size= budget, a leaked budget charge that
                // never returned; caught by the tmpfs-budget unit test).
                pages.clear();
            } else {
                let last_kept_page = new_size / PAGE_SIZE;
                let tail = new_size % PAGE_SIZE;
                pages.retain(|&idx, _| idx <= last_kept_page);
                if tail != 0 {
                    if let Some(page) = pages.get_mut(&last_kept_page) {
                        page.data[tail..PAGE_SIZE].fill(0);
                    }
                }
            }
        }
        self.size.store(new_size as u64, Ordering::Release);

        // Cache side (fsx-linux FXW family: MAP-MISMATCH/FD-MISMATCH after
        // byte-granular truncates). ZERO_PAGE lives in .rodata — a 4 KiB
        // stack array on the syscall path is avoidable weight.
        const ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
        let cache = crate::fs::page_cache::get_page_cache();
        if new_size < old_size {
            // Zero EVERY live frame byte in [new_size, old_size) — not
            // just the partial tail: fully-dropped pages' frames can
            // survive as invalidated-but-mapped aliases, and a later
            // regrow's fault REVIVES exactly those frames
            // (get_stale_revive); they must read as zeros then, not as
            // pre-truncate data.
            {
                let mut off = new_size as u64;
                let end = old_size as u64;
                while off < end {
                    let n = (end - off).min(PAGE_SIZE as u64) as usize;
                    cache.writeback_update_range_stale(
                        self.fs_id,
                        self.ino,
                        off,
                        &ZERO_PAGE[..n],
                    );
                    off += n as u64;
                }
            }
            let first_dropped =
                (new_size + PAGE_SIZE - 1) / PAGE_SIZE; // ceil: pages fully beyond new_size
            cache.invalidate_inode_range(self.fs_id, self.ino, first_dropped as u64);
        } else if new_size > old_size {
            let mut off = old_size as u64;
            let end = new_size as u64;
            while off < end {
                let n = (end - off).min(PAGE_SIZE as u64) as usize;
                cache.writeback_update_range_stale(self.fs_id, self.ino, off, &ZERO_PAGE[..n]);
                off += n as u64;
            }
        }

        self.mtime.store(wall_secs(), Ordering::Release);
    }

    /// Reserve one page of the mount's `size=` budget (used_pages +1).
    ///
    /// CAS against `used_pages` so two concurrent writers cannot both
    /// squeeze past the limit. Returns false when the budget is full —
    /// the caller must not allocate. Unlimited mounts (page_limit 0)
    /// always succeed and still count usage (statfs/df later).
    fn reserve_page(&self) -> bool {
        if self.sb.is_null() {
            // Root node before stamp_final_address: directories never
            // take data pages, and the root is always a directory.
            return true;
        }
        unsafe {
            let sb = &*self.sb;
            let limit = sb.page_limit_pages.load(Ordering::Acquire);
            if limit == 0 {
                sb.used_pages.fetch_add(1, Ordering::AcqRel);
                return true;
            }
            let mut cur = sb.used_pages.load(Ordering::Acquire);
            loop {
                if cur >= limit {
                    return false;
                }
                match sb
                    .used_pages
                    .compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => return true,
                    Err(actual) => cur = actual,
                }
            }
        }
    }
}

// ============================================================================
// Superblock & instances
// ============================================================================

pub struct TmpfsSuperBlock {
    pub sb: SuperBlock,
    pub root_node: Arc<TmpfsNode>,
    next_ino: AtomicU64,
    /// Instance id (icache key low bits).
    fs_id: u64,
    /// Page budget from the mount option string (`size=`/`nr_blocks=`),
    /// in 4 KiB pages. 0 = unlimited (Linux's default is half of RAM,
    /// which for this heap-backed tmpfs means "bounded by the kernel
    /// heap" — the pre-fix behavior).
    pub(crate) page_limit_pages: AtomicU64,
    /// Live data pages across ALL files of this mount. Incremented by
    /// TmpfsNode::reserve_page, decremented by TmpfsPage::drop.
    pub(crate) used_pages: AtomicU64,
}

impl TmpfsSuperBlock {
    pub(crate) fn new(instance: u64, page_limit_pages: u64) -> Self {
        let fs_id = FS_ID_TMPFS_BASE | instance;
        let root_node = Arc::new(TmpfsNode::new(
            b"/".to_vec(),
            TmpfsType::Directory,
            1,
            fs_id,
            core::ptr::null(), // patched by stamp_final_address() once boxed
        ));
        let mut sb = SuperBlock::new(PAGE_SIZE as usize, TMPFS_MAGIC);
        sb.set_flags(SuperBlockFlags::new(SuperBlockFlags::SB_ACTIVE));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),
            fs_id,
            page_limit_pages: AtomicU64::new(page_limit_pages),
            used_pages: AtomicU64::new(0),
        }
    }

    /// Stamp the root node's back-pointer with the superblock's FINAL
    /// address. Must be called after the value is placed at its permanent
    /// home (Box) — a pointer taken to `self` before the move would be a
    /// dangling stack address. No child nodes exist yet at mount time, so
    /// only the root needs stamping (children get the correct `sb` from
    /// `new_node`, which is only reachable through the final address).
    ///
    /// # Safety (caller obligations)
    /// `self_ptr` must point at this superblock's final storage and the
    /// tree must not yet be published to other threads.
    unsafe fn stamp_final_address(self_ptr: *mut TmpfsSuperBlock) {
        // SAFETY: caller guarantees self_ptr is the final, valid address.
        let root = Arc::as_ptr(&(*self_ptr).root_node) as *mut TmpfsNode;
        // SAFETY: the tree is unpublished; the root node is uniquely
        // reachable from this thread.
        (*root).sb = self_ptr as *const TmpfsSuperBlock;
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::AcqRel)
    }

    /// Create a fresh node stamped with this instance's identity.
    pub(crate) fn new_node(&self, name: Vec<u8>, node_type: TmpfsType) -> Arc<TmpfsNode> {
        Arc::new(TmpfsNode::new(
            name,
            node_type,
            self.alloc_ino(),
            self.fs_id,
            self as *const TmpfsSuperBlock,
        ))
    }
}

/// Instance counter + registry of leaked superblocks (mount-lifetime).
static TMPFS_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Parse the tmpfs page budget out of a mount(2) options string.
///
/// Recognized (Linux tmpfs option table): `size=<bytes>` and the legacy
/// alias `nr_blocks=<pages>`, each with optional k/K, m/M, g/G suffixes
/// (powers of 1024, case-insensitive — mount(8) normalizes to lower
/// case, tmpfs.kopt accepts both). `size` is rounded UP to whole pages,
/// like Linux. A percentage (`size=50%`) maps to the kernel heap's page
/// count (Linux uses totalram; the heap is what tmpfs draws from here).
/// Everything else in the string (mode=, nr_inodes=, mpol=...) is
/// accepted and ignored, like tmpfs does for options it does not
/// implement. Returns 0 when no size option is present (unlimited —
/// Linux's default is RAM/2, i.e. effectively unlimited for a heap of
/// the same box's RAM).
pub(crate) fn parse_size_pages(data: Option<&str>) -> u64 {
    let data = match data {
        Some(d) if !d.is_empty() => d,
        _ => return 0,
    };
    for token in data.split(',') {
        let token = token.trim();
        let (key, value) = match token.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim().to_ascii_lowercase();
        if key != "size" && key != "nr_blocks" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (digits, percent) = match value.strip_suffix('%') {
            Some(d) => (d, true),
            None => (value, false),
        };
        let digits = digits.trim();
        let (num_str, mult) = match digits.as_bytes().last() {
            Some(b'k') | Some(b'K') => (&digits[..digits.len() - 1], 1024u64),
            Some(b'm') | Some(b'M') => (&digits[..digits.len() - 1], 1024 * 1024),
            Some(b'g') | Some(b'G') => (&digits[..digits.len() - 1], 1024 * 1024 * 1024),
            _ => (digits, 1),
        };
        let num: u64 = match num_str.trim().parse() {
            Ok(n) => n,
            Err(_) => continue, // not a number: ignore the option
        };
        return if percent {
            // percentage of the kernel heap (page count) — Linux maps a
            // percentage onto totalram; the heap is what tmpfs draws
            // from here.
            let heap_pages = (crate::config::KERNEL_HEAP_SIZE / PAGE_SIZE) as u64;
            num.min(100) * heap_pages / 100
        } else if key == "nr_blocks" {
            // nr_blocks counts PAGES (the pre-size= ABI), memparse
            // suffix included.
            num.saturating_mul(mult)
        } else {
            // size= counts bytes; round UP to whole pages like Linux.
            num.saturating_mul(mult).div_ceil(PAGE_SIZE as u64)
        };
    }
    0
}

/// Wrapper so the raw-pointer registry is `Send` (access only under the
/// Spinlock; pointers are leaked mount-lifetime allocations, never freed).
pub(super) struct TmpfsInstances {
    list: Vec<*const TmpfsSuperBlock>,
}

static TMPFS_INSTANCES: Spinlock<TmpfsInstances> =
    Spinlock::new(TmpfsInstances { list: Vec::new() });

/// Create a new independent tmpfs instance and return its root VFS inode.
/// Called by `mount::do_mount("tmpfs")`. `data` is the mount(2) options
/// string ("size=32M,mode=700,..."); None = defaults (unlimited).
pub fn create_mount_instance(data: Option<&str>) -> Arc<Inode> {
    let instance = TMPFS_INSTANCE_COUNTER.fetch_add(1, Ordering::AcqRel);
    let sb = Box::new(TmpfsSuperBlock::new(instance, parse_size_pages(data)));
    let sb_ptr = Box::into_raw(sb) as *mut TmpfsSuperBlock; // leaked: mount lifetime
    // Stamp the root's sb back-pointer with the FINAL (heap) address.
    // SAFETY: sb_ptr is this superblock's final storage; the tree is not
    // yet published (no dentry/inode references exist yet).
    unsafe { TmpfsSuperBlock::stamp_final_address(sb_ptr); }
    TMPFS_INSTANCES.lock_irqsave().list.push(sb_ptr as *const TmpfsSuperBlock);

    // SAFETY: sb_ptr is a leaked, valid superblock.
    let root_node = unsafe { (*sb_ptr).root_node.clone() };
    tmpfs_make_inode(&root_node)
}

/// Retag a FRESH tmpfs mount root with a filesystem-type ops twin
/// (identity tag for statfs magic dispatch — see MQUEUE_INODE_OPS).
/// Valid only immediately after create_mount_instance, while the Arc
/// is still uniquely owned.
pub fn tag_mount_instance_type(root: &mut Arc<Inode>, ops: &'static INodeOps) {
    match Arc::get_mut(root) {
        Some(inode) => inode.ops = Some(ops),
        None => crate::println!("tmpfs: instance root already shared; type tag skipped"),
    }
}

/// Build the VFS inode view of a tmpfs node (shared by mount + iget).
fn tmpfs_make_inode(node: &Arc<TmpfsNode>) -> Arc<Inode> {
    let mut inode = Inode::new(node.ino, InodeMode::new(*node.mode.lock()));
    inode.fs_id = node.fs_id;
    inode.size.store(node.file_size(), Ordering::Release);
    inode.uid.store(node.uid.load(Ordering::Relaxed), Ordering::Relaxed);
    inode.gid.store(node.gid.load(Ordering::Relaxed), Ordering::Relaxed);
    // Stamp the device number so mmap(2) can identify /dev/zero-style
    // nodes through the inode's rdev without a getattr round trip (the
    // devfs discipline — see devfs_iget).
    inode.rdev = node.rdev.load(Ordering::Relaxed);
    inode.private_data = Some(Arc::into_raw(Arc::clone(node)) as *mut u8);
    inode.ops = Some(&TMPFS_INODE_OPS);
    Arc::new(inode)
}

// ============================================================================
// Mount / filesystem type registration
// ============================================================================

/// tmpfs mount callback (FsRegistry path). The registry path never wires a
/// dentry tree — it only validates the type name — so a plain SuperBlock is
/// returned. Real instances (with their trees) come from
/// `create_mount_instance()` via `mount::do_mount`.
// SAFETY: FsContext is a valid reference from the registry mount path.
fn tmpfs_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    Ok(Box::into_raw(Box::new(SuperBlock::new(PAGE_SIZE, TMPFS_MAGIC))))
}

pub static TMPFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "tmpfs",
    Some(tmpfs_mount),
    None,
    0,
);

static TMPFS_REGISTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Register the "tmpfs" filesystem type in the FsRegistry (idempotent).
/// Needed so the registry-based mount path (superblock::do_mount) accepts
/// the type name; sys_mount dispatches by string via mount::do_mount.
pub fn ensure_registered() {
    if !TMPFS_REGISTERED.swap(true, Ordering::AcqRel) {
        let _ = crate::fs::superblock::register_filesystem(&TMPFS_FS_TYPE);
    }
}

// ============================================================================
// Inode operations
// ============================================================================

/// Recover the node from a VFS inode's private_data.
// SAFETY: private_data was installed by tmpfs_make_inode / tmpfs_iget as an
// Arc::into_raw reference; it stays alive at least as long as the inode.
fn node_of<'a>(inode: &'a Inode) -> Result<&'a TmpfsNode, i32> {
    match crate::fs::edges::privdata_ref::<TmpfsNode>(inode) {
        Some(node) => Ok(node),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// Count directory entries referencing `ino` (st_nlink for hard links);
/// directories report 2 + subdirectory count (POSIX). Walks the owning
/// instance's tree — tmpfs trees are small (shm segments, container /tmp).
fn tmpfs_count_links(root: &TmpfsNode, ino: u64) -> u32 {
    fn walk(node: &TmpfsNode, ino: u64, count: &mut u32) {
        if node.is_dir() {
            for child in node.list_children() {
                walk(&child, ino, count);
            }
        } else if node.ino == ino {
            *count += 1;
        }
    }
    let mut count = 0u32;
    walk(root, ino, &mut count);
    count.max(1)
}

/// Reject renaming a directory into its own subtree (cycle guard).
fn tmpfs_subtree_contains(node: &TmpfsNode, candidate: &TmpfsNode) -> bool {
    if core::ptr::eq(node, candidate) {
        return true;
    }
    if node.is_dir() {
        for child in node.list_children() {
            if tmpfs_subtree_contains(&child, candidate) {
                return true;
            }
        }
    }
    false
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    match node.find_child(name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory),
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_create(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    // sb is a leaked mount-lifetime superblock (INV-FS-GLOBALSB-1
    // provenance; resolved through the framework edge).
    let sb = crate::fs::edges::static_ref_unchecked(node.sb as *mut TmpfsSuperBlock);

    let file = sb.new_node(name.to_vec(), TmpfsType::RegularFile);
    // Honor the caller's permission bits (umask is applied by the VFS layer).
    *file.mode.lock() = InodeMode::S_IFREG | (mode.bits() & 0o7777);
    node.add_child(file.clone());
    Ok(tmpfs_make_inode(&file))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_mkdir(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    // sb is a leaked mount-lifetime superblock (INV-FS-GLOBALSB-1
    // provenance; resolved through the framework edge).
    let sb = crate::fs::edges::static_ref_unchecked(node.sb as *mut TmpfsSuperBlock);

    let d = sb.new_node(name.to_vec(), TmpfsType::Directory);
    *d.mode.lock() = InodeMode::S_IFDIR | (mode.bits() & 0o7777);
    node.add_child(d.clone());
    Ok(tmpfs_make_inode(&d))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_symlink(dir: &Inode, name: &[u8], target: &[u8]) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    // sb is a leaked mount-lifetime superblock (INV-FS-GLOBALSB-1
    // provenance; resolved through the framework edge).
    let sb = crate::fs::edges::static_ref_unchecked(node.sb as *mut TmpfsSuperBlock);

    let link = sb.new_node(name.to_vec(), TmpfsType::SymbolicLink);
    // SAFETY: link is exclusively owned here (not yet in the tree).
    unsafe {
        let link_mut = Arc::as_ptr(&link) as *mut TmpfsNode;
        (*link_mut).link_target = Some(target.to_vec());
    }
    node.add_child(link.clone());
    Ok(tmpfs_make_inode(&link))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_mknod(
    dir: &Inode,
    name: &[u8],
    mode: InodeMode,
    dev: u64,
) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }

    // may_mknod (Linux): S_IFDIR → EPERM, S_IFLNK/unknown → EINVAL; a
    // zero type field translates to S_IFREG (do_mknodat).
    // (The syscall layer already rejects those for the syscall path; the
    // op-level check keeps direct VFS callers honest.)
    let ftype = mode.bits() & InodeMode::S_IFMT;
    match ftype {
        InodeMode::S_IFCHR | InodeMode::S_IFBLK | InodeMode::S_IFIFO
        | InodeMode::S_IFSOCK | InodeMode::S_IFREG | 0 => {}
        InodeMode::S_IFDIR => return Err(errno::Errno::OperationNotPermitted),
        _ => return Err(errno::Errno::InvalidArgument),
    }
    let ftype = if ftype == 0 { InodeMode::S_IFREG } else { ftype };

    // sb is a leaked mount-lifetime superblock (INV-FS-GLOBALSB-1
    // provenance; resolved through the framework edge).
    let sb = crate::fs::edges::static_ref_unchecked(node.sb as *mut TmpfsSuperBlock);

    // Device nodes are stored as RegularFile nodes whose MODE word carries
    // the type (the same representation the ATTR_MODE retype path uses for
    // mkfifo'd FIFOs) plus the rdev in the node. Reads/writes never touch
    // them: open() binds through the CharDev registry (get_file_ops).
    let file = sb.new_node(name.to_vec(), TmpfsType::RegularFile);
    *file.mode.lock() = ftype | (mode.bits() & 0o7777);
    file.rdev.store(dev, Ordering::Release);
    node.add_child(file.clone());
    Ok(tmpfs_make_inode(&file))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_link(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    let dir_node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let target_node = match node_of(target) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    if target_node.is_dir() {
        return errno::Errno::IsADirectory.as_neg_i32();
    }
    if dir_node.find_child(name).is_some() {
        return errno::Errno::FileExists.as_neg_i32();
    }

    // Hard link: a new directory entry with the SAME ino and the SHARED
    // page map (POSIX visibility semantics; pages Arc cloned, not copied).
    let mut new_node = TmpfsNode::new(
        name.to_vec(),
        target_node.node_type,
        target_node.ino,
        target_node.fs_id,
        target_node.sb,
    );
    new_node.pages = target_node.pages.clone();
    new_node.size.store(target_node.file_size(), Ordering::Release);
    *new_node.mode.lock() = *target_node.mode.lock();
    new_node.rdev.store(target_node.rdev.load(Ordering::Acquire), Ordering::Release);
    // SAFETY: link_target is set before publication (add_child).
    let new_link = {
        let p = &mut new_node as *mut TmpfsNode;
        unsafe { (*p).link_target = target_node.link_target.clone(); }
        Arc::new(new_node)
    };
    dir_node.add_child(new_link);
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_unlink(dir: &Inode, name: &[u8]) -> i32 {
    let node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    match node.find_child(name) {
        Some(child) => {
            if child.is_dir() {
                return errno::Errno::IsADirectory.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_rmdir(dir: &Inode, name: &[u8]) -> i32 {
    let node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    match node.find_child(name) {
        Some(child) => {
            if !child.is_dir() {
                return errno::Errno::NotADirectory.as_neg_i32();
            }
            if !child.list_children().is_empty() {
                return errno::Errno::DirectoryNotEmpty.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_rename(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    let old_dir_node = match node_of(old_dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let new_dir_node = match node_of(new_dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !old_dir_node.is_dir() || !new_dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    let source = match old_dir_node.find_child(old_name) {
        Some(n) => n,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };

    // Cycle guard: destination directory must not live inside source's
    // subtree (renaming a dir into its own child orphans it).
    if source.is_dir() && tmpfs_subtree_contains(&source, new_dir_node) {
        return errno::Errno::InvalidArgument.as_neg_i32();
    }

    // POSIX rename overwrites an existing destination (unless it is a
    // non-empty directory).
    if let Some(existing) = new_dir_node.find_child(new_name) {
        if existing.is_dir() {
            if !existing.list_children().is_empty() {
                return errno::Errno::DirectoryNotEmpty.as_neg_i32();
            }
        }
        new_dir_node.remove_child(new_name);
    }

    // Remove under the OLD name first (remove_child matches by name), then
    // rename, then add — source is exclusively held between the two.
    if !old_dir_node.remove_child(old_name) {
        return errno::Errno::NoSuchFileOrDirectory.as_neg_i32();
    }
    source.set_name(new_name.to_vec());
    new_dir_node.add_child(source);
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e as isize,
    };
    if !node.is_symlink() {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }
    match &node.link_target {
        Some(target) => {
            let len = target.len().min(buf.len());
            buf[..len].copy_from_slice(&target[..len]);
            len as isize
        }
        None => errno::Errno::IOError.as_neg_i32() as isize,
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e,
    };
    stat.st_ino = node.ino;
    stat.st_mode = *node.mode.lock();
    stat.st_size = node.file_size() as i64;
    stat.st_nlink = if node.is_dir() {
        // 2 ('.' + parent entry) + one per subdirectory.
        2 + node.list_children().iter().filter(|c| c.is_dir()).count() as u32
    } else {
        // Hard links share the ino; count referencing entries.
        let sb = node.sb as *const TmpfsSuperBlock;
        // SAFETY: sb is a leaked mount-lifetime superblock.
        unsafe { tmpfs_count_links(&(*sb).root_node, node.ino) }
    };
    stat.st_uid = node.uid.load(Ordering::Relaxed) as u32;
    stat.st_gid = node.gid.load(Ordering::Relaxed) as u32;
    // st_rdev: the userspace dev_t of mknod-created device nodes (0 for
    // everything else) — major()/minor() decode in userspace.
    stat.st_rdev = node.rdev.load(Ordering::Relaxed);
    stat.st_blksize = PAGE_SIZE as i64;
    stat.st_blocks = (node.page_count() * (PAGE_SIZE as u64 / 512)) as i64;
    stat.st_atime = node.atime.load(Ordering::Relaxed) as i64;
    stat.st_atime_nsec = 0;
    stat.st_mtime = node.mtime.load(Ordering::Relaxed) as i64;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = node.mtime.load(Ordering::Relaxed) as i64;
    stat.st_ctime_nsec = 0;
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_setattr(inode: &Inode, attr: u32, arg1: u64, arg2: u64) -> i32 {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match attr {
        setattr_attr::ATTR_SIZE => {
            node.truncate(arg1 as usize);
            inode.size.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MODE => {
            // Honor type bits in `value` — AF_UNIX bind(2) and mkfifo(3)
            // create their special nodes by create-regular-then-retype
            // (the mknod pattern). A perm-only word (chmod) preserves the
            // current file type from the stored mode word, like Linux.
            let v = arg1 as u32;
            let vtype = v & InodeMode::S_IFMT;
            let file_type = if vtype != 0 {
                vtype
            } else {
                *node.mode.lock() & InodeMode::S_IFMT
            };
            let new_mode = file_type | (v & 0o7777);
            *node.mode.lock() = new_mode;
            // Icache coherency: open()/DAC read the cached Inode.mode
            // while stat() reads through getattr (node.mode) — keep both
            // in sync or retyped FIFOs stay "regular" for open.
            inode.update_cached_mode(InodeMode::new(new_mode));
            node.mtime.store(wall_secs(), Ordering::Release);
            0
        }
        setattr_attr::ATTR_ATIME => {
            node.atime.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MTIME => {
            node.mtime.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_UID_GID => {
            // arg1 = uid, arg2 = gid (same encoding as ext4_setattr; the
            // chown syscall and the VFS setgid-directory gid inheritance
            // both use it). u32::MAX means "leave unchanged" (chown(2)
            // with -1). Keep the VFS-cached owner in sync — DAC and owner
            // checks read Inode.uid/gid while stat() goes through getattr.
            let old_uid = node.uid.load(Ordering::Relaxed);
            let old_gid = node.gid.load(Ordering::Relaxed);
            let new_uid = if arg1 == u32::MAX as u64 {
                old_uid
            } else {
                arg1 as u32
            };
            let new_gid = if arg2 == u32::MAX as u64 {
                old_gid
            } else {
                arg2 as u32
            };
            node.uid.store(new_uid, Ordering::Relaxed);
            node.gid.store(new_gid, Ordering::Relaxed);
            inode.uid.store(new_uid, Ordering::Relaxed);
            inode.gid.store(new_gid, Ordering::Relaxed);
            node.mtime.store(wall_secs(), Ordering::Release);
            0
        }
        _ => -95, // EOPNOTSUPP (matches rootfs_setattr's fallback)
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
    let node = node_of(inode).ok()?;
    if !node.is_dir() {
        return None;
    }
    let mut entries = Vec::new();
    for child in node.list_children() {
        // d_type from the MODE word, not the node type: FIFOs and unix
        // sockets are created as RegularFile nodes and retyped through
        // chmod-style ATTR_MODE (mkfifo / AF_UNIX bind). Deriving from
        // node_type would hide them as DT_REG — readdir on /run must show
        // bound sockets (DT_SOCK) and named pipes (DT_FIFO).
        let mode = *child.mode.lock();
        let dt = match mode & InodeMode::S_IFMT {
            InodeMode::S_IFDIR => file_type::DT_DIR,
            InodeMode::S_IFLNK => file_type::DT_LNK,
            InodeMode::S_IFIFO => file_type::DT_FIFO,
            InodeMode::S_IFSOCK => file_type::DT_SOCK,
            InodeMode::S_IFCHR => file_type::DT_CHR,
            InodeMode::S_IFBLK => file_type::DT_BLK,
            _ => {
                if child.is_dir() {
                    file_type::DT_DIR
                } else if child.is_symlink() {
                    file_type::DT_LNK
                } else {
                    file_type::DT_REG
                }
            }
        };
        entries.push(VfsDirEntry {
            ino: child.ino,
            name: child.name().to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(parent)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    let child = node
        .find_child(name)
        .filter(|c| c.ino == ino)
        .ok_or_else(|| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    Ok(tmpfs_make_inode(&child))
}

// SAFETY: VFS callback contract — called when the inode's refcount hits 0.
fn tmpfs_destroy_inode(inode: &mut Inode) {
    // Drop any MAP_SHARED page-cache aliases of this file (the frames
    // stay alive while user PTEs pin them; the ENTRIES must not survive
    // the inode — a recycled (fs_id, ino) pair would otherwise observe
    // the dead file's frames).
    crate::fs::page_cache::get_page_cache()
        .invalidate_inode(inode.fs_id, inode.ino as u64);
    if let Some(ptr) = inode.private_data.take() {
        // Reclaim the leaked Arc reference (idempotent via Option::take).
        // SAFETY: private_data holds an Arc leaked by into_raw at inode
        // creation (INV-FS-PRIVDATA-1); this Drop is the single reclaim.
        let _ = unsafe { Arc::from_raw(ptr as *const TmpfsNode) };
    }
}

// ============================================================================
// File operations (regular files)
// ============================================================================

/// tmpfs regular-file read: page-granular, zero-fill holes, update f_pos.
fn tmpfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: inode/private_data installed by tmpfs_make_inode; the File
    // holds an Arc<Inode> keeping the node reference alive.
    {

    let inode = match crate::fs::edges::file_inode(file) {
        Some(i) => i,
        None => return -9,
        };
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(_) => return -9,
    };
    // Same pos-race serialization as reg_file_read: forked children
    // sharing this open file description must not lose offset updates
    // (LTP fork07 — see File::io_lock).
    let _io_guard = file.io_lock.guard();
    let offset = file.get_pos() as usize;
    let n = node.read_at(offset, buf);
    if n > 0 {
        file.set_pos((offset + n) as u64);
    }
    // TEST-ONLY (gdmr24 dbtrace, removed before merge): udev db
    // files under /run/udev/data read back with "G:\n" spliced
    // before each tag line — sd-device parse aborts the tag
    // (EINVAL), so card0 loses master-of-seat in enumeration.
    // Trace db-shaped reads: first byte 'I'/'G'/'Q'/'S'/'E'/'V'
    // followed by ':' narrows to db lines.
    if n > 0
        && buf.len() > 1
        && matches!(buf[0], b'I' | b'G' | b'Q' | b'S' | b'E' | b'V')
        && buf[1] == b':'
        && crate::trace::EVENT_DBTRACE.armed()
    {
        let show = n.min(80);
        crate::println!(
            "DBR off={} n={} {:?}..",
            offset,
            n,
            core::str::from_utf8(&buf[..show]).map(|s| s.replace('\n', "\\n")).unwrap_or_else(|_| "<bin>".into())
        );
    }
    n as isize

    }
}

/// tmpfs regular-file write: demand page allocation, O_APPEND honored by
/// the shared reg-file convention (io_lock + end positioning).
///
/// A zero-byte write against a full `size=` budget returns ENOSPC
/// (POSIX: "if some bytes can be written, the write shall return the
/// number ... otherwise -1 with ENOSPC"). Short counts are returned as
/// written and the caller retries — the next call hits ENOSPC.
fn tmpfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: see tmpfs_file_read.
    {

    let inode = match crate::fs::edges::file_inode(file) {
        Some(i) => i,
        None => return -9,
        };
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(_) => return -9,
    };
    let _write_guard = file.io_lock.guard();
    let offset = if file.flags_bits() & crate::fs::FileFlags::O_APPEND != 0 {
        let end = node.file_size();
        file.set_pos(end);
        end as usize
    } else {
        file.get_pos() as usize
    };
    // TEST-ONLY (gdmr24 dbtrace, removed before merge): see the
    // matching hook in tmpfs_file_read.
    if !buf.is_empty()
        && matches!(buf[0], b'I' | b'G' | b'Q' | b'S' | b'E' | b'V')
        && buf.get(1) == Some(&b':')
        && crate::trace::EVENT_DBTRACE.armed()
    {
        let show = buf.len().min(80);
        crate::println!(
            "DBW off={} len={} {:?}..",
            offset,
            buf.len(),
            core::str::from_utf8(&buf[..show]).map(|s| s.replace('\n', "\\n")).unwrap_or_else(|_| "<bin>".into())
        );
    }
    let written = node.write_at(offset, buf);
    inode.size.store(node.file_size(), Ordering::Release);
    if written == 0 && !buf.is_empty() {
        // The mount's page budget (size=) is exhausted and this write
        // could not store a single byte. (write_at only returns 0 for
        // an empty buffer otherwise, handled above.)
        return crate::errno::Errno::NoSpaceLeftOnDevice.as_neg_i32() as isize;
    }
    if written > 0 {
        file.set_pos((offset + written) as u64);
    }
    written as isize

    }
}

fn tmpfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    let file_size = match crate::fs::edges::file_inode(file) {
        Some(inode) => match node_of(inode) {
            Ok(node) => node.file_size() as isize,
            Err(_) => return -9,
        },
        None => return -9,
    };
    let current_pos = file.get_pos() as isize;
    let new_pos = match whence {
        0 => offset,
        1 => current_pos + offset,
        2 => file_size + offset,
        _ => return -22,
    };
    if new_pos < 0 {
        return -22;
    }
    file.set_pos(new_pos as u64);
    new_pos
}

fn tmpfs_file_close(_file: &crate::fs::File) -> i32 {
    0
}

/// tmpfs file operations table (directories use the shared DIR_FILE_OPS).
pub static TMPFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(tmpfs_file_read)
    .write(tmpfs_file_write)
    .lseek(tmpfs_file_lseek)
    .close(tmpfs_file_close)
    .read_at(tmpfs_file_read_at)
    .build();

/// Is `file` a regular file on a tmpfs mount (ops-table identity, same
/// discipline as ext4's is_ext4_file)? Gates the MAP_SHARED page-cache
/// alias arm in the fault paths: LTP's harness prefers /dev/shm for its
/// shared results/checkpoint page (setup_ipc), so coherent cross-process
/// mmap of tmpfs files is load-bearing for the whole checkpoint-based
/// test family (execl01..execveat02, waitid08, ...).
pub fn is_tmpfs_file(file: &crate::fs::File) -> bool {
    file.get_ops()
        .is_some_and(|ops| core::ptr::eq(ops as *const _, &TMPFS_FILE_OPS as *const _))
}

/// tmpfs positional read (R71): explicit offset, shared file position
/// untouched — the generic set_pos/read/restore fallback races against
/// concurrent plain reads on the same open file description (demand
/// faults + worker threads), which is how mapped tmpfs pages (dconf's
/// /run/user/*/dc/shm) came back with wrong content.
///
/// SAFETY: inode/private_data installed by tmpfs_make_inode; the File
/// holds an Arc<Inode> keeping the node reference alive.
fn tmpfs_file_read_at(file: &crate::fs::File, offset: u64, buf: &mut [u8]) -> isize {
    {

    let inode = match crate::fs::edges::file_inode(file) {
        Some(i) => i,
        None => return -9,
        };
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(_) => return -9,
    };
    // Serialize with plain read/write for node-page consistency; the
    // file position is never accessed. No IN_ACCESS here: this runs
    // as ops.read_at behind File::read_at, whose native-op arm owns
    // the notify — self-notifying double-fired the event (hidden by
    // the inotify last-event merge except under an interleaving
    // watcher event; review CR-3).
    let _io_guard = file.io_lock.guard();
    let n = node.read_at(offset as usize, buf);
    n as isize

    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
fn tmpfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else if inode.mode.is_regular_file() {
        Some(&TMPFS_FILE_OPS)
    } else if inode.mode.is_char_device() {
        // mknod-created char nodes bind to the CharDev registry through
        // the node's stored rdev (the devfs discipline without the
        // DevfsEntry indirection). No registered driver → ENXIO at open,
        // exactly like devfs.
        let node = node_of(inode).ok()?;
        let rdev = node.rdev.load(Ordering::Relaxed);
        let devno = crate::fs::dev_t::DevNo::from_user_dev(rdev);
        crate::fs::devfs::registry::get_char_device_ops(devno)
    } else {
        // Block devices (and anything else): no block-driver open path —
        // the VFS device-open gate fails with ENXIO, the Linux behavior
        // for a node with no bound driver. FIFOs are dispatched by the
        // open path before get_file_ops; unix sockets by the socket layer.
        None
    }
}

pub static TMPFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_fill: None,
    readdir_cacheable: true,
    open: None,
    permission: None, // default: allow all (DAC via generic path)
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};

// Ops-table twins of TMPFS_INODE_OPS: IDENTICAL callbacks, distinct
// table identity. tmpfs-family mounts that are their own filesystem
// type (POSIX mqueue, securityfs, devpts) carry a magic in statfs that
// systemd verifies by statfs f_type after mount(2) — dev-mqueue.mount
// failed as a unit even with mount(2) == 0 because the instance
// reported TMPFS_MAGIC. Only the ROOT inode of such a mount needs the
// tag: statfs dispatches on the ops identity of the statfs target.
pub static MQUEUE_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_fill: None,
    readdir_cacheable: true,
    open: None,
    permission: None,
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};

/// Securityfs twin (SECURITYFS_MAGIC 0x73636673 — systemd's
/// sys-kernel-security.mount / mount_setup verification).
pub static SECURITYFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_fill: None,
    readdir_cacheable: true,
    open: None,
    permission: None,
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};

/// configfs twin (CONFIGFS_MAGIC 0x61636773 — launcher-errno lane: OH
/// init mounts configfs at /config via storage_daemon.cfg and chowns
/// the hmdfs/sharefs subsystem dirs; the mount previously failed
/// EINVAL on Rux, an ours-only error family). The instance is
/// pre-populated with the two subsystem dirs by the mount arm.
pub static CONFIGFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_fill: None,
    readdir_cacheable: true,
    open: None,
    permission: None,
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};

/// devpts twin (DEVPTS_MAGIC 0x1cd1 — devpts mounts are tmpfs-style
/// instances; glibc's getpt()/posix_openpt probes /dev/pts with
/// statfs).
pub static DEVPTS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_fill: None,
    readdir_cacheable: true,
    open: None,
    permission: None,
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};


