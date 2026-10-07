//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Android/OpenHarmony binder IPC driver — Spike S1 minimal implementation
//!
//! OpenHarmony port plan §7 Spike S1: enough of the Linux binder UAPI
//! (include/uapi/linux/android/binder.h, stable public ABI; semantics
//! mirrored from the OH kernel_linux tree's drivers/android/binder.c) to
//! run a real BC_TRANSACTION/BC_REPLY closed loop between two processes
//! over /dev/binder, including the node/ref handshake and the mmap'd
//! buffer zone with separate sync/async space.
//!
//! Implemented subset:
//! * misc char device /dev/binder (major 10, minor 0xB0)
//! * mmap buffer zone (silently truncated to 4 MiB like Linux binder_mmap);
//!   sync buffers first-fit from the low end, async (TF_ONE_WAY) buffers
//!   from the top half against a free_async_space budget
//! * BINDER_VERSION (protocol 8), BINDER_SET_MAX_THREADS,
//!   BINDER_SET_CONTEXT_MGR, BINDER_THREAD_EXIT, BINDER_WRITE_READ
//! * BC_*: TRANSACTION/REPLY (also _SG with 0 extra buffers), FREE_BUFFER,
//!   INCREFS/ACQUIRE/RELEASE/DECREFS, INCREFS_DONE/ACQUIRE_DONE,
//!   ENTER/EXIT/REGISTER_LOOPER, DEAD_BINDER_DONE (accepted, ignored)
//! * BR_*: ERROR, TRANSACTION, REPLY, DEAD_REPLY, TRANSACTION_COMPLETE,
//!   INCREFS/ACQUIRE/RELEASE/DECREFS, NOOP (leading, like
//!   binder_thread_read), SPAWN_LOOPER, FAILED_REPLY
//! * flat_binder_object translation for BINDER_TYPE_{WEAK_}BINDER and
//!   _HANDLE across processes; the BR_INCREFS/BR_ACQUIRE ->
//!   BC_INCREFS_DONE/BC_ACQUIRE_DONE node handshake works
//! * deferred BR_TRANSACTION_COMPLETE for sync transactions (the sender's
//!   read keeps sleeping until the reply carries it out — one wakeup)
//!
//! Deliberate S1 gaps (documented in the spike report):
//! * one device context (/dev/binder); hwbinder/vndbinder need their OWN
//!   contexts when added — the context-manager node must not be shared
//! * BINDER_TYPE_FD/FDA/PTR rejected with BR_FAILED_REPLY (no cross-process
//!   fd installation)
//! * a second TF_ONE_WAY transaction to a node whose async slot is busy
//!   fails with BR_FAILED_REPLY instead of parking on node->async_todo
//! * no death notifications, no priority inheritance, no freeze,
//!   no oneway-spam detection, no /dev/binderfs, no sender-info exts
//! * one wait queue per process (Linux uses per-thread queues; conforming
//!   userspace cannot observe the difference)
//!
//! Locking: the single WORLD spinlock guards every binder object; no
//! user-memory copies and no sleeping happen under it (BC payloads are
//! copied in before locking, BR payloads are staged into a kernel Vec
//! under the lock and copied out after release). Rux process exit tears
//! down the mm before fds, so the region's last page reference is dropped
//! by binder_close — the io_uring ring lifetime model.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::errno::constants::{EBADF, EBUSY, EFAULT, EAGAIN, EINTR, EINVAL, ESRCH};
use crate::fs::dev_t::{DevNo, MISC_MAJOR};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// UAPI (include/uapi/linux/android/binder.h; LP64/riscv64 _IOC encoding)
// ============================================================================

const fn ioc(dir: u32, typ: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (typ << 8) | nr
}
const fn iow(typ: u32, nr: u32, size: u32) -> u32 {
    ioc(1, typ, nr, size)
}
const fn ior(typ: u32, nr: u32, size: u32) -> u32 {
    ioc(2, typ, nr, size)
}
const fn io(typ: u32, nr: u32) -> u32 {
    ioc(0, typ, nr, 0)
}

pub const BINDER_TYPE_BINDER: u32 = u32::from_be_bytes([b's', b'b', b'*', 0x85]);
pub const BINDER_TYPE_WEAK_BINDER: u32 = u32::from_be_bytes([b'w', b'b', b'*', 0x85]);
pub const BINDER_TYPE_HANDLE: u32 = u32::from_be_bytes([b's', b'h', b'*', 0x85]);
pub const BINDER_TYPE_WEAK_HANDLE: u32 = u32::from_be_bytes([b'w', b'h', b'*', 0x85]);
pub const BINDER_TYPE_FD: u32 = u32::from_be_bytes([b'f', b'd', b'*', 0x85]);

pub const TF_ONE_WAY: u32 = 0x01;
pub const _TF_ROOT_OBJECT: u32 = 0x04;
pub const _TF_STATUS_CODE: u32 = 0x08;
pub const _TF_ACCEPT_FDS: u32 = 0x10;

const BINDER_CURRENT_PROTOCOL_VERSION: i32 = 8;

const SZ_BWR: u32 = 48;
const SZ_TR: u32 = 64;
const SZ_PTR_COOKIE: u32 = 16;
const SZ_U32: u32 = 4;
const SZ_U64: u32 = 8;
const SZ_VERSION: u32 = 4;
const SZ_TR_SG: u32 = 72;

pub const BINDER_WRITE_READ: u32 = ioc(3, b'b' as u32, 1, SZ_BWR);
pub const BINDER_SET_MAX_THREADS: u32 = iow(b'b' as u32, 5, SZ_U32);
pub const BINDER_SET_CONTEXT_MGR: u32 = iow(b'b' as u32, 7, SZ_U32);
pub const BINDER_THREAD_EXIT: u32 = iow(b'b' as u32, 8, SZ_U32);
pub const BINDER_VERSION: u32 = ioc(3, b'b' as u32, 9, SZ_VERSION);

pub const BC_TRANSACTION: u32 = iow(b'c' as u32, 0, SZ_TR);
pub const BC_REPLY: u32 = iow(b'c' as u32, 1, SZ_TR);
pub const BC_FREE_BUFFER: u32 = iow(b'c' as u32, 3, SZ_U64);
pub const BC_INCREFS: u32 = iow(b'c' as u32, 4, SZ_U32);
pub const BC_ACQUIRE: u32 = iow(b'c' as u32, 5, SZ_U32);
pub const BC_RELEASE: u32 = iow(b'c' as u32, 6, SZ_U32);
pub const BC_DECREFS: u32 = iow(b'c' as u32, 7, SZ_U32);
pub const BC_INCREFS_DONE: u32 = iow(b'c' as u32, 8, SZ_PTR_COOKIE);
pub const BC_ACQUIRE_DONE: u32 = iow(b'c' as u32, 9, SZ_PTR_COOKIE);
pub const BC_REGISTER_LOOPER: u32 = io(b'c' as u32, 11);
pub const BC_ENTER_LOOPER: u32 = io(b'c' as u32, 12);
pub const BC_EXIT_LOOPER: u32 = io(b'c' as u32, 13);
pub const BC_DEAD_BINDER_DONE: u32 = iow(b'c' as u32, 16, SZ_U64);
pub const BC_TRANSACTION_SG: u32 = iow(b'c' as u32, 17, SZ_TR_SG);
pub const BC_REPLY_SG: u32 = iow(b'c' as u32, 18, SZ_TR_SG);

pub const BR_ERROR: u32 = ior(b'r' as u32, 0, SZ_U32);
pub const BR_OK: u32 = io(b'r' as u32, 1);
pub const BR_TRANSACTION: u32 = ior(b'r' as u32, 2, SZ_TR);
pub const BR_REPLY: u32 = ior(b'r' as u32, 3, SZ_TR);
pub const BR_DEAD_REPLY: u32 = io(b'r' as u32, 5);
pub const BR_TRANSACTION_COMPLETE: u32 = io(b'r' as u32, 6);
pub const BR_INCREFS: u32 = ior(b'r' as u32, 7, SZ_PTR_COOKIE);
pub const BR_ACQUIRE: u32 = ior(b'r' as u32, 8, SZ_PTR_COOKIE);
pub const BR_RELEASE: u32 = ior(b'r' as u32, 9, SZ_PTR_COOKIE);
pub const BR_DECREFS: u32 = ior(b'r' as u32, 10, SZ_PTR_COOKIE);
pub const BR_NOOP: u32 = io(b'r' as u32, 12);
pub const BR_SPAWN_LOOPER: u32 = io(b'r' as u32, 13);
pub const BR_FAILED_REPLY: u32 = io(b'r' as u32, 17);

/// struct binder_transaction_data (LP64)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BinderTransactionData {
    pub target_handle: u32,
    pub _target_pad: u32,
    pub cookie: u64,
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub data_size: u64,
    pub offsets_size: u64,
    pub data_buffer: u64,
    pub data_offsets: u64,
}
const _: () = assert!(core::mem::size_of::<BinderTransactionData>() == 64);

/// struct binder_write_read (LP64)
#[repr(C)]
pub struct BinderWriteRead {
    pub write_size: u64,
    pub write_consumed: u64,
    pub write_buffer: u64,
    pub read_size: u64,
    pub read_consumed: u64,
    pub read_buffer: u64,
}
const _: () = assert!(core::mem::size_of::<BinderWriteRead>() == 48);

/// struct flat_binder_object (24 bytes on LP64)
#[repr(C)]
pub struct FlatBinderObject {
    pub hdr_type: u32,
    pub flags: u32,
    pub handle: u32,
    pub _handle_pad: u32,
    pub cookie: u64,
}
const _: () = assert!(core::mem::size_of::<FlatBinderObject>() == 24);

const LOOPER_REGISTERED: u32 = 0x01;
const LOOPER_ENTERED: u32 = 0x02;
const LOOPER_EXITED: u32 = 0x04;

/// /dev/binder misc device number (Linux uses dynamic minors; 0xB0 is
/// unused in-tree — loop-control owns 237 on major 10).
pub const BINDER_MINOR: u32 = 0xB0;
pub const DEV_BINDER: DevNo = DevNo::new(MISC_MAJOR, BINDER_MINOR);

const BINDER_MMAP_MAX: usize = 4 << 20;
const PAGE_SIZE: usize = crate::mm::page::PAGE_SIZE;

fn align8(v: usize) -> usize {
    (v + 7) & !7
}

// ============================================================================
// Core state — everything behind UnsafeCell is mutated ONLY under WORLD.
// ============================================================================

struct NodeState {
    internal_strong_refs: i32,
    local_strong_refs: i32,
    local_weak_refs: i32,
    has_strong_ref: bool,
    has_weak_ref: bool,
    pending_strong_ref: bool,
    pending_weak_ref: bool,
    /// TF_ONE_WAY serialization: one async transaction in flight per node
    /// (Linux node->has_async_transaction; S1 rejects extra oneways instead
    /// of parking them on node->async_todo).
    has_async_transaction: bool,
}

struct BinderNode {
    id: u64,
    ptr: u64,
    cookie: u64,
    proc_id: u64,
    st: core::cell::UnsafeCell<NodeState>,
}

impl BinderNode {
    const fn new(id: u64, ptr: u64, cookie: u64, proc_id: u64) -> Self {
        Self {
            id,
            ptr,
            cookie,
            proc_id,
            st: core::cell::UnsafeCell::new(NodeState {
                internal_strong_refs: 0,
                local_strong_refs: 0,
                local_weak_refs: 0,
                has_strong_ref: false,
                has_weak_ref: false,
                pending_strong_ref: false,
                pending_weak_ref: false,
                has_async_transaction: false,
            }),
        }
    }

    /// SAFETY: caller must hold WORLD.
    #[allow(clippy::mut_from_ref)]
    unsafe fn st(&self) -> &mut NodeState {
        &mut *self.st.get()
    }
}

impl NodeState {
    fn strong(&self) -> bool {
        self.internal_strong_refs > 0 || self.local_strong_refs > 0
    }
    fn weak(&self, external_ref_count: usize) -> bool {
        external_ref_count > 0 || self.local_weak_refs > 0 || self.strong()
    }
}

struct BinderRef {
    desc: u32,
    node: Arc<BinderNode>,
    strong: i32,
    weak: i32,
}

struct BufBlock {
    off: usize,
    len: usize,
    free: bool,
    async_block: bool,
    allow_user_free: bool,
    data_size: usize,
    offsets_size: usize,
    /// Oneway (async) buffers keep their transaction alive so the node's
    /// async slot is released when the receiver frees the buffer (Linux
    /// buffer->transaction). Sync buffers rely on the reply path instead.
    txn: Option<Arc<BinderTxn>>,
}

struct BinderAlloc {
    kvirt: *mut u8,
    phys: usize,
    npages: usize,
    order: usize,
    user_base: u64,
    blocks: Vec<BufBlock>,
    free_async_space: usize,
}

impl BinderAlloc {
    /// binder_alloc_new_buf: sync first-fit from the low end, async from
    /// the top half against the free_async_space budget.
    fn new_buf(&mut self, data_size: usize, offsets_size: usize, is_async: bool) -> Option<usize> {
        let need = align8(data_size) + align8(offsets_size);
        if need == 0 {
            return None;
        }
        if is_async && self.free_async_space < need {
            return None;
        }
        let total = self.npages * PAGE_SIZE;
        let mut off: Option<usize> = None;
        for b in self.blocks.iter() {
            if !b.free || b.len < need {
                continue;
            }
            if is_async && b.off < total / 2 {
                continue;
            }
            off = Some(b.off);
            break;
        }
        let o = off?;
        if let Some(b) = self.blocks.iter_mut().find(|b| b.off == o) {
            b.free = false;
            b.async_block = is_async;
            b.allow_user_free = false;
            b.data_size = data_size;
            b.offsets_size = offsets_size;
            if b.len > need {
                let rest = BufBlock {
                    off: o + need,
                    len: b.len - need,
                    free: true,
                    async_block: false,
                    allow_user_free: false,
                    data_size: 0,
                    offsets_size: 0,
                    txn: None,
                };
                b.len = need;
                self.blocks.push(rest);
                self.blocks.sort_by_key(|b| b.off);
            }
        }
        if is_async {
            self.free_async_space -= need;
        }
        Some(o)
    }

    /// binder_alloc_free_buf + adjacent coalescing. Returns Some(txn) when
    /// the buffer was freed and it kept an async (oneway) transaction
    /// alive, Some(none) when freed without one, None when the pointer
    /// does not match a freeable buffer.
    fn free_buf(&mut self, user_ptr: u64) -> Option<Option<Arc<BinderTxn>>> {
        let Some(d) = user_ptr.checked_sub(self.user_base) else {
            return None;
        };
        let d = d as usize;
        let Some(idx) = self.blocks.iter().position(|b| b.off == d && !b.free) else {
            return None;
        };
        if !self.blocks[idx].allow_user_free {
            return None;
        }
        let was_async = self.blocks[idx].async_block;
        let ds = self.blocks[idx].data_size;
        let os = self.blocks[idx].offsets_size;
        let txn = self.blocks[idx].txn.take();
        self.blocks[idx].free = true;
        self.blocks[idx].allow_user_free = false;
        if was_async {
            self.free_async_space += align8(ds) + align8(os);
        }
        // Coalesce (blocks stay sorted by offset).
        let mut i = 0;
        while i + 1 < self.blocks.len() {
            if self.blocks[i].free
                && self.blocks[i + 1].free
                && self.blocks[i].off + self.blocks[i].len == self.blocks[i + 1].off
            {
                self.blocks[i].len += self.blocks[i + 1].len;
                self.blocks.remove(i + 1);
            } else {
                i += 1;
            }
        }
        Some(txn)
    }
}

enum Work {
    TransactionComplete,
    Transaction(Arc<BinderTxn>),
    /// (cmd, param): BR_ERROR carries an s32 payload, the *_REPLY
    /// variants carry none.
    ReturnError(u32, i32),
    /// BINDER_WORK_NODE: ref-state change; the read side derives
    /// BR_INCREFS/ACQUIRE/RELEASE/DECREFS from the counters (like Linux).
    Node(Arc<BinderNode>),
}

struct BinderTxn {
    id: u64,
    from_proc: Option<Arc<BinderProc>>,
    from_tid: Option<u32>,
    to_proc: Arc<BinderProc>,
    /// Set when the transaction is delivered (BR_TRANSACTION): the
    /// receiving thread, which will later issue BC_REPLY.
    to_tid: core::cell::UnsafeCell<Option<u32>>,
    target_node: Option<Arc<BinderNode>>,
    /// Set while this txn holds the node's async slot (TF_ONE_WAY).
    holds_async_slot: bool,
    code: u32,
    flags: u32,
    sender_pid: i32,
    sender_euid: u32,
    data_size: usize,
    offsets_size: usize,
    buffer_off: usize,
}

impl BinderTxn {
    /// SAFETY: caller must hold WORLD.
    unsafe fn to_tid(&self) -> Option<u32> {
        *self.to_tid.get()
    }
    /// SAFETY: caller must hold WORLD.
    unsafe fn set_to_tid(&self, tid: u32) {
        *self.to_tid.get() = Some(tid);
    }
}

struct ThreadState {
    looper: u32,
    todo: VecDeque<Work>,
    /// process_todo: a deferred TRANSACTION_COMPLETE on the todo does NOT
    /// set this — the thread keeps sleeping until real work arrives.
    process_todo: bool,
    /// True while blocked in binder_thread_read waiting for proc work
    /// (Linux waiting_threads list, drives BR_SPAWN_LOOPER).
    waiting_for_proc_work: bool,
    txn_stack: Vec<Arc<BinderTxn>>,
}

struct BinderThread {
    tid: u32,
    st: core::cell::UnsafeCell<ThreadState>,
}

impl BinderThread {
    fn new(tid: u32) -> Self {
        Self {
            tid,
            st: core::cell::UnsafeCell::new(ThreadState {
                looper: 0,
                todo: VecDeque::new(),
                process_todo: false,
                waiting_for_proc_work: false,
                txn_stack: Vec::new(),
            }),
        }
    }

    /// SAFETY: caller must hold WORLD.
    #[allow(clippy::mut_from_ref)]
    unsafe fn st(&self) -> &mut ThreadState {
        &mut *self.st.get()
    }
}

struct ProcInner {
    alloc: Option<BinderAlloc>,
    nodes: Vec<Arc<BinderNode>>,
    refs: Vec<BinderRef>,
    next_desc: u32,
    threads: Vec<Arc<BinderThread>>,
    todo: VecDeque<Work>,
    max_threads: i32,
    requested_threads: i32,
    requested_threads_started: i32,
    is_dead: bool,
}

pub struct BinderProc {
    id: u64,
    pid: u32,
    wait: WaitQueueHead,
    inner: core::cell::UnsafeCell<ProcInner>,
}

impl BinderProc {
    fn new(id: u64, pid: u32) -> Self {
        Self {
            id,
            pid,
            wait: WaitQueueHead::new(),
            inner: core::cell::UnsafeCell::new(ProcInner {
                alloc: None,
                nodes: Vec::new(),
                refs: Vec::new(),
                next_desc: 0,
                threads: Vec::new(),
                todo: VecDeque::new(),
                max_threads: 0,
                requested_threads: 0,
                requested_threads_started: 0,
                is_dead: false,
            }),
        }
    }

    /// SAFETY: caller must hold WORLD.
    #[allow(clippy::mut_from_ref)]
    unsafe fn im(&self) -> &mut ProcInner {
        &mut *self.inner.get()
    }
}

impl Drop for BinderAlloc {
    /// io_uring ring-free discipline: pages carry one owner reference plus
    /// one per live user mapping (taken at mmap). Rux process exit unmaps
    /// before fds close, so by the time we get here the owner reference is
    /// the last one for each page — free the buddy block.
    fn drop(&mut self) {
        let base_pfn = crate::mm::phys_to_pfn(self.phys);
        let mut still_pinned = false;
        for i in 0..self.npages {
            let page = crate::mm::pfn_to_page_mut(base_pfn + i);
            if !page.is_null() {
                // SAFETY: page descriptor exists for this in-RAM page.
                let r = unsafe { (*page).put_page() };
                if r > 0 {
                    still_pinned = true;
                }
            }
        }
        if !still_pinned {
            crate::mm::page_alloc::free_pages(self.phys, self.order);
        }
    }
}

struct World {
    next_id: u64,
    procs: Vec<Arc<BinderProc>>,
    mgr_node: Option<Arc<BinderNode>>,
}

static WORLD: Spinlock<Option<World>> = Spinlock::new(None);
static NEXT_PROC_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

// SAFETY: binder state is shared across CPUs strictly under the WORLD
// spinlock; the wait queues are internally synchronized; the kvirt raw
// pointer is only dereferenced under WORLD while the mmap is alive.
unsafe impl Send for BinderAlloc {}
unsafe impl Sync for BinderAlloc {}
unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}
unsafe impl Send for BinderThread {}
unsafe impl Sync for BinderThread {}
unsafe impl Send for BinderTxn {}
unsafe impl Sync for BinderTxn {}
unsafe impl Send for BinderProc {}
unsafe impl Sync for BinderProc {}

fn world_lock() -> crate::sync::spinlock::SpinlockIrqGuard<'static, Option<World>> {
    let mut g = WORLD.lock_irqsave();
    if g.is_none() {
        *g = Some(World { next_id: 1, procs: Vec::new(), mgr_node: None });
    }
    g
}

/// &mut BinderProc for an Arc found in the world, without borrowing the
/// World — legal because every caller holds WORLD (the access contract).
/// SAFETY: caller must hold WORLD.
#[allow(clippy::mut_from_ref)]
unsafe fn pm<'a>(_world: &World, p: &Arc<BinderProc>) -> &'a mut BinderProc {
    let found = _world.procs.iter().find(|x| Arc::ptr_eq(x, p)).expect("proc not in world");
    &mut *(Arc::as_ptr(found) as *mut BinderProc)
}

fn proc_alive(world: &World, p: &Arc<BinderProc>) -> bool {
    world.procs.iter().any(|x| Arc::ptr_eq(x, p))
}

fn wake_proc(p: &BinderProc) {
    p.wait.wake_up(crate::process::wait::WakeUpHint::Normal, 0);
}

/// Number of refs (across all procs) pointing at a node.
fn node_external_refs(world: &World, node: &Arc<BinderNode>) -> usize {
    // SAFETY: read-only counting under WORLD.
    unsafe {
        world
            .procs
            .iter()
            .map(|p| (*Arc::as_ptr(p)).im().refs.iter().filter(|r| Arc::ptr_eq(&r.node, node)).count())
            .sum()
    }
}

// ============================================================================
// open / close / mmap / poll
// ============================================================================

/// Borrow the open's BinderProc Arc without changing the refcount.
/// SAFETY: ptr came from Arc::into_raw and the fd still owns it.
unsafe fn proc_ref(file: &crate::fs::File) -> Option<Arc<BinderProc>> {
    let raw = (*file.private_data.get())? as *const BinderProc;
    let arc = core::mem::ManuallyDrop::new(Arc::from_raw(raw));
    // SAFETY: ManuallyDrop never drops, so the count is unchanged; the
    // clone below is a proper +1 that the caller owns.
    Some((*arc).clone())
}

/// Called from devfs_open: allocate the per-open process context.
pub fn binder_open(file: &crate::fs::File) -> i32 {
    let pid = crate::process::current_pid();
    let id = NEXT_PROC_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let proc = Arc::new(BinderProc::new(id, pid));
    {
        let mut w = world_lock();
        w.as_mut().unwrap().procs.push(proc.clone());
    }
    crate::pr_info!("binder: pid {} opened the device (proc {})", pid, id);
    // SAFETY: private_data slot written once at open; reclaimed in
    // binder_close (the only other accessor).
    file.set_private_data(Arc::into_raw(proc) as *mut u8);
    0
}

/// FileOps.close: drop the process context. Drains all work lists (breaking
/// txn->proc Arc cycles), fails in-flight transactions targeting or
/// originating here, and releases the mmap region.
pub fn binder_close(file: &crate::fs::File) -> i32 {
    // SAFETY: private_data holds the Arc<BinderProc> from binder_open.
    let raw = match unsafe { *file.private_data.get() } {
        Some(p) => p as *const BinderProc,
        None => return 0,
    };
    // SAFETY: reclaim the single strong reference stashed at open.
    let proc = unsafe { Arc::from_raw(raw) };
    unsafe { *file.private_data.get() = None };

    let mut wake_targets: Vec<Arc<BinderProc>> = Vec::new();
    {
        let mut w = world_lock();
        let world = w.as_mut().unwrap();
        world.procs.retain(|p| !Arc::ptr_eq(p, &proc));
        if let Some(mgr) = &world.mgr_node {
            if mgr.proc_id == proc.id {
                world.mgr_node = None;
            }
        }
        // SAFETY: all inner access below is under WORLD.
        unsafe {
            proc.im().is_dead = true;

            // Fail in-flight transactions of OTHER procs that involve us.
            for p in world.procs.clone() {
                let mut needs_wake = false;
                for t in pm(world, &p).im().threads.clone() {
                    let th = t.st();
                    th.txn_stack.retain(|x| {
                        let involved = x.from_proc.as_ref().map(|fp| Arc::ptr_eq(fp, &proc)).unwrap_or(false)
                            || Arc::ptr_eq(&x.to_proc, &proc);
                        if involved && x.from_proc.is_some() {
                            // Someone is waiting for a reply from us.
                            th.todo.push_back(Work::ReturnError(BR_DEAD_REPLY, 0));
                            th.process_todo = true;
                            needs_wake = true;
                        }
                        !involved
                    });
                    if !th.todo.is_empty() {
                        needs_wake = true;
                    }
                    th.todo.retain(|wk| !work_involves(wk, &proc));
                }
                if !pm(world, &p).im().todo.is_empty() {
                    needs_wake = true;
                }
                pm(world, &p).im().todo.retain(|wk| !work_involves(wk, &proc));
                if needs_wake {
                    wake_targets.push(p.clone());
                }
            }

            // Drop our remaining work, releasing buffers and node pins.
            for t in proc.im().threads.clone() {
                let th = t.st();
                for wk in th.todo.drain(..) {
                    finish_work(world, &proc, wk);
                }
                th.txn_stack.clear();
            }
            for wk in proc.im().todo.drain(..) {
                finish_work(world, &proc, wk);
            }
            // Async buffers still held by userspace keep oneway txns
            // alive — release their node slots too.
            if let Some(alloc) = proc.im().alloc.as_mut() {
                let pending: Vec<Arc<BinderTxn>> = alloc.blocks.iter().filter_map(|b| b.txn.clone()).collect();
                for t in pending {
                    unpin_txn_node(world, &t);
                }
            }
            proc.im().alloc = None; // frees the region pages (see Drop)
        }
    }
    drop(proc);
    for p in wake_targets {
        wake_proc(&p);
    }
    0
}

fn work_involves(wk: &Work, dead: &Arc<BinderProc>) -> bool {
    match wk {
        Work::Transaction(t) => {
            t.from_proc.as_ref().map(|fp| Arc::ptr_eq(fp, dead)).unwrap_or(false) || Arc::ptr_eq(&t.to_proc, dead)
        }
        _ => false,
    }
}

/// End-of-life cleanup for one work item of a dying proc: release the
/// target buffer and un-pin the node.
/// SAFETY: caller holds WORLD.
unsafe fn finish_work(world: &World, dying: &Arc<BinderProc>, wk: Work) {
    let Work::Transaction(t) = wk else { return };
    if Arc::ptr_eq(&t.to_proc, dying) {
        if let Some(alloc) = pm(world, dying).im().alloc.as_mut() {
            if let Some(b) = alloc.blocks.iter_mut().find(|b| b.off == t.buffer_off) {
                b.allow_user_free = true;
            }
            let user_ptr = alloc.user_base + t.buffer_off as u64;
            let _ = alloc.free_buf(user_ptr);
        }
    }
    unpin_txn_node(world, &t);
}

/// Node un-pinning when a transaction finishes (sync: at reply time or
/// close; async: at close).
/// SAFETY: caller holds WORLD.
unsafe fn unpin_txn_node(world: &World, t: &BinderTxn) {
    let Some(node) = &t.target_node else { return };
    let ns = node.st();
    if t.flags & TF_ONE_WAY == 0 {
        if ns.internal_strong_refs > 0 {
            ns.internal_strong_refs -= 1;
        }
    } else if t.holds_async_slot {
        ns.has_async_transaction = false;
    }
    // A ref-state transition may now be visible to the owner.
    let external = node_external_refs(world, node);
    let owner = world.procs.iter().find(|p| p.id == node.proc_id);
    if let Some(owner) = owner {
        if (!ns.strong() && ns.has_strong_ref) || (!ns.weak(external) && ns.has_weak_ref) {
            owner.im().todo.push_back(Work::Node(node.clone()));
            wake_proc(owner);
        }
    }
}

/// mmap handler (sys_mmap dispatch for BINDER_OPS files).
pub fn binder_mmap_handler(
    file: &crate::fs::File,
    addr: usize,
    length: usize,
    offset: u64,
    prot: u32,
) -> Result<usize, i32> {
    use crate::arch::mm::{map_page, PageTableEntry, PhysAddr, VirtAddr};
    use crate::mm::GfpFlags;
    use crate::mm::page_alloc::{alloc_pages, free_pages};
    use crate::mm::vma::{Vma, VmaFlags};

    if offset != 0 {
        return Err(-22); // EINVAL
    }
    // Linux binder_mmap silently truncates to 4 MiB.
    let length = length.min(BINDER_MMAP_MAX);
    if length == 0 {
        return Err(-22);
    }

    // SAFETY: fd-backed lifetime; see proc_ref.
    let Some(proc) = (unsafe { proc_ref(file) }) else {
        return Err(-6); // ENXIO
    };

    let task = crate::sched::current().ok_or(-12)?;
    let addr_space = task.address_space().ok_or(-12)?;

    let npages = length.div_ceil(PAGE_SIZE);
    let order = npages.next_power_of_two().trailing_zeros() as usize;
    let phys = alloc_pages(GfpFlags::GFP_KERNEL, order);
    if phys == 0 {
        return Err(-12);
    }
    let size = (1usize << order) * PAGE_SIZE;
    // SAFETY: fresh buddy allocation of 2^order contiguous pages.
    let kvirt = crate::arch::mm::phys_to_virt(PhysAddr::new(phys as u64)).bits() as *mut u8;
    unsafe { core::ptr::write_bytes(kvirt, 0, size) };

    {
        let w = world_lock();
        // SAFETY: under WORLD.
        unsafe {
            if pm(w.as_ref().unwrap(), &proc).im().alloc.is_some() {
                free_pages(phys, order);
                return Err(-16); // EBUSY: already mapped
            }
        }
    }

    let vaddr = if addr == 0 {
        match addr_space.find_free_area(size) {
            Ok(v) => v.as_usize(),
            Err(_) => {
                free_pages(phys, order);
                return Err(-12);
            }
        }
    } else {
        addr & !(PAGE_SIZE - 1)
    };
    {
        let user_end = crate::arch::mm::user_addr::USER_END;
        let Some(end) = vaddr.checked_add(size) else {
            free_pages(phys, order);
            return Err(-22);
        };
        if end > user_end {
            free_pages(phys, order);
            return Err(-22);
        }
    }

    let root = addr_space.root_ppn();
    let mut pte_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x1 != 0 {
        pte_flags |= PageTableEntry::R;
    }
    if prot & 0x2 != 0 {
        pte_flags |= PageTableEntry::R | PageTableEntry::W;
    }
    // SAFETY: root is the task's page-table root; phys..phys+size is our
    // fresh contiguous allocation.
    unsafe {
        for i in 0..(size / PAGE_SIZE) {
            let va = vaddr + i * PAGE_SIZE;
            let pa = phys + i * PAGE_SIZE;
            map_page(root, VirtAddr::new(va as u64), PhysAddr::new(pa as u64), pte_flags);
            // Mapping reference (io_uring discipline): munmap drops one,
            // binder_close drops the owner one, the last drop frees.
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(pa));
            if !page.is_null() {
                (*page).get_page();
            }
        }
        crate::arch::mm::asid::flush_tlb_all();
    }

    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ);
    vma_flags.insert(VmaFlags::WRITE);
    vma_flags.insert(VmaFlags::SHARED);
    let vma = Vma::new(
        crate::mm::page::VirtAddr::new(vaddr),
        crate::mm::page::VirtAddr::new(vaddr + size),
        vma_flags,
    );
    if addr_space.vma_write().add(vma).is_err() {
        free_pages(phys, order);
        return Err(-12);
    }

    let mut alloc = BinderAlloc {
        kvirt,
        phys,
        npages: size / PAGE_SIZE,
        order,
        user_base: vaddr as u64,
        blocks: Vec::new(),
        free_async_space: size / 2,
    };
    alloc.blocks.push(BufBlock {
        off: 0,
        len: size,
        free: true,
        async_block: false,
        allow_user_free: false,
        data_size: 0,
        offsets_size: 0,
        txn: None,
    });
    {
        let mut w = world_lock();
        // SAFETY: under WORLD.
        unsafe {
            pm(w.as_mut().unwrap(), &proc).im().alloc = Some(alloc);
        }
    }
    crate::pr_info!("binder: proc {} mapped {:#x} bytes at {:#x}", proc.id, size, vaddr);
    Ok(vaddr)
}

/// FileOps.poll: readable while work is pending for this open.
fn binder_poll(file: &crate::fs::File, _events: u16) -> u16 {
    // SAFETY: fd-backed lifetime; see proc_ref.
    let Some(proc) = (unsafe { proc_ref(file) }) else {
        return 0;
    };
    let tid = current_tid();
    let w = world_lock();
    // SAFETY: under WORLD.
    let has = unsafe {
        let p = pm(w.as_ref().unwrap(), &proc);
        !p.im().todo.is_empty() || p.im().threads.iter().any(|t| t.tid == tid && !t.st().todo.is_empty())
    };
    drop(w);
    if has {
        crate::syscall::misc::poll_events::POLLIN | crate::syscall::misc::poll_events::POLLRDNORM
    } else {
        0
    }
}

/// File operations for /dev/binder (registered in the devfs char registry).
pub static BINDER_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: None,
    write: None,
    lseek: None,
    close: Some(binder_close),
    poll: Some(binder_poll),
};

// ============================================================================
// ioctl entry (sys_ioctl dispatch for BINDER_OPS files)
// ============================================================================

fn current_tid() -> u32 {
    crate::sched::current().map(|t| (*t).ns_pid_local()).unwrap_or(0)
}

/// Returns Some(ret) when the file is a binder fd, None otherwise.
pub fn binder_file_ioctl(file: &crate::fs::File, request: u32, arg: usize) -> Option<i64> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &BINDER_OPS as *const _) {
        return None;
    }
    Some(binder_ioctl(file, request, arg))
}

fn binder_ioctl(file: &crate::fs::File, request: u32, arg: usize) -> i64 {
    // SAFETY: fd-backed lifetime; see proc_ref.
    let Some(proc) = (unsafe { proc_ref(file) }) else {
        return -(EBADF as i64);
    };
    let tid = current_tid();

    // binder_get_thread: find or create the calling thread's state.
    let thread = {
        let mut w = world_lock();
        let world = w.as_mut().unwrap();
        if !proc_alive(world, &proc) {
            return -(ESRCH as i64);
        }
        // SAFETY: under WORLD.
        unsafe {
            let existing = pm(world, &proc).im().threads.iter().find(|t| t.tid == tid).cloned();
            match existing {
                Some(t) => t,
                None => {
                    let t = Arc::new(BinderThread::new(tid));
                    pm(world, &proc).im().threads.push(t.clone());
                    t
                }
            }
        }
    };

    match request {
        BINDER_WRITE_READ => binder_ioctl_write_read(&proc, &thread, arg, file),
        BINDER_SET_MAX_THREADS => {
            // SAFETY: get_user is fault-safe.
            match unsafe { crate::arch::uaccess::get_user(arg as *const u32) } {
                Some(v) => {
                    let w = world_lock();
                    // SAFETY: under WORLD.
                    unsafe {
                        pm(w.as_ref().unwrap(), &proc).im().max_threads = v as i32;
                    }
                    0
                }
                None => -(EINVAL as i64),
            }
        }
        BINDER_SET_CONTEXT_MGR => {
            let mut w = world_lock();
            let world = w.as_mut().unwrap();
            if world.mgr_node.is_some() {
                crate::pr_err!("binder: BINDER_SET_CONTEXT_MGR already set");
                return -(EBUSY as i64);
            }
            // Linux creates the node with local refs held and has_*_ref
            // already true, so no INCREFS/ACQUIRE handshake fires for it.
            let node = Arc::new(BinderNode::new(world.next_id, 0, 0, proc.id));
            world.next_id += 1;
            // SAFETY: under WORLD.
            unsafe {
                let ns = node.st();
                ns.local_strong_refs = 1;
                ns.local_weak_refs = 1;
                ns.has_strong_ref = true;
                ns.has_weak_ref = true;
                pm(world, &proc).im().nodes.push(node.clone());
            }
            world.mgr_node = Some(node);
            drop(w);
            crate::pr_info!("binder: proc {} (pid {}) is the context manager", proc.id, proc.pid);
            0
        }
        BINDER_THREAD_EXIT => {
            let mut w = world_lock();
            // SAFETY: under WORLD.
            unsafe {
                let world = w.as_mut().unwrap();
                let t = pm(world, &proc).im().threads.iter().find(|t| t.tid == tid).cloned();
                if let Some(t) = t {
                    let th = t.st();
                    for wk in th.todo.drain(..) {
                        finish_work(world, &proc, wk);
                    }
                    th.txn_stack.clear();
                }
                pm(world, &proc).im().threads.retain(|t| t.tid != tid);
            }
            0
        }
        BINDER_VERSION => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
                return -(EFAULT as i64);
            }
            // SAFETY: arg validated non-null, 4-byte writable.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut i32, BINDER_CURRENT_PROTOCOL_VERSION) } {
                return -(EFAULT as i64);
            }
            0
        }
        _ => {
            crate::pr_info!("binder: unhandled ioctl {:#x}", request);
            -(EINVAL as i64)
        }
    }
}

// ============================================================================
// BINDER_WRITE_READ
// ============================================================================

fn binder_ioctl_write_read(
    proc: &Arc<BinderProc>,
    thread: &Arc<BinderThread>,
    arg: usize,
    file: &crate::fs::File,
) -> i64 {
    let mut bwr = BinderWriteRead {
        write_size: 0,
        write_consumed: 0,
        write_buffer: 0,
        read_size: 0,
        read_consumed: 0,
        read_buffer: 0,
    };
    // SAFETY: arg points to a 48-byte user binder_write_read.
    if unsafe {
        crate::arch::uaccess::copy_from_user(
            &mut bwr as *mut BinderWriteRead as *mut u8,
            arg as *const u8,
            core::mem::size_of::<BinderWriteRead>(),
        )
    } != 0
    {
        return -(EFAULT as i64);
    }

    if bwr.write_size > 0 {
        let ret = binder_thread_write(
            proc,
            thread,
            bwr.write_buffer,
            bwr.write_size as usize,
            &mut bwr.write_consumed,
        );
        if ret < 0 {
            bwr.read_consumed = 0;
            // SAFETY: 48-byte copy back to the same location.
            unsafe {
                crate::arch::uaccess::copy_to_user(
                    arg as *mut u8,
                    &bwr as *const BinderWriteRead as *const u8,
                    core::mem::size_of::<BinderWriteRead>(),
                );
            }
            return ret;
        }
    }
    if bwr.read_size > 0 {
        let non_block = file.flags_bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0;
        let ret = binder_thread_read(
            proc,
            thread,
            bwr.read_buffer,
            bwr.read_size as usize,
            &mut bwr.read_consumed,
            non_block,
        );
        if ret < 0 {
            // SAFETY: same 48-byte copy-back.
            unsafe {
                crate::arch::uaccess::copy_to_user(
                    arg as *mut u8,
                    &bwr as *const BinderWriteRead as *const u8,
                    core::mem::size_of::<BinderWriteRead>(),
                );
            }
            return ret;
        }
    }
    // SAFETY: same 48-byte copy-back.
    unsafe {
        if crate::arch::uaccess::copy_to_user(
            arg as *mut u8,
            &bwr as *const BinderWriteRead as *const u8,
            core::mem::size_of::<BinderWriteRead>(),
        ) != 0
        {
            return -(EFAULT as i64);
        }
    }
    0
}

// ============================================================================
// binder_thread_write (BC commands)
// ============================================================================

fn get_user_at<T: Copy>(base: u64, off: usize) -> Option<T> {
    let addr = base.checked_add(off as u64)?;
    if !crate::arch::uaccess::access_ok(addr as usize, core::mem::size_of::<T>()) {
        return None;
    }
    // SAFETY: access_ok validated the range; get_user is fault-safe.
    unsafe { crate::arch::uaccess::get_user(addr as *const T) }
}

fn binder_thread_write(
    proc: &Arc<BinderProc>,
    thread: &Arc<BinderThread>,
    buffer: u64,
    size: usize,
    consumed: &mut u64,
) -> i64 {
    let mut ptr = *consumed as usize;
    loop {
        if ptr + 4 > size {
            break;
        }
        let Some(cmd) = get_user_at::<u32>(buffer, ptr) else {
            *consumed = ptr as u64;
            return -(EFAULT as i64);
        };
        ptr += 4;

        match cmd {
            BC_TRANSACTION | BC_REPLY | BC_TRANSACTION_SG | BC_REPLY_SG => {
                let sg = cmd == BC_TRANSACTION_SG || cmd == BC_REPLY_SG;
                let reply = cmd == BC_REPLY || cmd == BC_REPLY_SG;
                let Some(mut tr) = get_user_at::<BinderTransactionData>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += if sg { 72 } else { 64 };
                let mut err_cmd: u32 = 0;
                binder_transaction(proc, thread, &mut tr, reply, &mut err_cmd);
                if err_cmd != 0 {
                    // Linux stops processing write commands and queues the
                    // error as thread work for the following read.
                    let w = world_lock();
                    // SAFETY: under WORLD.
                    unsafe {
                        let th = thread.st();
                        th.todo.push_back(Work::ReturnError(err_cmd, 0));
                        th.process_todo = true;
                    }
                    drop(w);
                    wake_proc(proc);
                    break;
                }
            }
            BC_FREE_BUFFER => {
                let Some(data_ptr) = get_user_at::<u64>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 8;
                {
                    let w = world_lock();
                    // SAFETY: all inner access below is under WORLD.
                    unsafe {
                        let world = w.as_ref().unwrap();
                        let txn = match pm(world, proc).im().alloc.as_mut() {
                            Some(alloc) => alloc.free_buf(data_ptr),
                            None => None,
                        };
                        match txn {
                            None => {
                                crate::pr_info!(
                                    "binder: pid {} BC_FREE_BUFFER {:#x} no match or not freeable",
                                    proc.pid,
                                    data_ptr
                                );
                            }
                            Some(Some(t)) => {
                                // Oneway buffer release: free the node's
                                // async slot (Linux drains node->async_todo
                                // here; S1 never parks, so just release).
                                if t.holds_async_slot {
                                    unpin_txn_node(world, &t);
                                }
                            }
                            Some(None) => {
                                // Sync buffer freed (its txn was unlinked
                                // at reply time).
                            }
                        }
                    }
                }
            }
            BC_INCREFS | BC_ACQUIRE | BC_RELEASE | BC_DECREFS => {
                let Some(desc) = get_user_at::<u32>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 4;
                let strong = cmd == BC_ACQUIRE || cmd == BC_RELEASE;
                let increment = cmd == BC_INCREFS || cmd == BC_ACQUIRE;
                let mut wake_owner: Option<Arc<BinderProc>> = None;
                {
                    let w = world_lock();
                    // SAFETY: all inner access below is under WORLD.
                    unsafe {
                        let world = w.as_ref().unwrap();
                        let ridx = pm(world, proc)
                            .im()
                            .refs
                            .iter()
                            .position(|r| r.desc == desc);
                        let Some(ridx) = ridx else {
                            drop(w);
                            crate::pr_info!("binder: pid {} ref command on invalid handle {}", proc.pid, desc);
                            continue;
                        };
                        let node = pm(world, proc).im().refs[ridx].node.clone();
                        let owner = world.procs.iter().find(|p| p.id == node.proc_id).cloned();
                        let ns = node.st();

                        if increment {
                            if strong {
                                pm(world, proc).im().refs[ridx].strong += 1;
                                ns.internal_strong_refs += 1;
                            } else {
                                pm(world, proc).im().refs[ridx].weak += 1;
                            }
                        } else {
                            let (s, wk) = {
                                let r = &mut pm(world, proc).im().refs[ridx];
                                (r.strong, r.weak)
                            };
                            if strong {
                                if s == 0 {
                                    drop(w);
                                    crate::pr_info!("binder: pid {} BC_RELEASE on 0 strong refs", proc.pid);
                                    continue;
                                }
                                let r = &mut pm(world, proc).im().refs[ridx];
                                r.strong -= 1;
                                if r.strong == 0 && ns.internal_strong_refs > 0 {
                                    ns.internal_strong_refs -= 1;
                                }
                            } else {
                                if wk == 0 {
                                    drop(w);
                                    crate::pr_info!("binder: pid {} BC_DECREFS on 0 weak refs", proc.pid);
                                    continue;
                                }
                                let r = &mut pm(world, proc).im().refs[ridx];
                                r.weak -= 1;
                            }
                            if pm(world, proc).im().refs[ridx].strong == 0
                                && pm(world, proc).im().refs[ridx].weak == 0
                            {
                                pm(world, proc).im().refs.remove(ridx);
                            }
                        }
                        // Ref-state change: queue BINDER_WORK_NODE to the
                        // owner (read derives the actual BR_*_REFS from
                        // the counters; a no-op if nothing transitioned).
                        if let Some(owner) = &owner {
                            owner.im().todo.push_back(Work::Node(node.clone()));
                            wake_owner = Some(owner.clone());
                        }
                    }
                }
                if let Some(owner) = wake_owner {
                    wake_proc(&owner);
                }
            }
            BC_INCREFS_DONE | BC_ACQUIRE_DONE => {
                let Some(node_ptr) = get_user_at::<u64>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                let Some(_cookie) = get_user_at::<u64>(buffer, ptr + 8) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 16;
                let w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    let world = w.as_ref().unwrap();
                    let node = pm(world, proc)
                        .im()
                        .nodes
                        .iter()
                        .find(|n| n.ptr == node_ptr)
                        .cloned();
                    let Some(node) = node else {
                        drop(w);
                        crate::pr_info!(
                            "binder: pid {} BC_{}_DONE: no node {:#x}",
                            proc.pid,
                            if cmd == BC_ACQUIRE_DONE { "ACQUIRE" } else { "DECREFS" },
                            node_ptr
                        );
                        continue;
                    };
                    let ns = node.st();
                    if cmd == BC_ACQUIRE_DONE {
                        if !ns.pending_strong_ref {
                            drop(w);
                            crate::pr_info!("binder: BC_ACQUIRE_DONE without pending acquire");
                            continue;
                        }
                        ns.pending_strong_ref = false;
                        ns.local_strong_refs -= 1;
                    } else {
                        if !ns.pending_weak_ref {
                            drop(w);
                            crate::pr_info!("binder: BC_INCREFS_DONE without pending increfs");
                            continue;
                        }
                        ns.pending_weak_ref = false;
                        ns.local_weak_refs -= 1;
                    }
                }
            }
            BC_ENTER_LOOPER => {
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    thread.st().looper |= LOOPER_ENTERED;
                }
            }
            BC_EXIT_LOOPER => {
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    thread.st().looper |= LOOPER_EXITED;
                }
            }
            BC_REGISTER_LOOPER => {
                let w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    let world = w.as_ref().unwrap();
                    let p = pm(world, proc);
                    if p.im().requested_threads > 0 {
                        p.im().requested_threads -= 1;
                        p.im().requested_threads_started += 1;
                    }
                    thread.st().looper |= LOOPER_REGISTERED;
                }
            }
            BC_DEAD_BINDER_DONE => {
                let Some(_cookie) = get_user_at::<u64>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 8;
                // No death-notification support in S1: accept and ignore.
            }
            _ => {
                crate::pr_info!("binder: unknown BC command {:#x}, stopping", cmd);
                ptr -= 4; // not consumed
                break;
            }
        }
    }
    *consumed = ptr as u64;
    0
}

// ============================================================================
// binder_transaction
// ============================================================================

fn binder_transaction(
    proc: &Arc<BinderProc>,
    thread: &Arc<BinderThread>,
    tr: &mut BinderTransactionData,
    reply: bool,
    err_cmd: &mut u32,
) {
    let oneway = tr.flags & TF_ONE_WAY != 0;
    let data_size = tr.data_size as usize;
    let offsets_size = tr.offsets_size as usize;

    // ---- resolve target + allocate the target buffer ------------------
    let mut target_node: Option<Arc<BinderNode>> = None;
    let mut target_thread_key: Option<(Arc<BinderProc>, u32)> = None;
    let mut in_reply_to: Option<Arc<BinderTxn>> = None;
    let mut target_proc: Option<Arc<BinderProc>> = None;
    let mut block_off: usize;
    let mut holds_async_slot = false;
    let txn_id;

    {
        let mut w = world_lock();
        // SAFETY: all inner access in this block is under WORLD.
        unsafe {
            let world = w.as_mut().unwrap();

            if reply {
                // BC_REPLY: the thread's transaction stack top must be a
                // transaction delivered TO this thread.
                let stack_top = thread.st().txn_stack.last().cloned();
                let Some(t) = stack_top else {
                    *err_cmd = BR_FAILED_REPLY;
                    crate::pr_info!("binder: BC_REPLY with empty transaction stack");
                    return;
                };
                if t.to_tid() != Some(thread.tid) || !Arc::ptr_eq(&t.to_proc, proc) {
                    *err_cmd = BR_FAILED_REPLY;
                    return;
                }
                let t = thread.st().txn_stack.pop().unwrap();
                let (Some(from_proc), Some(from_tid)) = (t.from_proc.clone(), t.from_tid) else {
                    *err_cmd = BR_FAILED_REPLY;
                    return;
                };
                if from_proc.im().is_dead {
                    *err_cmd = BR_DEAD_REPLY;
                    return;
                }
                target_thread_key = Some((from_proc, from_tid));
                target_proc = Some(target_thread_key.as_ref().unwrap().0.clone());
                in_reply_to = Some(t);
            } else if tr.target_handle != 0 {
                let node = pm(world, proc)
                    .im()
                    .refs
                    .iter()
                    .find(|r| r.desc == tr.target_handle)
                    .map(|r| r.node.clone());
                let Some(node) = node else {
                    *err_cmd = BR_FAILED_REPLY;
                    crate::pr_info!("binder: transaction to invalid handle {}", tr.target_handle);
                    return;
                };
                target_node = Some(node);
            } else {
                let mgr = world.mgr_node.clone();
                let Some(mgr) = mgr else {
                    *err_cmd = BR_DEAD_REPLY;
                    crate::pr_info!("binder: no context manager installed");
                    return;
                };
                if mgr.proc_id == proc.id {
                    // Linux: a transaction to the context manager from its
                    // own process is a protocol error.
                    *err_cmd = BR_FAILED_REPLY;
                    return;
                }
                target_node = Some(mgr);
            }

            if target_proc.is_none() {
                let node = target_node.as_ref().unwrap();
                match world.procs.iter().find(|p| p.id == node.proc_id) {
                    Some(p) if !p.im().is_dead => target_proc = Some(p.clone()),
                    _ => {
                        *err_cmd = BR_DEAD_REPLY;
                        return;
                    }
                }
            }
            let tp = target_proc.as_ref().unwrap();
            if !reply && Arc::ptr_eq(tp, proc) {
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
            if tp.im().alloc.is_none() {
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
            if offsets_size % 8 != 0 {
                *err_cmd = BR_FAILED_REPLY;
                return;
            }

            txn_id = world.next_id;
            world.next_id += 1;

            let is_async = !reply && oneway;
            if let Some(node) = &target_node {
                let ns = node.st();
                if is_async {
                    if ns.has_async_transaction {
                        // S1 limitation: the node's async slot is busy —
                        // Linux parks this on node->async_todo and drains
                        // it when the in-flight buffer is freed.
                        *err_cmd = BR_FAILED_REPLY;
                        crate::pr_info!("binder: oneway to busy node rejected (S1)");
                        return;
                    }
                    ns.has_async_transaction = true;
                    holds_async_slot = true;
                }
            }

            let off = pm(world, tp)
                .im()
                .alloc
                .as_mut()
                .unwrap()
                .new_buf(data_size, offsets_size, is_async);
            let Some(off) = off else {
                *err_cmd = BR_FAILED_REPLY;
                crate::pr_info!(
                    "binder: buffer alloc failed (need {}+{})",
                    align8(data_size),
                    align8(offsets_size)
                );
                return;
            };
            block_off = off;

            // Sync transactions pin a strong node ref for their lifetime.
            if !reply && !oneway {
                if let Some(node) = &target_node {
                    node.st().internal_strong_refs += 1;
                }
            }
        }
    } // -- WORLD released: user copies below --------------------------------

    let target = target_proc.clone().unwrap();

    // ---- copy payload from the sender into the target's region ----------
    let kvirt = {
        let w = world_lock();
        // SAFETY: under WORLD.
        unsafe { pm(w.as_ref().unwrap(), &target).im().alloc.as_ref().unwrap().kvirt }
    };
    // SAFETY: the block is exclusively owned by this in-flight txn.
    unsafe {
        let dst = kvirt.add(block_off);
        if data_size > 0
            && crate::arch::uaccess::copy_from_user(dst, tr.data_buffer as *const u8, data_size) != 0
        {
            release_block(&target, block_off);
            unpin_after_failure(&target, target_node.as_ref(), reply, oneway, holds_async_slot);
            *err_cmd = BR_FAILED_REPLY;
            return;
        }
        if offsets_size > 0
            && crate::arch::uaccess::copy_from_user(
                dst.add(align8(data_size)),
                tr.data_offsets as *const u8,
                offsets_size,
            ) != 0
        {
            release_block(&target, block_off);
            unpin_after_failure(&target, target_node.as_ref(), reply, oneway, holds_async_slot);
            *err_cmd = BR_FAILED_REPLY;
            return;
        }
    }

    // ---- translate flat binder objects ----------------------------------
    if offsets_size > 0 {
        let n = offsets_size / 8;
        for i in 0..n {
            // SAFETY: offsets array inside the kernel-side block.
            let obj_off = unsafe {
                core::ptr::read_volatile((kvirt.add(block_off + align8(data_size)) as *const u64).add(i)) as usize
            };
            if obj_off + core::mem::size_of::<FlatBinderObject>() > data_size {
                release_block(&target, block_off);
                unpin_after_failure(&target, target_node.as_ref(), reply, oneway, holds_async_slot);
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
            // SAFETY: validated within the data area.
            let obj = unsafe { &mut *(kvirt.add(block_off + obj_off) as *mut FlatBinderObject) };
            if !translate_object(proc, obj, &target) {
                release_block(&target, block_off);
                unpin_after_failure(&target, target_node.as_ref(), reply, oneway, holds_async_slot);
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
        }
    }

    // ---- queue the transaction ------------------------------------------
    let (sender_pid, sender_euid) = match crate::sched::current() {
        // SAFETY: current task is valid; a cred read here races at most
        // with a setuid on the same task, which is benign for IPC labels.
        Some(t) => ((*t).pid() as i32, (*t).cred().euid),
        None => (0, 0),
    };
    let txn = Arc::new(BinderTxn {
        id: txn_id,
        from_proc: if !reply && !oneway { Some(proc.clone()) } else { None },
        from_tid: if !reply && !oneway { Some(thread.tid) } else { None },
        to_proc: target.clone(),
        to_tid: core::cell::UnsafeCell::new(None),
        target_node: target_node.clone(),
        holds_async_slot,
        code: tr.code,
        flags: tr.flags,
        sender_pid,
        sender_euid,
        data_size,
        offsets_size,
        buffer_off: block_off,
    });

    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    {
        let w = world_lock();
        // SAFETY: all inner access in this block is under WORLD.
        unsafe {
            let world = w.as_ref().unwrap();
            if reply {
                // tcomplete to the replier (non-deferred) ...
                let th = thread.st();
                th.todo.push_back(Work::TransactionComplete);
                th.process_todo = true;
                // ... and the reply to the exact originating thread.
                let (tproc, rtid) = target_thread_key.as_ref().unwrap();
                let rthread = pm(world, tproc)
                    .im()
                    .threads
                    .iter()
                    .find(|t| t.tid == *rtid)
                    .cloned();
                if let Some(rthread) = rthread {
                    let rth = rthread.st();
                    match &in_reply_to {
                        Some(orig) => rth.txn_stack.retain(|t| !Arc::ptr_eq(t, orig)),
                        None => {}
                    }
                    rth.todo.push_back(Work::Transaction(txn.clone()));
                    rth.process_todo = true;
                } else {
                    *err_cmd = BR_DEAD_REPLY;
                    release_block(&target, block_off);
                    return;
                }
                // The original transaction is finished: unpin its node.
                if let Some(orig) = &in_reply_to {
                    unpin_txn_node(world, orig);
                }
                wakes.push(tproc.clone());
                wakes.push(proc.clone());
            } else if oneway {
                let th = thread.st();
                th.todo.push_back(Work::TransactionComplete);
                th.process_todo = true;
                pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                // The async buffer keeps its txn alive for the node-slot
                // release at BC_FREE_BUFFER.
                if let Some(a) = pm(world, &target).im().alloc.as_mut() {
                    if let Some(b) = a.blocks.iter_mut().find(|b| b.off == block_off) {
                        b.txn = Some(txn.clone());
                    }
                }
                wakes.push(target.clone());
                wakes.push(proc.clone());
            } else {
                // Sync: deferred TRANSACTION_COMPLETE (no process_todo) +
                // push on the sender's txn stack + queue on the target.
                let th = thread.st();
                th.todo.push_back(Work::TransactionComplete);
                th.txn_stack.push(txn.clone());
                pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                wakes.push(target.clone());
            }
        }
    }
    for p in wakes {
        wake_proc(&p);
    }
}

/// Undo the pins taken at resolve time when the transaction failed after
/// them.
fn unpin_after_failure(
    _target: &Arc<BinderProc>,
    node: Option<&Arc<BinderNode>>,
    reply: bool,
    oneway: bool,
    holds_async_slot: bool,
) {
    let Some(node) = node else { return };
    let _w = world_lock();
    // SAFETY: under WORLD.
    unsafe {
        let ns = node.st();
        if !reply && !oneway && ns.internal_strong_refs > 0 {
            ns.internal_strong_refs -= 1;
        }
        let _ = holds_async_slot;
    }
}

fn release_block(proc: &Arc<BinderProc>, off: usize) {
    let w = world_lock();
    // SAFETY: under WORLD.
    unsafe {
        let world = w.as_ref().unwrap();
        if let Some(alloc) = pm(world, proc).im().alloc.as_mut() {
            if let Some(b) = alloc.blocks.iter_mut().find(|b| b.off == off) {
                b.allow_user_free = true;
            }
            let user_ptr = alloc.user_base + off as u64;
            let _ = alloc.free_buf(user_ptr);
        }
    }
}

/// flat_binder_object translation across processes: local binder -> handle
/// for the target, handle -> handle rebind (or back to a local binder when
/// the target IS the sender).
fn translate_object(sender: &Arc<BinderProc>, obj: &mut FlatBinderObject, target: &Arc<BinderProc>) -> bool {
    let mut wake_owner: Option<Arc<BinderProc>> = None;
    let result = {
        let mut w = world_lock();
        // SAFETY: all inner access below is under WORLD.
        unsafe {
            let world = w.as_mut().unwrap();
            match obj.hdr_type {
                BINDER_TYPE_BINDER | BINDER_TYPE_WEAK_BINDER => {
                    let strong = obj.hdr_type == BINDER_TYPE_BINDER;
                    let ptr = (obj.handle as u64) | ((obj._handle_pad as u64) << 32);
                    let node = {
                        let sp = pm(world, sender);
                        match sp.im().nodes.iter().find(|n| n.ptr == ptr).cloned() {
                            Some(n) => n,
                            None => {
                                let n = Arc::new(BinderNode::new(world.next_id, ptr, obj.cookie, sender.id));
                                world.next_id += 1;
                                sp.im().nodes.push(n.clone());
                                n
                            }
                        }
                    };
                    let desc = get_or_create_ref(world, target, &node, strong, &mut wake_owner);
                    obj.hdr_type = if strong { BINDER_TYPE_HANDLE } else { BINDER_TYPE_WEAK_HANDLE };
                    obj.handle = desc;
                    obj._handle_pad = 0;
                    obj.cookie = 0;
                    true
                }
                BINDER_TYPE_HANDLE | BINDER_TYPE_WEAK_HANDLE => {
                    let strong = obj.hdr_type == BINDER_TYPE_HANDLE;
                    let node = pm(world, sender)
                        .im()
                        .refs
                        .iter()
                        .find(|r| r.desc == obj.handle)
                        .map(|r| r.node.clone());
                    let Some(node) = node else { return false };
                    // Sending a handle BACK to the node's owner restores
                    // the local BINDER_TYPE_*BINDER object.
                    let target_is_owner =
                        world.procs.iter().any(|p| Arc::ptr_eq(p, target) && p.id == node.proc_id);
                    if target_is_owner {
                        obj.hdr_type = if strong { BINDER_TYPE_BINDER } else { BINDER_TYPE_WEAK_BINDER };
                        obj.handle = node.ptr as u32;
                        obj._handle_pad = (node.ptr >> 32) as u32;
                        obj.cookie = node.cookie;
                        return true;
                    }
                    let desc = get_or_create_ref(world, target, &node, strong, &mut wake_owner);
                    obj.handle = desc;
                    obj._handle_pad = 0;
                    true
                }
                BINDER_TYPE_FD => {
                    crate::pr_info!("binder: BINDER_TYPE_FD translation unsupported in S1");
                    false
                }
                _ => {
                    crate::pr_info!("binder: object type {:#x} unsupported in S1", obj.hdr_type);
                    false
                }
            }
        }
    };
    if let Some(owner) = wake_owner {
        wake_proc(&owner);
    }
    result
}

/// get_or_create_ref + node pinning (binder_get_ref_for_node semantics).
/// SAFETY: caller holds WORLD.
unsafe fn get_or_create_ref(
    world: &mut World,
    proc: &Arc<BinderProc>,
    node: &Arc<BinderNode>,
    strong: bool,
    wake_owner: &mut Option<Arc<BinderProc>>,
) -> u32 {
    let p = pm(world, proc);
    if let Some(r) = p.im().refs.iter_mut().find(|r| Arc::ptr_eq(&r.node, node)) {
        if strong {
            r.strong += 1;
            let ns = node.st();
            ns.internal_strong_refs += 1;
        } else {
            r.weak += 1;
        }
        return r.desc;
    }
    let desc = p.im().next_desc;
    p.im().next_desc += 1;
    // A new reference holds the node alive (internal strong) and triggers
    // the owner-side INCREFS/ACQUIRE handshake if not yet reported.
    let ns = node.st();
    ns.internal_strong_refs += 1;
    p.im().refs.push(BinderRef {
        desc,
        node: node.clone(),
        strong: if strong { 1 } else { 0 },
        weak: if strong { 0 } else { 1 },
    });
    if let Some(owner) = world.procs.iter().find(|p| p.id == node.proc_id) {
        owner.im().todo.push_back(Work::Node(node.clone()));
        if wake_owner.is_none() {
            *wake_owner = Some(owner.clone());
        }
    }
    desc
}

// ============================================================================
// binder_thread_read (BR commands)
// ============================================================================

/// has_work under WORLD (the wait_event condition).
/// SAFETY: caller holds WORLD.
unsafe fn thread_has_work(world: &World, proc: &Arc<BinderProc>, thread: &Arc<BinderThread>) -> bool {
    let th = thread.st();
    let avail_for_proc = th.txn_stack.is_empty()
        && th.todo.is_empty()
        && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
    th.process_todo || (avail_for_proc && !pm(world, proc).im().todo.is_empty())
}

fn binder_thread_read(
    proc: &Arc<BinderProc>,
    thread: &Arc<BinderThread>,
    read_buffer: u64,
    read_size: usize,
    consumed: &mut u64,
    non_block: bool,
) -> i64 {
    let mut staged: Vec<u8> = Vec::new();

    macro_rules! stage_cmd {
        ($cmd:expr) => {
            staged.extend_from_slice(&$cmd.to_ne_bytes())
        };
    }
    macro_rules! stage_u64 {
        ($v:expr) => {
            staged.extend_from_slice(&$v.to_ne_bytes())
        };
    }

    if *consumed == 0 && read_size >= 4 {
        stage_cmd!(BR_NOOP);
    }

    let mut got_transaction = false;
    loop {
        // ---- wait for work (binder_wait_for_work) ------------------------
        let has_work = {
            let w = world_lock();
            // SAFETY: under WORLD.
            unsafe { thread_has_work(w.as_ref().unwrap(), proc, thread) }
        };
        if !has_work {
            if non_block {
                if staged.len() > 4 || *consumed != 0 {
                    break;
                }
                return -(EAGAIN as i64);
            }
            {
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    thread.st().waiting_for_proc_work = true;
                }
            }
            let r = crate::wait_event_interruptible!(&proc.wait, {
                let w = world_lock();
                // SAFETY: under WORLD.
                unsafe { thread_has_work(w.as_ref().unwrap(), proc, thread) }
            });
            {
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    thread.st().waiting_for_proc_work = false;
                }
            }
            if r != 0 {
                return -(EINTR as i64);
            }
        }

        // ---- drain work into the staging buffer ---------------------------
        let stop_full = {
            let w = world_lock();
            // SAFETY: all inner access below is under WORLD.
            unsafe {
                let world = w.as_ref().unwrap();
                let mut stop_full = false;
                loop {
                    if staged.len() + 4 + core::mem::size_of::<BinderTransactionData>() > read_size {
                        stop_full = true;
                        break;
                    }
                    let from_thread = !thread.st().todo.is_empty();
                    let avail_for_proc = {
                        let th = thread.st();
                        th.txn_stack.is_empty() && th.todo.is_empty() && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0
                    };
                    let work = if from_thread {
                        thread.st().todo.pop_front()
                    } else if avail_for_proc && !pm(world, proc).im().todo.is_empty() {
                        pm(world, proc).im().todo.pop_front()
                    } else {
                        None
                    };
                    let Some(work) = work else { break };
                    if from_thread && thread.st().todo.is_empty() {
                        thread.st().process_todo = false;
                    }
                    match work {
                        Work::TransactionComplete => {
                            stage_cmd!(BR_TRANSACTION_COMPLETE);
                        }
                        Work::ReturnError(cmd, param) => {
                            stage_cmd!(cmd);
                            if cmd == BR_ERROR {
                                staged.extend_from_slice(&(param as u32).to_ne_bytes());
                            }
                        }
                        Work::Node(node) => {
                            // BINDER_WORK_NODE: derive the BR_*_REFS
                            // commands from the counters, exactly like the
                            // Linux read-side handler.
                            let external_refs = node_external_refs(world, &node);
                            let ns = node.st();
                            let strong = ns.strong();
                            let weak = ns.weak(external_refs);
                            let had_weak = ns.has_weak_ref;
                            let had_strong = ns.has_strong_ref;
                            if weak && !had_weak {
                                ns.has_weak_ref = true;
                                ns.pending_weak_ref = true;
                                ns.local_weak_refs += 1;
                            }
                            if strong && !had_strong {
                                ns.has_strong_ref = true;
                                ns.pending_strong_ref = true;
                                ns.local_strong_refs += 1;
                            }
                            if !weak && had_weak {
                                ns.has_weak_ref = false;
                            }
                            if !strong && had_strong {
                                ns.has_strong_ref = false;
                            }
                            let (ptr, cookie) = (node.ptr, node.cookie);
                            if weak && !had_weak {
                                stage_cmd!(BR_INCREFS);
                                stage_u64!(ptr);
                                stage_u64!(cookie);
                            }
                            if strong && !had_strong {
                                stage_cmd!(BR_ACQUIRE);
                                stage_u64!(ptr);
                                stage_u64!(cookie);
                            }
                            if !strong && had_strong {
                                stage_cmd!(BR_RELEASE);
                                stage_u64!(ptr);
                                stage_u64!(cookie);
                            }
                            if !weak && had_weak {
                                stage_cmd!(BR_DECREFS);
                                stage_u64!(ptr);
                                stage_u64!(cookie);
                            }
                        }
                        Work::Transaction(t) => {
                            got_transaction = true;
                            let is_reply = t.target_node.is_none();
                            let (tptr, tcookie) = match &t.target_node {
                                Some(n) => (n.ptr, n.cookie),
                                None => (0, 0),
                            };
                            let (buf_user, data_size, offsets_size) = {
                                let a = t.to_proc.im().alloc.as_ref().unwrap();
                                (a.user_base + t.buffer_off as u64, a.data_size_now(t.buffer_off), a.offsets_size_now(t.buffer_off))
                            };
                            let off_user = buf_user + align8(data_size) as u64;
                            let td = BinderTransactionData {
                                target_handle: tptr as u32,
                                _target_pad: (tptr >> 32) as u32,
                                cookie: tcookie,
                                code: t.code,
                                flags: t.flags,
                                sender_pid: t.sender_pid,
                                sender_euid: t.sender_euid,
                                data_size: data_size as u64,
                                offsets_size: offsets_size as u64,
                                data_buffer: buf_user,
                                data_offsets: off_user,
                            };
                            stage_cmd!(if is_reply { BR_REPLY } else { BR_TRANSACTION });
                            // SAFETY: BinderTransactionData is Copy + repr(C).
                            staged.extend_from_slice(core::slice::from_raw_parts(
                                &td as *const BinderTransactionData as *const u8,
                                core::mem::size_of::<BinderTransactionData>(),
                            ));
                            // The receiving side now owns the buffer.
                            if let Some(a) = t.to_proc.im().alloc.as_mut() {
                                if let Some(b) = a.blocks.iter_mut().find(|b| b.off == t.buffer_off) {
                                    b.allow_user_free = true;
                                }
                            }
                            if !is_reply && t.flags & TF_ONE_WAY == 0 {
                                // The receiver stacks it for its BC_REPLY.
                                t.set_to_tid(thread.tid);
                                thread.st().txn_stack.push(t);
                            }
                        }
                    }
                }
                // Linux done: BR_SPAWN_LOOPER when no transaction was read
                // and more threads are allowed and none are ready.
                if !got_transaction {
                    let pim = pm(world, proc).im();
                    let ready = pim
                        .threads
                        .iter()
                        .any(|t| t.st().waiting_for_proc_work && t.st().todo.is_empty() && t.st().txn_stack.is_empty());
                    if pim.requested_threads == 0
                        && !ready
                        && pim.max_threads > 0
                        && pim.threads.len() < pim.max_threads as usize
                        && staged.len() + 4 <= read_size
                    {
                        pim.requested_threads += 1;
                        stage_cmd!(BR_SPAWN_LOOPER);
                    }
                }
                stop_full
            }
        };
        if stop_full {
            break;
        }
        if staged.len() > 4 || *consumed != 0 {
            break;
        }
        // Only the leading BR_NOOP staged and no work found: retry the wait
        // (Linux "goto retry").
    }

    // Copy the staged commands out to user memory.
    let Some(dst) = read_buffer.checked_add(*consumed) else {
        return -(EFAULT as i64);
    };
    // SAFETY: staged is a kernel Vec; copy_to_user is fault-safe.
    unsafe {
        if crate::arch::uaccess::copy_to_user(dst as *mut u8, staged.as_ptr(), staged.len()) != 0 {
            return -(EFAULT as i64);
        }
    }
    *consumed += staged.len() as u64;
    0
}

impl BinderAlloc {
    fn data_size_now(&self, off: usize) -> usize {
        self.blocks.iter().find(|b| b.off == off).map(|b| b.data_size).unwrap_or(0)
    }
    fn offsets_size_now(&self, off: usize) -> usize {
        self.blocks.iter().find(|b| b.off == off).map(|b| b.offsets_size).unwrap_or(0)
    }
}
