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
pub const BINDER_TYPE_FDA: u32 = u32::from_be_bytes([b'f', b'd', b'a', 0x85]);
pub const BINDER_TYPE_PTR: u32 = u32::from_be_bytes([b'p', b't', b'*', 0x85]);

pub const TF_ONE_WAY: u32 = 0x01;
pub const _TF_ROOT_OBJECT: u32 = 0x04;
pub const _TF_STATUS_CODE: u32 = 0x08;
pub const TF_ACCEPT_FDS: u32 = 0x10;

/// flat_binder_object.flags: the target accepts BINDER_TYPE_FD objects.
pub const FLAT_BINDER_FLAG_ACCEPTS_FDS: u32 = 0x100;
/// flat_binder_object.flags: transactions to this node carry the sender's
/// security context (upstream FLAT_BINDER_FLAG_TXN_SECURITY_CTX).
pub const FLAT_BINDER_FLAG_TXN_SECURITY_CTX: u32 = 0x1000;
/// binder_buffer_object.flags: fix up the parent buffer with this
/// object's translated address (binder_fixup_parent).
pub const BINDER_BUFFER_FLAG_HAS_PARENT: u32 = 0x01;

const BINDER_CURRENT_PROTOCOL_VERSION: i32 = 8;

const SZ_BWR: u32 = 48;
const SZ_TR: u32 = 64;
const SZ_PTR_COOKIE: u32 = 16;
/// OH's uapi binder.h packs struct binder_handle_cookie (u32 handle +
/// u64 cookie) with `} __packed;` — 12 bytes, not the aligned 16 of
/// binder_ptr_cookie. The BC command words for death notifications
/// therefore encode size 12.
const SZ_HANDLE_COOKIE: u32 = 12;
const SZ_U32: u32 = 4;
const SZ_U64: u32 = 8;
const SZ_VERSION: u32 = 4;
const SZ_TR_SG: u32 = 72;

pub const BINDER_WRITE_READ: u32 = ioc(3, b'b' as u32, 1, SZ_BWR);
pub const BINDER_SET_MAX_THREADS: u32 = iow(b'b' as u32, 5, SZ_U32);
pub const BINDER_SET_CONTEXT_MGR: u32 = iow(b'b' as u32, 7, SZ_U32);
pub const BINDER_THREAD_EXIT: u32 = iow(b'b' as u32, 8, SZ_U32);
pub const BINDER_VERSION: u32 = ioc(3, b'b' as u32, 9, SZ_VERSION);
/// _IOW('b', 13, struct flat_binder_object) — 24 bytes on LP64. OH samgr
/// tries this FIRST; rejecting it sent samgr into a retry loop (803 dead
/// handle-0 transactions in one boot) before falling back to the plain
/// ioctl.
pub const BINDER_SET_CONTEXT_MGR_EXT: u32 = iow(b'b' as u32, 13, 24);
/// _IOWR('b', 30, struct binder_feature_set) — OpenHarmony addition; the
/// IPC library probes it right after BINDER_VERSION on every open.
pub const BINDER_FEATURE_SET: u32 = ioc(3, b'b' as u32, 30, SZ_U64);
pub const BINDER_GET_ACCESS_TOKEN: u32 = ioc(3, b'b' as u32, 31, 32);
pub const BINDER_GET_SENDER_INFO: u32 = ioc(3, b'b' as u32, 32, 40);

/// OH binder enhancement features we support (sys_binder.h masks):
/// bit0 ACCESS_TOKEN (BINDER_GET_ACCESS_TOKEN), bit2 SENDER_INFO
/// (BINDER_GET_SENDER_INFO — also carries the real sender pid).
pub const BINDER_SUPPORTED_FEATURES: u64 = (1 << 0) | (1 << 2);

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
/// OH packs binder_handle_cookie (__packed, 12 bytes) so the command word
/// is _IOW('c', 14, 12) = 0x400c630e. With the aligned 16-byte size the
/// word mismatches and the write loop reports "unknown BC command
/// 0x400c630e, stopping" (b117-b119: pid 326 froze right after samgr
/// accepted SA 5100, wedging the whole SA registration storm).
pub const BC_REQUEST_DEATH_NOTIFICATION: u32 = iow(b'c' as u32, 14, SZ_HANDLE_COOKIE);
pub const BC_CLEAR_DEATH_NOTIFICATION: u32 = iow(b'c' as u32, 15, SZ_HANDLE_COOKIE);
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
/// OH uapi: BR_DEAD_BINDER = _IOR('r', 15, binder_uintptr_t) and
/// BR_CLEAR_DEATH_NOTIFICATION_DONE = _IOR('r', 16, binder_uintptr_t).
/// nr 11 is BR_ATTEMPT_ACQUIRE in the same header — libipc switch-matches
/// the exact word, so the old nr 11/14 encodings would be misparsed.
pub const BR_DEAD_BINDER: u32 = ior(b'r' as u32, 15, SZ_U64);
pub const BR_CLEAR_DEATH_NOTIFICATION_DONE: u32 = ior(b'r' as u32, 16, SZ_U64);
pub const BR_FAILED_REPLY: u32 = io(b'r' as u32, 17);
/// struct binder_transaction_data_secctx (upstream): the transaction data
/// followed by a pointer to the sender's NUL-terminated security-context
/// string in the buffer's extra area. NOTE the ioctl number: nr=2 with a
/// 72-byte size (0x80487202) — the SAME nr as BR_TRANSACTION (_IOR('r',2,
/// struct binder_transaction_data), 64 bytes); the size field is what
/// distinguishes them. OH libbinder switch-matches the exact value
/// (sys_binder.h BR_TRANSACTION_SEC_CTX); an earlier 0x8048722A (nr=42)
/// staging was rejected with HandleCommands error:404 and samgr silently
/// dropped every SECCTX delivery (b45/b59 wedges).
pub const BR_TRANSACTION_SECCTX: u32 = ior(b'r' as u32, 2, 72);

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

/// struct binder_buffer_object (40 bytes on LP64): BINDER_TYPE_PTR —
/// points at additional data outside the parcel that the driver copies
/// into the target's sg (extra) area and rewrites to a target address.
#[repr(C)]
#[derive(Clone, Copy)]
struct BinderBufferObject {
    hdr_type: u32,
    flags: u32,
    buffer: u64,
    length: u64,
    /// index into the offsets array of the parent BINDER_TYPE_PTR object
    parent: u64,
    /// offset in the parent buffer where this buffer's address is written
    parent_offset: u64,
}
const _: () = assert!(core::mem::size_of::<BinderBufferObject>() == 40);

/// struct binder_fd_array_object (32 bytes on LP64): BINDER_TYPE_FDA —
/// an array of u32 fds living inside the parent buffer object.
#[repr(C)]
#[derive(Clone, Copy)]
struct BinderFdArrayObject {
    hdr_type: u32,
    flags: u32,
    num_fds: u64,
    /// index into the offsets array of the parent BINDER_TYPE_PTR object
    parent: u64,
    /// offset of the fd array within the parent's buffer
    parent_offset: u64,
}
const _: () = assert!(core::mem::size_of::<BinderFdArrayObject>() == 32);

/// Security-context string copied into the transaction's sg area when the
/// target node opted into FLAT_BINDER_FLAG_TXN_SECURITY_CTX — OH libbinder
/// reads it as the caller's SELinux sid (IPCSkeleton::GetCallingSid).
/// All Rux tasks share this synthesized permissive domain.
const SECCTX_SID: &str = "u:r:init:s0";
const _: () = {
    let bytes = SECCTX_SID.as_bytes();
    let mut i = 0;
    assert!(bytes.len() > 0);
    while i < bytes.len() {
        assert!(bytes[i] != 0, "SECCTX_SID must have no interior NULs");
        i += 1;
    }
};

/// Minimum in-buffer size of each object type (binder_validate_object;
/// on LP64 the fd object carries an 8-byte union like flat_binder_object).
fn binder_object_size(ty: u32) -> usize {
    match ty {
        BINDER_TYPE_BINDER | BINDER_TYPE_WEAK_BINDER | BINDER_TYPE_HANDLE | BINDER_TYPE_WEAK_HANDLE => 24,
        BINDER_TYPE_FD => 24,
        BINDER_TYPE_FDA => 32,
        BINDER_TYPE_PTR => 40,
        _ => 0,
    }
}

const LOOPER_REGISTERED: u32 = 0x01;
const LOOPER_ENTERED: u32 = 0x02;
const LOOPER_EXITED: u32 = 0x04;

/// /dev/binder misc device numbers (Linux uses dynamic minors; 0xB0-0xB2
/// are unused in-tree — loop-control owns 237 on major 10).
pub const BINDER_MINOR: u32 = 0xB0;
pub const HWBINDER_MINOR: u32 = 0xB1;
pub const VNDBINDER_MINOR: u32 = 0xB2;
pub const DEV_BINDER: DevNo = DevNo::new(MISC_MAJOR, BINDER_MINOR);
pub const DEV_HWBINDER: DevNo = DevNo::new(MISC_MAJOR, HWBINDER_MINOR);
pub const DEV_VNDBINDER: DevNo = DevNo::new(MISC_MAJOR, VNDBINDER_MINOR);

/// Which device context an open belongs to. /dev/binder, /dev/hwbinder
/// and /dev/vndbinder each carry their OWN context-manager node (Linux
/// binder_device.context); handle-0 resolves within the opener's context
/// only. Ref tables are per-open, so handles cannot leak across contexts.
#[derive(Clone, Copy, PartialEq)]
pub enum BinderCtx {
    Binder = 0,
    HwBinder = 1,
    VndBinder = 2,
}

pub fn devno_ctx(dev: &DevNo) -> Option<BinderCtx> {
    if *dev == DEV_BINDER {
        Some(BinderCtx::Binder)
    } else if *dev == DEV_HWBINDER {
        Some(BinderCtx::HwBinder)
    } else if *dev == DEV_VNDBINDER {
        Some(BinderCtx::VndBinder)
    } else {
        None
    }
}

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
    /// (Linux node->has_async_transaction); extra oneways park on
    /// node->async_todo until the in-flight buffer is freed.
    has_async_transaction: bool,
    /// Oneway transactions waiting for the node's async slot (Linux
    /// node->async_todo). Their buffers are already allocated and fixed
    /// up; delivery is just deferred.
    async_todo: VecDeque<Arc<BinderTxn>>,
}

struct BinderNode {
    id: u64,
    ptr: u64,
    cookie: u64,
    proc_id: u64,
    /// sc157: tgid of the task that REGISTERED the node. When it differs
    /// from the owner proc's pid, the node belongs to a forked child
    /// sharing the parent's binder_proc — deliveries to such nodes must
    /// be visible to the APPTXN tracer (the AMS→child ScheduleStartAbility
    /// hop rides exactly this shape).
    reg_tgid: u32,
    /// flat_binder_object.flags at registration (accept_fds and
    /// txn_security_ctx bits).
    flags: u32,
    /// sc161: the registering task group exited while the OWNER proc
    /// lives on (forked appspawn children share the daemon's proc). Set
    /// by binder_on_tgid_exit; makes node_is_dead true and fail-fasts
    /// new transactions through stale refs (BR_DEAD_REPLY), mirroring
    /// what proc teardown does for whole-process death.
    dead: core::sync::atomic::AtomicBool,
    st: core::cell::UnsafeCell<NodeState>,
}

impl BinderNode {
    const fn new(id: u64, ptr: u64, cookie: u64, proc_id: u64, reg_tgid: u32, flags: u32) -> Self {
        Self {
            id,
            ptr,
            cookie,
            proc_id,
            reg_tgid,
            flags,
            dead: core::sync::atomic::AtomicBool::new(false),
            st: core::cell::UnsafeCell::new(NodeState {
                internal_strong_refs: 0,
                local_strong_refs: 0,
                local_weak_refs: 0,
                has_strong_ref: false,
                has_weak_ref: false,
                pending_strong_ref: false,
                pending_weak_ref: false,
                has_async_transaction: false,
                async_todo: VecDeque::new(),
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
    /// Death-notification request on this handle (Linux ref->death).
    death: Option<RefDeath>,
}

/// Death-notification request state (Linux struct binder_ref_death +
/// its work-item states).
struct RefDeath {
    cookie: u64,
    /// Armed: requested, node still alive. Queued: Work::DeadBinder sits
    /// on the proc todo (node dead, BR not yet read). Delivered:
    /// BR_DEAD_BINDER was read, awaiting BC_DEAD_BINDER_DONE.
    phase: DeathPhase,
    /// BC_CLEAR_DEATH_NOTIFICATION arrived while the notification was
    /// queued or delivered: after BC_DEAD_BINDER_DONE, deliver
    /// BR_CLEAR_DEATH_NOTIFICATION_DONE and drop the request (Linux
    /// BINDER_WORK_DEAD_BINDER_AND_CLEAR).
    clear_pending: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum DeathPhase {
    Armed,
    Queued,
    Delivered,
}

struct BufBlock {
    off: usize,
    len: usize,
    free: bool,
    async_block: bool,
    allow_user_free: bool,
    data_size: usize,
    offsets_size: usize,
    /// Per-allocation id (the reference's buffer->debug_id): assigned
    /// by new_buf when a block is carved, 0 while free. Carried by the
    /// binder_transaction_alloc_buf / buffer_release trace events.
    buf_id: u64,
    /// sg (extra) area size: BINDER_TYPE_PTR payload copies (layout:
    /// [data][offsets][extra], each u64-aligned).
    extra_size: usize,
    /// Oneway (async) buffers keep their transaction alive so the node's
    /// async slot is released when the receiver frees the buffer (Linux
    /// buffer->transaction). Sync buffers rely on the reply path instead.
    txn: Option<Arc<BinderTxn>>,
    /// sc243 (leg-17): the sync txn this buffer carried has been
    /// REPLIED — the request data is protocol-dead and only the
    /// receiver's contractual BC_FREE_BUFFER remains. The OHOS
    /// cc54-family services never issue that BC_FREE, so their allocs
    /// filled with delivered-but-unfreed blocks (sc243 census: 329K
    /// live BinderTxn arcs, every one pinned by such a block; at the
    /// launcher floor this is ~210MB of the 831MB heap). On alloc
    /// pressure these blocks are reclaimed (see alloc_new_buf).
    reply_done: bool,
}

struct BinderAlloc {
    kvirt: *mut u8,
    phys: usize,
    npages: usize,
    order: usize,
    user_base: u64,
    blocks: Vec<BufBlock>,
    free_async_space: usize,
    /// sc243 (leg-17): jiffies of the last stale-async sweep (see
    /// sweep_stale_async) — rate-limits the O(blocks) scan to one per
    /// second per allocator.
    last_sweep_j: u32,
    /// Monotone buffer-id source (see BufBlock::buf_id).
    next_buf_id: u64,
}

impl BinderAlloc {
    fn block_need(data_size: usize, offsets_size: usize, extra_size: usize) -> usize {
        // Linux binder_alloc_new_buf allocates for EVERY transaction,
        // including all-zero payloads (the block carries data_size=0
        // metadata; empty parcels are legal — OH's samgrClient
        // registration path sends one). The old `0 -> reject` rule
        // failed every such transaction with BR_FAILED_REPLY, so no
        // service could ever register with samgr.
        (align8(data_size) + align8(offsets_size) + align8(extra_size)).max(8)
    }

    /// binder_alloc_new_buf: sync first-fit from the low end, async from
    /// the top half against the free_async_space budget.
    fn new_buf(
        &mut self,
        data_size: usize,
        offsets_size: usize,
        extra_size: usize,
        is_async: bool,
    ) -> Option<usize> {
        let need = Self::block_need(data_size, offsets_size, extra_size);
        if is_async && self.free_async_space < need {
            return None;
        }
        let total = self.npages * PAGE_SIZE;
        if is_async {
            // Async buffers tail-allocate from the top half (a fresh
            // region is one block at offset 0 — a head-only placement
            // scan would never find an eligible block).
            let mut hit: Option<usize> = None;
            for (i, b) in self.blocks.iter().enumerate() {
                if !b.free || b.len < need {
                    continue;
                }
                if b.off + b.len - need >= total / 2 {
                    hit = Some(i);
                    break;
                }
            }
            let i = hit?;
            let cand = self.blocks[i].off + self.blocks[i].len - need;
            self.blocks[i].len -= need;
            let buf_id = self.next_buf_id;
            self.next_buf_id += 1;
            self.blocks.push(BufBlock {
                off: cand,
                len: need,
                free: false,
                async_block: true,
                allow_user_free: false,
                data_size,
                offsets_size,
                extra_size,
                buf_id,
                txn: None,
                reply_done: false,
            });
            self.blocks.sort_by_key(|b| b.off);
            self.free_async_space -= need;
            crate::trace::trace_binder_transaction_alloc_buf(
                buf_id, data_size as u64, offsets_size as u64, extra_size as u64,
            );
            return Some(cand);
        }
        // Sync: first-fit from the low end.
        let mut off: Option<usize> = None;
        for b in self.blocks.iter() {
            if !b.free || b.len < need {
                continue;
            }
            off = Some(b.off);
            break;
        }
        let o = off?;
        let buf_id = self.next_buf_id;
        self.next_buf_id += 1;
        if let Some(b) = self.blocks.iter_mut().find(|b| b.off == o) {
            b.free = false;
            b.async_block = false;
            b.allow_user_free = false;
            b.data_size = data_size;
            b.offsets_size = offsets_size;
            b.extra_size = extra_size;
            b.buf_id = buf_id;
            if b.len > need {
                let rest = BufBlock {
                    off: o + need,
                    len: b.len - need,
                    free: true,
                    async_block: false,
                    allow_user_free: false,
                    data_size: 0,
                    offsets_size: 0,
                    extra_size: 0,
                    buf_id: 0,
                    txn: None,
                    reply_done: false,
                };
                b.len = need;
                self.blocks.push(rest);
                self.blocks.sort_by_key(|b| b.off);
            }
        }
        crate::trace::trace_binder_transaction_alloc_buf(
            buf_id, data_size as u64, offsets_size as u64, extra_size as u64,
        );
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
        let es = self.blocks[idx].extra_size;
        let buf_id = self.blocks[idx].buf_id;
        let txn = self.blocks[idx].txn.take();
        self.blocks[idx].free = true;
        self.blocks[idx].allow_user_free = false;
        self.blocks[idx].buf_id = 0;
        if was_async {
            self.free_async_space += Self::block_need(ds, os, es);
        }
        // binder:binder_transaction_buffer_release — the free chokepoint
        // every release path (BC_FREE_BUFFER, teardown, stale reclaim)
        // funnels through.
        crate::trace::trace_binder_transaction_buffer_release(
            buf_id, ds as u64, os as u64, es as u64,
        );
        self.coalesce();
        Some(txn)
    }

    /// Merge adjacent free blocks (blocks stay sorted by offset).
    fn coalesce(&mut self) {
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
    /// BINDER_WORK_DEAD_BINDER(_AND_CLEAR): (ref desc, cookie). The node
    /// died; deliver BR_DEAD_BINDER.
    DeadBinder { desc: u32, cookie: u64 },
    /// BINDER_WORK_CLEAR_DEATH_NOTIFICATION: deliver
    /// BR_CLEAR_DEATH_NOTIFICATION_DONE (cookie).
    ClearDeathDone { cookie: u64 },
}


/// sc243 (leg-17): reclaim DELIVERED-but-never-freed binder buffers of a
/// receiver that went silent. The cc54-family OHOS services leak their
/// BC_FREE_BUFFER duty (render_service RW-leak victims): every oneway
/// txn's buffer stays `!free` pinning its Arc, the node's async slot
/// never hands onward, and parked txns pile on node.async_todo — the
/// sc243 census measured 329K live BinderTxn arcs at the 10-minute park
/// (at the launcher floor this family is ~210MB of the 831MB heap:
/// c256 = the txns, c1024+o9-o12 = the Vec<BufBlock> arrays).
///
/// The sweep (rate-limited to 1/s per allocator, called on the buffer
/// alloc path) frees blocks whose txn was delivered (pick_j != 0) more
/// than 30s ago, or whose sync txn was replied (reply_done), and hands
/// the async slot onward exactly like BC_FREE_BUFFER's drain — which
/// also UNWEDGES nodes parked behind a receiver that stopped freeing.
/// A late userspace BC_FREE for an already-reclaimed block is a no-op
/// (free_buf's position check fails benignly).
///
/// SAFETY: caller holds WORLD. Returns the number of blocks reclaimed.
/// leg-32B: `wake_out` collects child threads that received a drained
/// parked txn (drain_deliver_parked_txn) — the caller wakes them after
/// WORLD is released; the proc-todo fallback keeps its historical
/// no-wake shape (silent receivers pick the todo up on their next read).
unsafe fn sweep_stale_async(
    world: &World,
    target: &Arc<BinderProc>,
    wake_out: &mut Vec<Arc<BinderThread>>,
) -> usize {
    const GRACE_J: u32 = (crate::config::KERNEL_HZ as u32).saturating_mul(30);
    let min_gap: u32 = (crate::config::KERNEL_HZ as u32).max(1);
    let now = crate::drivers::timer::get_jiffies() as u32;
    let Some(alloc) = pm(world, target).im().alloc.as_mut() else {
        return 0;
    };
    if alloc.last_sweep_j != 0 && now.wrapping_sub(alloc.last_sweep_j) < min_gap {
        return 0;
    }
    alloc.last_sweep_j = now;
    let mut reclaimed = 0usize;
    let mut slot_holders: Vec<Arc<BinderTxn>> = Vec::new();
    for b in alloc.blocks.iter_mut() {
        if b.free {
            continue;
        }
        let stale = match b.txn.as_ref() {
            Some(t) => {
                // SAFETY: WORLD held; pick_j is written under WORLD.
                let pj = unsafe { *t.pick_j.get() };
                pj != 0 && now.wrapping_sub(pj) > GRACE_J
            }
            None => b.reply_done,
        };
        if !stale {
            continue;
        }
        if let Some(t) = b.txn.take() {
            // leg-30B AMS46: the 30 s stale-buffer reclaim acted on a
            // traced txn — the receiver read it but never BC_FREE'd it
            // (userspace leak shape), or it held the slot the traced
            // StartUser needs.
            if ams46_on() && ams46_txns_contains(t.id) {
                crate::pr_warn!(
                    "AMS46 SWEEP-RECLAIM txn{} code={} (delivered but never freed)",
                    t.id,
                    t.code
                );
            }
            if t.holds_async_slot() {
                slot_holders.push(t);
            }
        }
        if b.async_block {
            alloc.free_async_space +=
                BinderAlloc::block_need(b.data_size, b.offsets_size, b.extra_size);
        }
        b.free = true;
        b.allow_user_free = false;
        b.reply_done = false;
        reclaimed += 1;
    }
    for t in slot_holders {
        if let Some(node) = &t.target_node {
            let ns = node.st();
            // SAFETY: WORLD held (set_holds_async_slot requires it).
            match ns.async_todo.pop_front() {
                Some(h) => {
                    unsafe { h.set_holds_async_slot(true) };
                    let tp = h.to_proc.clone();
                    // leg-32B: child-thread preference for the drained
                    // txn (see drain_deliver_parked_txn).
                    let (ct, _pref) = drain_deliver_parked_txn(world, &h, &tp);
                    if let Some(ct) = ct {
                        wake_out.push(ct);
                    }
                    if ams46_on() && ams46_txns_contains(h.id) {
                        crate::pr_warn!(
                            "AMS46 SWEEP slot txn{} -> txn{} (parked code-46 DELIVERED by sweep drain)",
                            t.id,
                            h.id
                        );
                    }
                }
                None => {
                    ns.has_async_transaction = false;
                }
            }
        }
    }
    if reclaimed > 0 {
        alloc.coalesce();
    }
    reclaimed
}

/// Leg-19: the enq_j silence rule — leg-17's banked design, the dominant
/// floor population's lever. The delivered-stale sweep (pick_j+30s)
/// cannot free UNDELIVERED parked oneways: a txn with pick_j==0 that
/// has sat on a todo for longer than any plausible retry ladder (OH's
/// own ladder tops out at 40 s) is dead by the same silence proof — its
/// receiver's readers are all parked (the WAKE-RUN-LEAK census shape)
/// and no BR_TRANSACTION will ever deliver it. Retirement mirrors the
/// node-death pattern (binder_on_tgid_exit): the txn drops its async
/// slot, sync senders get BR_DEAD_REPLY (unstacking them from
/// WaitForReply), finish_work frees the buffer and unpins the node, and
/// the drained work is simply not re-queued. The async-slot handoff
/// after a front-of-node retirement matches sweep_stale_async, so the
/// next live parked txn stays deliverable.
///
/// Sync txns parked undelivered get a longer silence window than
/// oneways: a sync sender is parked waiting and BR_DEAD_REPLY unwedges
/// it (the b139 hdf_devmgr shape), but losing a sync call that a
/// merely-slow receiver would have picked up is costlier, so the bar is
/// 60 s.
///
/// Returns (retired count, procs whose readers must be re-checked for
/// wake AFTER WORLD is released — same contract as
/// binder_on_tgid_exit).
///
/// SAFETY: caller holds WORLD.
unsafe fn sweep_silent_txns(
    world: &World,
) -> (usize, Vec<Arc<BinderProc>>, Vec<Arc<BinderThread>>) {
    const ONEWAY_SILENCE_J: u32 = (crate::config::KERNEL_HZ as u32).saturating_mul(30);
    const SYNC_SILENCE_J: u32 = (crate::config::KERNEL_HZ as u32).saturating_mul(60);
    let now = crate::drivers::timer::get_jiffies() as u32;
    let mut retired = 0usize;
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    // leg-32B: child threads that received a handed-on parked txn via
    // drain_deliver_parked_txn — the caller wakes them after WORLD.
    let mut wake_ths: Vec<Arc<BinderThread>> = Vec::new();
    let procs = world.procs.clone();
    for p in &procs {
        // 1. Proc todo: retire silent parked transactions.
        let queued: Vec<Work> = pm(world, p).im().todo.drain(..).collect();
        for wk in queued {
            let retire = match &wk {
                Work::Transaction(t) => {
                    // SAFETY: WORLD held; pick_j is written under WORLD.
                    let pj = unsafe { *t.pick_j.get() };
                    if pj != 0 {
                        false
                    } else {
                        let age = now.wrapping_sub(t.enq_j);
                        if t.flags & TF_ONE_WAY != 0 {
                            age > ONEWAY_SILENCE_J
                        } else {
                            age > SYNC_SILENCE_J
                        }
                    }
                }
                _ => false,
            };
            if retire {
                let Work::Transaction(t) = &wk else {
                    unreachable!("retire only set for Transaction");
                };
                // leg-30B AMS46: the traced StartUser sat UNDELIVERED on
                // the proc todo past the 30 s silence window — the
                // kernel-side drop shape for Gate 1.
                if ams46_on() && ams46_txns_contains(t.id) {
                    crate::pr_warn!(
                        "AMS46 RETIRED(proc-todo) txn{} age={}js — DROPPED undelivered",
                        t.id,
                        now.wrapping_sub(t.enq_j)
                    );
                }
                // SAFETY: WORLD held.
                unsafe { t.set_holds_async_slot(false) };
                if t.flags & TF_ONE_WAY == 0 {
                    // SAFETY: WORLD held.
                    wakes.extend(unsafe { dead_reply_to_sender(world, p, t) });
                }
                // SAFETY: WORLD held; frees the buffer + unpins the node.
                unsafe { finish_work(world, p, wk) };
                retired += 1;
            } else {
                pm(world, p).im().todo.push_back(wk);
            }
        }
        // 2. Node async_todo: the oneway parking lot. Retire the silent
        //    ones, keep delivery order, hand the async slot onward when a
        //    slot holder retires.
        let nodes = pm(world, p).im().nodes.clone();
        for n in &nodes {
            // SAFETY: WORLD held.
            let parked: Vec<Arc<BinderTxn>> = unsafe { n.st().async_todo.drain(..).collect() };
            if parked.is_empty() {
                continue;
            }
            let mut keep: Vec<Arc<BinderTxn>> = Vec::with_capacity(parked.len());
            let mut slot_vacated = false;
            for t in parked {
                // SAFETY: WORLD held; pick_j is written under WORLD.
                let pj = unsafe { *t.pick_j.get() };
                let silent = pj == 0 && now.wrapping_sub(t.enq_j) > ONEWAY_SILENCE_J;
                if !silent {
                    keep.push(t);
                    continue;
                }
                // SAFETY: WORLD held.
                if unsafe { t.holds_async_slot() } {
                    slot_vacated = true;
                }
                // leg-30B AMS46: parked past the silence window without
                // ever being delivered — the other kernel-side drop.
                if ams46_on() && ams46_txns_contains(t.id) {
                    crate::pr_warn!(
                        "AMS46 RETIRED(async-park) txn{} age={}js — DROPPED parked behind slot holder",
                        t.id,
                        now.wrapping_sub(t.enq_j)
                    );
                }
                // SAFETY: WORLD held.
                unsafe { t.set_holds_async_slot(false) };
                // SAFETY: WORLD held; frees the buffer + unpins the node.
                unsafe { finish_work(world, p, Work::Transaction(t)) };
                retired += 1;
            }
            for t in keep {
                // SAFETY: WORLD held.
                unsafe { n.st().async_todo.push_back(t) };
            }
            if slot_vacated {
                // SAFETY: WORLD held (set_holds_async_slot requires it).
                unsafe {
                    match n.st().async_todo.pop_front() {
                        Some(h) => {
                            h.set_holds_async_slot(true);
                            let tp = h.to_proc.clone();
                            let hid = h.id;
                            if ams46_on() && ams46_txns_contains(hid) {
                                crate::pr_warn!(
                                    "AMS46 slot -> txn{} (retire handoff DELIVERED parked code-46)",
                                    hid
                                );
                            }
                            // leg-32B: child-thread preference for the
                            // handed-on parked txn (see
                            // drain_deliver_parked_txn); the proc-todo
                            // fallback keeps its historical no-wake shape.
                            let (ct, _pref) = drain_deliver_parked_txn(world, &h, &tp);
                            if let Some(ct) = ct {
                                wake_ths.push(ct);
                            }
                        }
                        None => {
                            n.st().has_async_transaction = false;
                        }
                    }
                }
            }
        }
    }
    (retired, wakes, wake_ths)
}

// sc140 (Session Q): registry of forked appspawn child pids (written by
// the clone census at each appspawn fork, first 128 per boot). The
// APPTXN tracer prints any binder transaction whose TARGET is one of
// these — the AMS→child Schedule* delivery is the suspected missing hop
// behind the launcher-family park.
static APP_CHILD_PIDS: [core::sync::atomic::AtomicU32; 128] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 128];

// leg-25: module-visible mirror of the SAREG probe's latched samgr pid,
// so the SLRD read-side tracer can cover samgr's own reads (its leader
// comm 'samgr' matches no app prefix) — the Add/Remove (code 3/4)
// delivery record for the on-demand unload question.
static SAREG_SAMGR_PID_PUB: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

pub(crate) fn binder_sareg_samgr_pid() -> u32 {
    SAREG_SAMGR_PID_PUB.load(core::sync::atomic::Ordering::Relaxed)
}

// ============================================================================
// leg-30B: AMS46 — the Gate-1 arsenal (dfx=ams46).
//
// The wall (sc306/307/309/311B/312B/313B): accountmgr's
// AbilityManagerAdapter::StartUser sends IAbilityManager code 46
// (interface token "ohos.aafwk.AbilityManager", MessageOption(1, 8) =
// TF_ASYNC + 8 s wait — binder flags 0x11, oneway + ACCEPT_FDS) to
// foundation, SendRequest returns 0, yet SwitchToUser never executes
// inside foundation (zero StartResidentAbility lines), with no error
// log, no hang, no TXN-HANG on the pair. This tracer follows EVERY
// matched transaction through the kernel lifecycle, so a walled draw
// names the exact loss point:
//
//   AMS46 latch  — first token-matched send latches the AMS node id +
//                  foundation pid
//   AMS46 ENQ    — code-46 send accepted (flags, sender, parcel head)
//   AMS46 Q      — queueing verdict: proc todo / child looper / PARKED
//                  on node.async_todo behind an un-freed earlier oneway
//   AMS46 READ   — a foundation thread read it out (BR_TRANSACTION) —
//                  kernel delivery COMPLETED; anything lost past this
//                  point is foundation userspace (deferred-task drop)
//   AMS46 FREE   — receiver freed the buffer; async slot released or
//                  handed to the next parked txn
//   AMS46 SWEEP  — stale-buffer reclaim handed the slot onward (>30 s)
//   AMS46 RETIRED— the enq_j silence rule DROPPED it undelivered
//                  (>30 s parked) — the kernel-side loss shape
//   AMS46 census — ~5 s node/proc state: async slot held, parked depth,
//                  foundation proc todo depth, waiting readers
//
// All prints budget-capped; zero cost when dfx=ams46 is off.
// ============================================================================

static AMS46_NODE_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static AMS46_TPID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static AMS46_TXNS: [core::sync::atomic::AtomicU64; 16] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 16];
static AMS46_ENQ_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn ams46_on() -> bool {
    crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Ams46)
}

// ============================================================================
// POOL CENSUS (leg-33B, dfx=poolcensus) — reader-exhaustion adjudication
//
// One line per binder proc with queued todo: todo depth, max_threads,
// requested_threads, and the worker-pool shape (looper-flag workers split
// into waiting / busy-client stacks, plus client-only threads that must
// NOT consume the max_threads budget — the sc329B gate-pollution shape).
// The POOL-SPAWN line (read path, above) proves the done:-shaped spawn
// emission firing live. Together they decide kernel-issued vs
// image-refused at the exact moment a proc stops draining.
// ============================================================================

static POOL_SPAWN_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

fn pool_census_on() -> bool {
    crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::PoolCensus)
}

fn ams46_txns_contains(id: u64) -> bool {
    AMS46_TXNS
        .iter()
        .any(|s| s.load(core::sync::atomic::Ordering::Relaxed) == id)
}

fn ams46_txns_push(id: u64) {
    for s in AMS46_TXNS.iter() {
        if s.load(core::sync::atomic::Ordering::Relaxed) == 0 {
            s.store(id, core::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
}

fn ams46_node_id() -> u64 {
    AMS46_NODE_ID.load(core::sync::atomic::Ordering::Relaxed)
}

/// Does this sender parcel head carry the IAbilityManager interface
/// token? OH parcel layout: [8B header][u32 string len][UTF-16LE chars];
/// "ohos.aafwk.Abil..." starts at offset 12.
fn ams46_token_match(head: &[u8]) -> bool {
    // UTF-16LE of "ohos.aafwk.Abil" at parcel offset 12 (14 chars).
    const PAT: [u8; 28] = [
        0x6f, 0x00, 0x68, 0x00, 0x6f, 0x00, 0x73, 0x00, 0x2e, 0x00, 0x61, 0x00, 0x61, 0x00,
        0x66, 0x00, 0x77, 0x00, 0x6b, 0x00, 0x2e, 0x00, 0x41, 0x00, 0x62, 0x00, 0x69, 0x00,
    ];
    head.len() >= 12 + PAT.len() && head[12..12 + PAT.len()] == PAT
}


pub fn register_app_child_pid(pid: u32) {
    for slot in APP_CHILD_PIDS.iter() {
        let s = slot.load(core::sync::atomic::Ordering::Relaxed);
        if s == pid {
            return;
        }
        if s == 0 {
            let _ = slot.compare_exchange(
                0,
                pid,
                core::sync::atomic::Ordering::Relaxed,
                core::sync::atomic::Ordering::Relaxed,
            );
            return;
        }
    }
}

fn app_child_pids_contains(pid: u32) -> bool {
    for slot in APP_CHILD_PIDS.iter() {
        let s = slot.load(core::sync::atomic::Ordering::Relaxed);
        if s == 0 {
            return false;
        }
        if s == pid {
            return true;
        }
    }
    false
}

/// sc141 (Session Q): the parked children send zero successful binder
/// transactions — print their FAILED sends (invalid handle, dead
/// context-manager target, dead target proc) so "never tried" separates
/// from "tried and failed silently".
fn app_txn_fail_print(proc: &BinderProc, why: &str, handle: u64, code: u64) {
    let comm = crate::sched::current()
        .map(|t| unsafe { (*t).comm() })
        .map(|c| core::str::from_utf8(c).unwrap_or(""))
        .unwrap_or("")
        .trim_end_matches('\0');
    let fam = comm.contains("launcher")
        || comm.contains("settingsdata")
        || comm.contains("systemui")
        || comm.contains("dataability")
        || comm == "appspawn";
    if !fam && !app_child_pids_contains(proc.pid) {
        return;
    }
    static FAIL_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if FAIL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
        crate::pr_warn!(
            "APPTXN-FAIL pid={} comm='{}' why={} handle={:#x} code={:#x}",
            proc.pid,
            comm,
            why,
            handle,
            code
        );
    }
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
    /// Interior-mutable: a parked oneway claims the slot only when the
    /// in-flight buffer is freed and it graduates to the proc todo.
    holds_async_slot: core::cell::UnsafeCell<bool>,
    code: u32,
    flags: u32,
    sender_pid: i32,
    sender_euid: u32,
    data_size: usize,
    offsets_size: usize,
    buffer_off: usize,
    /// Target-user address of the NUL-terminated secctx string in the sg
    /// area (0: the target node did not request a security context).
    security_ctx: u64,
    /// sc208 (launcher leg 13, C-line cross-finding: SAMgr SendRequest
    /// ~30 s per call once the clock was fixed): lifecycle stamps for the
    /// SLOW-TXN probe. enq_j = jiffies at BC_TRANSACTION queue time;
    /// pick_j = jiffies at BR_TRANSACTION delivery (server pickup), 0
    /// until then. The reply pop computes queue-vs-exec and prints the
    /// split for any sync txn over 5 s — where the 30 s goes (server not
    /// picking up vs handler running long) names the next chain link.
    enq_j: u32,
    pick_j: core::cell::UnsafeCell<u32>,
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
    fn holds_async_slot(&self) -> bool {
        // SAFETY: single word read under WORLD (the access contract).
        unsafe { *self.holds_async_slot.get() }
    }
    /// SAFETY: caller must hold WORLD.
    unsafe fn set_holds_async_slot(&self, v: bool) {
        *self.holds_async_slot.get() = v;
    }
}

/// Per-thread transition history (sc74 wedge forensics): the observed death
/// loop ends in a state — server thread parked in the blocking read owing a
/// reply, own todo empty, no nested txn anywhere in the world — that no
/// single-threaded execution of this driver plus OH libipc can produce. The
/// ring records the last BHIST state transitions under WORLD so the next
/// stall dump names the exact step that never happened (or shows a
/// transition sequence only corruption can produce).
#[derive(Clone, Copy)]
pub struct BinderEv {
    pub j: u32,
    pub ev: u8,
    pub a: u32,
    pub b: u32,
}

pub const BHIST: usize = 24;

pub const EV_TXN_SENT: u8 = 1;
pub const EV_TXN_TQUEUED: u8 = 2;
pub const EV_TXN_PQUEUED: u8 = 3;
pub const EV_TXN_PARKED_ASYNC: u8 = 4;
pub const EV_REPLY_POP: u8 = 5;
pub const EV_REPLY_PUSHED: u8 = 6;
pub const EV_REPLY_GONE: u8 = 7;
pub const EV_TXN_RECV: u8 = 8;
pub const EV_REPLY_RECV: u8 = 9;
pub const EV_TC_RECV: u8 = 10;
pub const EV_ERR_RECV: u8 = 11;
pub const EV_PARK: u8 = 12;
pub const EV_WAKE_RET: u8 = 13;
pub const EV_BWR: u8 = 14;
pub const EV_REAP: u8 = 15;
pub const EV_DEAD_REPLY: u8 = 16;
pub const EV_REKICK: u8 = 17;
pub const EV_TXN_RECV_END: u8 = 18;

pub fn ev_name(ev: u8) -> &'static str {
    match ev {
        EV_TXN_SENT => "TXN_SENT",
        EV_TXN_TQUEUED => "TXN_TQUEUED",
        EV_TXN_PQUEUED => "TXN_PQUEUED",
        EV_TXN_PARKED_ASYNC => "PARKED_ASYNC",
        EV_REPLY_POP => "REPLY_POP",
        EV_REPLY_PUSHED => "REPLY_PUSHED",
        EV_REPLY_GONE => "REPLY_GONE",
        EV_TXN_RECV => "TXN_RECV",
        EV_REPLY_RECV => "REPLY_RECV",
        EV_TC_RECV => "TC_RECV",
        EV_ERR_RECV => "ERR_RECV",
        EV_PARK => "PARK",
        EV_WAKE_RET => "WAKE_RET",
        EV_BWR => "BWR",
        EV_REAP => "REAP",
        EV_DEAD_REPLY => "DEAD_REPLY",
        EV_REKICK => "REKICK",
        EV_TXN_RECV_END => "TXN_RECV_END",
        _ => "?",
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
    /// Transition ring (sc74 forensics); written under WORLD only.
    hist: [BinderEv; BHIST],
    hcur: usize,
}

struct BinderThread {
    tid: u32,
    /// Owning task's TGID (sc161 child-tgid routing): several appspawn
    /// children share ONE binder proc (the daemon's inherited fd), so the
    /// proc's thread list mixes daemon-tgid and child-tgid threads. The
    /// tgid decides which threads may consume a transaction aimed at a
    /// child-registered node (reg_tgid) — without it the daemon's OS_IPC
    /// reader threads steal the AMS→child deliveries.
    tgid: u32,
    /// Per-thread wait queue (Linux thread->wait): replies and tcomplete
    /// wake the exact thread; proc work wakes one selected waiter.
    wait: WaitQueueHead,
    st: core::cell::UnsafeCell<ThreadState>,
    /// Jiffies when this thread entered the blocking binder read wait (0
    /// while not waiting) — read atomically by the khungtaskd binder-stall
    /// scan without taking WORLD.
    read_wait_since: core::sync::atomic::AtomicU64,
    /// One stall report per wait episode (avoids per-scan spam).
    stall_reported: core::sync::atomic::AtomicBool,
    /// One REKICK hist record per wait episode (the re-kick itself still
    /// runs every scan; only the ring entry is gated so the forensics
    /// history is not flooded by heal noise).
    rekicked: core::sync::atomic::AtomicBool,
    /// Kicks issued in the current wait episode — >= 2 means a previous
    /// kick failed to run the task (wake→run leak); the scheduler view is
    /// dumped then (WAKE-RUN-LEAK line).
    rekick_count: core::sync::atomic::AtomicU32,
    /// sc252/sc253 attribution (leg 19): outcome counters for this
    /// thread's OWN wait-queue kicks, read by the WAKE-RUN-LEAK dump —
    /// wok>=1 with the task still parked means the wake linked the task
    /// and the PICK side never ran it; wno-only means every kick was
    /// silently refused (the R82 DL-ENQ-DEFERRED / R80-phantom families).
    kick_wake_ok: core::sync::atomic::AtomicU32,
    kick_wake_refused: core::sync::atomic::AtomicU32,
}

impl BinderThread {
    fn new(tid: u32, tgid: u32) -> Self {
        Self {
            tid,
            tgid,
            wait: WaitQueueHead::new(),
            read_wait_since: core::sync::atomic::AtomicU64::new(0),
            stall_reported: core::sync::atomic::AtomicBool::new(false),
            rekicked: core::sync::atomic::AtomicBool::new(false),
            rekick_count: core::sync::atomic::AtomicU32::new(0),
            kick_wake_ok: core::sync::atomic::AtomicU32::new(0),
            kick_wake_refused: core::sync::atomic::AtomicU32::new(0),
            st: core::cell::UnsafeCell::new(ThreadState {
                looper: 0,
                todo: VecDeque::new(),
                process_todo: false,
                waiting_for_proc_work: false,
                txn_stack: Vec::new(),
                hist: [BinderEv { j: 0, ev: 0, a: 0, b: 0 }; BHIST],
                hcur: 0,
            }),
        }
    }

    /// SAFETY: caller must hold WORLD.
    #[allow(clippy::mut_from_ref)]
    unsafe fn st(&self) -> &mut ThreadState {
        &mut *self.st.get()
    }

    /// Append one transition to the ring. SAFETY: caller must hold WORLD.
    unsafe fn hist(&self, ev: u8, a: u32, b: u32) {
        let th = self.st();
        let j = crate::drivers::timer::get_jiffies() as u32;
        th.hist[th.hcur] = BinderEv { j, ev, a, b };
        th.hcur = (th.hcur + 1) % BHIST;
    }
}

struct ProcInner {
    alloc: Option<BinderAlloc>,
    nodes: Vec<Arc<BinderNode>>,
    refs: Vec<BinderRef>,
    /// Next handle descriptor to hand out. Starts at 1: desc 0 is
    /// reserved for the context manager in every process (Linux
    /// binder_get_ref_for_node starts normal refs at 1).
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
    ctx: BinderCtx,
    wait: WaitQueueHead,
    inner: core::cell::UnsafeCell<ProcInner>,
}

impl BinderProc {
    fn new(id: u64, pid: u32, ctx: BinderCtx) -> Self {
        Self {
            id,
            pid,
            ctx,
            wait: WaitQueueHead::new(),
            inner: core::cell::UnsafeCell::new(ProcInner {
                alloc: None,
                nodes: Vec::new(),
                refs: Vec::new(),
                next_desc: 1,
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
    /// Context-manager node per device context (index by BinderCtx).
    mgr_node: [Option<Arc<BinderNode>>; 3],
}

static WORLD: Spinlock<Option<World>> = Spinlock::new(None);
static NEXT_PROC_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// Waiters parked while the context manager for their device context is
/// not yet installed (see wait_for_context_manager). set_context_mgr
/// wakes everyone on success.
static MANAGER_WAIT: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

/// Bounded block for handle-0 transactions issued before the context
/// manager exists (OH boot bring-up).
///
/// Every early-boot OH client (storage_daemon, hdf_devmgr, appspawn
/// helpers, ...) reaches for the samgr proxy with a BOUNDED retry budget
/// (hdf_devmgr: 16 tries). On this image samgr must fault in ~100 shared
/// libraries before its BINDER_SET_CONTEXT_MGR(_EXT) lands, and under
/// TCG that window outlasted those budgets: hdf_devmgr exited 255
/// ("failed to start service manager"), SA 5100 never registered and
/// every devhost then waited on it forever (boot b46, lines 3046-8623 —
/// the manager arrived ~400 serial lines AFTER the last retry). Instead
/// of failing BR_DEAD_REPLY instantly, park the transaction for up to
/// MANAGER_WAIT_MAX_MS total so the client budget stretches across the
/// gap. Linux answers BR_DEAD_REPLY immediately — this is a deliberate,
/// contained divergence for bring-up; nothing on OH probes for manager
/// absence on purpose.
fn wait_for_context_manager(ctx: BinderCtx) {
    const MANAGER_WAIT_MAX_MS: u64 = 8000;
    const SLICE_MS: u64 = 250;
    const SLICE_JIFFIES: u64 = (SLICE_MS * crate::config::KERNEL_HZ as u64) / 1000;
    let hz = crate::config::KERNEL_HZ as u64;

    let mut waited_ms = 0u64;
    loop {
        {
            let w = world_lock();
            let installed = w
                .as_ref()
                .map(|world| world.mgr_node[ctx as usize].is_some())
                .unwrap_or(true);
            if installed {
                return;
            }
        }
        if waited_ms >= MANAGER_WAIT_MAX_MS {
            return;
        }
        if crate::signal::signal_pending() {
            return;
        }
        // Park on MANAGER_WAIT with a timer backstop so a missing
        // set_context_mgr wake cannot strand the caller for the full
        // budget (khungtaskd discipline).
        let current = match crate::sched::current() {
            Some(t) => t as *mut crate::process::task::Task,
            None => return,
        };
        // SAFETY: current is this task's own pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + SLICE_JIFFIES.max(1);
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        MANAGER_WAIT.prepare_to_wait(current, false, true);
        if crate::drivers::timer::get_jiffies() >= target || crate::signal::signal_pending() {
            MANAGER_WAIT.finish_wait(current);
            if timer_id != 0 {
                crate::timer::del_timer(timer_id);
            }
            // SAFETY: a racing wake may have enqueued us while we are
            // still executing — take ourselves back off (NEW-C2).
            unsafe {
                (*current).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING,
                ));
                crate::sched::dequeue_task(&*current);
            }
        } else {
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            MANAGER_WAIT.finish_wait(current);
            if timer_id != 0 && crate::timer::timer_pending(timer_id) {
                crate::timer::del_timer(timer_id);
            }
        }
        waited_ms += SLICE_MS;
        let _ = hz;
    }
}

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
        *g = Some(World { next_id: 1, procs: Vec::new(), mgr_node: [None, None, None] });
    }
    g
}

/// sc160: non-blocking WORLD acquire for diagnostic readers that may be
/// called re-entrantly from fault handlers (see binder_mmap_owner_of).
/// Returns None when the lock is contended OR the world is not
/// initialized yet — both mean "no attribution available".
fn world_lock_try() -> Option<crate::sync::spinlock::SpinlockIrqGuard<'static, Option<World>>> {
    let mut g = WORLD.try_lock_irqsave()?;
    if g.is_none() {
        // Initializing the world from a diagnostic path is harmless but
        // pointless (no procs exist yet); just report empty.
        return None;
    }
    Some(g)
}

/// &mut BinderProc for an Arc, without borrowing the World. The Arc keeps
/// the object alive (callers hold clones) and every mutation is
/// serialized by WORLD — that is the access contract, NOT membership in
/// world.procs. b88 kernel-panicked on the old expect("proc not in
/// world") because binder_close removes the proc from the world BEFORE
/// draining its pending work, and finish_work then hit the expect: a
/// dying service must take its in-flight transactions down quietly, not
/// kill the kernel. Membership loss is now a bounded warning; the cast
/// itself only needs the Arc.
/// SAFETY: caller must hold WORLD.
#[allow(clippy::mut_from_ref)]
unsafe fn pm<'a>(_world: &World, p: &Arc<BinderProc>) -> &'a mut BinderProc {
    let ptr = Arc::as_ptr(p);
    if !_world.procs.iter().any(|x| core::ptr::eq(Arc::as_ptr(x), ptr)) {
        static DEAD_PM_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if DEAD_PM_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_warn!(
                "binder: access to dead proc {} (pid {}) — draining or replying to a closed service",
                p.id,
                p.pid
            );
        }
    }
    &mut *(ptr as *mut BinderProc)
}

fn proc_alive(world: &World, p: &Arc<BinderProc>) -> bool {
    world.procs.iter().any(|x| Arc::ptr_eq(x, p))
}

fn wake_thread(t: &BinderThread) {
    let n = t.wait.wake_up(crate::process::wait::WakeUpHint::Normal, 0);
    // sc252/sc253 attribution: wake_up counts entries it transitioned (a
    // refused wake is not counted — see WaitQueueHead::wake_up's W1
    // clear-and-continue). 0 therefore means "no entry transitioned":
    // either the queue is empty (the thread parks elsewhere — a futex)
    // or every attempt was refused.
    if n > 0 {
        t.kick_wake_ok.fetch_add(n as u32, core::sync::atomic::Ordering::Relaxed);
    } else {
        t.kick_wake_refused.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// sc163: post a transaction aimed at a forked-child node directly onto
/// a thread of THAT child. Children never enter the binder looper (sc163
/// boots: zero ENTER_LOOPER ioctls from any child thread), so no child
/// thread is ever eligible to pull proc-todo work (the looper gate in
/// binder_thread_read) — while the daemon's looper workers are
/// reg_tgid-gated off the item. Thread-todo delivery bypasses both:
/// own-thread todo is drained regardless of looper flags (the
/// `from_thread` pull) and a blocked WRITE_READ wakes through
/// thread_has_work's `!th.todo.is_empty()` check. Prefers a thread with
/// the shallowest txn stack; returns the woken thread (caller wakes it
/// AFTER WORLD is released, same contract as wake_ths).
/// SAFETY: caller must hold WORLD; pushes onto the thread's todo only.
unsafe fn post_txn_to_child_thread(
    world: &World,
    target: &Arc<BinderProc>,
    pref_tgid: u32,
    txn: &Arc<BinderTxn>,
) -> Option<Arc<BinderThread>> {
    let pick = pm(world, target)
        .im()
        .threads
        .iter()
        .filter(|t| {
            t.tgid == pref_tgid && {
                // sc165 fix: only LOOPER-entered threads reliably drain
                // their own todo (a plain client thread parked on the
                // EventHandler futex never returns to a binder read —
                // sc165 deliveries thread-targeted onto a parked child
                // MAIN thread died there while AMS clocked
                // LIFECYCLE_TIMEOUT).
                let th = t.st();
                (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0
            }
        })
        .min_by_key(|t| t.st().txn_stack.len())
        .cloned();
    let t = pick?;
    let th = t.st();
    th.todo.push_back(Work::Transaction(txn.clone()));
    th.process_todo = true;
    let tid = t.tid;
    t.hist(EV_TXN_TQUEUED, txn.id as u32, tid);
    Some(t)
}

/// leg-32B: deliver a parked oneway that is being handed the node's
/// async slot. Three drain/handoff paths do this (BC_FREE_BUFFER's
/// drain, sweep_stale_async, sweep_silent_txns' retire handoff) and all
/// three used to push straight onto the node OWNER's proc todo — the
/// fresh-enqueue path's forked-child preference (post_txn_to_child_thread,
/// sc163) was never applied here. For a child-registered node the owner
/// proc is appspawn: children never enter the looper so they never pull
/// proc todo, and the reg_tgid read gate keeps the daemon's readers off
/// the item — the drained txn strands forever. sc326B decode: AMS's
/// 0x5-code Schedule oneways to settingsdata (pid 1176) parked behind a
/// busy slot on node 13085, then "drained to proc 337" (appspawn); the
/// ability never loaded and LIFECYCLE_TIMEOUT x302 followed. Route
/// through the child's thread first; fall back to the owner proc todo
/// (the pre-leg-32B shape) when no eligible child thread exists.
/// Returns (child thread to wake AFTER WORLD release, prefer_tgid for
/// the proc wake when the fallback fired).
/// SAFETY: caller must hold WORLD.
unsafe fn drain_deliver_parked_txn(
    world: &World,
    txn: &Arc<BinderTxn>,
    to_proc: &Arc<BinderProc>,
) -> (Option<Arc<BinderThread>>, u32) {
    let pref = txn
        .target_node
        .as_ref()
        .map(|n| n.reg_tgid)
        .filter(|&g| g != 0 && g != to_proc.pid)
        .unwrap_or(0);
    if pref != 0 {
        if let Some(ct) = post_txn_to_child_thread(world, to_proc, pref, txn) {
            return (Some(ct), 0);
        }
    }
    pm(world, to_proc).im().todo.push_back(Work::Transaction(txn.clone()));
    (None, pref)
}

/// binder_select_thread_ilocked + wake: pick one thread of the proc that
/// is blocked waiting for proc work (Linux keeps them on
/// proc->waiting_threads) and wake just that one — no thundering herd
/// across a 16-thread samgr pool. The predicate must match
/// binder_available_for_proc_work_ilocked exactly (empty txn stack, empty
/// own todo, ENTERED/REGISTERED looper): b85 forensics showed wakes being
/// eaten by a parked reader without looper flags, which re-checked
/// thread_has_work, found itself ineligible and went back to sleep — the
/// queued txn then sat on the proc todo while eligible workers slept on.
///
/// sc161: `prefer_tgid` (0 = any) — when the posted work aims at a node
/// registered by a forked appspawn child (reg_tgid), prefer waking a
/// thread of THAT child: the proc mixes the daemon's OS_IPC readers with
/// all children's threads, and a daemon reader that wins the wake cannot
/// dispatch a child stub (its ProcessSkeleton has no such object — the
/// sc156 AMS→child black hole). The read-side ownership gate
/// (work_readable_by) is the backstop; this is the routing hint.
fn wake_proc_one(p: &Arc<BinderProc>, prefer_tgid: u32) {
    let w = world_lock();
    // SAFETY: read-only scan under WORLD.
    let mut pick = unsafe {
        pm(w.as_ref().unwrap(), p)
            .im()
            .threads
            .iter()
            .filter(|t| {
                let th = t.st();
                th.waiting_for_proc_work
                    && th.todo.is_empty()
                    && th.txn_stack.is_empty()
                    && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    drop(w);
    if let Some(t) = if prefer_tgid != 0 {
        pick.iter().position(|t| t.tgid == prefer_tgid).map(|i| pick.remove(i))
    } else {
        if !pick.is_empty() { Some(pick.remove(0)) } else { None }
    } {
        wake_thread(&t);
    }
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

/// A node is dead when its owner is gone from the world (Linux sets
/// node->proc = NULL in the owner's binder_release before delivering
/// death notifications; here the owner is removed from world.procs
/// first, under the same WORLD hold).
fn node_is_dead(world: &World, node: &Arc<BinderNode>) -> bool {
    node.dead.load(core::sync::atomic::Ordering::Acquire)
        || !world.procs.iter().any(|p| p.id == node.proc_id)
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
pub fn binder_open(file: &crate::fs::File, ctx: BinderCtx) -> i32 {
    // Linux stamps binder_proc->pid from current->group_leader: the
    // process id, not the opening thread's tid. GET_SENDER_INFO feeds
    // this into the access-token registry lookup — with a worker-thread
    // tid there, samgr would see a synthetic token for a tid ATM never
    // allocated (same failure family as the tid-keyed access_token
    // ioctl, b5).
    let pid = crate::sched::current()
        .map(|t| unsafe { (*t).tgid() as u32 })
        .unwrap_or_else(crate::process::current_pid);
    let id = NEXT_PROC_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let proc = Arc::new(BinderProc::new(id, pid, ctx));
    {
        let mut w = world_lock();
        w.as_mut().unwrap().procs.push(proc.clone());
    }
    // dfx=schb: record the new proc for the heartbeat's SC-PROC drain
    // (no-op unless the switch is armed; never any I/O on this path).
    crate::dfx::schb::note_proc_open(pid);
    crate::pr_info!("binder: pid {} opened the device (proc {})", pid, id);
    // sc157: device-open visibility at warn level (boot loglevel drops
    // the info line). Decides in one boot whether forked appspawn
    // children EVER open /dev/binder themselves (own proc, own pid) or
    // reuse the daemon's inherited fd (shared proc — the sc156 APPTXN
    // "from pid=330 comm='m.ohos.systemui'" shape).
    {
        static OPEN_PROBE_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if OPEN_PROBE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 220 {
            crate::pr_warn!("binder-open: pid {} proc {}", pid, id);
        }
    }
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
        if let Some(mgr) = &world.mgr_node[proc.ctx as usize] {
            if mgr.proc_id == proc.id {
                world.mgr_node[proc.ctx as usize] = None;
            }
        }
        // SAFETY: all inner access below is under WORLD.
        unsafe {
            proc.im().is_dead = true;

            // sc105 OsAccount probe: a service's binder fd closing is the
            // ONLY transition that removes a proc (and its nodes) from the
            // world — clients then get BR_DEAD_REPLY (userspace error 32,
            // "proxy is already dead") forever after. The OH accountmgr
            // service shows that signature while its process is still
            // serving (UnlockUser logs minutes later), so the close is the
            // event to localize: who closed, what died with it. Rate-
            // limited budget so a chatty boot cannot flood the UART.
            {
                use core::sync::atomic::Ordering::Relaxed;
                static CLOSE_PROBE_BUDGET: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(24);
                let owned = proc.im().nodes.len();
                if owned > 0
                    && CLOSE_PROBE_BUDGET.fetch_sub(1, Relaxed) > 0
                {
                    let mut victims = alloc::vec::Vec::new();
                    for q in world.procs.iter() {
                        if q.id == proc.id {
                            continue;
                        }
                        let n = pm(world, q)
                            .im()
                            .refs
                            .iter()
                            .filter(|r| r.node.proc_id == proc.id)
                            .count();
                        if n > 0 {
                            victims.push((q.pid, n));
                        }
                    }
                    crate::pr_warn!(
                        "binder: CLOSE proc {} pid {} owned_nodes={} foreign_refs={:?}",
                        proc.id,
                        proc.pid,
                        owned,
                        victims
                    );
                }
            }

            // Deliver death notifications: every ref in a surviving proc
            // that points at one of our nodes and carries an Armed death
            // request fires now (Linux binder_node_release walking
            // node->refs).
            for q in world.procs.clone() {
                let mut deaths: Vec<(u32, u64)> = Vec::new();
                {
                    let qim = pm(world, &q).im();
                    for r in qim.refs.iter_mut() {
                        if r.node.proc_id != proc.id {
                            continue;
                        }
                        if let Some(d) = r.death.as_mut() {
                            if d.phase == DeathPhase::Armed {
                                d.phase = DeathPhase::Queued;
                                deaths.push((r.desc, d.cookie));
                            }
                        }
                    }
                }
                if !deaths.is_empty() {
                    let qim = pm(world, &q).im();
                    for (desc, cookie) in deaths {
                        crate::pr_info!(
                            "binder: death notice pid {} desc {} cookie {:#x} (owner {} died)",
                            q.pid,
                            desc,
                            cookie,
                            proc.pid
                        );
                        qim.todo.push_back(Work::DeadBinder { desc, cookie });
                    }
                    wake_targets.push(q.clone());
                }
            }

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
                let queued: Vec<Work> = th.todo.drain(..).collect();
                for wk in queued {
                    if let Work::Transaction(x) = &wk {
                        wake_targets.extend(dead_reply_to_sender(world, &proc, x));
                    }
                    finish_work(world, &proc, wk);
                }
                // In-flight txns the thread was processing: their senders
                // wait for a reply the dying proc can never produce
                // (Linux binder_release_work / send_failed_reply parity).
                let in_flight: Vec<Arc<BinderTxn>> = th.txn_stack.drain(..).collect();
                for x in in_flight {
                    wake_targets.extend(dead_reply_to_sender(world, &proc, &x));
                    finish_work(world, &proc, Work::Transaction(x));
                }
            }
            let proc_queued: Vec<Work> = proc.im().todo.drain(..).collect();
            for wk in proc_queued {
                if let Work::Transaction(x) = &wk {
                    wake_targets.extend(dead_reply_to_sender(world, &proc, x));
                }
                finish_work(world, &proc, wk);
            }
            // Parked oneway transactions on our nodes: their buffers live
            // in our region — free them before the alloc goes away.
            for node in proc.im().nodes.clone() {
                for parked in node.st().async_todo.drain(..) {
                    finish_work(world, &proc, Work::Transaction(parked));
                }
            }
            // Async buffers still held by userspace keep oneway txns
            // alive — release their node slots too.
            if let Some(alloc) = proc.im().alloc.as_mut() {
                let pending: Vec<Arc<BinderTxn>> = alloc.blocks.iter().filter_map(|b| b.txn.clone()).collect();
                for t in pending {
                    if t.holds_async_slot() {
                        if let Some(node) = &t.target_node {
                            node.st().has_async_transaction = false;
                        }
                    }
                    unpin_txn_node(world, &t);
                }
            }
            proc.im().alloc = None; // frees the region pages (see Drop)
        }
    }
    drop(proc);
    for p in wake_targets {
        wake_proc_one(&p, 0);
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
    }
    // A ref-state transition may now be visible to the owner.
    let external = node_external_refs(world, node);
    let owner = world.procs.iter().find(|p| p.id == node.proc_id);
    if let Some(owner) = owner {
        if (!ns.strong() && ns.has_strong_ref) || (!ns.weak(external) && ns.has_weak_ref) {
            owner.im().todo.push_back(Work::Node(node.clone()));
            // pick-one wake; runs under WORLD like the S1 wake did
            let pick = {
                let th = owner.im().threads.iter().find(|t| {
                    let x = t.st();
                    x.waiting_for_proc_work && x.todo.is_empty() && x.txn_stack.is_empty()
                }).cloned();
                th
            };
            if let Some(t) = pick {
                wake_thread(&t);
            }
        }
    }
}

/// Deliver a BR_DEAD_REPLY to the SENDER of a sync transaction whose
/// processing side is going away — and drop the txn from the sender's
/// transaction stack.
///
/// Used by the thread-reap and proc-close paths for transactions that
/// were queued (thread/proc todo) but never read, in addition to the
/// classic in-flight (txn_stack) case. Without the reply the sender's
/// BC_TRANSACTION blocks forever, and the stale stack entry keeps
/// routing that thread's later NESTED sync calls to the dead target
/// thread (the from-walk in binder_transaction matches on it), so one
/// exited thread strands every future nested call of the same client
/// thread too (b139: hdf_devmgr tid 341 exited with txn2053 queued on
/// its todo; pid 532 tid 685 then sat 124s+ "waiting for reply, 1 txn
/// stacked" while devmgr idled). Oneway txns never wait for a reply —
/// no notification, matching Linux binder_release_work.
///
/// Returns the procs whose readers must be re-checked for wake-up
/// AFTER WORLD is released (same contract as binder_reap_thread).
///
/// SAFETY: caller holds WORLD.
unsafe fn dead_reply_to_sender(
    world: &World,
    dying: &Arc<BinderProc>,
    txn: &Arc<BinderTxn>,
) -> Vec<Arc<BinderProc>> {
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    if txn.flags & TF_ONE_WAY != 0 {
        return wakes; // oneway: nobody is waiting
    }
    let Some(fp) = &txn.from_proc else {
        return wakes;
    };
    let same = Arc::ptr_eq(fp, dying);
    let mut delivered = false;
    if let Some(ftid) = txn.from_tid {
        let pool = if same {
            dying.im().threads.clone()
        } else {
            pm(world, fp).im().threads.clone()
        };
        if let Some(ft) = pool.into_iter().find(|ct| ct.tid == ftid) {
            let fth = ft.st();
            // The call this thread is parked in is dead: unstack the
            // txn so the from-walk stops routing nested calls to the
            // vanished target (mirrors the BC_REPLY retain).
            fth.txn_stack.retain(|t| !Arc::ptr_eq(t, txn));
            fth.todo.push_back(Work::ReturnError(BR_DEAD_REPLY, 0));
            fth.process_todo = true;
            ft.hist(EV_DEAD_REPLY, txn.id as u32, ftid);
            // Exact-thread wake, same as the BC_REPLY path: the proc-wide
            // pick-one wake (wake_proc_one) only selects threads with an
            // EMPTY todo, so a thread carrying this ReturnError would
            // sleep on until unrelated work arrived.
            wake_thread(&ft);
            delivered = true;
        }
    }
    if !delivered {
        if same {
            dying.im().todo.push_back(Work::ReturnError(BR_DEAD_REPLY, 0));
        } else {
            pm(world, fp).im().todo.push_back(Work::ReturnError(BR_DEAD_REPLY, 0));
        }
    }
    if same {
        wakes.push(dying.clone());
    } else {
        wakes.push(fp.clone());
    }
    wakes
}

/// Abort a pool thread's in-flight state and remove it from the pool.
///
/// Shared by BINDER_THREAD_EXIT (musl's clean IPC-thread shutdown) and
/// the task-exit hook `binder_task_died` (threads killed by a signal
/// never reach the ioctl — the b86-b93 heap-corruption victims).
/// Every transaction the thread was PROCESSING gets its sender a
/// BR_DEAD_REPLY (Linux binder_thread_release semantics — the reply
/// will never come); queued work is finished (buffers freed, node pins
/// dropped). Returns the procs whose readers must be re-checked for
/// wake-up AFTER WORLD is released.
///
/// SAFETY: caller must hold WORLD.
unsafe fn binder_reap_thread(
    world: &World,
    proc: &Arc<BinderProc>,
    tid: u32,
) -> Vec<Arc<BinderProc>> {
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    let Some(t) = proc.im().threads.iter().find(|t| t.tid == tid).cloned() else {
        return wakes;
    };
    let th = t.st();
    let mut self_wake = false;
    // Materialize before iterating: dead_reply_to_sender can retain()
    // the SENDER's txn_stack, which for a self-referencing txn is this
    // very list — a lazy drain would be mutated mid-iteration.
    let in_flight: Vec<Arc<BinderTxn>> = th.txn_stack.drain(..).collect();
    for x in in_flight {
        // Sender wake-up for the never-coming reply.
        for fp in dead_reply_to_sender(world, proc, &x) {
            if Arc::ptr_eq(&fp, proc) {
                self_wake = true;
            } else {
                wakes.push(fp);
            }
        }
        // Free the delivered buffer + drop the node pin: an abandoned
        // sync txn held its pin forever when the processing thread died
        // (b92 samgr: pinned nodes never reached the 0-refs transition).
        finish_work(world, proc, Work::Transaction(x));
    }
    let queued: Vec<Work> = th.todo.drain(..).collect();
    for wk in queued {
        // QUEUED work was never read: a sync txn's sender still waits
        // for the reply that now can never come (b139 txn2053 shape).
        if let Work::Transaction(x) = &wk {
            for fp in dead_reply_to_sender(world, proc, x) {
                if Arc::ptr_eq(&fp, proc) {
                    self_wake = true;
                } else {
                    wakes.push(fp);
                }
            }
        }
        finish_work(world, proc, wk);
    }
    proc.im().threads.retain(|ct| ct.tid != tid);
    if self_wake {
        wakes.push(proc.clone());
    }
    wakes
}

/// Task-exit hook (called from do_exit): a thread that dies without
/// reaching BINDER_THREAD_EXIT must leave its process's binder pool.
/// A corpse pool entry blocks the BR_SPAWN_LOOPER replenishment gate
/// (`threads.len() < max_threads` counts it as a live worker), the pool
/// starves, proc todo grows without a processor, and every client of
/// the process stalls (b92: samgr todo depth 13 with 16 registered
/// threads — all dead — and hdf_devhost waiters parked 162s).
pub fn binder_task_died(dying_pid: u32, dying_tgid: u32, dying_tid: u32) {
    // sc161: group-leader death first — a forked appspawn child's NODES
    // live inside the DAEMON's shared proc, so the membership lookup
    // below misses them entirely and they would keep accepting
    // transactions no thread can ever read. Scoped Linux-style death for
    // reg_tgid nodes (fail queued txns, queue DeadBinder for armed
    // recipients, mark + unlist the nodes).
    if dying_pid == dying_tgid {
        binder_on_tgid_exit(dying_tgid);
    }
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    {
        let mut w = world_lock();
        let Some(world) = w.as_mut() else { return };
        // SAFETY: under WORLD.
        unsafe {
            // Membership: OH opens /dev/binder on the group leader
            // (proc.pid == tgid); a non-leader opener is matched by its
            // own pid so its personal pool entry is still reaped. sc161:
            // a forked CHILD's threads share the daemon's proc — match
            // by the dying tid's pool entry so their reap runs too.
            let proc = world
                .procs
                .iter()
                .find(|p| p.pid == dying_tgid || p.pid == dying_pid)
                .cloned()
                .or_else(|| {
                    world.procs.iter().find(|p| {
                        pm(world, p).im().threads.iter().any(|t| t.tid == dying_tid)
                    }).cloned()
                });
            let Some(proc) = proc else { return };
            if proc.im().is_dead {
                return;
            }
            // sc76: samgr died thread-in-place — every worker futex-blocked
            // behind lock words whose owners are gone (16 phantom rwlock
            // readers with only 7 live threads). A worker that dies while
            // SERVICING (txn stacked) or with undrained work leaks its
            // userspace locks permanently (pthread mutexes are not robust).
            // This detector names every such victim at death time.
            if let Some(t) = proc.im().threads.iter().find(|t| t.tid == dying_tid) {
                let th = t.st();
                if !th.txn_stack.is_empty() || !th.todo.is_empty() {
                    crate::pr_warn!(
                        "binder: THREAD-DIED-SERVICING pid {} tid {} stack={} todo={} top=txn{} from pid {} tid {}",
                        proc.pid,
                        dying_tid,
                        th.txn_stack.len(),
                        th.todo.len(),
                        th.txn_stack.last().map(|x| x.id).unwrap_or(0),
                        th.txn_stack.last().and_then(|x| x.from_proc.as_ref()).map(|p| p.pid).unwrap_or(0),
                        th.txn_stack.last().and_then(|x| x.from_tid).unwrap_or(0)
                    );
                }
            }
            wakes = binder_reap_thread(world, &proc, dying_tid);
        }
    }
    for p in wakes {
        wake_proc_one(&p, 0);
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

    // Double-mmap check FIRST, before any allocation or VMA surgery: a
    // second binder mmap on the same proc must only cost the EBUSY.
    {
        let w = world_lock();
        // SAFETY: under WORLD.
        unsafe {
            if pm(w.as_ref().unwrap(), &proc).im().alloc.is_some() {
                return Err(-16); // EBUSY: already mapped
            }
        }
    }

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

    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ);
    vma_flags.insert(VmaFlags::WRITE);
    vma_flags.insert(VmaFlags::SHARED);
    // sc157 stamped VM_DONTCOPY here (Linux binder_mmap parity) so forked
    // children lose the buffer mapping and "must open the device
    // themselves". sc159/sc160 falsified the premise for THIS userspace:
    // OH's appspawn DAEMON initializes the binder singleton pre-fork (the
    // ace_adapter PreloadModule -> Rosen DisplayManagerAdapter -> IPC
    // chain), and the forked children inherit BinderConnector::instance_
    // (a plain C++ singleton, no fork/pid guard anywhere in
    // foundation/communication/ipc) — they NEVER re-open /dev/binder.
    // With the mapping dropped, every child's first reply-parcel read
    // derefs the stale daemon buffer VA (BR_REPLY carries
    // alloc.user_base + offset; sc160: cr2 0x7ff099f1d110 inside the
    // daemon proc-11 buffer) and the whole launcher cohort dies in a
    // crash-retry loop (681 SIGSEGV reports in sc159). Internal choice
    // (external binder ABI unchanged): inherit the buffer as SHARED
    // pages at fork — pre-sc157 behavior — while the sc157 reg_tgid
    // stamp plus child-tgid transaction routing (sc161) keep AMS->child
    // deliveries out of the daemon's reader threads. The DONTFORK
    // machinery in mm fork stays (unused by binder now) for future
    // Linux-parity work if OH ever grows a fork-safe IPC layer.

    // ATOMIC pick + reserve (b91 heap-corruption fix): the old shape ran
    // find_free_area on a READ lock, then spent the 4 MiB memset + eager
    // PTE-install window BEFORE taking vma_write to add the VMA. A second
    // thread of the same mm (mallocng group mmap, pthread stack) was
    // handed the same "free" address in that window; whichever side lost
    // either (a) had its demand-installed PTEs overwritten by this loop's
    // eager map_page — orphaning live heap frames and pointing the VMA at
    // binder frames — or (b) failed the VmaManager::add here with Overlap
    // AFTER its PTEs were installed, then FREE_PAGES'd the block while
    // still mapped: the buddy re-handed those frames and every later
    // store through the surviving VMA corrupted the new owner in place
    // (the b86-b91 zeroed-buffer / mallocng-meta family). Mirrors the
    // sys_mmap pick-and-insert fix (mm_ops.rs mmap).
    let vaddr = {
        let mut vmas = addr_space.vma_write();
        let user_end = crate::arch::mm::user_addr::USER_END;
        let mut base = if addr == 0 {
            None
        } else {
            let b = addr & !(PAGE_SIZE - 1);
            let Some(end) = b.checked_add(size) else {
                free_pages(phys, order);
                return Err(-22);
            };
            if end > user_end {
                free_pages(phys, order);
                return Err(-22);
            }
            // Hint honored only when the whole range is VMA-free.
            let probe = Vma::new(
                crate::mm::page::VirtAddr::new(b),
                crate::mm::page::VirtAddr::new(b + size),
                vma_flags,
            );
            if vmas.iter().any(|v| v.overlaps(&probe)) {
                None
            } else {
                Some(b)
            }
        };
        if base.is_none() {
            // Bottom-up free-area scan from MMAP_START under the WRITE
            // lock (same placement rule as MmStruct::find_free_area).
            use crate::arch::mm::user_addr as ua;
            let mut search_start = ua::MMAP_START;
            let search_end = ua::MMAP_END.min(user_end.saturating_sub(size));
            let mut hit: Option<usize> = None;
            for v in vmas.iter() {
                let vs = v.start().as_usize();
                if vs > search_start && vs - search_start >= size {
                    hit = Some(search_start);
                    break;
                }
                if v.end().as_usize() > search_start {
                    search_start = v.end().as_usize();
                }
                if search_start > search_end {
                    break;
                }
            }
            let b = match hit {
                Some(b) => b,
                None => {
                    if search_start <= search_end && search_end - search_start >= size {
                        search_start
                    } else {
                        drop(vmas);
                        free_pages(phys, order);
                        return Err(-12);
                    }
                }
            };
            base = Some(b);
        }
        let b = base.unwrap();
        let mut vma = Vma::new(
            crate::mm::page::VirtAddr::new(b),
            crate::mm::page::VirtAddr::new(b + size),
            vma_flags,
        );
        vma.set_origin(crate::mm::vma::VmaOrigin::from_bits(
            crate::mm::vma::VmaOrigin::BINDER,
        ));
        if let Err(_overlap) = vmas.add(vma) {
            drop(vmas);
            free_pages(phys, order);
            return Err(-12);
        }
        b
    };

    let root = addr_space.root_ppn();
    let mut pte_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;
    if prot & 0x1 != 0 {
        pte_flags |= PageTableEntry::R;
    }
    if prot & 0x2 != 0 {
        pte_flags |= PageTableEntry::R | PageTableEntry::W;
    }
    // Eager install under PTE_MODIFY_LOCK (the NEW2 class: an unlocked
    // leaf-PTE writer racing fork's COW walk or a COW fault diverges
    // refcounts and leaves dangling child PTEs). The VMA is already
    // reserved, so a demand fault on this range in the reserve→map window
    // is only possible from a same-mm thread touching a brand-new address
    // it cannot know; if one did win a page anyway, its anon zero page is
    // exclusive inside OUR VMA — zap it and install the binder frame so
    // the kvirt/PTE pairing (binder's coherence contract) holds.
    {
        #[cfg(feature = "x86_64")]
        let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
        #[cfg(not(feature = "x86_64"))]
        let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
        for i in 0..(size / PAGE_SIZE) {
            let va = vaddr + i * PAGE_SIZE;
            let pa = phys + i * PAGE_SIZE;
            // SAFETY: root is the task's live page-table root; the walk is
            // read-only and lock-protected.
            if let Some((old_ppn, _bits)) =
                unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va as u64) }
            {
                // Demand-fault winner: free its exclusive anon page, then
                // replace below. put_page→0 frees; refcount is 1 (fresh
                // zero page inside our private VMA window).
                let page = crate::mm::pfn_to_page_mut(old_ppn as usize);
                if !page.is_null() {
                // SAFETY: descriptor of the mapped frame under PTE lock.
                unsafe {
                    (*page).rmap_alt_remove(
                        addr_space as *const _ as usize,
                        va / PAGE_SIZE,
                    );
                        (*page).dec_mapcount();
                        if (*page).put_page() == 0 {
                            crate::mm::page_alloc::free_pages((old_ppn as usize) << 12, 0);
                        }
                    }
                }
                addr_space.sub_rss(1);
            }
            // SAFETY: root is the task's page-table root; phys..phys+size is our
            // fresh contiguous allocation, mapped page-aligned under the PTE lock.
            unsafe {
                map_page(root, VirtAddr::new(va as u64), PhysAddr::new(pa as u64), pte_flags);
            }
            // Mapping reference (io_uring discipline): munmap drops one,
            // binder_close drops the owner one, the last drop frees.
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(pa));
            if !page.is_null() {
                // SAFETY: descriptor of our freshly allocated frame.
                unsafe { (*page).get_page(); }
            }
        }
        crate::arch::mm::asid::flush_tlb_all();
    }

    let mut alloc = BinderAlloc {
        kvirt,
        phys,
        npages: size / PAGE_SIZE,
        order,
        user_base: vaddr as u64,
        blocks: Vec::new(),
        free_async_space: size / 2,
        last_sweep_j: 0,
        next_buf_id: 1,
    };
    alloc.blocks.push(BufBlock {
        off: 0,
        len: size,
        free: true,
        async_block: false,
        allow_user_free: false,
        data_size: 0,
        offsets_size: 0,
        extra_size: 0,
        buf_id: 0,
        txn: None,
        reply_done: false,
    });
    {
        let mut w = world_lock();
        // SAFETY: under WORLD.
        unsafe {
            pm(w.as_mut().unwrap(), &proc).im().alloc = Some(alloc);
        }
    }
    // sc160 (Session R leg 2): every binder buffer range at warn level —
    // the sc159 launcher children crash on a stale pointer at
    // 0x7ff099f3e370 (an unmapped hole above their mapped libs) and the
    // DONTFORK drop of the daemon's binder VMA is the prime suspect;
    // the daemon's [base, base+size) vs the crash VA decides it in one
    // boot without a guest-side maps dump.
    {
        static MMAP_PROBE_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if MMAP_PROBE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
            crate::pr_warn!(
                "binder-mmap: pid {} proc {} base {:#x} pages {}",
                task.pid(),
                proc.id,
                vaddr,
                size / PAGE_SIZE
            );
        }
    }
    crate::pr_info!("binder: proc {} mapped {:#x} bytes at {:#x}", proc.id, size, vaddr);
    Ok(vaddr)
}

/// sc160: does this user VA fall inside any live binder buffer mapping?
/// Forked appspawn children lost exactly those mappings (VmaFlags
/// DONTFORK, sc157) while keeping the daemon's pointers in their
/// ProcessSkeleton tables — a child SIGSEGV whose cr2 lands in one of
/// these ranges is the stale-binder-buffer crash, attributed to the
/// owning proc here. Returns (proc_id, owner_pid, base, size).
pub fn binder_mmap_owner_of(va: u64) -> Option<(u64, u64, u64, u64)> {
    // try_lock: this is called from the SIGSEGV forensics path, which can
    // fire while a binder thread already holds WORLD on this CPU (a
    // copy_from_user fault under the lock) — a blocking acquire there
    // would self-deadlock; skipping attribution in that corner is fine.
    let w = world_lock_try()?;
    let world = w.as_ref()?;
    for p in &world.procs {
        // SAFETY: WORLD held for the whole scan; only reading alloc.
        if let Some(a) = unsafe { pm(world, p).im().alloc.as_ref() } {
            let size = (a.npages * PAGE_SIZE) as u64;
            if va >= a.user_base && va < a.user_base + size {
                return Some((p.id, p.pid as u64, a.user_base, size));
            }
        }
    }
    None
}

/// sc161: a task group exited. Forked appspawn children register binder
/// NODES inside the DAEMON's shared proc (reg_tgid = child); when the
/// child dies those nodes outlive it — the daemon's proc lives on, so
/// node_is_dead stays false, AMS's stale refs keep resolving, and every
/// new transaction to such a node queues forever on proc work only the
/// (gone) child's tgid may read (work_readable_by gate). A sync caller
/// of a dead child would then park in WaitForReply forever. Linux gets
/// this for free from per-process binder teardown; replicate the death
/// semantics scoped to the child's nodes:
/// - queued proc-todo / node async-todo transactions aimed at a victim
///   node get BR_DEAD_REPLY to their senders (dead_reply_to_sender),
/// - the node is marked dead (node_is_dead now true → armed death
///   recipients in every holder proc are queued DeadBinder work; new
///   transactions through stale refs fail fast),
/// - the node leaves the owner's list.
pub fn binder_on_tgid_exit(tgid: u32) {
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    {
        let mut w = world_lock();
        let Some(world) = w.as_mut() else { return };
        // Collect (owner, node) pairs registered by the dying tgid.
        // SAFETY: all access below is under WORLD.
        let mut victims: Vec<(Arc<BinderProc>, Arc<BinderNode>)> = Vec::new();
        for p in world.procs.clone() {
            // SAFETY: under WORLD.
            for n in unsafe { pm(world, &p).im().nodes.clone() } {
                if n.reg_tgid == tgid && n.reg_tgid != p.pid as u32 {
                    victims.push((p.clone(), n));
                }
            }
        }
        if victims.is_empty() {
            return;
        }
        static EXIT_PROBE_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if EXIT_PROBE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            crate::pr_warn!(
                "binder: tgid {} exit reaped {} child-registered nodes",
                tgid,
                victims.len()
            );
        }
        // SAFETY: WORLD held for the rest of the block.
        unsafe {
            for (owner, node) in &victims {
                // 1. Fail proc-todo transactions aimed at this node.
                let queued: Vec<Work> = pm(world, owner).im().todo.drain(..).collect();
                for wk in queued {
                    let hit = match &wk {
                        Work::Transaction(t)
                            if t
                                .target_node
                                .as_ref()
                                .map(|n| Arc::ptr_eq(n, node))
                                .unwrap_or(false) =>
                        {
                            wakes.extend(dead_reply_to_sender(world, owner, t));
                            true
                        }
                        _ => false,
                    };
                    if hit {
                        finish_work(world, owner, wk);
                    } else {
                        pm(world, owner).im().todo.push_back(wk);
                    }
                }
                // 2. Fail the node's parked async transactions.
                let parked: Vec<Arc<BinderTxn>> = node.st().async_todo.drain(..).collect();
                for t in parked {
                    t.set_holds_async_slot(false);
                    wakes.extend(dead_reply_to_sender(world, owner, &t));
                    finish_work(world, owner, Work::Transaction(t));
                }
                // 3. Mark dead + queue DeadBinder for armed death
                //    recipients across every holder proc.
                node.dead.store(true, core::sync::atomic::Ordering::Release);
                for q in world.procs.clone() {
                    if Arc::ptr_eq(&q, owner) {
                        continue;
                    }
                    let qim = pm(world, &q);
                    for r in qim.im().refs.iter_mut() {
                        if Arc::ptr_eq(&r.node, node) {
                            if let Some(d) = r.death.as_mut() {
                                if matches!(d.phase, DeathPhase::Armed) {
                                    d.phase = DeathPhase::Queued;
                                    qim.im().todo.push_back(Work::DeadBinder {
                                        desc: r.desc,
                                        cookie: d.cookie,
                                    });
                                    wakes.push(q.clone());
                                }
                            }
                        }
                    }
                }
                // 4. Drop the node from the owner's list.
                pm(world, owner)
                    .im()
                    .nodes
                    .retain(|n| !Arc::ptr_eq(n, node));
            }
        }
    }
    for p in wakes {
        wake_proc_one(&p, 0);
    }
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
    read_at: None,
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

/// One-line summary of a thread for the stall dump (b85 follow-up: the
/// wedge state was invisible — "1 txn(s) stacked" said nothing about
/// what the txn was or why nobody took the proc todo).
/// SAFETY: caller holds WORLD.
unsafe fn stall_thread_line(
    proc: &Arc<BinderProc>,
    t: &Arc<BinderThread>,
    out: &mut alloc::string::String,
) {
    use core::fmt::Write;
    let th = t.st();
    let looper = |l: u32| {
        let mut s = [b'-'; 3];
        if l & LOOPER_REGISTERED != 0 {
            s[0] = b'R';
        }
        if l & LOOPER_ENTERED != 0 {
            s[1] = b'E';
        }
        if l & LOOPER_EXITED != 0 {
            s[2] = b'X';
        }
        s
    };
    let lstr = looper(th.looper);
    let loop_str = core::str::from_utf8(&lstr).unwrap_or("-=-");
    // sc89: formatting (this fn) stays under WORLD; the slow parts —
    // console output and the holder-task page-table walks — run after
    // the guard drops (see holder_forensics + the flush at the end of
    // stall_report). sc82: the whole dump under the IRQ-saving guard
    // ran ~500 ms with irqs off and killed khungtaskd.
    match th.txn_stack.last() {
        Some(txn) => {
            let shape = if txn.to_tid() == Some(t.tid) {
                "SERVER-OWING"
            } else if txn.from_tid == Some(t.tid) {
                "CLIENT-WAIT"
            } else {
                "AMBIGUOUS"
            };
            let _ = writeln!(
                out,
                "  pid {} tid {} loop={} wait={} todo={} stack={} [{}] top=txn{} code {} oneway={} from pid {} tid {} to_tid {}",
                proc.pid,
                t.tid,
                loop_str,
                th.waiting_for_proc_work,
                th.todo.len(),
                th.txn_stack.len(),
                shape,
                txn.id,
                txn.code,
                txn.flags & TF_ONE_WAY != 0,
                txn.from_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                txn.from_tid.unwrap_or(0),
                txn.to_tid().unwrap_or(0),
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  pid {} tid {} loop={} wait={} todo={} stack=0",
                proc.pid,
                t.tid,
                loop_str,
                th.waiting_for_proc_work,
                th.todo.len(),
            );
        }
    }
}

/// Holder-task forensics (b128-b135: a samgr thread parks 120s+ INSIDE
/// a txn and every display-chain client queues behind it). The binder
/// view can't say where the thread actually is, so read its Task:
/// scheduling state plus the user entry frame at kernel_stack_top -
/// PT_REGS_SIZE (both arches place the user entry frame at that fixed
/// offset — see the pt_regs module docs). Reads ONLY scheduler/futex
/// state and the holder's own page tables — sc89: safe (and required)
/// to run OUTSIDE the WORLD guard; `t.tid` is immutable.
/// SAFETY: `t` must point at a live BinderThread.
unsafe fn holder_forensics(t: &Arc<BinderThread>, out: &mut alloc::string::String) {
    use core::fmt::Write;
    let task = unsafe { crate::sched::find_task_by_pid(t.tid) };
    if task.is_null() {
        return;
    }
    let tk = unsafe { &*task };
    let comm = {
        let c = tk.comm();
        core::str::from_utf8(&c[..])
            .unwrap_or("")
            .trim_end_matches('\0')
    };
    // sc261: an exited thread's kernel stack is back in the cache and
    // kernel_stack_top() reports KERNEL_STACK_SIZE — an entry-frame
    // read then page-faults the kernel (sc261, the kill_dump path
    // died exactly so). The old SAFETY claim "the kernel stack stays
    // mapped while the Task lives" was wrong: the stack is freed at
    // thread exit while the Task struct survives until its last pin.
    // top = 0 wraps to a noncanonical address on purpose-never-deref:
    // the guards below skip the read instead.
    let top = if tk.owns_kernel_stack() {
        tk.kernel_stack_top()
    } else {
        0
    };
    #[cfg(feature = "x86_64")]
    let (rip, sys, a0, a1) = if top == 0 {
        (0, 0, 0, 0)
    } else {
        let regs = (top.wrapping_sub(
            crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
        )) as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's own entry
        // frame; the kernel stack stays mapped while the Task
        // lives, and the frame slot is written on every entry.
        unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi) }
    };
    #[cfg(feature = "riscv64")]
    let (rip, sys, a0, a1) = if top == 0 {
        (0, 0, 0, 0)
    } else {
        let regs = (top.wrapping_sub(
            crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
        )) as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 twin above.
        unsafe { ((*regs).epc, (*regs).a7, (*regs).a0, (*regs).a1) }
    };
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    let (rip, sys, a0, a1) = (0u64, 0u64, 0u64, 0u64);
    let _ = writeln!(
        out,
        "   holder task comm='{}' state={:#x} rip={:#x} sys={:#x} a0={:#x} a1={:#x}",
        comm,
        tk.state().bits(),
        rip,
        sys,
        a0,
        a1
    );
    // If the holder is parked in futex_wait, correlate its
    // waiter entry: woken=true-but-still-asleep indicts the
    // wake→run path; woken=false indicts the key match (key_mm
    // vs the task's own current identity) or the releaser.
    if let Some((uaddr, key_mm, woken)) = crate::sync::futex::waiter_of(task) {
        let _ = writeln!(
            out,
            "   futex waiter uaddr={:#x} key_mm={:#x} own_mm={:#x} woken={}",
            uaddr,
            key_mm,
            crate::sync::futex::task_futex_mm_id(task),
            woken
        );
        // sc75: read the futex WORD through the HOLDER's own
        // page tables (khungtaskd's get_user would use the
        // wrong mm). The word says whether the lock is still
        // owned (pthread mutexes store the owner tid) or
        // already released — a released word plus a sleeping
        // waiter is a lost futex wake; an owned word names the
        // owner to chase.
        let word = tk.address_space().and_then(|aspace| {
            let root = aspace.root_ppn();
            // SAFETY: read-only diagnostic walk of the
            // holder's live user page tables.
            unsafe {
                crate::arch::mm::mm_ops::PageTableWalker::walk(root, uaddr as u64)
            }
            .map(|(ppn, _)| {
                let pa = (ppn as usize) << 12;
                let kv = crate::arch::mm::phys_to_virt(
                    crate::arch::mm::PhysAddr::new(pa as u64),
                )
                .bits() as usize
                    + (uaddr & 0xfff);
                // SAFETY: the walked leaf maps the holder's
                // own futex word; one volatile u32 read.
                unsafe { core::ptr::read_volatile(kv as *const u32) }
            })
        });
        match word {
            Some(v) => {
                // Dump the neighbor words too (libc lock structs
                // pack owner/counts beside the futex word — musl
                // recursive/rwlock layouts need them to decode).
                let mut nb = [0u32; 4];
                let mut got = 0;
                for (k, slot) in nb.iter_mut().enumerate() {
                    let addr = uaddr + 4 + 4 * k;
                    if let Some(x) = tk.address_space().and_then(|aspace| {
                        let root = aspace.root_ppn();
                        // SAFETY: read-only diagnostic walk.
                        unsafe {
                            crate::arch::mm::mm_ops::PageTableWalker::walk(
                                root,
                                addr as u64,
                            )
                        }
                        .map(|(ppn, _)| {
                            let pa = (ppn as usize) << 12;
                            let kv = crate::arch::mm::phys_to_virt(
                                crate::arch::mm::PhysAddr::new(pa as u64),
                            )
                            .bits() as usize
                                + (addr & 0xfff);
                            // SAFETY: one volatile u32 in the
                            // holder's mapped user page.
                            unsafe { core::ptr::read_volatile(kv as *const u32) }
                        })
                    }) {
                        *slot = x;
                        got += 1;
                    }
                }
                let _ = writeln!(
                    out,
                    "   futex word = {:#x} (owner tid {}) next[{}]={:08x?}",
                    v, v, got, nb
                );
                // Chase a plausible owner tid (glibc stores the
                // owner TID in the mutex word for
                // error-checking/recursive mutexes).
                if v > 0 && v < 0x00ff_ffff {
                    let owner = unsafe { crate::sched::find_task_by_pid(v) };
                    if !owner.is_null() {
                        let ot = unsafe { &*owner };
                        // Stack-freed guard (KILLDUMP census race class):
                        // the owner may have exited and freed its stack
                        // before this lookup — skip, never fault.
                        if let Some(otop) = ot.kernel_stack_top_checked() {
                            #[cfg(feature = "x86_64")]
                            let (osys, orip) = {
                                let regs = (otop.wrapping_sub(
                                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                                )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                                // SAFETY: diagnostic read of the
                                // owner's entry frame.
                                unsafe { ((*regs).orig_rax, (*regs).rip) }
                            };
                            #[cfg(feature = "riscv64")]
                            let (osys, orip) = {
                                let regs = (otop.wrapping_sub(
                                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                                )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                                // SAFETY: same contract, riscv64.
                                unsafe { ((*regs).a7, (*regs).epc) }
                            };
                            #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                            let (osys, orip) = (0u64, 0u64);
                            let _ = writeln!(
                                out,
                                "   owner tid {} state={:#x} sys={:#x} rip={:#x}",
                                v,
                                ot.state().bits(),
                                osys,
                                orip
                            );
                        } else {
                            let _ = writeln!(
                                out,
                                "   owner tid {} stack-freed (dump skipped)",
                                v
                            );
                        }
                    } else {
                        let _ = writeln!(
                            out,
                            "   owner tid {} NOT FOUND (exited while holding?)",
                            v
                        );
                    }
                }
            }
            None => {
                let _ = writeln!(out, "   futex word unreadable (page absent)");
            }
        }
    }
}

/// Binder stall scan, called from khungtaskd (process context) alongside
/// the D-state scan. A thread parked in the blocking binder read for
/// longer than the threshold is almost always an IPC chain stall —
/// typically a service that never answered a synchronous transaction.
/// S-state tasks are invisible to the D-state detector, so report these
/// here: pid/tid, wait duration, and — when the thread is waiting for a
/// REPLY — the pid of the process that owes it (txn_stack top's owner).
/// One report per wait episode. The first owing process gets a full
/// per-thread dump (proc todo depth, looper/wait/stack state of every
/// worker) so the next wedge names the exact thread that ate the wake.
/// dfx=sleepsnap (b11 appspawn wedge hunt): one line per S-state task —
/// comm/tid/state, the syscall number the thread is parked in, its rip
/// and first two args, read from the fixed user entry frame at
/// kernel_stack_top - PT_REGS_SIZE (same contract as the holder-task
/// forensics in stall_thread_line). The appspawn server swallows client
/// requests (write OK, reply never comes, no fork) while its kickdog
/// thread lives: this names the exact syscall the server main thread is
/// sleeping in, at khungtaskd cadence. Purely diagnostic; OFF unless
/// dfx=sleepsnap is on the cmdline.
pub fn sleep_snapshot() {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SleepSnap) {
        return;
    }
    // sc207 (launcher leg 13): NEVER print inside the for_each_task
    // callback - it runs under the pid-hash bucket lock, and one printk
    // per S-state task held each bucket across console port-IO for ~a
    // second in total. A timer tick landing on that CPU mid-walk runs
    // stallwatch_tick's own for_each_task at irq-exit softirq time,
    // which then spins on the very bucket the interrupted walker holds
    // (self-deadlock: the releaser is the interrupted task); the other
    // CPU's scan spins on the same bucket and the machine hard-freezes
    // with the serial cut mid-line (sc207: both vCPUs 100%, RIP in
    // pid_hash::lock_bucket from stallwatch_tick). Collect lines here,
    // print ONCE after the walk with every bucket released.
    let mut out = alloc::string::String::new();
    use core::fmt::Write as _;
    crate::sched::for_each_task(|task| {
        // SAFETY: callback runs under the pid-hash bucket lock (same
        // contract as hung_task's check_tasks); reads are confined to
        // the task's own fields and its mapped kernel stack.
        unsafe {
            let tk = &*task;
            if tk.pid() == 0 {
                return;
            }
            let state = tk.state();
            if !state.contains(crate::process::task::TaskState::INTERRUPTIBLE) {
                return;
            }
            // Stack-freed guard (KILLDUMP census race class): the walk
            // can visit a task whose exit already freed its stack.
            let Some(top) = tk.kernel_stack_top_checked() else {
                return;
            };
            #[cfg(feature = "x86_64")]
            let (rip, sys, a0, a1) = if top == 0 {
                (0, 0, 0, 0)
            } else {
                let regs = (top.wrapping_sub(
                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's own entry frame;
                // owns_kernel_stack checked above (sc261).
                unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi) }
            };
            #[cfg(feature = "riscv64")]
            let (rip, sys, a0, a1) = if top == 0 {
                (0, 0, 0, 0)
            } else {
                let regs = (top.wrapping_sub(
                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                // SAFETY: same contract as the x86_64 twin above.
                unsafe { ((*regs).epc, (*regs).a7, (*regs).a0, (*regs).a1) }
            };
            #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
            let (rip, sys, a0, a1) = (0u64, 0u64, 0u64, 0u64);
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let _ = writeln!(
                out,
                "sleepsnap: {} tid {} state={:#x} sys={} rip={:#x} a0={:#x} a1={:#x}",
                name,
                tk.pid(),
                state.bits(),
                sys,
                rip,
                a0,
                a1
            );        }
    });
    // Emit with every pid-hash bucket released (see the sc207 note
    // above). One printk for the whole snapshot: ring records have a
    // length cap, so chunk on line boundaries at a size well under it.
    if !out.is_empty() {
        let mut rest = out.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(1024);
            if cut < rest.len() {
                // Extend to the next newline so lines stay whole.
                match rest[cut..].find('\n') {
                    Some(nl) => cut += nl,
                    None => cut = rest.len(),
                }
            }
            let (chunk, tail) = rest.split_at(cut);
            crate::pr_info!("{}", chunk);
            rest = tail;
        }
    }
}

/// dfx=txnhunt (leg-27): in-flight sync-txn hang audit, called at the
/// ~1 Hz timer heartbeat. The 15 s samgr code-7 family (SLOW-TXN census,
/// queue=0 exec=15020ms) only becomes visible at REPLY time — 15 s too
/// late to see what the server was doing — and the 60 s khungtaskd
/// cadence of sleep_snapshot/stall_report usually misses the window.
/// This audit walks every thread's txn stack under WORLD, collects
/// (only) server-side entries (to_tid == owning thread) picked up but
/// unreplied for >= 1 s, then — with the guard released (sc89/sc207
/// lessons: no task lookups, stack reads or prints under WORLD) —
/// snapshots each hung server thread's task state and entry-frame
/// syscall/rip (same read contract as sleep_snapshot) and prints one
/// TXN-HANG line per txn. It also collects every other in-flight txn
/// (queued on any todo, or stacked sender-side elsewhere) issued from
/// the hung servers' PROCESSES, printed as TXN-HANG-OUT lines — the
/// synchronous fan-out chain the handler is stuck behind, named
/// end-to-end. Rate-capped to one burst per 5 s; bounded (8 + 8 lines);
/// zero cost when dfx=txnhunt is off.
pub fn txn_hang_audit() {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TxnHang) {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u32;
    // One burst per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Relaxed, Relaxed)
        .is_err()
    {
        return;
    }

    struct Hung {
        id: u64,
        code: u32,
        spid: u32,
        stid: u32,
        cpid: u32,
        ctid: u32,
        exec_ms: u32,
        total_ms: u32,
        // leg-36B discriminator: the hung SERVER thread's own todo
        // (length + head item) and its binder read/looper posture. A
        // thread parked in BINDER_WRITE_READ with work on ITS OWN todo
        // = lost wake (kernel-side); empty own todo + no outstanding
        // nested txn anywhere = the handler deferred the reply
        // userspace-side (image-side, e.g. an FFRT worker that never
        // completes).
        thtodo: usize,
        thtodo_head: alloc::string::String,
        looper: u32,
        wfpw: bool,
        outstanding: alloc::string::String,
        // leg-40B family A: true when the outstanding scan found nested
        // txns FROM this hung server thread — the mutual-ABBA candidate
        // side (sc334B tid472<->841 shape). Drives the parkdump hook.
        nested: bool,
    }
    struct Out {
        id: u64,
        code: u32,
        fpid: u32,
        tpid: u32,
        ttid: u32,
        queued: bool,
        ms: u32,
    }
    let mut hung: alloc::vec::Vec<Hung> = alloc::vec::Vec::new();
    let mut outs: alloc::vec::Vec<Out> = alloc::vec::Vec::new();
    let mut hung_pids: [u32; 4] = [0; 4];
    let mut hung_n = 0usize;
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts only reach
        // each live proc's thread/todo lists; snapshot POD only — sc89).
        unsafe {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            fn is_hung_proc(pids: &[u32], pid: u32) -> bool {
                pids.iter().any(|&p| p == pid)
            }
            // Pass 1: server-side txn-stack entries executing >= 1 s.
            for p in &world.procs {
                let proc = pm(world, p);
                for t in &proc.im().threads {
                    for x in &t.st().txn_stack {
                        // SAFETY: pick_j/to_tid read under WORLD.
                        let pick = unsafe { *x.pick_j.get() };
                        let mine = unsafe { x.to_tid() } == Some(t.tid);
                        if !mine || pick == 0 {
                            continue;
                        }
                        let exec = now.wrapping_sub(pick);
                        if exec >= 100 && hung.len() < 8 {
                            // leg-36B: hung-server forensics, all under
                            // WORLD (outstanding_from_tid contract).
                            use core::fmt::Write;
                            let th = t.st();
                            let thtodo = th.todo.len();
                            let mut thtodo_head = alloc::string::String::new();
                            if let Some(wk) = th.todo.front() {
                                match wk {
                                    Work::Transaction(xt) => {
                                        let _ = write!(
                                            thtodo_head,
                                            "txn{} code{}",
                                            xt.id, xt.code
                                        );
                                    }
                                    Work::TransactionComplete => {
                                        thtodo_head.push_str("txn-complete");
                                    }
                                    Work::ReturnError(..) => {
                                        thtodo_head.push_str("return-error");
                                    }
                                    Work::Node(n) => {
                                        let _ = write!(thtodo_head, "node{}", n.id);
                                    }
                                    Work::DeadBinder { .. } => {
                                        thtodo_head.push_str("dead-binder");
                                    }
                                    Work::ClearDeathDone { .. } => {
                                        thtodo_head.push_str("clear-death-done");
                                    }
                                }
                            }
                            let outstanding =
                                outstanding_from_tid(world, proc.pid, t.tid);
                            let nested = !outstanding.is_empty();
                            hung.push(Hung {
                                id: x.id,
                                code: x.code,
                                spid: proc.pid,
                                stid: t.tid,
                                cpid: x.from_proc.as_ref().map(|f| f.pid).unwrap_or(0),
                                ctid: x.from_tid.unwrap_or(0),
                                exec_ms: exec.saturating_mul(10),
                                total_ms: now.wrapping_sub(x.enq_j).saturating_mul(10),
                                thtodo,
                                thtodo_head,
                                looper: th.looper,
                                wfpw: th.waiting_for_proc_work,
                                outstanding,
                                nested,
                            });
                            if hung_n < 4 {
                                hung_pids[hung_n] = proc.pid;
                                hung_n += 1;
                            }
                        }
                    }
                }
            }
            if hung.is_empty() {
                return;
            }
            // Pass 2 (same guard): every in-flight txn FROM a hung
            // server's process — stacked sender-side anywhere (its
            // threads included) or still queued on any todo. That is
            // the handler's synchronous fan-out chain.
            for p in &world.procs {
                let proc = pm(world, p);
                let mut scan = |x: &BinderTxn, tpid: u32, ttid: u32| {
                    let fpid = x.from_proc.as_ref().map(|f| f.pid).unwrap_or(0);
                    if fpid == 0 || !is_hung_proc(&hung_pids[..hung_n], fpid) || outs.len() >= 8 {
                        return;
                    }
                    // SAFETY: pick_j read under WORLD.
                    let pick = unsafe { *x.pick_j.get() };
                    let queued = pick == 0;
                    let ms = if queued {
                        now.wrapping_sub(x.enq_j)
                    } else {
                        now.wrapping_sub(pick)
                    };
                    outs.push(Out {
                        id: x.id,
                        code: x.code,
                        fpid,
                        tpid,
                        ttid,
                        queued,
                        ms: ms.saturating_mul(10),
                    });
                };
                for wk in &proc.im().todo {
                    if let Work::Transaction(x) = wk {
                        // SAFETY: to_tid read under WORLD.
                        let ttid = unsafe { x.to_tid() }.unwrap_or(0);
                        scan(x, proc.pid, ttid);
                    }
                }
                for t in &proc.im().threads {
                    for wk in &t.st().todo {
                        if let Work::Transaction(x) = wk {
                            // SAFETY: to_tid read under WORLD.
                            let ttid = unsafe { x.to_tid() }.unwrap_or(0);
                            scan(x, proc.pid, ttid);
                        }
                    }
                    for x in &t.st().txn_stack {
                        // Skip the hung server-side entries themselves
                        // (pass 1 already reported them).
                        // SAFETY: to_tid read under WORLD.
                        if unsafe { x.to_tid() } == Some(t.tid) {
                            continue;
                        }
                        // SAFETY: to_tid read under WORLD.
                        let ttid = unsafe { x.to_tid() }.unwrap_or(0);
                        scan(x, x.to_proc.pid, ttid);
                    }
                }
            }
        }
    }
    // WORLD released — task forensics + prints happen here (sc207).
    let comm_of = |p: *mut crate::process::Task| -> &str {
        if p.is_null() {
            return "?";
        }
        // SAFETY: diagnostic comm read of a live task.
        let c = unsafe { (*p).comm() };
        core::str::from_utf8(&c[..])
            .unwrap_or("?")
            .trim_end_matches('\0')
    };
    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
    use core::fmt::Write as _;
    // leg-40B: parkdump bursts-per-audit cap (see the TXN-HANG-PARK hook).
    let mut burst_dumps = 0u32;
    for h in &hung {
        let srv = unsafe { crate::sched::find_task_by_pid(h.stid) };
        let (state, sys, rip, a0, a1) = if srv.is_null() {
            (0u64, 0u64, 0u64, 0u64, 0u64)
        } else {
            // SAFETY: diagnostic reads of the task's own fields and its
            // mapped kernel stack (sleep_snapshot contract, sc261).
            let tk = unsafe { &*srv };
            let state = tk.state().bits() as u64;
            match tk.kernel_stack_top_checked() {
                None => (state, 0, 0, 0, 0),
                Some(top) => {
                    #[cfg(feature = "x86_64")]
                    let (sys, rip, a0, a1) = {
                        let regs = (top.wrapping_sub(
                            crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                        )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                        // SAFETY: entry-frame read of this task's own stack.
                        unsafe { ((*regs).orig_rax, (*regs).rip, (*regs).rdi, (*regs).rsi) }
                    };
                    #[cfg(feature = "riscv64")]
                    let (sys, rip, a0, a1) = {
                        let regs = (top.wrapping_sub(
                            crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                        )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                        // SAFETY: entry-frame read of this task's own stack.
                        unsafe { ((*regs).a7, (*regs).epc, (*regs).a0, (*regs).a1) }
                    };
                    (state, sys as u64, rip as u64, a0 as u64, a1 as u64)
                }
            }
        };
        let _ = writeln!(
            w,
            "TXN-HANG txn{} code{} from pid {} tid {} ({}) to pid {} tid {} ({}): exec {} ms (total {} ms): server state={:#x} sys={} rip={:#x} a0={:#x} a1={:#x}",
            h.id,
            h.code,
            h.cpid,
            h.ctid,
            comm_of(unsafe { crate::sched::find_task_by_pid(h.ctid) }),
            h.spid,
            h.stid,
            comm_of(srv),
            h.exec_ms,
            h.total_ms,
            state,
            sys,
            rip,
            a0,
            a1
        );
        // leg-36B: kernel-vs-image discriminator for the hung server
        // thread (the sc331B txn14230 shape — picked a txn, parked in
        // BINDER_WRITE_READ, no nested txn visible). thtodo>0 while the
        // thread sits in read = work pending on its OWN todo = lost
        // wake (kernel bug — the read must return it); thtodo=0 and an
        // empty outstanding list = the reply was deferred userspace-
        // side (image-side handler).
        let _ = writeln!(
            w,
            "TXN-HANG-OWE txn{} tid{} thtodo={} head=[{}] looper={:#x} wait-proc-work={} outstanding=[{}]",
            h.id,
            h.stid,
            h.thtodo,
            if h.thtodo_head.is_empty() { "-" } else { &h.thtodo_head },
            h.looper,
            h.wfpw,
            if h.outstanding.is_empty() {
                "NONE - reply deferred userspace-side"
            } else {
                &h.outstanding
            }
        );
        // leg-37B family B: EXT4_BIG_LOCK state + the fsw free/bitmap
        // counters AT the hang instant. Cumulative since boot; the delta
        // between consecutive 5 s bursts is the episode's volume (the 60 s
        // FSW cadence and the 30 s census straddle 1-100 s ftruncate
        // episodes — sc335B proved both unusable for this family).
        // bl=<locked>.<owner>.<depth>.<waiters>.<queued>.<held_ms>:
        // owner/held_ms name the holder and its outermost acquire age —
        // a multi-second held_ms with the audit's own server-tid sys=
        // ftruncate entry frame is the smoking-gun alignment.
        {
            let (bl, owner, depth, waiters, queued, held_ms) =
                crate::fs::ext4::EXT4_BIG_LOCK.audit_snapshot();
            let (fr, fseg, fblk, bs, bsus) = crate::dfx::fsw::hang_snapshot();
            let _ = writeln!(
                w,
                "TXN-HANG-FSW txn{} bl={}.{}.{}.{}.{}.{} fr={}.{}.{} bs={}.{}",
                h.id, bl, owner, depth, waiters, queued, held_ms, fr, fseg, fblk, bs, bsus
            );
        }
        // Leg-28 item 2 + leg-29A: name the FILE when the hung server
        // thread is stuck in a file syscall family — the pathstash
        // entry was written by this tid's own entry path (open family:
        // sys_openat; regular-file writes: sys_write; fsync family:
        // sys_fsync — sc310's hiview handler sat in fdatasync 43 min
        // with only the raw fd visible).
        // Family must MATCH the stash op: a writev to a pipe/socket
        // never re-stashes (pipe fast path), so the slot would still
        // hold an older open's path — printing it against a writev
        // entry frame mislabels the file (seen on sc314A wave-1).
        #[cfg(feature = "x86_64")]
        let fam = if matches!(sys, 2 | 257 | 437) {
            Some(crate::dfx::pathstash::OP_OPEN)
        } else if matches!(sys, 1 | 20) {
            Some(crate::dfx::pathstash::OP_WRITE)
        } else if matches!(sys, 74 | 75) {
            Some(crate::dfx::pathstash::OP_SYNC)
        } else if matches!(sys, 77 | 76) {
            // x86_64 ABI: 77 ftruncate, 76 truncate (leg-36B).
            Some(crate::dfx::pathstash::OP_TRUNC)
        } else {
            None
        };
        #[cfg(feature = "riscv64")]
        let fam = if matches!(sys, 56 | 24) {
            Some(crate::dfx::pathstash::OP_OPEN)
        } else if matches!(sys, 64 | 66) {
            Some(crate::dfx::pathstash::OP_WRITE)
        } else if matches!(sys, 82 | 83) {
            Some(crate::dfx::pathstash::OP_SYNC)
        } else if matches!(sys, 46 | 45) {
            // asm-generic: 46 ftruncate, 45 truncate (leg-36B).
            Some(crate::dfx::pathstash::OP_TRUNC)
        } else {
            None
        };
        if let Some(want) = fam {
            if let Some((op, p)) = crate::dfx::pathstash::lookup_file(h.stid) {
                if op == want {
                    let opn = match op {
                        crate::dfx::pathstash::OP_OPEN => "open",
                        crate::dfx::pathstash::OP_WRITE => "write",
                        crate::dfx::pathstash::OP_SYNC => "sync",
                        crate::dfx::pathstash::OP_TRUNC => "truncate",
                        _ => "?",
                    };
                    let _ = writeln!(
                        w,
                        "TXN-HANG txn{} file-op={} path='{}'",
                        h.id, opn, p
                    );
                }
            }
        }
        // Leg-29A stuck-site probe: kernel-text words off the hung
        // thread's kernel stack, so the serial log names WHERE in the
        // kernel the handler is parked (sc310's RUNNING 43-min
        // fdatasync vs sc303's D-state open need the stuck function,
        // not just the syscall). Two windows:
        //   sp-up   from thread().sp upward — the exact parked chain
        //           when the thread SLEEPS (context-switch-fresh sp;
        //           same contract as khungtaskd's HUNG-STK).
        //   top-dn  from just under the entry pt_regs downward — the
        //           newest frames when the thread is RUNNING on
        //           another CPU (thread().sp is then stale; the live
        //           frames sit between the entry frame and the current
        //           sp, so scanning from the top catches them plus
        //           dead-frame noise).
        // Cross-CPU reads are diagnostic-only (sc261 sleep_snapshot
        // contract); symbolize offline with addr2line.
        if !srv.is_null() {
            // SAFETY: diagnostic reads of the task's fields and its
            // pinned kernel-stack pages (the Task exists).
            let tk = unsafe { &*srv };
            if let Some(top) = tk.kernel_stack_top_checked() {
                #[cfg(feature = "x86_64")]
                extern "C" {
                    static __stext: u8;
                    static __etext: u8;
                }
                #[cfg(not(feature = "x86_64"))]
                extern "C" {
                    static _stext: u8;
                    static _etext: u8;
                }
                #[cfg(feature = "x86_64")]
                let (lo, hi) = (
                    core::ptr::addr_of!(__stext) as u64,
                    core::ptr::addr_of!(__etext) as u64,
                );
                #[cfg(not(feature = "x86_64"))]
                let (lo, hi) = (
                    core::ptr::addr_of!(_stext) as u64,
                    core::ptr::addr_of!(_etext) as u64,
                );
                let is_text = |v: u64| v >= lo && v < hi && (v & 1) == 0;
                let base = top - crate::config::KERNEL_STACK_SIZE;
                let thsp = tk.thread().sp;
                let mut line = alloc::string::String::new();
                // leg-33B: continuation record for the deepened sp-up
                // window (words 9-24) — printk records cap at 256 bytes.
                let mut extra = alloc::string::String::new();
                // Window (a): parked-chain words upward from sp.
                if thsp as usize >= base
                    && thsp as usize + 8 <= top
                    && (thsp & 7) == 0
                {
                let p = thsp as *const u64;
                let mut shown = 0;
                let _ = write!(line, " sp-up:");
                // 640-word window, leg-33B deepened: the sc329B 8-word
                // capture stopped inside the scheduler plumbing above
                // the saved sp (context_switch / irq_restore) and never
                // reached the wait-site frames of deep fs waits
                // (journal commit, bio completion). Capture up to 24
                // kernel-text words and split across a second record —
                // printk records cap at 256 bytes.
                for k in 0..640usize {
                    // SAFETY: diagnostic read inside the task's own
                    // kernel-stack window; bounds checked above.
                    let v = unsafe { core::ptr::read_volatile(p.add(k)) };
                    if is_text(v) {
                        if shown < 8 {
                            let _ = write!(line, " {v:#x}");
                        } else {
                            let _ = write!(extra, " {v:#x}");
                        }
                        shown += 1;
                        if shown >= 24 {
                            break;
                        }
                    }
                }
                if shown == 0 {
                    line.truncate(0);
                    extra.truncate(0);
                }
                }
                // Window (b): newest frames downward from the entry frame.
                #[cfg(feature = "x86_64")]
                let entry_sz = crate::arch::x86_64::pt_regs::PT_REGS_SIZE;
                #[cfg(feature = "riscv64")]
                let entry_sz = crate::arch::riscv64::pt_regs::PT_REGS_SIZE;
                let start = (top - entry_sz).saturating_sub(8) & !7usize;
                let mut bline = alloc::string::String::new();
                let _ = write!(bline, " top-dn:");
                let mut shown = 0;
                let mut k = 0usize;
                while shown < 10 && k < 0x600 / 8 {
                    let addr = start - k * 8;
                    if addr < base + 8 {
                        break;
                    }
                    // SAFETY: diagnostic read inside the task's own
                    // kernel-stack window; bounds checked above.
                    let v = unsafe { core::ptr::read_volatile(addr as *const u64) };
                    if is_text(v) {
                        let _ = write!(bline, " {v:#x}");
                        shown += 1;
                    }
                    k += 1;
                }
                if shown > 0 {
                    let _ = writeln!(
                        w,
                        "TXN-HANG-STK txn{} tid{} state={:#x} thsp={:#x}{}{}",
                        h.id,
                        h.stid,
                        state,
                        thsp,
                        line,
                        bline
                    );
                    if !extra.is_empty() {
                        let _ = writeln!(w, "TXN-HANG-STK2 txn{}{}", h.id, extra);
                    }
                }
            }
        }
    }
    // leg-40B family A: user-stack park dump of hung server threads
    // (dfx=thpark — deliberately its own switch, NOT ParkDump, which
    // also arms the freeze-scan full-tgid dumps that dodge boot
    // timing, sc186-188). Names the image-side handler call chain
    // above the parked syscall — the input the oh-img-p mitigation
    // needs for the mutual-ABBA pair. Candidates are ordered
    // nested-outstanding first (the ABBA side), then the
    // reply-deferred class. Once per tid per boot (wedge stacks are
    // frozen), max 2 per burst and 16 per boot. sc338B lesson: a
    // latch slot is claimed ONLY when the dump actually printed —
    // dead/exited tids return false and must not silently consume
    // the budget; a tid that fails to print once is recorded and
    // skipped for the rest of the boot so live wedges are not
    // starved by retry churn.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ThPark) {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO_TID: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static DUMPED_TIDS: [core::sync::atomic::AtomicU32; 64] = [ZERO_TID; 64];
        static FAIL_TIDS: [core::sync::atomic::AtomicU32; 64] = [ZERO_TID; 64];
        static DUMP_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        use core::sync::atomic::Ordering::Relaxed as Rel;
        let mut cands: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        for h in hung.iter().filter(|h| h.nested) {
            if !cands.contains(&h.stid) {
                cands.push(h.stid);
            }
        }
        for h in hung.iter().filter(|h| !h.nested) {
            if !cands.contains(&h.stid) {
                cands.push(h.stid);
            }
        }
        for tid in cands {
            if burst_dumps >= 2 || DUMP_N.load(Rel) >= 16 {
                break;
            }
            if DUMPED_TIDS.iter().any(|d| d.load(Rel) == tid) {
                continue;
            }
            if FAIL_TIDS.iter().any(|d| d.load(Rel) == tid) {
                continue; // two silent attempts already burned
            }
            // SAFETY: read-only diagnostic walk of the parked
            // thread's user pages, outside WORLD
            // (user_park_stack_dump_stid contract).
            let printed = unsafe {
                user_park_stack_dump_stid(tid, "TXN-HANG-PARK:", 48)
            };
            if printed {
                for d in DUMPED_TIDS.iter() {
                    if d.compare_exchange(0, tid, Rel, Rel).is_ok() {
                        break;
                    }
                }
                DUMP_N.fetch_add(1, Rel);
                burst_dumps += 1;
            } else {
                // Silent (task gone / stack freed): record the tid so
                // it is not retried every burst; it cannot become
                // printable later within a boot's audit window.
                for d in FAIL_TIDS.iter() {
                    if d.load(Rel) == tid {
                        break;
                    }
                    if d.compare_exchange(0, tid, Rel, Rel).is_ok() {
                        break;
                    }
                }
            }
        }
    }
    for o in &outs {
        let _ = writeln!(
            w,
            "TXN-HANG-OUT txn{} code{} from pid {} -> pid {} tid {}: {} {} ms",
            o.id,
            o.code,
            o.fpid,
            o.tpid,
            o.ttid,
            if o.queued { "queue" } else { "exec" },
            o.ms
        );
    }
}

/// dfx=ams46 (leg-30B): ~5 s census of the latched AMS node + its
/// foundation process. Prints ONLY while there is pending work on the
/// node/proc (async slot held, txns parked, or proc todo non-empty), so
/// a healthy quiesced draw costs zero lines. The census is the
/// adjudication backdrop for the traced code-46 txn: parked>0 with a
/// held slot names slot starvation (the holder's buffer never freed);
/// todo>0 with waiting==0 names a reader-less foundation (no eligible
/// looper thread to wake); everything zero with no READ line after ENQ
/// means the txn already vanished (retire — see the RETIRED prints).
pub fn ams46_audit() {
    if !ams46_on() {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u32;
    // One census per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let latched = ams46_node_id();
    if latched == 0 {
        return;
    }
    let tpid = AMS46_TPID.load(Relaxed);
    let mut todo_d = 0usize;
    let mut waiting = 0usize;
    let mut threads_n = 0usize;
    let mut parked = 0usize;
    let mut async_held = false;
    let mut found = false;
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts only reach
        // the foundation proc's thread/node/todo lists; POD snapshot
        // only — sc89).
        unsafe {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            if let Some(p) = world.procs.iter().find(|p| p.pid == tpid) {
                let pim = pm(world, p).im();
                todo_d = pim.todo.len();
                threads_n = pim.threads.len();
                waiting = pim
                    .threads
                    .iter()
                    .filter(|t| {
                        let th = t.st();
                        th.waiting_for_proc_work && th.todo.is_empty() && th.txn_stack.is_empty()
                    })
                    .count();
                if let Some(n) = pim.nodes.iter().find(|n| n.id == latched) {
                    let ns = n.st();
                    async_held = ns.has_async_transaction;
                    parked = ns.async_todo.len();
                }
                found = true;
            }
        }
    }
    if !found {
        return;
    }
    if async_held || parked > 0 || todo_d > 0 {
        static CEN_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if CEN_N.fetch_add(1, Relaxed) < 240 {
            crate::pr_warn!(
                "AMS46 census j={} pid={} todo={} waiting={} thr={} node{} async={} parked={}",
                now / 100,
                tpid,
                todo_d,
                waiting,
                threads_n,
                latched,
                async_held,
                parked
            );
        }
    }
}

/// POOL CENSUS (leg-33B, dfx=poolcensus): every ~5 s, one line per
/// binder proc that has queued todo work — the reader-exhaustion
/// adjudication backdrop (sc329B: foundation proc todo grew while every
/// OS_IPC reader was parked as a client and the pool never grew).
/// Line shape (compact, one printk record):
///   POOL pid={} todo={} max={} req={} st={} thr={} w={} wwait={} wstk={} clients={}"
/// plus, for the proc with the deepest todo, one detail line naming each
/// worker thread's state (waiting / stacked / loitering without flags).
pub fn pool_census_audit() {
    if !pool_census_on() {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u32;
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }

    // Snapshot under WORLD: (pid, todo, max, req, thr, workers, waiting,
    // stacked, clients, worst-thread detail for the deepest-todo proc).
    struct Snap {
        pid: u32,
        todo: usize,
        max_t: i32,
        req: i32,
        started: i32,
        thr: usize,
        workers: usize,
        wwait: usize,
        wstk: usize,
        clients: usize,
    }
    let mut snaps: alloc::vec::Vec<Snap> = alloc::vec::Vec::new();
    // Deepest-todo proc's per-thread states: (tid, waiting, stacked).
    let mut detail: alloc::vec::Vec<(u32, bool, bool)> = alloc::vec::Vec::new();
    let mut detail_pid = 0u32;
    // Deepest-todo proc's first queued items (the strand victims):
    // (txn id, code, oneway, reg_tgid, sender pid, enqueue age jiffies).
    let mut todo_items: alloc::vec::Vec<(u64, u32, bool, u32, u32, u32)> =
        alloc::vec::Vec::new();
    {
        let w = world_lock();
        // SAFETY: read-only POD snapshot under WORLD (sc89 discipline —
        // no prints or task walks under the lock).
        unsafe {
            let Some(world) = w.as_ref() else { return };
            let mut deepest_pid = 0u32;
            let mut deepest_todo = 0usize;
            for p in &world.procs {
                let pim = p.im();
                let todo = pim.todo.len();
                if todo == 0 {
                    continue;
                }
                if todo > deepest_todo {
                    deepest_todo = todo;
                    deepest_pid = p.pid;
                }
                let mut workers = 0usize;
                let mut wwait = 0usize;
                let mut wstk = 0usize;
                let mut clients = 0usize;
                for t in pim.threads.iter() {
                    let st = t.st();
                    let is_worker = (st.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
                    if is_worker {
                        workers += 1;
                        if st.waiting_for_proc_work {
                            wwait += 1;
                        }
                        if !st.txn_stack.is_empty() {
                            wstk += 1;
                        }
                    } else if !st.txn_stack.is_empty() || !st.todo.is_empty() {
                        clients += 1;
                    }
                }
                if snaps.len() < 12 {
                    snaps.push(Snap {
                        pid: p.pid,
                        todo,
                        max_t: pim.max_threads,
                        req: pim.requested_threads,
                        started: pim.requested_threads_started,
                        thr: pim.threads.len(),
                        workers,
                        wwait,
                        wstk,
                        clients,
                    });
                }
            }
            if deepest_pid != 0 {
                if let Some(p) = world.procs.iter().find(|p| p.pid == deepest_pid) {
                    let pim = p.im();
                    for t in pim.threads.iter().take(16) {
                        let st = t.st();
                        detail.push((t.tid, st.waiting_for_proc_work, !st.txn_stack.is_empty()));
                    }
                    // First queued items, with the reg_tgid read-gate
                    // verdict: a waiting daemon worker takes this item
                    // only when daemon_readable (reg 0 / daemon-owned).
                    // Non-Transaction items are named by kind — a
                    // stranded Node/DeadBinder/ReturnError work item is
                    // a different protocol stall than a parked txn.
                    for w in pim.todo.iter().take(3) {
                        if let Work::Transaction(t) = w {
                            let reg = t.target_node.as_ref().map_or(0, |n| n.reg_tgid);
                            todo_items.push((
                                t.id,
                                t.code,
                                t.flags & TF_ONE_WAY != 0,
                                reg,
                                t.from_proc.as_ref().map_or(0, |fp| fp.pid),
                                now.wrapping_sub(t.enq_j),
                            ));
                        } else {
                            let kind = match w {
                                Work::TransactionComplete => 0x7C01,
                                Work::ReturnError(..) => 0x7C02,
                                Work::Node(..) => 0x7C03,
                                Work::DeadBinder { .. } => 0x7C04,
                                Work::ClearDeathDone { .. } => 0x7C05,
                                Work::Transaction(..) => unreachable!(),
                            };
                            todo_items.push((kind, 0, false, 0, 0, 0));
                        }
                    }
                    detail_pid = deepest_pid;
                }
            }
        }
    }
    snaps.sort_by(|a, b| b.todo.cmp(&a.todo));
    for s in snaps.iter().take(6) {
        crate::pr_warn!(
            "POOL pid={} todo={} max={} req={} st={} thr={} w={} wwait={} wstk={} clients={}",
            s.pid,
            s.todo,
            s.max_t,
            s.req,
            s.started,
            s.thr,
            s.workers,
            s.wwait,
            s.wstk,
            s.clients
        );
    }
    if !detail.is_empty() {
        let mut line = alloc::string::String::new();
        use core::fmt::Write as _;
        for (tid, waiting, stacked) in detail.iter() {
            let _ = write!(
                line,
                " {}{}",
                tid,
                if *waiting {
                    "W"
                } else if *stacked {
                    "S"
                } else {
                    "L"
                }
            );
        }
        crate::pr_warn!("POOL-THR pid={}{}", detail_pid, line);
    }
    for (id, code, oneway, reg, from, age) in todo_items.iter() {
        // daemon_readable: would THIS proc's own waiting worker take the
        // item (work_readable_by with the daemon tgid)? False = gated to
        // a child (reg_tgid) — only the child's threads may consume it.
        let daemon_readable = *reg == 0 || *reg == detail_pid;
        let kind = match *id {
            0x7C01 => "txn-complete",
            0x7C02 => "return-error",
            0x7C03 => "node-refs",
            0x7C04 => "dead-binder",
            0x7C05 => "clear-death-done",
            _ => "",
        };
        if kind.is_empty() {
            crate::pr_warn!(
                "POOL-TODO pid={} txn{} code{} {} reg={} from={} age={}j{}",
                detail_pid,
                id,
                code,
                if *oneway { "oneway" } else { "sync" },
                reg,
                from,
                age,
                if daemon_readable { "" } else { " CHILD-GATED" }
            );
        } else {
            crate::pr_warn!("POOL-TODO pid={} item={}", detail_pid, kind);
        }
    }
}


/// World-wide search for queued/stacked transactions SENT by (pid, tid) —
/// the sc74 discriminator: a SERVER-OWING thread (parked owing a reply,
/// top txn to_tid == self) either has a nested sync txn outstanding
/// somewhere (legitimate wait) or has nothing (kernel lost it / userspace
/// never sent it / state corruption). Returns a short human-readable
/// location list, empty when nothing is outstanding.
/// SAFETY: caller holds WORLD.
unsafe fn outstanding_from_tid(world: &World, pid: u32, tid: u32) -> alloc::string::String {
    let mut found = alloc::string::String::new();
    for p in &world.procs {
        let pim = p.im();
        for (i, wk) in pim.todo.iter().enumerate() {
            if let Work::Transaction(t) = wk {
                if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    let _ = write!(
                        found,
                        " [p{}todo{} txn{} code{}]",
                        p.pid, i, t.id, t.code
                    );
                }
            }
        }
        for ct in &pim.threads {
            let cth = ct.st();
            for (i, wk) in cth.todo.iter().enumerate() {
                if let Work::Transaction(t) = wk {
                    if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                        use core::fmt::Write;
                        let _ = write!(
                            found,
                            " [p{}t{}todo{} txn{} code{}]",
                            p.pid, ct.tid, i, t.id, t.code
                        );
                    }
                }
            }
            for (i, st) in cth.txn_stack.iter().enumerate() {
                if st.from_tid == Some(tid) && st.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    let _ = write!(
                        found,
                        " [p{}t{}stack{} txn{} code{}]",
                        p.pid, ct.tid, i, st.id, st.code
                    );
                }
            }
        }
        for node in &pim.nodes {
            for t in node.st().async_todo.iter() {
                if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    let _ = write!(found, " [p{}node{} async txn{}]", p.pid, node.id, t.id);
                }
            }
        }
    }
    found
}

/// One line of the transition ring (sc74 forensics). sc89: the ring is
/// SNAPSHOTTED under WORLD into a fixed array; formatting + printing
/// happen after the guard drops.
/// SAFETY: caller holds WORLD.
unsafe fn hist_snapshot(
    t: &Arc<BinderThread>,
) -> [(u8, u32, u32, u32); BHIST] {
    let th = t.st();
    let mut ring = [(0u8, 0u32, 0u32, 0u32); BHIST];
    for k in 0..BHIST {
        let idx = (th.hcur + k) % BHIST;
        let e = &th.hist[idx];
        ring[k] = (e.ev, e.a, e.b, e.j);
    }
    ring
}

/// Format a snapshot taken by hist_snapshot. No binder state touched.
fn hist_line_of(tid: u32, ring: &[(u8, u32, u32, u32); BHIST]) -> alloc::string::String {
    use core::fmt::Write;
    // ONE printk call with embedded newlines every 4 entries — separate
    // continuation prints were spliced by concurrent console traffic at
    // exactly the decisive ring tail (sc77: "REPLY_P|binder-stall:").
    let mut s = alloc::string::String::new();
    let _ = write!(s, "  hist tid {} ({} events):", tid, ring.iter().filter(|e| e.0 != 0).count());
    let mut n = 0;
    for &(ev, a, b, j) in ring.iter() {
        if ev == 0 {
            continue;
        }
        let _ = write!(s, " {}({},{})@{}", ev_name(ev), a, b, j);
        n += 1;
        if n % 4 == 0 {
            let _ = writeln!(s);
        }
    }
    s
}

/// sc77: for a SERVER-OWING holder parked in the binder read with NO
/// outstanding nested txn (the "impossible shape" — OH libipc cannot
/// legally park there), snapshot the USER stack at the park: the entry
/// frame's rsp/rip plus candidate return addresses, each resolved to its
/// covering VMA (fd + file offset) so the frames symbolize offline
/// against the image's libraries.
/// SAFETY: sc89 — reads only the holder TASK's own page tables (found
/// by t.tid, which is immutable; the Arc pins the BinderThread). Safe
/// OUTSIDE the WORLD guard and must run there: this is one of the slow
/// page-walk dumps that made the old WORLD-held stall_report a ~500 ms
/// IRQ-off window (sc82, khungtaskd death).
unsafe fn user_park_stack_dump(t: &Arc<BinderThread>) {
    // SAFETY: tid-only core, same contract (leg-40B).
    let _ = unsafe { user_park_stack_dump_stid(t.tid, "binder-stall:  ", 16) };
}

/// leg-40B (family A): tid-taking core of the sc77 park dump so the
/// txn-hang audit can dump a hung SERVER thread without holding a
/// BinderThread ref. `words` is the frame-window depth above the
/// parked syscall's entry sp: 16 keeps the sc77 stall-scan shape, 48
/// covers ~6 handler frames for the audit — the OH handler chain that
/// entered the nested txn is what the image-side ABBA mitigation
/// needs named. `tag` prefixes every line so audit dumps are greppable
/// apart from binder-stall dumps.
/// SAFETY: same contract as user_park_stack_dump — read-only walk of
/// the target task's own page tables; run OUTSIDE the WORLD guard.
/// Returns true iff the PARK-STACK header was actually printed (the
/// task was found with a live address space and kernel stack) — the
/// leg-40B audit latch claims a slot only on a real print, so dead /
/// freed tids cannot silently consume the dump budget (sc338B: the
/// 16-slot latch filled with silent early-returns and the live wedge
/// tids never got dumped).
unsafe fn user_park_stack_dump_stid(tid: u32, tag: &str, words: u64) -> bool {
    let task = crate::sched::find_task_by_pid(tid);
    if task.is_null() {
        return false;
    }
    let tk = &*task;
    let Some(aspace) = tk.address_space() else { return false };
    let root = aspace.root_ppn();
    let read_u64_at = |va: u64| -> Option<u64> {
        // SAFETY: read-only diagnostic walk of the holder's tables.
        let (ppn, _) = crate::arch::mm::mm_ops::PageTableWalker::walk(root, va & !0xfff)?;
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64))
            .bits() as usize
            + (va & 0xfff) as usize;
        if va & 0xfff > 0xff8 {
            return None; // crosses a page — skip rather than split
        }
        // SAFETY: one volatile u64 in the holder's mapped user page.
        Some(core::ptr::read_volatile(kv as *const u64))
    };
    // Stack-freed guard (KILLDUMP census race class): the holder may
    // have exited and freed its stack — skip, never fault.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return false;
    };
    #[cfg(feature = "x86_64")]
    let (rsp, rip) = if top == 0 {
        (0, 0)
    } else {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of the holder's entry frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = if top == 0 {
        (0, 0)
    } else {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract, riscv64.
        unsafe { ((*regs).sp, (*regs).epc) }
    };
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    let (rsp, rip) = (0u64, 0u64);
    crate::pr_warn!(
        "{} PARK-STACK tid {} rip={:#x} rsp={:#x}",
        tag,
        tid,
        rip,
        rsp
    );
    // Candidate frame words above rsp; each that lands in a mapped
    // exec-looking VMA prints with the file offset for offline symbolize.
    let vmas = aspace.vma_read();
    for i in 0..words {
        let Some(v) = read_u64_at(rsp + 8 * i) else { continue };
        // Skip obvious non-pointers (small ints, non-canonical).
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        if let Some(vm) = vmas.find(crate::mm::page::VirtAddr::new(v as usize)) {
            let off = vm.offset() + (v as usize - vm.start().as_usize());
            // Instruction bytes at the frame (Session F's byte-match method:
            // search these bytes at fileoff in the debugfs-extracted .so).
            let bytes = read_u64_at(v).unwrap_or(0);
            crate::pr_warn!(
                "{} ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} bytes={:#018x}",
                tag,
                i,
                v,
                vm.start().as_usize(),
                vm.end().as_usize(),
                vm.file_fd(),
                off,
                bytes
            );
        }
    }
    true
}

/// Wait-queue census + task state for a stalled reader — the live-wake
/// discriminator: an entry flagged woken while its task still sleeps is the
/// sticky-skip lost wake (W1); woken=0 means nobody ever issued the wake.
/// sc89: pure wait-queue + scheduler reads (no binder state) — safe
/// outside the WORLD guard; returns the formatted line.
fn thread_census_of(t: &Arc<BinderThread>) -> alloc::string::String {
    use core::fmt::Write;
    let (total, woken) = t.wait.vw_queue_census();
    let task = unsafe { crate::sched::find_task_by_pid(t.tid) };
    let state = if task.is_null() {
        0xFF
    } else {
        unsafe { (*task).state().bits() }
    };
    let verdict = if task.is_null() {
        "TASK-GONE"
    } else if woken > 0 && (state & 0x3) != 0 {
        // sleeping (INTERRUPTIBLE=1/UNINTERRUPTIBLE=2) with a woken entry
        "LOST-WAKE-SHAPE"
    } else if woken > 0 {
        "woken+runnable"
    } else {
        "never-woken"
    };
    let mut s = alloc::string::String::new();
    let _ = write!(
        s,
        "  census tid {} q={} woken={} task_state={:#x} {}",
        t.tid, total, woken, state, verdict
    );
    s
}

/// dfx=schb heartbeat counts (sc-series quiet-quiet soft-wedge hunt):
/// cheap per-beat binder summary — live proc count, threads parked in
/// the blocking binder read, and waiters older than 15s (the rekick
/// threshold; a healthy boot parks readers for 1-3s at most). Read-only
/// under WORLD; no formatting, no I/O — all output happens in schbd.
pub fn schb_beat() -> (usize, usize, usize) {
    let now = crate::drivers::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    let mut procs = 0usize;
    let mut waiters = 0usize;
    let mut waiters15 = 0usize;
    let w = world_lock();
    // SAFETY: read-only traversal under WORLD (pm() casts only reach
    // each live proc's thread list).
    unsafe {
        if let Some(world) = w.as_ref() {
            for p in &world.procs {
                procs += 1;
                let proc = pm(world, p);
                for t in &proc.im().threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    waiters += 1;
                    if hz != 0 && now.saturating_sub(since) >= 15 * hz {
                        waiters15 += 1;
                    }
                }
            }
        }
    }
    (procs, waiters, waiters15)
}

/// dfx=schb wedge census: EVERY read-wait binder thread regardless of
/// age. stall_report only prints waiters older than STALL_SECS (120s);
/// the census fires at pid-plateau+60s where the wedge's waiters are
/// younger. Formats under WORLD (no I/O); the caller emits the lines via
/// ConsoleWriter after the guard drops (the sc89 stall_report
/// discipline). No stall_reported latching, no rekick side effects.
pub fn schb_waiter_census(out: &mut alloc::string::String) {
    use core::fmt::Write as _;
    let now = crate::drivers::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return;
    }
    let w = world_lock();
    // SAFETY: read-only traversal under WORLD.
    unsafe {
        if let Some(world) = w.as_ref() {
            for p in &world.procs {
                let proc = pm(world, p);
                for t in &proc.im().threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    let age = now.saturating_sub(since) / hz;
                    let th = t.st();
                    let depth = th.txn_stack.len();
                    // Same discrimination as stall_report's stall lines:
                    // SERVER-OWING (parked in read owing a reply — the
                    // death-loop terminal shape) vs CLIENT-WAIT (waiting
                    // for its own reply).
                    let (shape, owed) = match th.txn_stack.last() {
                        Some(txn) if txn.to_tid() == Some(t.tid) => {
                            ("SERVER-OWING", txn.to_proc.pid)
                        }
                        Some(txn) => ("CLIENT-WAIT", txn.to_proc.pid),
                        None => ("NO-TXN", 0),
                    };
                    let _ = writeln!(
                        out,
                        "SC-WEDGE-BW pid={} tid={} age={}s depth={} shape={} owed_pid={} kicks={}",
                        proc.pid,
                        t.tid,
                        age,
                        depth,
                        shape,
                        owed,
                        t.rekick_count.load(core::sync::atomic::Ordering::Relaxed)
                    );
                }
            }
        }
    }
}

pub fn stall_report() {
    const STALL_SECS: u64 = 120;
    /// Re-kick threshold (heal): any binder reader parked longer than this
    /// gets an idempotent wake — if the wedge is a lost wake anywhere in
    /// the chain, the re-kick unwedges it; a legitimately-parked thread
    /// just re-checks its condition and re-parks. OH's own retry ladder
    /// runs 40 s timeouts, so a 15 s re-kick keeps chains alive without
    /// spamming healthy 1-3 s waits.
    const REKICK_SECS: u64 = 15;
    let now = crate::drivers::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return;
    }
    // sc81: rwlock-freeze watch (samgr abilityMapLock_ terminal shape) —
    // word-delta + futex-traffic + tid-census + full-tgid park-stack dump.
    crate::sync::futex::freeze_scan(now, hz);
    let mut dump_owners: Vec<Arc<BinderProc>> = Vec::new();
    // Threads to re-kick after WORLD is released (heal + forensics event).
    let mut rekick: Vec<Arc<BinderThread>> = Vec::new();
    // Procs with pending todo: re-run the pick-one wake (heals a txn that
    // sits on a proc todo because its wake was consumed in vain).
    let mut retodo: Vec<Arc<BinderProc>> = Vec::new();
    // sc89: EVERYTHING slow — console output, page-table walks (user park
    // stacks, holder forensics), futex-graph walks — moved OUT of the
    // WORLD guard. sc82 measured the old WORLD-held dump at ~500 ms with
    // irqs off (SpinlockIrqGuard<World> drop); khungtaskd died to the
    // watchdog/softlockup side effects and the freeze scanner stopped
    // scanning. Under the guard we now only FORMAT text (fast, no I/O)
    // and snapshot POD; the one pr_warn at the end flushes it all with
    // interrupts on.
    let mut out = alloc::string::String::new();
    // Impossible-shape threads: user-stack dumps AFTER the guard.
    let mut late_parkdumps: Vec<Arc<BinderThread>> = Vec::new();
    // Owner pids for the deferred futex-waiter graph walk.
    let mut late_futex_graph: Vec<u32> = Vec::new();
    // Per-owner thread list with pre-formatted lines + hist snapshots,
    // built under the guard, printed after.
    struct OwnerDump {
        pid: u32,
        name: alloc::string::String,
        todo_depth: usize,
        lines: Vec<(alloc::string::String, bool)>, // (stall line, stack non-empty)
        hists: Vec<Option<[(u8, u32, u32, u32); BHIST]>>,
        futex_line: alloc::string::String,
        threads: Vec<Arc<BinderThread>>, // for deferred holder forensics
    }
    let mut owner_dumps: Vec<OwnerDump> = Vec::new();
    let mut retire_wakes: Vec<Arc<BinderProc>> = Vec::new();
    // leg-32B: child threads that received a retired-slot handoff txn —
    // woken in the heal block below, outside WORLD.
    let mut retire_wake_ths: Vec<Arc<BinderThread>> = Vec::new();
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts are only used
        // to reach each live proc's thread list).
        unsafe {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            // Leg-19: the enq_j silence rule (leg-17's banked design) —
            // retire undelivered transactions whose receivers have been
            // silent past any plausible retry ladder. Frees the dominant
            // floor population (parked oneways, pick_j==0) and unstacks
            // the senders of parked sync calls.
            let (retired, mut rw, mut rth) = sweep_silent_txns(world);
            retire_wakes.append(&mut rw);
            retire_wake_ths.append(&mut rth);
            if retired > 0 {
                let _ = writeln!(
                    out,
                    "binder-stall: SILENT-TXN-RETIRE retired {} txn(s) this scan",
                    retired
                );
            }
            for p in &world.procs {
                let proc = pm(world, p);
                if !proc.im().todo.is_empty() {
                    retodo.push(p.clone());
                }
                for t in &proc.im().threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    let age = now.saturating_sub(since);
                    if age >= REKICK_SECS * hz {
                        if !t.rekicked.swap(true, core::sync::atomic::Ordering::AcqRel) {
                            t.hist(EV_REKICK, (age / hz) as u32, 0);
                        }
                        let kicks = t.rekick_count.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                        if kicks >= 1 {
                            // A previous kick failed to run this task within
                            // a full scan interval — wake→run leak (sc76:
                            // REKICK@6007 ignored for 230 s on a healthy
                            // idle worker; the WAKE-STRANDED family). Dump
                            // the task's scheduler view: linked? on_cpu?
                            // pick_state? — names which stage ate the wake.
                            let task = unsafe { crate::sched::find_task_by_pid(t.tid) };
                            if !task.is_null() {
                                let tk = unsafe { &*task };
                                let linked = match crate::sched::sched::grq_diag_cfs_linked(task) {
                                    Some(b) => {
                                        if b { "1" } else { "0" }
                                    }
                                    None => "n/a",
                                };
                                use core::fmt::Write;
                                // sc99: the leak persists through kicks —
                                // name WHICH cpu slot (if any) still owns the
                                // task (the wake_up_enqueue curr_on scan) and
                                // each CPU's idle bit: a LYING BUSY slot
                                // refuses every wake silently (R56c heals
                                // only idle-marked slots) and only the idle
                                // loop's harvester could steal it — a CPU
                                // that never idles strands the task forever.
                                let mut slots = alloc::string::String::from(" slots=");
                                for (pid, idle, cur) in
                                    crate::sched::sched::grq_diag_cpu_currents().iter()
                                {
                                    if *pid == u32::MAX {
                                        continue;
                                    }
                                    let _ = write!(
                                        slots,
                                        "[pid{}{}{}]",
                                        pid,
                                        if *idle { "/idle" } else { "" },
                                        if *cur == task { "/OWNER" } else { "" },
                                    );
                                }
                                // Leg-19 attribution: policy/on_rq/dl + the
                                // kick outcome counters — decider between
                                // "wake linked it, pick never ran it"
                                // (wok>=1, on_rq=1) and "wake silently
                                // refused" (wno-only — R82 DL defer or an
                                // empty queue: the thread parks on a
                                // futex, not on the binder waitqueue).
                                let pol = tk.policy() as u8;
                                let onrq = match tk.policy() {
                                    crate::process::task::SchedPolicy::Fifo
                                    | crate::process::task::SchedPolicy::Rr => {
                                        tk.rt_entity().is_on_rq()
                                    }
                                    crate::process::task::SchedPolicy::Deadline => {
                                        tk.dl_entity().is_on_rq()
                                    }
                                    _ => tk.sched_entity().is_on_rq(),
                                };
                                let dlthr = tk
                                    .dl_entity()
                                    .dl_throttled
                                    .load(core::sync::atomic::Ordering::Acquire);
                                let _ = writeln!(
                                    out,
                                    "binder-stall: WAKE-RUN-LEAK tid {} kicked {}x age {}s state={:#x} cfs_linked={} on_cpu={} pick_state={} cpu={} pol={} on_rq={} dl_thr={} wok={} wno={}",
                                    t.tid,
                                    kicks + 1,
                                    age / hz,
                                    tk.state().bits(),
                                    linked,
                                    tk.on_cpu(),
                                    tk.pick_state(),
                                    tk.ti_cpu(),
                                    pol,
                                    onrq,
                                    if dlthr { 1 } else { 0 },
                                    t.kick_wake_ok.load(core::sync::atomic::Ordering::Relaxed),
                                    t.kick_wake_refused.load(core::sync::atomic::Ordering::Relaxed),
                                );
                                let _ = writeln!(out, "binder-stall:{}", slots);
                            }
                        }
                        rekick.push(t.clone());
                    }
                    if age < STALL_SECS * hz {
                        continue;
                    }
                    if t.stall_reported.swap(true, core::sync::atomic::Ordering::AcqRel) {
                        continue;
                    }
                    let secs = (age / hz).max(1);
                    let th = t.st();
                    let pending = th.txn_stack.len();
                    let owed_by = th.txn_stack.last().map(|txn| txn.to_proc.clone());
                    let top_txn = th.txn_stack.last().cloned();
                    use core::fmt::Write;
                    match (pending, &owed_by) {
                        (0, _) => {
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for work (no pending txn)",
                                proc.pid, t.tid, secs
                            );
                        }
                        (n, Some(owner)) => {
                            // sc74 discriminator: the top txn is either one
                            // this thread SENT (from_tid == self — classic
                            // client waiting for its reply) or one it is
                            // SERVICING (to_tid == self — a server parked in
                            // read owing a reply, the death-loop terminal
                            // shape; to_proc == own pid is NOT sufficient to
                            // tell them apart with same-process IPC).
                            let shape = match &top_txn {
                                Some(txn) if txn.to_tid() == Some(t.tid) => "SERVER-OWING",
                                Some(txn) if txn.from_tid == Some(t.tid) => "CLIENT-WAIT",
                                _ => "CLIENT-WAIT?",
                            };
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for reply from pid {} ({} txn(s) stacked) {}",
                                proc.pid, t.tid, secs, owner.pid, n, shape
                            );
                            if let Some(txn) = top_txn {
                                let _ = writeln!(
                                    out,
                                    "binder-stall:   top txn{} code {} oneway={} from pid {} tid {} to_tid {}",
                                    txn.id,
                                    txn.code,
                                    txn.flags & TF_ONE_WAY != 0,
                                    txn.from_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                                    txn.from_tid.unwrap_or(0),
                                    txn.to_tid().unwrap_or(0)
                                );
                            }
                            // SERVER-OWING: does a nested txn from this
                            // thread exist anywhere in the world?
                            if shape == "SERVER-OWING" {
                                let from = outstanding_from_tid(world, proc.pid, t.tid);
                                if from.is_empty() {
                                    let _ = writeln!(
                                        out,
                                        "binder-stall:   NO outstanding txn from tid {} — nested call vanished or never sent",
                                        t.tid
                                    );
                                    // sc77: the impossible shape gets a user-stack
                                    // snapshot — names the OH path that parked.
                                    // sc89: deferred past the guard (page-walk).
                                    late_parkdumps.push(t.clone());
                                } else {
                                    let _ = writeln!(
                                        out,
                                        "binder-stall:   outstanding from tid {}:{}",
                                        t.tid,
                                        from
                                    );
                                }
                            }
                            let _ = writeln!(out, "binder-stall:{}", thread_census_of(t));
                            let _ = writeln!(out, "binder-stall:{}", hist_line_of(t.tid, &unsafe { hist_snapshot(t) }));
                            if dump_owners.len() < 2 && !dump_owners.iter().any(|o| Arc::ptr_eq(o, owner)) {
                                dump_owners.push(owner.clone());
                            }
                        }
                        (n, None) => {
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for reply ({} txn(s) stacked)",
                                proc.pid, t.tid, secs, n
                            );
                        }
                    }
                }
            }
            // Full dump of up to two owing procs: proc todo depth + every
            // thread's looper/wait/stack/todo state. sc89: lines formatted
            // and hist rings snapshotted under the guard; census lines,
            // holder forensics, futex graphs and all printing are deferred.
            for owner in &dump_owners {
                if let Some(op) = world.procs.iter().find(|p| Arc::ptr_eq(p, owner)) {
                    let opm = pm(world, op);
                    // Name the owing process via its main thread's comm.
                    let pname = unsafe {
                        let mt = crate::sched::find_task_by_pid(op.pid);
                        if mt.is_null() {
                            "?"
                        } else {
                            let c = (*mt).comm();
                            core::str::from_utf8(&c[..])
                                .unwrap_or("?")
                                .trim_end_matches('\0')
                        }
                    };
                    let mut od = OwnerDump {
                        pid: op.pid,
                        name: alloc::string::String::from(pname),
                        todo_depth: opm.im().todo.len(),
                        lines: Vec::new(),
                        hists: Vec::new(),
                        futex_line: alloc::string::String::new(),
                        threads: Vec::new(),
                    };
                    for t in &opm.im().threads {
                        let mut line = alloc::string::String::new();
                        // SAFETY: WORLD held (thread-state read).
                        unsafe { stall_thread_line(op, t, &mut line) };
                        let has_stack = !t.st().txn_stack.is_empty();
                        od.hists.push(if has_stack {
                            // SAFETY: WORLD held.
                            Some(unsafe { hist_snapshot(t) })
                        } else {
                            None
                        });
                        od.threads.push(t.clone());
                        od.lines.push((line, has_stack));
                    }
                    late_futex_graph.push(op.pid);
                    owner_dumps.push(od);
                }
            }
        }
    }
    // Heals, outside WORLD (wake_proc_one takes the world lock itself).
    for t in rekick {
        wake_thread(&t);
    }
    // leg-32B: retired-slot handoff txns delivered to child threads.
    for t in retire_wake_ths.drain(..) {
        wake_thread(&t);
    }
    for p in retodo {
        wake_proc_one(&p, 0);
    }
    for p in retire_wakes.drain(..) {
        wake_proc_one(&p, 0);
    }
    // ---- sc89: slow forensics + output, all with irqs on, WORLD free ----
    use core::fmt::Write;
    for od in &owner_dumps {
        let _ = writeln!(
            out,
            "binder-stall: owing pid {} ('{}') proc todo depth {} threads {}",
            od.pid,
            od.name,
            od.todo_depth,
            od.lines.len()
        );
        for (((line, has_stack), hist), thr) in od
            .lines
            .iter()
            .zip(od.hists.iter())
            .zip(od.threads.iter())
        {
            let _ = writeln!(out, "binder-stall:{}", line);
            if *has_stack {
                let _ = writeln!(out, "binder-stall:{}", thread_census_of(thr));
                if let Some(ring) = hist {
                    let _ = writeln!(out, "binder-stall:{}", hist_line_of(thr.tid, ring));
                }
                // SAFETY: no binder state touched (sched/futex/page tables).
                unsafe { holder_forensics(thr, &mut out) };
            }
        }
    }
    // sc75: the owing proc's full futex-waiter graph — the
    // intra-process contention set (each entry: waiting tid,
    // uaddr, woken). The wedge terminal so far is handlers
    // blocked on per-thread-stack condvar/mutex words.
    for pid in late_futex_graph {
        let fw = crate::sync::futex::waiters_of_tgid(pid);
        if !fw.is_empty() {
            let mut s = alloc::string::String::new();
            for (wpid, uaddr, woken) in fw {
                let _ = write!(s, " [{}:{:#x} w={}]", wpid, uaddr, woken);
            }
            let _ = writeln!(out, "binder-stall: futex waiters of pid {}:", pid);
            let _ = writeln!(out, "binder-stall:{}", s);
        }
    }
    // Impossible-shape user-stack snapshots: the page-table walks.
    for t in &late_parkdumps {
        // SAFETY: no binder state touched (task lookup + mm walk).
        unsafe { user_park_stack_dump(t) };
    }
    // One flush, interrupts on. Chunked: split on the last newline below
    // ~950 bytes so no single console write stalls the console lock.
    if !out.is_empty() {
        let bytes = out.as_bytes();
        let mut start = 0usize;
        while start < bytes.len() {
            let mut end = (start + 950).min(bytes.len());
            if end < bytes.len() {
                // extend/cut to a line boundary
                match bytes[start..end].iter().rposition(|&b| b == b'\n') {
                    Some(rel) if rel > 200 => end = start + rel + 1,
                    _ => {
                        match bytes[start..].iter().position(|&b| b == b'\n') {
                            Some(rel) if rel < 1600 => end = start + rel + 1,
                            _ => {}
                        }
                    }
                }
            }
            let chunk = core::str::from_utf8(&bytes[start..end]).unwrap_or("");
            crate::pr_warn!("binder-stall:{}", chunk);
            start = end;
        }
    }
}

/// BINDER_SET_CONTEXT_MGR / _EXT body (binder_ioctl_set_ctx_mgr). The EXT
/// variant passes the flat_binder_object's ptr/cookie/flags; the plain one
/// passes zeros. Linux creates the node with local refs held and has_*_ref
/// already true, so no INCREFS/ACQUIRE handshake fires for it.
fn set_context_mgr(proc: &Arc<BinderProc>, ptr: u64, cookie: u64, flags: u32) -> i64 {
    let mut w = world_lock();
    let world = w.as_mut().unwrap();
    if world.mgr_node[proc.ctx as usize].is_some() {
        crate::pr_err!(
            "binder: BINDER_SET_CONTEXT_MGR already set (ctx {})",
            proc.ctx as u8
        );
        return -(EBUSY as i64);
    }
    // Keep FLAT_BINDER_FLAG_TXN_SECURITY_CTX (0x1000) live on the manager
    // node: OH samgr registers with it and REQUIRES BR_TRANSACTION_SEC_CTX
    // deliveries — libbinder's BinderInvoker::Transaction() derives
    // callerSid_ (IPCSkeleton::GetCallingSid()) from the secctx record as a
    // C string, and a plain BR_TRANSACTION leaves it "" (boots b52-b58:
    // every GetServiceCheck failed with an empty callingSid because an
    // earlier wedge workaround masked the flag). The b45 wedge itself was
    // the garbage binary secctx record plus the broken selinuxfs status
    // mmap, both fixed alongside this.
    let node = Arc::new(BinderNode::new(world.next_id, ptr, cookie, proc.id, proc.pid, flags));
    world.next_id += 1;
    // SAFETY: under WORLD.
    unsafe {
        let ns = node.st();
        ns.local_strong_refs = 1;
        ns.local_weak_refs = 1;
        ns.has_strong_ref = true;
        ns.has_weak_ref = true;
        pm(world, proc).im().nodes.push(node.clone());
    }
    world.mgr_node[proc.ctx as usize] = Some(node);
    drop(w);
    // Release any handle-0 transactions parked waiting for the manager
    // (wait_for_context_manager).
    MANAGER_WAIT.wake_up_all();
    crate::pr_info!(
        "binder: proc {} (pid {}) is the context manager (ctx {}, flags {:#x})",
        proc.id,
        proc.pid,
        proc.ctx as u8,
        flags
    );
    0
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
                    // tgid, NOT current_pid(): process::current_pid()
                    // returns the per-TASK pid (a worker thread's tid) —
                    // stamping that as this thread's tgid made the sc161
                    // ownership gate reject the daemon's own work.
                    let tgid = crate::sched::current()
                        .map(|t| unsafe { (*t).tgid() } as u32)
                        .unwrap_or(0);
                    let t = Arc::new(BinderThread::new(tid, tgid));
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
        BINDER_SET_CONTEXT_MGR => set_context_mgr(&proc, 0, 0, 0),
        BINDER_SET_CONTEXT_MGR_EXT => {
            // flat_binder_object (LP64): type@0 u32, flags@4 u32,
            // binder@8 u64, cookie@16 u64. Linux only consumes
            // fp->binder (node ptr), fp->cookie and fp->flags here.
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 24) {
                return -(EFAULT as i64);
            }
            let ftype = unsafe { crate::arch::uaccess::get_user(arg as *const u32) };
            let flags = unsafe { crate::arch::uaccess::get_user((arg + 4) as *const u32) };
            let ptr = unsafe { crate::arch::uaccess::get_user((arg + 8) as *const u64) };
            let cookie = unsafe { crate::arch::uaccess::get_user((arg + 16) as *const u64) };
            match (ftype, flags, ptr, cookie) {
                (Some(_), Some(flags), Some(ptr), Some(cookie)) => {
                    set_context_mgr(&proc, ptr, cookie, flags)
                }
                _ => -(EFAULT as i64),
            }
        }
        BINDER_FEATURE_SET => {
            // OpenHarmony binder feature negotiation: report the enhanced
            // features this driver supports. With the access-token registry
            // (ipc::access_token) we now implement the token transport the
            // samgr security chain needs — without it samgr's CanRequest()
            // saw a uid-as-token, denied every AddSystemAbility, and the
            // whole HDF host chain (composer included) starved.
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 8) {
                return -(EFAULT as i64);
            }
            // SAFETY: arg validated non-null, 8-byte writable.
            if !unsafe {
                crate::arch::uaccess::put_user(arg as *mut u64, BINDER_SUPPORTED_FEATURES)
            } {
                return -(EFAULT as i64);
            }
            0
        }
        BINDER_GET_ACCESS_TOKEN | BINDER_GET_SENDER_INFO => {
            // OH extension: the server asks for the SENDER of the
            // transaction it is currently servicing. struct access_token
            // {u64 sender_tokenid; u64 first_tokenid; u64 reserved[2]},
            // struct binder_sender_info = access_token + u64 sender_pid_nr.
            let with_pid = request == BINDER_GET_SENDER_INFO;
            let len = if with_pid { 40 } else { 32 };
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, len) {
                return -(EFAULT as i64);
            }
            // The calling thread's transaction stack top is the txn being
            // serviced (Linux thread->transaction_stack, OH reads the same).
            let (sender_pid, token, ftoken) = {
                // dfx=syscalltrace: the GET_SENDER_INFO round-trip trace.
                // Ungated it prints twice per call — samgr-side polls make
                // this the highest-rate binder ioctl of an OH boot, and
                // hilogd.rd_kmsg then eats a vCPU copying it to /data.
                let trace = crate::dfx::switches::enabled(
                    crate::dfx::switches::DfxSwitch::SyscallTrace,
                );
                if trace {
                    crate::pr_info!("btx: GET_SENDER_INFO caller tid {}", current_tid());
                }
                let tid = current_tid();
                let mut out = (0u32, 0u64, 0u64);
                let w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    let world = w.as_ref().unwrap();
                    if let Some(th) = world
                        .procs
                        .iter()
                        .find(|q| Arc::ptr_eq(q, &proc))
                        .and_then(|q| q.im().threads.iter().find(|t| t.tid == tid))
                    {
                        if let Some(txn) = th.st().txn_stack.last() {
                            if let Some(fp) = &txn.from_proc {
                                out = (fp.pid, 0, 0);
                            }
                        }
                    }
                }
                drop(w);
                let (t, f) = crate::ipc::access_token::token_of(out.0);
                if trace {
                    crate::pr_info!("btx: GET_SENDER_INFO sender pid {} token {:#x} ftoken {:#x}", out.0, t, f);
                }
                (out.0, t, f)
            };
            let mut rec = [0u64; 5];
            rec[0] = token;
            rec[1] = ftoken;
            if with_pid {
                rec[4] = sender_pid as u64;
            }
            // SAFETY: arg validated, len bytes.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    arg as *mut u8,
                    rec.as_ptr() as *const u8,
                    len,
                )
            } != 0
            {
                return -(EFAULT as i64);
            }
            0
        }
        BINDER_THREAD_EXIT => {
            let mut w = world_lock();
            // SAFETY: under WORLD.
            unsafe {
                let world = w.as_mut().unwrap();
                let wakes = binder_reap_thread(world, &proc, tid);
                // Wake outside WORLD (lock discipline; see binder_close).
                drop(w);
                for p in wakes {
                    wake_proc_one(&p, 0);
                }
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
        // sc74 forensics: the first write command of every BWR — the wedged
        // server threads owed replies yet parked in a read, so their LAST
        // BWR's write side is the discriminator (empty / ref-commands /
        // a BC_REPLY or BC_TRANSACTION that never reached the kernel).
        {
            let first = get_user_at::<u32>(bwr.write_buffer, 0).unwrap_or(0xFFFF_FFFF);
            let w = world_lock();
            // SAFETY: under WORLD.
            unsafe { thread.hist(EV_BWR, first, bwr.write_size as u32) };
        }
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

/// BC-FORENSICS: snapshot the state of the userspace page backing a bad
/// binder write command — the "unknown BC command 0x0" family from the
/// b86-b90 heap-corruption hunt. A zero command word means copy_from_user
/// read zeros where userspace believes it wrote binder commands; the
/// page-table bits, page-descriptor counts and raw page contents below
/// distinguish the corruption modes:
///   - PTE present, page contents zeroed  -> data scribbled in place
///     (or the page was re-handed after a bogus free — free-while-mapped
///     tripwire never fired, so in-place is the leading theory),
///   - PTE present onto a DIFFERENT frame  -> PTE overwrite/aliasing,
///   - PTE absent / swap-marker            -> mapping torn down live,
///   - VMA missing                         -> "vanishing VMA" family (b89).
/// Error-path only (never on the hot path).
fn bc_corruption_snapshot(
    proc: &Arc<BinderProc>,
    thread: &Arc<BinderThread>,
    buffer: u64,
    size: usize,
    bad_off: usize,
    cmd: u32,
) {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};

    let bad_va = buffer.wrapping_add(bad_off as u64);
    crate::pr_err!(
        "BC-FORENSICS: pid {} tid {} cmd {:#x} buffer={:#x} size={} bad_off={}",
        proc.pid,
        thread.tid,
        cmd,
        buffer,
        size,
        bad_off
    );

    // Userspace view of the buffer: head words + words around the bad
    // offset (0xDEAD_BEEF marks "unmapped/unreadable", not "zero").
    let mut head = [0u32; 8];
    for i in 0..8usize {
        head[i] = get_user_at::<u32>(buffer, i * 4).unwrap_or(0xDEAD_BEEF);
    }
    crate::pr_err!("BC-FORENSICS: buffer[0..32] ={:08x?}", head);
    if bad_off > 0 {
        let mut around = [0u32; 8];
        let base = bad_off.saturating_sub(16) & !3;
        for i in 0..8usize {
            let off = base + i * 4;
            if off < size {
                around[i] = get_user_at::<u32>(buffer, off).unwrap_or(0xDEAD_BEEF);
            } else {
                around[i] = 0xCAFE_0000;
            }
        }
        crate::pr_err!("BC-FORENSICS: buffer[{}..] ={:08x?}", base, around);
    }

    // Binder-zone membership: a write buffer inside the proc's own mmap'd
    // binder region points at a different failure mode (kernel-side block
    // corruption) than one on the malloc heap.
    {
        // WORLD held for the alloc inspection (guard lives to scope end).
        let _w = world_lock();
        unsafe {
            if let Some(alloc) = proc.im().alloc.as_ref() {
                let lo = alloc.user_base;
                let hi = alloc.user_base + (alloc.npages * PAGE_SIZE) as u64;
                crate::pr_err!(
                    "BC-FORENSICS: binder mmap zone {:#x}-{:#x} bad_va_in_zone={}",
                    lo,
                    hi,
                    bad_va >= lo && bad_va < hi
                );
            }
        }
    }

    let Some(task) = crate::sched::current() else { return };
    let Some(aspace) = task.address_space() else { return };
    let root = aspace.root_ppn();
    let page_va = bad_va & !((PAGE_SIZE - 1) as u64);

    // Covering VMA (find() is on the page-safe address bad_va).
    {
        let vmas = aspace.vma_read();
        match vmas.find(VirtAddr::new(bad_va as usize)) {
            Some(v) => crate::pr_err!(
                "BC-FORENSICS: vma {:#x}-{:#x} flags={:#x} type={:?}",
                v.start().as_usize(),
                v.end().as_usize(),
                v.flags().bits(),
                v.vma_type()
            ),
            None => crate::pr_err!("BC-FORENSICS: NO VMA covers {:#x}", bad_va),
        }
    }

    // Raw leaf PTE first: catches P=0 swap/migration markers that the
    // walker below reports as simply "absent". x86_64-only (the OH target
    // arch); riscv64 keeps the walker read.
    #[cfg(target_arch = "x86_64")]
    {
        if let Some(raw) =
            crate::arch::mm::page_fault::read_pte_raw(root, crate::arch::mm::VirtAddr::new(bad_va))
        {
            crate::pr_err!("BC-FORENSICS: raw leaf pte = {:#x}", raw);
        }
    }

    // SAFETY: read-only diagnostic walk of the current task's live user
    // page tables.
    let walked = unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, page_va) };
    match walked {
        Some((ppn, bits)) => {
            #[cfg(target_arch = "x86_64")]
            crate::pr_err!(
                "BC-FORENSICS: pte(page) ppn={:#x} bits={:#x} P={} W={} U={} NX={} A={} D={}",
                ppn,
                bits,
                bits & 0x1 != 0,
                bits & 0x2 != 0,
                bits & 0x4 != 0,
                bits & 0x8000_0000_0000_0000 != 0,
                bits & 0x20 != 0,
                bits & 0x40 != 0
            );
            #[cfg(target_arch = "riscv64")]
            crate::pr_err!(
                "BC-FORENSICS: pte(page) ppn={:#x} bits={:#x} V={} R={} W={} X={}",
                ppn,
                bits,
                bits & 0x1 != 0,
                bits & 0x2 != 0,
                bits & 0x4 != 0,
                bits & 0x8 != 0
            );

            let page = crate::mm::pfn_to_page_mut(ppn as usize);
            if !page.is_null() {
                // SAFETY: read-only descriptor access for a live frame.
                let p = unsafe { &*page };
                crate::pr_err!(
                    "BC-FORENSICS: page desc refcount={} mapcount={} Cow={} Anon={} SwapBacked={} OnFreelist={} Reserved={} Migrating={}",
                    p.refcount(),
                    p.mapcount(),
                    p.test_flag(crate::mm::page_desc::PageFlag::Cow),
                    p.test_flag(crate::mm::page_desc::PageFlag::Anonymous),
                    p.test_flag(crate::mm::page_desc::PageFlag::SwapBacked),
                    p.test_flag(crate::mm::page_desc::PageFlag::OnFreelist),
                    p.test_flag(crate::mm::page_desc::PageFlag::Reserved),
                    p.test_flag(crate::mm::page_desc::PageFlag::Migrating),
                );
                let mut mappings = alloc::string::String::new();
                // SAFETY: same read-only descriptor.
                unsafe {
                    (*page).rmap_alt_for_each(|mm, vpn| {
                        use core::fmt::Write;
                        let _ = write!(mappings, " (mm:{:#x},vpn:{:#x})", mm, vpn);
                    });
                }
                if !mappings.is_empty() {
                    crate::pr_err!("BC-FORENSICS: page alt mappings:{}", mappings);
                }
            } else {
                crate::pr_err!("BC-FORENSICS: page desc NULL for ppn {:#x}", ppn);
            }

            // Raw page contents through the direct map: is the WHOLE page
            // zero, or only the binder words? (frame may be any user page;
            // the linear map covers all RAM on both targets)
            if crate::mm::pfn_valid(ppn as usize) {
                let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(ppn << 12));
                let base = kv.bits() as usize as *const u64;
                // SAFETY: base points at the physical page behind the PTE
                // just walked, inside the kernel direct map; 16 u64 words
                // are within the page.
                let words = unsafe { core::slice::from_raw_parts(base, 16) };
                crate::pr_err!("BC-FORENSICS: page[{:#x}] first 128B ={:016x?}", ppn << 12, words);
            }
        }
        None => {
            crate::pr_err!(
                "BC-FORENSICS: page {:#x} NOT MAPPED in pid {}'s tables (PTE absent)",
                page_va,
                proc.pid
            );
        }
    }
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
                // struct binder_transaction_data_sg: the trailing u64
                // buffers_size covers the sg (extra) area the PTR objects
                // will copy into (Linux extra_buffers_size).
                let sg_size = if sg {
                    let Some(bs) = get_user_at::<u64>(buffer, ptr + 64) else {
                        *consumed = ptr as u64;
                        return -(EFAULT as i64);
                    };
                    bs as usize
                } else {
                    0
                };
                ptr += if sg { 72 } else { 64 };
                // dfx=bindertrace: payload head for small parcels — the
                // interface token ("ohos.system...." / hdf names) is ASCII
                // at the head and names the target interface; replies carry
                // the result int. Read BEFORE the world lock (uaccess).
                if crate::dfx::switches::enabled(
                    crate::dfx::switches::DfxSwitch::BinderTrace,
                ) && tr.data_size > 0
                    && tr.data_size <= 160
                {
                    // Full parcel for small transactions: the
                    // FLAT_BINDER_OBJECT past the interface token is the
                    // prime suspect in the AddSystemAbility(5100)
                    // ERR_TRANSACTION_FAILED loop (driver must translate
                    // BINDER_TYPE_BINDER -> HANDLE for the target).
                    let n = tr.data_size as usize;
                    let mut pb = [0u8; 160];
                    if unsafe {
                        crate::arch::uaccess::copy_from_user(
                            pb.as_mut_ptr(),
                            tr.data_buffer as *const u8,
                            n,
                        )
                    } == 0
                    {
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        let mut hx = alloc::string::String::with_capacity(n * 2);
                        for &by in pb[..n].iter() {
                            hx.push(HEX[(by >> 4) as usize] as char);
                            hx.push(HEX[(by & 0xf) as usize] as char);
                        }
                        crate::pr_info!(
                            "btxp: pid {} {} code {} ds {} p {}",
                            proc.pid,
                            if reply { "R" } else { "T" },
                            tr.code,
                            tr.data_size,
                            hx
                        );
                    }
                }
                let mut err_cmd: u32 = 0;
                binder_transaction(proc, thread, &mut tr, reply, sg_size, &mut err_cmd);
                if err_cmd != 0 {
                    crate::pr_info!(
                        "binder: pid {} {} failed with BR {:#x} (code {}, ds {} os {})",
                        proc.pid,
                        if reply { "BC_REPLY" } else { "BC_TRANSACTION" },
                        err_cmd,
                        tr.code,
                        tr.data_size,
                        tr.offsets_size
                    );
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
                    wake_thread(thread);
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
                    let mut drain_wake: Option<(Arc<BinderProc>, u32)> = None;
                    // leg-32B: child thread targeted by the drained txn's
                    // node (reg_tgid) — woken after WORLD release.
                    let mut drain_wake_thread: Option<Arc<BinderThread>> = None;
                    {
                        let mut w = world_lock();
                        // SAFETY: all inner access below is under WORLD.
                        unsafe {
                            let world = w.as_mut().unwrap();
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
                                    // Oneway buffer release: hand the node's
                                    // async slot to the next parked txn or
                                    // free it (Linux drains node->async_todo
                                    // in binder_free_buf).
                                    // leg-30B AMS46: the receiver's BC_FREE
                                    // for a traced/AMS-node txn — slot
                                    // release or handoff, the (c) leg of the
                                    // adjudication.
                                    {
                                        static AMS46_FR_N: core::sync::atomic::AtomicUsize =
                                            core::sync::atomic::AtomicUsize::new(0);
                                        let latched = ams46_node_id();
                                        let is_ams = latched != 0
                                            && t.target_node.as_ref().map(|n| n.id)
                                                == Some(latched);
                                        if ams46_on()
                                            && (ams46_txns_contains(t.id) || is_ams)
                                            && AMS46_FR_N
                                                .fetch_add(
                                                    1,
                                                    core::sync::atomic::Ordering::Relaxed,
                                                )
                                                < 96
                                        {
                                            crate::pr_warn!(
                                                "AMS46 FREE txn{} code={} by pid={} held_slot={}",
                                                t.id,
                                                t.code,
                                                proc.pid,
                                                t.holds_async_slot()
                                            );
                                        }
                                    }
                                    if t.holds_async_slot() {
                                        let mut next: Option<Arc<BinderTxn>> = None;
                                        if let Some(node) = &t.target_node {
                                            let ns = node.st();
                                            next = ns.async_todo.pop_front();
                                            if next.is_none() {
                                                ns.has_async_transaction = false;
                                                if ams46_on()
                                                    && ams46_txns_contains(t.id)
                                                {
                                                    crate::pr_warn!(
                                                        "AMS46 slot released by txn{}",
                                                        t.id
                                                    );
                                                }
                                            }
                                        }
                                        if let Some(h) = next {
                                            h.set_holds_async_slot(true);
                                            let tp = h.to_proc.clone();
                                            let tid = h.id;
                                            let tpid = tp.pid;
                                            // leg-32B: apply the forked-child
                                            // preference to the drained txn
                                            // (see drain_deliver_parked_txn) —
                                            // the plain proc-todo push here is
                                            // what lost AMS's Schedule oneways
                                            // to appspawn's dead todo.
                                            let (ct, pref) =
                                                drain_deliver_parked_txn(world, &h, &tp);
                                            match ct {
                                                Some(ct) => drain_wake_thread = Some(ct),
                                                None => drain_wake = Some((tp, pref)),
                                            }
                                            if ams46_on() && ams46_txns_contains(tid) {
                                                crate::pr_warn!(
                                                    "AMS46 slot -> txn{} (parked code-46 DELIVERED by drain)",
                                                    tid
                                                );
                                            }
                                            crate::pr_info!(
                                                "binder: oneway txn {} drained to proc {}",
                                                tid,
                                                tpid
                                            );
                                        }
                                    }
                                }
                                Some(None) => {
                                    // Sync buffer freed (its txn was unlinked
                                    // at reply time).
                                }
                            }
                        }
                    }
                    if let Some(t) = drain_wake_thread {
                        wake_thread(&t);
                    }
                    if let Some((tp, pref)) = drain_wake {
                        wake_proc_one(&tp, pref);
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
                        // Linux binder_thread_write special case: an
                        // INCREMENT on handle 0 (the context-manager
                        // proxy every BpBinder(0) takes on creation)
                        // resolves to the context-manager node via
                        // binder_inc_ref_for_node, creating the ref if
                        // this proc does not have one yet. Without it,
                        // every OH process's first samgr proxy ref was
                        // dropped on the floor.
                        let ridx = match (ridx, increment && desc == 0) {
                            (Some(i), _) => Some(i),
                            (None, true) => {
                                let mgr = world.mgr_node[proc.ctx as usize].clone();
                                match mgr {
                                    Some(mgr) if mgr.proc_id != proc.id => {
                                        let p = pm(world, proc);
                                        if let Some(i) = p
                                            .im()
                                            .refs
                                            .iter()
                                            .position(|r| Arc::ptr_eq(&r.node, &mgr))
                                        {
                                            Some(i)
                                        } else {
                                            // Create with zero counts (the
                                            // normal increment path below
                                            // brings them to 1/0 or 0/1),
                                            // pinning the node like
                                            // get_or_create_ref does.
                                            let d = p.im().next_desc;
                                            p.im().next_desc += 1;
                                            mgr.st().internal_strong_refs += 1;
                                            p.im().refs.push(BinderRef {
                                                desc: d,
                                                node: mgr.clone(),
                                                strong: 0,
                                                weak: 0,
                                                death: None,
                                            });
                                            let i = p.im().refs.len() - 1;
                                            if let Some(owner) = world
                                                .procs
                                                .iter()
                                                .find(|o| o.id == mgr.proc_id)
                                            {
                                                owner.im().todo.push_back(Work::Node(mgr.clone()));
                                                if wake_owner.is_none() {
                                                    wake_owner = Some(owner.clone());
                                                }
                                            }
                                            Some(i)
                                        }
                                    }
                                    _ => None,
                                }
                            }
                            (None, false) => None,
                        };
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
                                let desc = pm(world, proc).im().refs[ridx].desc;
                                pm(world, proc).im().refs.remove(ridx);
                                // Drop a still-queued death notification for
                                // the removed handle (binder_free_ref
                                // dequeues ref->death->work).
                                pm(world, proc).im().todo.retain(|wk| {
                                    !matches!(wk, Work::DeadBinder { desc: d, .. } if *d == desc)
                                });
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
                    wake_proc_one(&owner, 0);
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
                    // sc167 node-identity parity: prefer the issuing
                    // thread's OWN node at this ptr ((ptr, reg_tgid) key;
                    // see translate_object) — forked siblings in this
                    // shared proc can each own a stub at the same VA.
                    let my_tgid = crate::sched::current()
                        .map(|t| (*t).tgid() as u32)
                        .unwrap_or(0);
                    let nodes = &pm(world, proc).im().nodes;
                    let node = nodes
                        .iter()
                        .find(|n| n.ptr == node_ptr && n.reg_tgid == my_tgid)
                        .or_else(|| nodes.iter().find(|n| n.ptr == node_ptr))
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
            BC_REQUEST_DEATH_NOTIFICATION | BC_CLEAR_DEATH_NOTIFICATION => {
                // Wire format (binder_thread_write): u32 target handle,
                // then u64 cookie — 12 bytes consumed.
                let Some(target) = get_user_at::<u32>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                let Some(cookie) = get_user_at::<u64>(buffer, ptr + 4) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 12;
                let request = cmd == BC_REQUEST_DEATH_NOTIFICATION;
                let mut wake_self = false;
                {
                    let mut w = world_lock();
                    // SAFETY: all inner access below is under WORLD.
                    unsafe {
                        let world = w.as_mut().unwrap();
                        let pim = pm(world, proc).im();
                        let Some(ridx) = pim.refs.iter().position(|r| r.desc == target) else {
                            drop(w);
                            crate::pr_info!(
                                "binder: pid {} death command on invalid handle {}",
                                proc.pid,
                                target
                            );
                            continue;
                        };
                        if request {
                            if pim.refs[ridx].death.is_some() {
                                drop(w);
                                crate::pr_info!(
                                    "binder: pid {} BC_REQUEST_DEATH_NOTIFICATION already set (desc {})",
                                    proc.pid,
                                    target
                                );
                                continue;
                            }
                            let already_dead = node_is_dead(world, &pim.refs[ridx].node);
                            pim.refs[ridx].death = Some(RefDeath {
                                cookie,
                                phase: if already_dead { DeathPhase::Queued } else { DeathPhase::Armed },
                                clear_pending: false,
                            });
                            if already_dead {
                                // Linux queues the DEAD_BINDER work right
                                // away when the node is already dead.
                                pim.todo.push_back(Work::DeadBinder { desc: target, cookie });
                                wake_self = true;
                            }
                        } else {
                            let ok = pim.refs[ridx]
                                .death
                                .as_ref()
                                .map(|d| d.cookie == cookie)
                                .unwrap_or(false);
                            if !ok {
                                drop(w);
                                crate::pr_info!(
                                    "binder: pid {} BC_CLEAR_DEATH_NOTIFICATION not active or cookie mismatch (desc {})",
                                    proc.pid,
                                    target
                                );
                                continue;
                            }
                            match pim.refs[ridx].death.as_ref().unwrap().phase {
                                DeathPhase::Armed => {
                                    // Never fired: drop the request and
                                    // deliver BR_CLEAR_DEATH_NOTIFICATION_DONE.
                                    pim.refs[ridx].death = None;
                                    let th = thread.st();
                                    th.todo.push_back(Work::ClearDeathDone { cookie });
                                    th.process_todo = true;
                                    wake_self = true;
                                }
                                DeathPhase::Queued | DeathPhase::Delivered => {
                                    // Notification already in flight
                                    // (Linux retypes the queued work to
                                    // DEAD_BINDER_AND_CLEAR).
                                    pim.refs[ridx].death.as_mut().unwrap().clear_pending = true;
                                }
                            }
                        }
                    }
                }
                if wake_self {
                    wake_thread(thread);
                }
            }
            BC_DEAD_BINDER_DONE => {
                let Some(cookie) = get_user_at::<u64>(buffer, ptr) else {
                    *consumed = ptr as u64;
                    return -(EFAULT as i64);
                };
                ptr += 8;
                let mut wake_self = false;
                {
                    let mut w = world_lock();
                    // SAFETY: all inner access below is under WORLD.
                    unsafe {
                        let world = w.as_mut().unwrap();
                        let pim = pm(world, proc).im();
                        let hit = pim
                            .refs
                            .iter()
                            .find(|r| {
                                r.death.as_ref().map(|d| d.cookie == cookie && d.phase == DeathPhase::Delivered).unwrap_or(false)
                            })
                            .map(|r| r.desc);
                        let Some(desc) = hit else {
                            drop(w);
                            crate::pr_info!(
                                "binder: pid {} BC_DEAD_BINDER_DONE {:#x} not found",
                                proc.pid,
                                cookie
                            );
                            continue;
                        };
                        let r = pim.refs.iter_mut().find(|r| r.desc == desc).unwrap();
                        if r.death.as_ref().unwrap().clear_pending {
                            r.death = None;
                            let th = thread.st();
                            th.todo.push_back(Work::ClearDeathDone { cookie });
                            th.process_todo = true;
                            wake_self = true;
                        } else {
                            // Dequeue from delivered_death; the record
                            // stays armed on the ref (Linux keeps
                            // ref->death until CLEAR or ref release).
                            r.death.as_mut().unwrap().phase = DeathPhase::Armed;
                        }
                    }
                }
                if wake_self {
                    wake_thread(thread);
                }
            }
            _ => {
                // b85 forensics: the last binder event before the samgr
                // wedge was this print WITHOUT a pid — name the issuer.
                crate::pr_info!(
                    "binder: pid {} tid {} unknown BC command {:#x}, stopping",
                    proc.pid,
                    thread.tid,
                    cmd
                );
                // b86-b90 heap-corruption hunt: a zero command word is the
                // "zeroed write buffer" signature — snapshot the page
                // state before the userspace side dies (error path only).
                if cmd == 0 {
                    bc_corruption_snapshot(proc, thread, buffer, size, ptr - 4, cmd);
                }
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
    sg_size: usize,
    err_cmd: &mut u32,
) {
    let oneway = tr.flags & TF_ONE_WAY != 0;
    let data_size = tr.data_size as usize;
    let offsets_size = tr.offsets_size as usize;
    // Threads to wake directly (thread-targeted delivery). Declared in
    // the prologue because the pre-alloc stale sweep below can already
    // collect child-thread wakes for drained parked txns (leg-32B).
    let mut wake_ths: Vec<Arc<BinderThread>> = Vec::new();

    // Bring-up crutch for the samgr-install race — see
    // wait_for_context_manager: stretch bounded client retry budgets
    // across the manager's late install instead of failing instantly.
    if !reply && tr.target_handle == 0 {
        wait_for_context_manager(proc.ctx);
    }

    // ---- resolve target + allocate the target buffer ------------------
    let mut target_node: Option<Arc<BinderNode>> = None;
    let mut target_thread_key: Option<(Arc<BinderProc>, u32)> = None;
    let mut in_reply_to: Option<Arc<BinderTxn>> = None;
    let mut target_proc: Option<Arc<BinderProc>> = None;
    let mut block_off: usize;
    // BINDER_TYPE_FD/FDA gate (binder_translate_fd): replies need the
    // original transaction's TF_ACCEPT_FDS; transactions need the target
    // node's FLAT_BINDER_FLAG_ACCEPTS_FDS.
    let mut allows_fd = false;
    // Sender-info record space folded into the sg area for nodes opted
    // into FLAT_BINDER_FLAG_TXN_SECURITY_CTX.
    let mut secctx_space = 0usize;
    let mut sg_total = sg_size;
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
                    crate::pr_info!(
                        "binder: BC_REPLY stack top not ours (to_tid={:?}, me={})",
                        t.to_tid(),
                        thread.tid
                    );
                    return;
                }
                let t = thread.st().txn_stack.pop().unwrap();
                // sc208 SLOW-TXN probe (C-line: SAMgr SendRequest ~30 s
                // per call with the fixed clock): split the sync-txn
                // lifecycle into QUEUE (enq -> server pickup) and EXEC
                // (pickup -> reply) and print both for anything over the
                // threshold. sc269 (leg-25): 5 s hid the launcher-load
                // family (IOsAccount ~2 s, IDisplayComposer ~3 s,
                // AbilityManager 4-6 s, all client-visible in hilog's
                // BlockMonitor); threshold 150 jiffies (1.5 s), sender +
                // server comm named inline. Direct console (quietcon
                // boots drop pr_warn pre-ring at loglevel 3), rate-capped
                // to one line per 2 s. Printed from inside the WORLD
                // guard like the surrounding pr_info!s on this path
                // (single short line, bounded).
                {
                    let now = crate::drivers::timer::get_jiffies() as u32;
                    let total = now.wrapping_sub(t.enq_j);
                    if total >= 150 {
                        static LAST: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        use core::sync::atomic::Ordering::Relaxed;
                        let last = LAST.load(Relaxed);
                        if now.wrapping_sub(last) >= 200
                            && LAST
                                .compare_exchange(last, now, Relaxed, Relaxed)
                                .is_ok()
                        {
                            let pick = unsafe { *t.pick_j.get() };
                            let q = if pick != 0 { pick.wrapping_sub(t.enq_j) } else { 0 };
                            let x = if pick != 0 { now.wrapping_sub(pick) } else { total };
                            let from_pid =
                                t.from_proc.as_ref().map(|p| p.pid).unwrap_or(0);
                            let from_comm = crate::sched::find_task_by_pid(
                                t.from_tid.unwrap_or(0),
                            );
                            let to_comm = crate::sched::find_task_by_pid(
                                t.to_tid().unwrap_or(t.to_proc.pid),
                            );
                            let comm_of = |p: *mut crate::process::Task| -> &str {
                                if p.is_null() {
                                    return "?";
                                }
                                // SAFETY: diagnostic comm read of a live task.
                                let c = unsafe { (*p).comm() };
                                core::str::from_utf8(&c[..])
                                    .unwrap_or("?")
                                    .trim_end_matches('\0')
                            };
                            let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                            use core::fmt::Write as _;
                            let _ = writeln!(
                                w,
                                "SLOW-TXN txn{} code {} from pid {} tid {} ({}) to pid {} tid {} ({}): total {} ms (queue {} ms, exec {} ms)",
                                t.id,
                                t.code,
                                from_pid,
                                t.from_tid.unwrap_or(0),
                                comm_of(from_comm),
                                t.to_proc.pid,
                                thread.tid,
                                comm_of(to_comm),
                                total * 10,
                                q * 10,
                                x * 10
                            );
                        }
                    }
                }
                let (Some(from_proc), Some(from_tid)) = (t.from_proc.clone(), t.from_tid) else {
                    *err_cmd = BR_FAILED_REPLY;
                    crate::pr_info!("binder: BC_REPLY on oneway");
                    return;
                };
                // sc74 forensics: the pop is the point of no return — after
                // it, a failed reply leaves the original sender unwakeable.
                thread.hist(EV_REPLY_POP, t.id as u32, from_proc.pid);
                if from_proc.im().is_dead {
                    *err_cmd = BR_DEAD_REPLY;
                    crate::pr_info!("binder: BC_REPLY requester dead");
                    return;
                }
                target_thread_key = Some((from_proc, from_tid));
                target_proc = Some(target_thread_key.as_ref().unwrap().0.clone());
                allows_fd = t.flags & TF_ACCEPT_FDS != 0;
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
                    // sc141: failed-send visibility for the launcher lane —
                    // parked children send ZERO successful transactions;
                    // whether they TRY and fail here decides the next probe.
                    app_txn_fail_print(proc, "invalid-handle", tr.target_handle as u64, tr.code as u64);
                    return;
                };
                target_node = Some(node);
            } else {
                let mgr = world.mgr_node[proc.ctx as usize].clone();
                let Some(mgr) = mgr else {
                    *err_cmd = BR_DEAD_REPLY;
                    crate::pr_info!("binder: no context manager installed (ctx {})", proc.ctx as u8);
                    app_txn_fail_print(proc, "no-ctxmgr", 0, tr.code as u64);
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
                // sc161: child-registered node whose task group died —
                // the ref still resolves, but no thread can ever read a
                // delivery (work_readable_by gate); fail fast like a
                // dead-owner node instead of queueing forever.
                if node.dead.load(core::sync::atomic::Ordering::Acquire) {
                    *err_cmd = BR_DEAD_REPLY;
                    app_txn_fail_print(proc, "child-node-dead", tr.target_handle as u64, tr.code as u64);
                    return;
                }
                match world.procs.iter().find(|p| p.id == node.proc_id) {
                    Some(p) if !p.im().is_dead => target_proc = Some(p.clone()),
                    _ => {
                        *err_cmd = BR_DEAD_REPLY;
                        return;
                    }
                }
            }
            let tp = target_proc.as_ref().unwrap();
            // NOTE: no same-proc rejection here. Linux 5.10 (the OH
            // baseline) has no "self transaction" check — only master
            // gained one — so a transaction to a node owned by the
            // caller's own process queues on the (own) proc todo and is
            // served by a sibling worker (b85: samgr worker 357's calls
            // into samgr-hosted services died here with BR_FAILED_REPLY,
            // wedging the whole SA storm). Handle 0 from the manager's
            // own process stays an error above, exactly like 5.10's
            // "got transaction to context manager from process owning it".
            if tp.im().alloc.is_none() {
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
            if offsets_size % 8 != 0 {
                *err_cmd = BR_FAILED_REPLY;
                return;
            }
            if !reply {
                if let Some(node) = &target_node {
                    allows_fd = node.flags & FLAT_BINDER_FLAG_ACCEPTS_FDS != 0;
                    if node.flags & FLAT_BINDER_FLAG_TXN_SECURITY_CTX != 0 {
                        secctx_space = align8(SECCTX_SID.len() + 1);
                    }
                }
            }

            txn_id = world.next_id;
            world.next_id += 1;

            // dfx=bindertrace: one line per resolved transaction — the
            // HDF devmgr↔devhost handshake debugging needed the actual
            // conversation (who calls whom, which codes, oneway or not),
            // not just the failures.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::BinderTrace) {
                crate::pr_info!(
                    "btx: #{} pid {}{} -> pid {} h{} code {} {} ds {} os {}",
                    txn_id,
                    proc.pid,
                    if reply { " R" } else { "" },
                    target_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                    tr.target_handle,
                    tr.code,
                    if oneway { "oneway" } else { "twoway" },
                    data_size,
                    offsets_size
                );
            }

            let is_async = !reply && oneway;
            let _ = is_async;

            sg_total += secctx_space;
            // sc243 (leg-17): rate-limited stale-buffer reclaim BEFORE
            // the allocation — see sweep_stale_async. Keeps a silent
            // receiver's never-freed buffers from pinning txn arcs and
            // wedging the node's async slot forever. The handoff txns
            // land on their proc todo; no wake is issued (the receivers
            // this heals are silent by definition — they pick the todo
            // up on their next read).
            let swept = unsafe { sweep_stale_async(world, tp, &mut wake_ths) };
            if swept > 0 {
                crate::pr_info!(
                    "binder: reclaimed {} stale buffer(s) of pid {}",
                    swept,
                    tp.pid
                );
            }
            let off = pm(world, tp)
                .im()
                .alloc
                .as_mut()
                .unwrap()
                .new_buf(data_size, offsets_size, sg_total, is_async);
            let off = match off {
                Some(o) => o,
                None => {
                    // One forced retry: the 1/s rate stamp may have
                    // skipped the sweep above on this exact call.
                    if let Some(a) = pm(world, tp).im().alloc.as_mut() {
                        a.last_sweep_j = 0;
                    }
                    let swept2 = unsafe { sweep_stale_async(world, tp, &mut wake_ths) };
                    match (swept2 > 0).then(|| {
                        pm(world, tp)
                            .im()
                            .alloc
                            .as_mut()
                            .unwrap()
                            .new_buf(data_size, offsets_size, sg_total, is_async)
                    }) {
                        Some(Some(o)) => o,
                        _ => {
                            *err_cmd = BR_FAILED_REPLY;
                            crate::pr_info!(
                                "binder: buffer alloc failed (need {}+{}+{})",
                                align8(data_size),
                                align8(offsets_size),
                                align8(sg_size)
                            );
                            return;
                        }
                    }
                }
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

    // mud1 panic (Agent B report): a txn racing its target's death — the
    // target resolves alive under the first WORLD section (buffer
    // allocated), then the proc closes and its teardown frees the
    // allocator before this re-lock. Reply DEAD instead of unwrapping a
    // freed alloc; the cleanup mirrors the copy-fault paths below.
    let Some(target) = target_proc.clone() else {
        *err_cmd = BR_DEAD_REPLY;
        return;
    };

    // fd installs to undo if a later fixup step fails (Linux t->fd_count).
    let mut installed_fds: Vec<(u32, usize)> = Vec::new();

    // ---- copy payload from the sender into the target's region ----------
    let pair = {
        let w = world_lock();
        // SAFETY: under WORLD.
        unsafe {
            let world = w.as_ref().unwrap();
            let alive = proc_alive(world, &target) && !pm(world, &target).im().is_dead;
            if alive {
                pm(world, &target)
                    .im()
                    .alloc
                    .as_ref()
                    .map(|a| (a.kvirt, a.user_base))
            } else {
                None
            }
        }
    };
    let (kvirt, user_base) = match pair {
        Some(p) => p,
        None => {
            release_block(&target, block_off);
            unpin_after_failure(&target, target_node.as_ref(), reply, oneway);
            *err_cmd = BR_DEAD_REPLY;
            crate::pr_info!(
                "binder: target pid {} died before payload copy — BR_DEAD_REPLY",
                target.pid
            );
            return;
        }
    };
    // SAFETY: the block is exclusively owned by this in-flight txn.
    unsafe {
        let dst = kvirt.add(block_off);
        if data_size > 0
            && crate::arch::uaccess::copy_from_user(dst, tr.data_buffer as *const u8, data_size) != 0
        {
            release_block(&target, block_off);
            unpin_after_failure(&target, target_node.as_ref(), reply, oneway);
            *err_cmd = BR_FAILED_REPLY;
            crate::pr_info!("binder: txn data copy fault from {:#x}", tr.data_buffer);
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
            unpin_after_failure(&target, target_node.as_ref(), reply, oneway);
            *err_cmd = BR_FAILED_REPLY;
            crate::pr_info!("binder: txn offsets copy fault from {:#x}", tr.data_offsets);
            return;
        }
    }

    // ---- secctx record (BR_TRANSACTION_SECCTX payload) ------------------
    // A NUL-terminated security-context STRING at the start of the sg area,
    // ahead of any BINDER_TYPE_PTR payloads (upstream puts the context
    // there). OH libbinder's BinderInvoker::Transaction() casts the secctx
    // pointer to `const char*` for callerSid_ (IPCSkeleton::GetCallingSid)
    // — a binary struct here reads as garbage bytes up to an accidental
    // NUL (pid low byte), so it must be a string. Every Rux task is
    // unlabeled; "u:r:init:s0" is the permissive synthesized domain. With
    // the class/ tree absent, string_to_security_class fails and
    // deny_unknown=0 turns every such check into an allow anyway — the
    // string only needs to be well-formed and non-empty.
    let extra_base = align8(data_size) + align8(offsets_size);
    let mut sg_used = 0usize;
    let mut security_ctx: u64 = 0;
    if secctx_space > 0 {
        let bytes = SECCTX_SID.as_bytes();
        // SAFETY: the sg area is sized for secctx_space (>= len+1, NUL
        // included; SECCTX_SID has no interior NULs).
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                kvirt.add(block_off + extra_base),
                bytes.len(),
            );
            core::ptr::write_volatile(kvirt.add(block_off + extra_base + bytes.len()), 0u8);
        }
        security_ctx = user_base + block_off as u64 + extra_base as u64;
        sg_used += secctx_space;
    }

    // ---- translate flat binder objects ----------------------------------
    // Block layout: [data][offsets][sg extra area]. The sg cursor follows
    // BINDER_TYPE_PTR copies (Linux sg_bufp). Object fixups run WITHOUT the
    // world lock: the block is exclusively ours and fd installs must never
    // happen under WORLD.
    if offsets_size > 0 {
        let n = offsets_size / 8;
        let block_user = user_base + block_off as u64;
        for i in 0..n {
            // SAFETY: offsets array inside the kernel-side block.
            let obj_off = unsafe {
                core::ptr::read_volatile((kvirt.add(block_off + align8(data_size)) as *const u64).add(i)) as usize
            };
            if obj_off + 4 > data_size {
                fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                crate::pr_info!("binder: object offset {} out of range", obj_off);
                return;
            }
            // SAFETY: header word validated within the data area.
            let ty = unsafe { core::ptr::read_volatile(kvirt.add(block_off + obj_off) as *const u32) };
            let osz = binder_object_size(ty);
            if osz == 0 || obj_off + osz > data_size {
                fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                crate::pr_info!("binder: object type {:#x} at {} out of range", ty, obj_off);
                return;
            }
            match ty {
                BINDER_TYPE_FD => {
                    // fd lives in the handle field; pad/cookie cleared.
                    // SAFETY: validated within the data area.
                    let fd = unsafe { core::ptr::read_volatile(kvirt.add(block_off + obj_off + 8) as *const u32) };
                    if !allows_fd {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: fd {} to target that does not accept fds", fd);
                        return;
                    }
                    let Some(tfd) = translate_fd_into(fd, &target) else {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: fd {} install into pid {} failed", fd, target.pid);
                        return;
                    };
                    installed_fds.push((target.pid, tfd));
                    // SAFETY: validated within the data area.
                    unsafe {
                        core::ptr::write_volatile(kvirt.add(block_off + obj_off + 8) as *mut u32, tfd as u32);
                        core::ptr::write_volatile(kvirt.add(block_off + obj_off + 12) as *mut u32, 0);
                    }
                }
                BINDER_TYPE_PTR => {
                    // SAFETY: validated within the data area.
                    let mut bp = unsafe { core::ptr::read_volatile((kvirt.add(block_off + obj_off) as *const BinderBufferObject).cast_mut()) };
                    let bp_len = bp.length as usize;
                    if bp_len > sg_total - sg_used {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: sg buffer overruns buffers_size");
                        return;
                    }
                    // SAFETY: block layout bounds were validated above.
                    let sg_dst = unsafe { kvirt.add(block_off + extra_base + sg_used) };
                    // SAFETY: sg area sized for buffers_size.
                    if bp_len > 0
                        && unsafe {
                            crate::arch::uaccess::copy_from_user(sg_dst, bp.buffer as *const u8, bp_len)
                        } != 0
                    {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: sg copy fault from {:#x}", bp.buffer);
                        return;
                    }
                    let child_user = block_user + (extra_base + sg_used) as u64;
                    sg_used += align8(bp_len);
                    // Rewrite to the target's address of the copy.
                    bp.buffer = child_user;
                    // SAFETY: validated within the data area.
                    unsafe { core::ptr::write_volatile(kvirt.add(block_off + obj_off) as *mut BinderBufferObject, bp) };
                    if bp.flags as u64 & (BINDER_BUFFER_FLAG_HAS_PARENT as u64) != 0 {
                        // binder_fixup_parent: write this buffer's target
                        // address into the parent buffer at parent_offset.
                        let Some((parent_kernel, parent_len)) =
                            (unsafe { resolve_parent_ptr(kvirt, block_off, block_user, align8(data_size), data_size, offsets_size, sg_total, i, bp.parent) })
                        else {
                            fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                            crate::pr_info!("binder: PTR fixup parent invalid");
                            return;
                        };
                        if parent_len < 8 || bp.parent_offset > (parent_len - 8) as u64 {
                            fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                            crate::pr_info!("binder: PTR fixup offset out of parent");
                            return;
                        }
                        // SAFETY: parent buffer is inside our sg area.
                        unsafe {
                            core::ptr::write_volatile(parent_kernel.add(bp.parent_offset as usize) as *mut u64, child_user);
                        }
                    }
                }
                BINDER_TYPE_FDA => {
                    // SAFETY: validated within the data area.
                    let fda = unsafe { core::ptr::read_volatile((kvirt.add(block_off + obj_off) as *const BinderFdArrayObject).cast_mut()) };
                    if !allows_fd {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: fda to target that does not accept fds");
                        return;
                    }
                    if fda.num_fds >= usize::MAX as u64 / 4 {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        return;
                    }
                    let Some((parent_kernel, parent_len)) =
                        (unsafe { resolve_parent_ptr(kvirt, block_off, block_user, align8(data_size), data_size, offsets_size, sg_total, i, fda.parent) })
                    else {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: FDA parent invalid");
                        return;
                    };
                    let fd_buf_size = 4 * fda.num_fds as usize;
                    if fd_buf_size > parent_len || fda.parent_offset as usize > parent_len - fd_buf_size {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!("binder: FDA does not fit in parent buffer");
                        return;
                    }
                    // SAFETY: fd array inside the parent's sg buffer.
                    let fd_array = unsafe { parent_kernel.add(fda.parent_offset as usize) as *mut u32 };
                    for k in 0..fda.num_fds as usize {
                        // SAFETY: bounds validated above against the parent.
                        let fd = unsafe { core::ptr::read_volatile(fd_array.add(k)) };
                        let Some(tfd) = translate_fd_into(fd, &target) else {
                            fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                            crate::pr_info!("binder: fda fd {} install into pid {} failed", fd, target.pid);
                            return;
                        };
                        installed_fds.push((target.pid, tfd));
                        // SAFETY: same slot we read.
                        unsafe { core::ptr::write_volatile(fd_array.add(k), tfd as u32) };
                    }
                }
                _ => {
                    // flat binder/handle objects (24 bytes).
                    // SAFETY: validated within the data area.
                    let obj = unsafe { &mut *(kvirt.add(block_off + obj_off) as *mut FlatBinderObject) };
                    if !translate_object(proc, obj, &target) {
                        fail_txn_after_fixups(&target, block_off, &installed_fds, target_node.as_ref(), reply, oneway, err_cmd);
                        crate::pr_info!(
                            "binder: object type {:#x} handle {:#x} translate failed",
                            obj.hdr_type,
                            obj.handle
                        );
                        return;
                    }
                }
            }
        }
    }

    // ---- queue the transaction ------------------------------------------
    // Linux stamps binder_txn.sender_pid with the sender's PROCESS id
    // (task_tgid_nr), not the thread id: OH's render_service validates
    // client-built node ids against GetCallingPid() (IsValidCallingPid:
    // ExtractPid(nodeId) == callingPid, where the client builds the id
    // from its own getpid()). Stamping the TID made every surface-node
    // CREATE from a worker thread fail "CREATE_NODE_AND_SURFACE invalid
    // nodeId[<tgid><<32|n>] pid[<tid>]" — the bootanimation could never
    // create its surface (r10). [rs-egl: BINDER LANE — route to
    // coordinator for ownership; one-line, verified live]
    let (sender_pid, sender_euid) = match crate::sched::current() {
        // SAFETY: current task is valid; a cred read here races at most
        // with a setuid on the same task, which is benign for IPC labels.
        Some(t) => ((*t).tgid() as i32, (*t).cred().euid),
        None => (0, 0),
    };
    let txn = Arc::new(BinderTxn {
        id: txn_id,
        from_proc: if !reply && !oneway { Some(proc.clone()) } else { None },
        from_tid: if !reply && !oneway { Some(thread.tid) } else { None },
        to_proc: target.clone(),
        to_tid: core::cell::UnsafeCell::new(None),
        target_node: target_node.clone(),
        holds_async_slot: core::cell::UnsafeCell::new(false),
        code: tr.code,
        flags: tr.flags,
        sender_pid,
        sender_euid,
        data_size,
        offsets_size,
        buffer_off: block_off,
        security_ctx,
        enq_j: crate::drivers::timer::get_jiffies() as u32,
        pick_j: core::cell::UnsafeCell::new(0),
    });

    // binder:binder_transaction — the transaction was built and is about
    // to land on its target. to_thread is 0 here (the receiving thread
    // is only chosen at delivery, the same value the reference records
    // at this point).
    crate::trace::trace_binder_transaction(
        txn.id,
        target_node.as_ref().map(|n| n.id).unwrap_or(0),
        target.pid as u32,
        0,
        reply,
        tr.code,
        tr.flags,
    );

    // sc155/156 (Session R): SA-table mutation tracer. Session Q closed
    // the park cause to "SA 501 (AppMgrService) never lands in samgr's
    // table" but could not see WHO fails to register it: APPTXN covers
    // only the app family, and samgr's own insert/not-found hilog lines
    // (I0/D0) are dropped by hilogd flow control long before the
    // console. This hook is binder-level and sender-agnostic: requests
    // carrying a samgr table-mutation code (3 ADD, 4 REMOVE, 10
    // ADD_ONDEMAND, 21 UNLOAD) whose parcel carries the samgr interface
    // token (decodes "oh..." UTF-16 — HDI interfaces use "OH"/"HDI"
    // prefixes, which burned the sc155 budget via powermgr/netmanager
    // noise) print with the first 64 parcel bytes (offline decode:
    // 8-byte OH parcel header, then string16 [u32 len][u16 chars], then
    // u32 saId). Replies from the latched samgr pid print the reply's
    // first u32 (the int32 result). Comparing the send count/content
    // with the reference Linux run's 102 "insert SA" lines decides
    // "registrar never sent" vs "samgr rejected/lost" for SA 501.
    {
        const SAMGR_ADD: u32 = 3;
        const SAMGR_REMOVE: u32 = 4;
        const SAMGR_ADD_ONDEMAND: u32 = 10;
        const SAMGR_UNLOAD: u32 = 21;
        // sc209 (leg 14): also trace LOAD (code 7). sc208's 15 s
        // SLOW-TXN storm was 22/24 foundation->samgr code-7 loads with
        // exec exactly 15000/30000/89390 ms — samgr's
        // StartDynamicSystemProcess waiting out the param client's 15 s
        // set-ack timeout for the "installs" on-demand process (SA 511)
        // that init never spawns. The parcel head (header + token + said)
        // names the target of every load offline.
        const SAMGR_LOAD: u32 = 7;
        // The samgr interface token starts UTF-16 "oh" at parcel offset
        // 12 (after the 8-byte OH parcel header + u32 string length).
        static SAREG_SAMGR_PID: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static SAREG_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let mut is_samgr_req = false;
        if !reply
            && matches!(
                tr.code,
                SAMGR_ADD | SAMGR_REMOVE | SAMGR_ADD_ONDEMAND | SAMGR_UNLOAD | SAMGR_LOAD
            )
        {
            let mut hdr = [0u8; 16];
            let ok = tr.data_buffer != 0
                && unsafe {
                    crate::arch::uaccess::copy_from_user(
                        hdr.as_mut_ptr(),
                        tr.data_buffer as *const u8,
                        hdr.len(),
                    ) == 0
                };
            // "oh" in UTF-16LE at offsets 12..16.
            if ok && &hdr[12..16] == &[0x6f, 0x00, 0x68, 0x00] {
                is_samgr_req = true;
                SAREG_SAMGR_PID.store(target.pid as u32, core::sync::atomic::Ordering::Relaxed);
                // leg-25 mirror for the SLRD read-side gate (see
                // binder_sareg_samgr_pid): samgr's leader comm matches
                // no app prefix, so the tracer keys on the pid.
                SAREG_SAMGR_PID_PUB.store(
                    target.pid as u32,
                    core::sync::atomic::Ordering::Relaxed,
                );
            }
        }
        let reply_to_mutation = reply
            && in_reply_to
                .as_ref()
                .map(|t| {
                    matches!(
                        t.code,
                        SAMGR_ADD
                            | SAMGR_REMOVE
                            | SAMGR_ADD_ONDEMAND
                            | SAMGR_UNLOAD
                            | SAMGR_LOAD
                    )
                })
                .unwrap_or(false)
            && proc.pid as u32
                == SAREG_SAMGR_PID.load(core::sync::atomic::Ordering::Relaxed);
        if is_samgr_req || reply_to_mutation {
            if SAREG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 600 {
                // leg-25: the enqueued txn's id, so the send line pairs
                // 1:1 with the SLRD read line (offline delivery audit).
                let t_id = txn.id;
                let comm = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                // sc209: 64 -> 96 bytes so LOAD parcels' SA id survives
                // past the (longer) samgr interface token.
                let mut pb = [0u8; 96];
                let mut pb_ok = false;
                if tr.data_buffer != 0 {
                    pb_ok = unsafe {
                        crate::arch::uaccess::copy_from_user(
                            pb.as_mut_ptr(),
                            tr.data_buffer as *const u8,
                            pb.len(),
                        ) == 0
                    };
                }
                let hex: alloc::string::String = if pb_ok {
                        pb.iter()
                            .map(|b| alloc::format!("{:02x}", b))
                            .collect()
                    } else {
                        alloc::string::String::from("unmapped")
                    };
                crate::pr_warn!(
                    "SAREG {} txn{} code={} from pid={} tgid={} comm='{}' -> pid={} dlen={} parcel={}",
                    if reply { "reply" } else { "send " },
                    t_id,
                    if reply {
                        in_reply_to.as_ref().map(|t| t.code).unwrap_or(0)
                    } else {
                        tr.code
                    },
                    proc.pid,
                    crate::process::current_pid(),
                    comm,
                    target.pid,
                    data_size,
                    hex
                );
            }
        }
    }

    // leg-30B (dfx=ams46): Gate-1 arsenal ENQ hook. Match the
    // IAbilityManager interface token on the sender's parcel head,
    // latch the AMS node id + foundation pid on first sight, and ring
    // every code-46 (StartUser) send for read/free/retire correlation.
    if ams46_on() && !reply {
        let mut hb = [0u8; 64];
        let tok_ok = tr.data_buffer != 0
            && unsafe {
                crate::arch::uaccess::copy_from_user(
                    hb.as_mut_ptr(),
                    tr.data_buffer as *const u8,
                    hb.len(),
                ) == 0
            };
        if tok_ok && ams46_token_match(&hb) {
            let nid = target_node.as_ref().map(|n| n.id).unwrap_or(0);
            if ams46_node_id() == 0 && nid != 0 {
                AMS46_NODE_ID.store(nid, core::sync::atomic::Ordering::Relaxed);
                AMS46_TPID.store(target.pid, core::sync::atomic::Ordering::Relaxed);
                crate::pr_warn!("AMS46 latch node={} pid={}", nid, target.pid);
            }
            if tr.code == 46 {
                ams46_txns_push(txn.id);
                if AMS46_ENQ_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32 {
                    let comm = crate::sched::current()
                        .map(|t| unsafe { (*t).comm() })
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let hex: alloc::string::String = hb[..48]
                        .iter()
                        .map(|b| alloc::format!("{:02x}", b))
                        .collect();
                    crate::pr_warn!(
                        "AMS46 ENQ txn{} from pid={} tid={} comm='{}' flags={:#x} dlen={} parcel={}",
                        txn.id,
                        proc.pid,
                        thread.tid,
                        comm,
                        tr.flags,
                        data_size,
                        hex
                    );
                }
            }
        }
    }

    // sc262 (leg-23): launcher lifecycle-path tracer. Every sc261-class
    // boot ends in the AMS restart loop — LIFECYCLE_TIMEOUT for
    // com.ohos.launcher.MainAbility ("load" phase, the same timeout
    // that hits settingsdata's DataExtAbility) while the parked
    // launcher looks healthy: main thread in poll, OS_IPC threads in
    // binder read, and hilog never logs a single ability-schedule
    // event. This prints BOTH directions of the launcher's binder
    // traffic at ENQUEUE time — its own sends and every transaction
    // queued TO it. Offline adjudication: an in-line printed here
    // while the app still times out = delivery/wake loss (kernel
    // read path); no in-line during the load window = AMS never sent
    // (AMS-side stall).
    //
    // sc262 READBACK CORRECTED THE LATCH: appspawn children NEVER open
    // /dev/binder — they send through the daemon's INHERITED fd, so
    // proc.pid is the DAEMON's (329) for every child, and the first
    // latch burned the 500-line budget on the daemon's whole proc.
    // v2: gate by the SENDER's tgid-leader comm (app-family child),
    // not by proc identity; in-lines additionally print the target
    // node's reg_tgid (the intended child, already known to APPTXN).
    // 1500/boot.
    {
        static SLTX_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // App-family child gate: the tgid LEADER's comm (OS_IPC/OS_FFRT
        // are generic worker names present in every service — matching
        // the reading thread's comm burned the read-side budget in one
        // boot).
        fn sltx_app_child(tgid: u32) -> bool {
            if tgid == 0 {
                return false;
            }
            let t = unsafe { crate::sched::find_task_by_pid(tgid) };
            if t.is_null() {
                return false;
            }
            let name = core::str::from_utf8(unsafe { (*t).comm() })
                .unwrap_or("")
                .trim_end_matches('\0');
            name.starts_with("m.ohos.")
                || name.starts_with("os.")
                || name.starts_with("com.")
                || name.starts_with("tonelibrarydata")
        }
        let sender_tgid = crate::sched::current()
            .map(|t| unsafe { (*t).tgid() as u32 })
            .unwrap_or(0);
        let sender_is_app = sltx_app_child(sender_tgid);
        let target_child = target_node
            .as_ref()
            .map(|n| n.reg_tgid)
            .filter(|&g| g != 0 && g != target.pid)
            .unwrap_or(0);
        if (sender_is_app || target_child != 0)
            && SLTX_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 1500
        {
            let mut pb = [0u8; 32];
            let hex: alloc::string::String = if tr.data_buffer != 0
                && unsafe {
                    crate::arch::uaccess::copy_from_user(
                        pb.as_mut_ptr(),
                        tr.data_buffer as *const u8,
                        pb.len(),
                    ) == 0
                }
            {
                pb.iter().map(|b| alloc::format!("{:02x}", b)).collect()
            } else {
                alloc::string::String::from("-")
            };
            crate::pr_warn!(
                "SLTX {} tgid={} pid={} -> pid={} child={} code={:#x} oneway={} dlen={} parcel={}",
                if sender_is_app { "out" } else { "in " },
                sender_tgid,
                proc.pid,
                target.pid,
                target_child,
                tr.code,
                oneway,
                data_size,
                hex
            );
        }
    }

    // sc184 (session R, the screen gate): composer-path tracer. The render
    // service never publishes SA 10 because its Init blocks in
    // DisplayComposerHdiImpl::Create's wait loop — the GetService("display_
    // composer_service") IPC into hdf_devmgr is delivered but never
    // answered (devmgr's IPC threads park on a frozen rwlock; SERVER-OWING
    // in every stall dump). This tracer prints (a) every transaction whose
    // parcel carries the UTF-16 service name (composer_host's add-service,
    // every client's GetService) plus the replies to those transactions,
    // and (b) every transaction issued BY the render_service process (its
    // whole binder conversation up to the block point — the last line
    // before silence names the never-answered hop). Latches: RS pid by
    // comm prefix "render_servi"; devmgr pid by the SAREG ADD of SA 5100.
    {
        static SACMP_RS_PID: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static SACMP_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        static SACMP_RS_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // Matched-transaction ring: txn ids whose send carried the name, so
        // the (much later) replies are printed too.
        static SACMP_TXNS: [core::sync::atomic::AtomicU64; 32] =
            [const { core::sync::atomic::AtomicU64::new(0) }; 32];

        if !reply {
            let comm_now = crate::sched::current()
                .map(|t| unsafe { (*t).comm() })
                .map(|c| core::str::from_utf8(c).unwrap_or(""))
                .unwrap_or("")
                .trim_end_matches('\0');
            if comm_now.starts_with("render_servi") {
                let prev = SACMP_RS_PID
                    .swap(proc.pid, core::sync::atomic::Ordering::Relaxed);
                if prev != proc.pid {
                    crate::pr_warn!("SACMP rs-latch pid={}", proc.pid);
                }
                if SACMP_RS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 120 {
                    let mut pb = [0u8; 32];
                    let hex: alloc::string::String = if tr.data_buffer != 0
                        && unsafe {
                            crate::arch::uaccess::copy_from_user(
                                pb.as_mut_ptr(),
                                tr.data_buffer as *const u8,
                                pb.len(),
                            ) == 0
                        }
                    {
                        pb.iter().map(|b| alloc::format!("{:02x}", b)).collect()
                    } else {
                        alloc::string::String::from("-")
                    };
                    crate::pr_warn!(
                        "SACMP-RS #{} pid={} -> pid={} code={:#x} oneway={} dlen={} parcel={}",
                        SACMP_RS_N.load(core::sync::atomic::Ordering::Relaxed),
                        proc.pid,
                        target.pid,
                        tr.code,
                        oneway,
                        data_size,
                        hex
                    );
                }
            }
            // hdf_devmgr announces itself by registering SA 5100 (0x13EC)
            // with samgr (code 3 = ADD_SYSTEM_ABILITY; parcel layout:
            // 8-byte header + string16 token + u32 said at offset 60).
            // Latch it for the FXDV frozen-rwlock tracer in futex.rs.
            if tr.code == 3 && tr.data_buffer != 0 {
                let mut sb = [0u8; 64];
                if unsafe {
                    crate::arch::uaccess::copy_from_user(
                        sb.as_mut_ptr(),
                        tr.data_buffer as *const u8,
                        sb.len(),
                    ) == 0
                } && sb[12] == 0x6f
                    && sb[13] == 0x00
                    && sb[14] == 0x68
                    && sb[15] == 0x00
                    && sb[60] == 0xec
                    && sb[61] == 0x13
                {
                    crate::sync::futex::fxdv_latch_devmgr(proc.pid);
                }
            }
        }

        // Name match on the first 192 parcel bytes, both encodings: the
        // samgr-family parcels carry UTF-16LE "display_composer_service"
        // while the HDI IServiceManager's GetService writes the service
        // name as ASCII bytes (sc184: RS's GetService parcel was 92 bytes
        // — too small for the UTF-16 form, so the ASCII form is what the
        // composer lookup actually uses). Match "display_co" in either.
        const NAME16: [u8; 20] = [
            0x64, 0x00, 0x69, 0x00, 0x73, 0x00, 0x70, 0x00, 0x6c, 0x00, 0x61,
            0x00, 0x79, 0x00, 0x5f, 0x00, 0x63, 0x00, 0x6f, 0x00,
        ];
        const NAME8: [u8; 10] = [
            0x64, 0x69, 0x73, 0x70, 0x6c, 0x61, 0x79, 0x5f, 0x63, 0x6f,
        ];
        let mut name_hit = false;
        if !reply && tr.data_buffer != 0 && data_size >= 4 {
            let mut buf = [0u8; 192];
            let n = core::cmp::min(buf.len(), data_size);
            if unsafe {
                crate::arch::uaccess::copy_from_user(
                    buf.as_mut_ptr(),
                    tr.data_buffer as *const u8,
                    n,
                ) == 0
            } {
                'outer: for i in 0..n.saturating_sub(NAME16.len()) {
                    for (k, &b) in NAME16.iter().enumerate() {
                        if buf[i + k] != b {
                            // sc184 follow-up: also try the ASCII form at
                            // the same offset (mutually exclusive encodings
                            // can never both match at i).
                            if i + NAME8.len() <= n {
                                for (j, &a) in NAME8.iter().enumerate() {
                                    if buf[i + j] != a {
                                        continue 'outer;
                                    }
                                }
                                name_hit = true;
                                break 'outer;
                            }
                            continue 'outer;
                        }
                    }
                    name_hit = true;
                    break;
                }
            }
        }
        let reply_to_named = reply
            && in_reply_to
                .as_ref()
                .map(|t| {
                    SACMP_TXNS.iter().any(|s| {
                        s.load(core::sync::atomic::Ordering::Relaxed) == t.id
                    })
                })
                .unwrap_or(false);
        if name_hit || reply_to_named {
            if name_hit {
                for s in SACMP_TXNS.iter() {
                    if s.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                        s.store(txn_id, core::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
            }
            if SACMP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 300 {
                let comm2 = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                crate::pr_warn!(
                    "SACMP {} code={} from pid={} comm='{}' -> pid={} dlen={} reply={}",
                    if name_hit { "name" } else { "reply" },
                    if reply {
                        in_reply_to.as_ref().map(|t| t.code).unwrap_or(0)
                    } else {
                        tr.code
                    },
                    proc.pid,
                    comm2,
                    target.pid,
                    data_size,
                    reply,
                );
            }
        }
    }

    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    // sc161: child-routed wakes — (proc, child tgid) for transactions
    // aimed at forked-child nodes; wake_proc_one prefers that child's
    // threads so the delivery is not eaten by the daemon's readers.
    let mut wakes_child: Vec<(Arc<BinderProc>, u32)> = Vec::new();
    // BC_REPLY whose originating thread vanished: fail after WORLD is
    // released (release_block/close_installed_fds must not run under it).
    let mut reply_target_gone = false;
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
                    // exact-thread wake (wake_up_interruptible_sync on
                    // target_thread->wait in Linux)
                    wake_thread(&rthread);
                    rthread.hist(EV_REPLY_PUSHED, txn.id as u32, *rtid);
                    // The original transaction is finished: unpin its node.
                    if let Some(orig) = &in_reply_to {
                        unpin_txn_node(world, orig);
                        // sc243 (leg-17): the replied txn's request
                        // buffer in the replier's alloc is
                        // protocol-dead — mark it reclaimable for
                        // sweep_stale_async (the receiver's BC_FREE may
                        // never come; cc54 family).
                        if let Some(a) = pm(world, &orig.to_proc).im().alloc.as_mut() {
                            if let Some(b) = a
                                .blocks
                                .iter_mut()
                                .find(|b| b.off == orig.buffer_off && !b.free)
                            {
                                b.reply_done = true;
                            }
                        }
                    }
                    wakes.push(tproc.clone());
                    wakes.push(proc.clone());
                } else {
                    *err_cmd = BR_DEAD_REPLY;
                    reply_target_gone = true;
                    thread.hist(
                        EV_REPLY_GONE,
                        in_reply_to.as_ref().map(|x| x.id as u32).unwrap_or(0),
                        *rtid,
                    );
                    // Unpin the original transaction's node while we still
                    // hold WORLD (unpin_txn_node expects it); the buffer
                    // and any installed fds are freed after the lock.
                    if let Some(orig) = &in_reply_to {
                        unpin_txn_node(world, orig);
                    }
                    crate::pr_info!(
                        "binder: BC_REPLY originator thread {} gone (proc {})",
                        rtid,
                        tproc.pid
                    );
                }
            } else if oneway {
                let th = thread.st();
                th.todo.push_back(Work::TransactionComplete);
                th.process_todo = true;
                // binder_proc_transaction: take the node's async slot or
                // park on node->async_todo (buffer already allocated and
                // fixed up; only the delivery is deferred).
                let mut parked = false;
                if let Some(node) = &target_node {
                    let ns = node.st();
                    if ns.has_async_transaction {
                        ns.async_todo.push_back(txn.clone());
                        parked = true;
                        thread.hist(EV_TXN_PARKED_ASYNC, txn.id as u32, ns.async_todo.len() as u32);
                        crate::pr_info!(
                            "binder: oneway txn {} parked on node {} (queue len {})",
                            txn.id,
                            node.id,
                            ns.async_todo.len()
                        );
                        // leg-30B AMS46: the traced StartUser parked
                        // behind an un-freed earlier oneway — delivery
                        // deferred indefinitely until the slot holder's
                        // buffer is freed (or the 30 s sweeps act).
                        if ams46_on() && ams46_txns_contains(txn.id) {
                            crate::pr_warn!(
                                "AMS46 Q txn{} PARKED on node{} qlen={} (slot held by earlier oneway)",
                                txn.id,
                                node.id,
                                ns.async_todo.len()
                            );
                        }
                    } else {
                        ns.has_async_transaction = true;
                    }
                }
                if !parked {
                    // SAFETY: under WORLD.
                    unsafe { txn.set_holds_async_slot(true) };
                    // sc163: child-aimed oneway — thread-target the
                    // child directly (children never enter the looper;
                    // see post_txn_to_child_thread).
                    let pref = target_node
                        .as_ref()
                        .map(|n| n.reg_tgid)
                        .filter(|&g| g != 0 && g != target.pid)
                        .unwrap_or(0);
                    let posted_thread = if pref != 0 {
                        post_txn_to_child_thread(world, &target, pref, &txn)
                    } else {
                        None
                    };
                    // sc167: name the oneway route too (the sync branch
                    // below carries route= — the AMS->child hop is mostly
                    // ONEWAY (ScheduleStartAbility family), so the queue
                    // verdict must be readable on both halves).
                    let txn_route_label = if posted_thread.is_some() {
                        "child-looper"
                    } else if pref != 0 {
                        "child-proctodo"
                    } else {
                        "proctodo"
                    };
                    match posted_thread {
                        Some(ct) => wake_ths.push(ct),
                        None => {
                            pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                            thread.hist(EV_TXN_PQUEUED, txn.id as u32, target.pid);
                            if pref != 0 {
                                wakes_child.push((target.clone(), pref));
                            } else {
                                wakes.push(target.clone());
                            }
                        }
                    }
                    // leg-30B AMS46: the traced StartUser took the async
                    // slot and is queued for delivery.
                    if ams46_on() && ams46_txns_contains(txn.id) {
                        crate::pr_warn!(
                            "AMS46 Q txn{} route={} (slot taken)",
                            txn.id,
                            txn_route_label
                        );
                    }
                    // sc140: oneway half of the APPTXN tracer (see the
                    // sync branch below for the rationale).
                    {
                        let comm = crate::sched::current()
                            .map(|t| unsafe { (*t).comm() })
                            .map(|c| core::str::from_utf8(c).unwrap_or(""))
                            .unwrap_or("")
                            .trim_end_matches('\0');
                        let fam = comm.contains("launcher")
                            || comm.contains("settingsdata")
                            || comm.contains("systemui")
                            || comm.contains("dataability")
                            || comm == "appspawn";
                        // sc157: see the sync branch — child-registered
                        // nodes print from any sender.
                        let to_child_node = target_node
                            .as_ref()
                            .map(|n| n.reg_tgid != 0 && n.reg_tgid != target.pid)
                            .unwrap_or(false);
                        if fam || app_child_pids_contains(target.pid) || to_child_node {
                            static TXN_NO: core::sync::atomic::AtomicUsize =
                                core::sync::atomic::AtomicUsize::new(0);
                            if TXN_NO.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 160 {
                                crate::pr_warn!(
                                    "APPTXN id={} from pid={} comm='{}' -> pid={} code={:#x} oneway=1 proc child={} route={}",
                                    txn.id,
                                    proc.pid,
                                    comm,
                                    target.pid,
                                    txn.code,
                                    target_node.as_ref().map(|n| n.reg_tgid).unwrap_or(0),
                                    txn_route_label
                                );
                            }
                        }
                    }
                }
                // The async buffer keeps its txn alive for the node-slot
                // release at BC_FREE_BUFFER (parked ones for close cleanup).
                if let Some(a) = pm(world, &target).im().alloc.as_mut() {
                    if let Some(b) = a.blocks.iter_mut().find(|b| b.off == block_off) {
                        b.txn = Some(txn.clone());
                    }
                }
                wakes.push(proc.clone());
            } else {
                // Sync: TRANSACTION_COMPLETE on the sender's todo (visible
                // to its next read via thread_has_work's todo check) +
                // push on the sender's txn stack + queue on the target.
                let th = thread.st();
                th.todo.push_back(Work::TransactionComplete);
                th.process_todo = true;
                // Linux 5.10 from-walk, BEFORE the new txn is stacked:
                // while this thread is handling a sync txn T whose
                // originator thread C lives in the target proc, a nested
                // sync call into that same proc is delivered to C's OWN
                // thread todo — C is parked in read waiting for T's reply
                // and takes thread work despite its txn stack — instead of
                // competing for the proc todo. This is also what makes
                // same-process transactions deadlock-free (nested samgr
                // calls unwind LIFO through the blocked originator).
                let mut t_thread: Option<Arc<BinderThread>> = None;
                for st in th.txn_stack.iter().rev() {
                    if let (Some(fp), Some(ftid)) = (&st.from_proc, st.from_tid) {
                        if Arc::ptr_eq(fp, &target) {
                            if let Some(t) =
                                pm(world, &target).im().threads.iter().find(|t| t.tid == ftid).cloned()
                            {
                                t_thread = Some(t);
                            }
                            break;
                        }
                    }
                }
                th.txn_stack.push(txn.clone());
                thread.hist(EV_TXN_SENT, txn.id as u32, target.pid);
                // POSTING FIRST (sc166): the tracer below names the route
                // this transaction actually took.
                let via_origin_thread = t_thread.is_some();
                let mut txn_route_label: &str = "origin-thread";
                if let Some(tt) = t_thread {
                    let tth = tt.st();
                    tth.todo.push_back(Work::Transaction(txn.clone()));
                    tth.process_todo = true;
                    tt.hist(EV_TXN_TQUEUED, txn.id as u32, tt.tid);
                    wake_ths.push(tt);
                    txn_route_label = "origin-thread";
                } else {
                    // sc163: sync txn to a forked-child node — thread-target
                    // a LOOPER-entered thread of the child (sc165: a plain
                    // client thread parked on the EventHandler futex never
                    // returns to a binder read). The gated proc todo +
                    // preferred wake is the fallback a child's FIRST
                    // IPCWorkThread read picks up.
                    let pref = target_node
                        .as_ref()
                        .map(|n| n.reg_tgid)
                        .filter(|&g| g != 0 && g != target.pid)
                        .unwrap_or(0);
                    let posted_thread = if pref != 0 {
                        post_txn_to_child_thread(world, &target, pref, &txn)
                    } else {
                        None
                    };
                    match posted_thread {
                        Some(ct) => {
                            wake_ths.push(ct);
                            txn_route_label = "child-looper";
                        }
                        None => {
                            pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                            thread.hist(EV_TXN_PQUEUED, txn.id as u32, target.pid);
                            if pref != 0 {
                                wakes_child.push((target.clone(), pref));
                                txn_route_label = "child-proctodo";
                            } else {
                                wakes.push(target.clone());
                                txn_route_label = "proctodo";
                            }
                        }
                    }
                }
                // sc140 (Session Q, launcher lane): the parked appspawn
                // children poll WRITE_READ forever with an EMPTY read —
                // the AMS ScheduleStartAbility transaction never reaches
                // them (or was never sent). One line per transaction
                // whose sender is app-family or whose target pid is a
                // registered forked appspawn child: names the last hop
                // that ever targeted a parked child.
                {
                    let comm = crate::sched::current()
                        .map(|t| unsafe { (*t).comm() })
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let fam = comm.contains("launcher")
                        || comm.contains("settingsdata")
                        || comm.contains("systemui")
                        || comm.contains("dataability")
                        || comm == "appspawn";
                    // sc157: deliveries to a node registered by a FORKED
                    // CHILD (reg_tgid != owner proc pid) print from ANY
                    // sender — the AMS→child ScheduleStartAbility hop. The
                    // per-node flag says which child the txn is for.
                    let to_child_node = target_node
                        .as_ref()
                        .map(|n| n.reg_tgid != 0 && n.reg_tgid != target.pid)
                        .unwrap_or(false);
                    if fam || app_child_pids_contains(target.pid) || to_child_node {
                        static TXN_N: core::sync::atomic::AtomicUsize =
                            core::sync::atomic::AtomicUsize::new(0);
                        if TXN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 160 {
                            crate::pr_warn!(
                                "APPTXN id={} from pid={} comm='{}' -> pid={} code={:#x} oneway={} {} child={} route={}",
                                txn.id,
                                proc.pid,
                                comm,
                                target.pid,
                                txn.code,
                                oneway as u8,
                                if via_origin_thread { "thr" } else { "proc" },
                                target_node.as_ref().map(|n| n.reg_tgid).unwrap_or(0),
                                txn_route_label
                            );
                        }
                    }
                }
                // leg-30B AMS46: defensive sync-side trace (the StartUser
                // send is TF_ASYNC-shaped; a twoway variant would land
                // here and its sender would park for the reply).
                if ams46_on() && ams46_txns_contains(txn.id) {
                    crate::pr_warn!(
                        "AMS46 Q txn{} SYNC route={}",
                        txn.id,
                        txn_route_label
                    );
                }
            }
        }
    }
    if reply_target_gone {
        release_block(&target, block_off);
        close_installed_fds(&installed_fds);
        return;
    }
    for t in wake_ths {
        wake_thread(&t);
    }
    for p in wakes {
        wake_proc_one(&p, 0);
    }
    for (p, g) in wakes_child {
        wake_proc_one(&p, g);
    }
}

/// Common failure path after object fixups began: free the block, close
/// fds already installed in the target (Linux binder_transaction_buffer_
/// release fd cleanup), unpin the node, BR_FAILED_REPLY.
fn fail_txn_after_fixups(
    target: &Arc<BinderProc>,
    block_off: usize,
    installed_fds: &[(u32, usize)],
    node: Option<&Arc<BinderNode>>,
    reply: bool,
    oneway: bool,
    err_cmd: &mut u32,
) {
    release_block(target, block_off);
    close_installed_fds(installed_fds);
    unpin_after_failure(target, node, reply, oneway);
    *err_cmd = BR_FAILED_REPLY;
}

/// Resolve the parent BINDER_TYPE_PTR object referenced by offsets-array
/// index from an FDA or a PTR fixup (binder_validate_ptr): the parent must
/// appear EARLIER in the offsets array (already fixed up) and be a PTR
/// object. Returns the parent's kernel-side buffer address (in our sg
/// area) and its length.
/// SAFETY: caller owns the target block exclusively (in-flight txn).
unsafe fn resolve_parent_ptr(
    kvirt: *mut u8,
    block_off: usize,
    block_user: u64,
    offsets_at: usize,
    data_size: usize,
    offsets_size: usize,
    sg_size: usize,
    cur_idx: usize,
    parent_idx: u64,
) -> Option<(*mut u8, usize)> {
    if parent_idx >= cur_idx as u64 {
        return None;
    }
    // SAFETY: offsets array inside the kernel-side block; index < cur_idx
    // bounds it within offsets_size (validated 8-aligned at resolve time).
    let parent_off =
        core::ptr::read_volatile((kvirt.add(block_off + offsets_at) as *const u64).add(parent_idx as usize)) as usize;
    if parent_off + core::mem::size_of::<BinderBufferObject>() > data_size {
        return None;
    }
    // SAFETY: validated within the data area.
    let p = core::ptr::read_volatile((kvirt.add(block_off + parent_off) as *const BinderBufferObject).cast_mut());
    if p.hdr_type != BINDER_TYPE_PTR {
        return None;
    }
    // The parent's .buffer was already rewritten to the target-user
    // address of its sg copy; map it back to the kernel address and bound
    // it inside the sg area.
    let extra_base = offsets_at + align8(offsets_size);
    let rel = p.buffer.checked_sub(block_user)? as usize;
    if rel < extra_base || rel + p.length as usize > extra_base + sg_size {
        return None;
    }
    Some((kvirt.add(block_off + rel), p.length as usize))
}

/// Undo the pins taken at resolve time when the transaction failed after
/// them.
fn unpin_after_failure(
    _target: &Arc<BinderProc>,
    node: Option<&Arc<BinderNode>>,
    reply: bool,
    oneway: bool,
) {
    let Some(node) = node else { return };
    let _w = world_lock();
    // SAFETY: under WORLD.
    unsafe {
        let ns = node.st();
        if !reply && !oneway && ns.internal_strong_refs > 0 {
            ns.internal_strong_refs -= 1;
        }
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

/// binder_translate_fd: install a sender fd into the TARGET process's fd
/// table (fget in the sender, get_unused_fd + fd_install on the target
/// opener task). Runs in the sender's ioctl context and NEVER under WORLD
/// — fd-table locks and file close ops may nest back into binder code.
/// Returns the new target fd, or None (invalid fd / target gone / full).
fn translate_fd_into(sender_fd: u32, target: &Arc<BinderProc>) -> Option<usize> {
    let cur = crate::sched::current()?;
    let sender_fdt = cur.try_fdtable()?;
    let file = sender_fdt.get_file(sender_fd as usize)?;
    let ttask = crate::process::pid_hash::pid_hash_lookup_pinned(target.pid);
    if ttask.is_null() {
        return None;
    }
    // SAFETY: pinned task pointer (task_refcnt held by the lookup).
    let r = unsafe {
        match (*ttask).try_fdtable().and_then(|t| t.alloc_fd()) {
            Some(fd) => {
                let fdt = (*ttask).try_fdtable().unwrap();
                if fdt.install_fd(fd, file).is_err() {
                    // Return the slot (cannot realistically fail on a
                    // just-allocated fd, but do not leak it).
                    let _ = fdt.close_fd(fd);
                    None
                } else {
                    Some(fd)
                }
            }
            None => None,
        }
    };
    // SAFETY: drop the pin taken by pid_hash_lookup_pinned.
    unsafe { crate::process::task::Task::task_put(ttask) };
    r
}

/// Close fds installed into target processes when their transaction
/// failed after the installs (binder_transaction_buffer_release's
/// fd cleanup loop). NEVER under WORLD: close_fd runs file close ops.
fn close_installed_fds(installed: &[(u32, usize)]) {
    for (pid, fd) in installed {
        let t = crate::process::pid_hash::pid_hash_lookup_pinned(*pid);
        if t.is_null() {
            continue;
        }
        // SAFETY: pinned task pointer.
        unsafe {
            if let Some(fdt) = (*t).try_fdtable() {
                let _ = fdt.close_fd(*fd);
            }
            crate::process::task::Task::task_put(t);
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
                    // reg_tgid must be the registering THREAD's TGID
                    // (sc161 bug: current_pid() is the per-task tid, so
                    // worker-registered nodes got reg_tgid=worker_tid and
                    // the ownership gate rejected every reader).
                    let reg_tgid = crate::sched::current()
                        .map(|t| unsafe { (*t).tgid() } as u32)
                        .unwrap_or(0);
                    // sc167 node-identity parity: on Linux every forked
                    // child is a FRESH binder proc with its own node
                    // space, so two byte-identical siblings can each own
                    // a stub at the same VA. In THIS shared proc the node
                    // key is therefore (ptr, reg_tgid), not ptr alone —
                    // a bare-ptr lookup would hand sibling B's
                    // registration to sibling A's live node (and route
                    // B's incoming transactions into A, the cross-child
                    // black hole), or to a dead predecessor's node.
                    let node = {
                        let sp = pm(world, sender);
                        match sp
                            .im()
                            .nodes
                            .iter()
                            .find(|n| n.ptr == ptr && n.reg_tgid == reg_tgid)
                            .cloned()
                        {
                            Some(n) => n,
                            None => {
                                let n = Arc::new(BinderNode::new(
                                    world.next_id,
                                    ptr,
                                    obj.cookie,
                                    sender.id,
                                    reg_tgid,
                                    obj.flags,
                                ));
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
                BINDER_TYPE_FD | BINDER_TYPE_FDA | BINDER_TYPE_PTR => {
                    // Handled by the fd/sg machinery in binder_transaction
                    // BEFORE the world lock (fd-table ops never run under
                    // WORLD). Reaching here means a re-parse bug.
                    crate::pr_info!("binder: object type {:#x} reached node translation", obj.hdr_type);
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
        wake_proc_one(&owner, 0);
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
        death: None,
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
/// sc161 child-tgid ownership: may THIS thread (of `tgid`) consume the
/// given proc-todo work item? A transaction aimed at a node registered by
/// a forked child (reg_tgid set and != owner pid) belongs to that child's
/// threads only — the daemon's own reader threads must leave it queued or
/// every AMS→child delivery is stolen and dies in the daemon's
/// ProcessSkeleton lookup (the sc156 park).
/// SAFETY: caller must hold WORLD (reads txn target nodes).
unsafe fn work_readable_by(w: &Work, tgid: u32, proc: &BinderProc) -> bool {
    match w {
        Work::Transaction(t) => match t.target_node.as_ref() {
            Some(n) => n.reg_tgid == 0 || n.reg_tgid == proc.pid || n.reg_tgid == tgid,
            // Replies never sit on the proc todo (thread-targeted), but
            // treat any such item as unowned rather than blocking it.
            None => true,
        },
        _ => true,
    }
}

unsafe fn thread_has_work(world: &World, proc: &Arc<BinderProc>, thread: &Arc<BinderThread>) -> bool {
    let th = thread.st();
    // Own todo non-empty = binder_has_thread_work (Linux): covers work
    // pushed without an explicit wake, e.g. the deferred sync-send
    // TRANSACTION_COMPLETE that must reach the sender before it sleeps
    // awaiting the reply.
    if th.process_todo || !th.todo.is_empty() {
        return true;
    }
    let avail_for_proc = th.txn_stack.is_empty()
        && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
    // sc161: only items this thread OWNS count as work for it (child-tgid
    // routing) — otherwise a daemon reader sees "work available", pops,
    // and would steal a child's transaction (or spin on requeue).
    avail_for_proc && {
        let pim = pm(world, proc);
        pim.im().todo.iter().any(|w| work_readable_by(w, thread.tgid, pim))
    }
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

    // Pool-census note (dfx-gated, leg-33B): whether any iteration's
    // done:-shaped spawn emission fired, for the rate-capped POOL-SPAWN
    // proof print after the WORLD guard drops.
    let mut pool_spawn_fired = false;

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
                // The binder:binder_wait_for_work tracepoint's three
                // availability words, captured under the same WORLD
                // lock that sets the park state (the reference reads
                // them under binder_inner_proc_lock):
                //   proc_work        = binder_available_for_proc_work_
                //                     ilocked (empty txn stack + empty
                //                     todo + entered/registered looper)
                //   transaction_stack = a txn is stacked on this thread
                //   thread_todo       = work sits on the thread's list
                let mut t_proc_work = false;
                let mut t_txn_stack = false;
                let mut t_thread_todo = false;
                let t_armed = crate::trace::EVENT_BINDER_WAIT_FOR_WORK.armed();
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    // waiting_threads membership: only a thread that can
                    // actually take proc work registers for wakeups
                    // (binder_available_for_proc_work_ilocked). A reader
                    // without looper flags or with a stacked txn must not
                    // — it would eat wake_proc_one's single pick.
                    let th = thread.st();
                    th.waiting_for_proc_work = th.todo.is_empty()
                        && th.txn_stack.is_empty()
                        && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
                    if t_armed {
                        t_proc_work = th.waiting_for_proc_work;
                        t_txn_stack = !th.txn_stack.is_empty();
                        t_thread_todo = !th.todo.is_empty();
                    }
                    thread.hist(EV_PARK, th.txn_stack.len() as u32, 0);
                }
                drop(_w);
                // Recorded outside WORLD (the ring takes its own locks);
                // the reference records before the blocking wait, which
                // this is.
                if t_armed {
                    crate::trace::record(
                        &crate::trace::EVENT_BINDER_WAIT_FOR_WORK,
                        [
                            t_proc_work as u64,
                            t_txn_stack as u64,
                            t_thread_todo as u64,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ],
                    );
                }
            }
            thread
                .read_wait_since
                .store(crate::drivers::timer::get_jiffies(), core::sync::atomic::Ordering::Release);
            thread.stall_reported.store(false, core::sync::atomic::Ordering::Release);
            thread.rekicked.store(false, core::sync::atomic::Ordering::Release);
            thread.rekick_count.store(0, core::sync::atomic::Ordering::Release);
            thread.kick_wake_ok.store(0, core::sync::atomic::Ordering::Release);
            thread.kick_wake_refused.store(0, core::sync::atomic::Ordering::Release);
            let r = crate::wait_event_interruptible!(&thread.wait, {
                let w = world_lock();
                // SAFETY: under WORLD.
                unsafe { thread_has_work(w.as_ref().unwrap(), proc, thread) }
            });
            thread.read_wait_since.store(0, core::sync::atomic::Ordering::Release);
            {
                let _w = world_lock();
                // SAFETY: under WORLD.
                unsafe {
                    thread.st().waiting_for_proc_work = false;
                    thread.hist(EV_WAKE_RET, r as u32, 0);
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
                    // Budget covers BR_TRANSACTION_SECCTX (transaction
                    // data + trailing secctx pointer) even when unused.
                    if staged.len() + 4 + core::mem::size_of::<BinderTransactionData>() + 8 > read_size {
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
                        // sc161 child-tgid routing: take the FIRST item
                        // this thread owns; leave child-aimed transactions
                        // (reg_tgid != this tgid) queued for their child
                        // instead of stealing them into the daemon's
                        // ProcessSkeleton dead-end.
                        let pim = pm(world, proc);
                        match pim
                            .im()
                            .todo
                            .iter()
                            .position(|w| work_readable_by(w, thread.tgid, pim))
                        {
                            Some(i) => pim.im().todo.remove(i),
                            None => None,
                        }
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
                            thread.hist(EV_TC_RECV, 0, 0);
                        }
                        Work::ReturnError(cmd, param) => {
                            stage_cmd!(cmd);
                            if cmd == BR_ERROR {
                                staged.extend_from_slice(&(param as u32).to_ne_bytes());
                            }
                            thread.hist(EV_ERR_RECV, cmd, param as u32);
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
                        Work::DeadBinder { desc, cookie } => {
                            stage_cmd!(BR_DEAD_BINDER);
                            stage_u64!(cookie);
                            // Mark delivered on the ref (move onto the
                            // delivered_death list).
                            if let Some(r) = pm(world, proc).im().refs.iter_mut().find(|r| r.desc == desc) {
                                if let Some(d) = r.death.as_mut() {
                                    if d.cookie == cookie {
                                        d.phase = DeathPhase::Delivered;
                                    }
                                }
                            }
                            // Linux "goto done": a death notification can
                            // make userspace issue transactions at once —
                            // stop reading more work in this round.
                            break;
                        }
                        Work::ClearDeathDone { cookie } => {
                            stage_cmd!(BR_CLEAR_DEATH_NOTIFICATION_DONE);
                            stage_u64!(cookie);
                        }
                        Work::Transaction(t) => {
                            let is_reply = t.target_node.is_none();
                            if is_reply {
                                thread.hist(EV_REPLY_RECV, t.id as u32, 0);
                            } else {
                                thread.hist(
                                    EV_TXN_RECV,
                                    t.id as u32,
                                    t.from_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                                );
                            }
                            // binder:binder_transaction_received — the
                            // transaction is being read out to its
                            // receiving thread (BR_TRANSACTION/BR_REPLY).
                            crate::trace::trace_binder_transaction_received(t.id);
                            // leg-30B (dfx=ams46): read-side trace — a
                            // foundation thread picked up the traced
                            // code-46 StartUser. This line is the
                            // kernel-delivery-COMPLETE marker: everything
                            // lost past it is foundation userspace.
                            if ams46_on() && ams46_txns_contains(t.id) {
                                let name = crate::sched::current()
                                    .map(|ct| unsafe { (*ct).comm() })
                                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                                    .unwrap_or("")
                                    .trim_end_matches('\0');
                                crate::pr_warn!(
                                    "AMS46 READ txn{} by pid={} tid={} comm='{}' code={} reply={}",
                                    t.id,
                                    proc.pid,
                                    thread.tid,
                                    name,
                                    t.code,
                                    is_reply
                                );
                            }
                            // sc262 (leg-23): SLTX read-side — an
                            // app-family child's thread picked this
                            // transaction up out of its queue. An
                            // enqueue-side "SLTX in child=X" line with
                            // no matching read here, while the app
                            // still parks, names the wake/read path as
                            // the loss point. sc262 v1 burned its budget
                            // on samgr/OS_IPC noise (OS_IPC/OS_FFRT are
                            // generic worker names in EVERY service)
                            // and required tgid == proc.pid, which can
                            // never hold for children reading through
                            // the daemon's INHERITED fd; v2 gates on
                            // the tgid LEADER's comm only.
                            // leg-25 (sc276): samgr's own reads were
                            // invisible (comm 'samgr' matches no app
                            // prefix) exactly where the on-demand
                            // unload question lives — samgr killing
                            // services 13 ms after releases while
                            // clients are mid-call needs the
                            // Add/Remove (SAREG code 3/4) delivery
                            // record on samgr's side. Gate on the
                            // SAREG-known samgr pid too.
                            if let Some(cur) = crate::sched::current() {
                                let tg = unsafe { (*cur).tgid() as u32 };
                                if tg != 0 {
                                    let lt = unsafe { crate::sched::find_task_by_pid(tg) };
                                    if !lt.is_null() {
                                        let lname = core::str::from_utf8(unsafe { (*lt).comm() })
                                            .unwrap_or("")
                                            .trim_end_matches('\0');
                                        let samgr_pid = binder_sareg_samgr_pid();
                                        if lname.starts_with("m.ohos.")
                                            || lname.starts_with("os.")
                                            || lname.starts_with("com.")
                                            || (samgr_pid != 0 && tg == samgr_pid)
                                        {
                                            static SLRD_N: core::sync::atomic::AtomicUsize =
                                                core::sync::atomic::AtomicUsize::new(0);
                                            if SLRD_N
                                                .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                                                < 1500
                                            {
                                                let name = core::str::from_utf8(unsafe { (*cur).comm() })
                                                    .unwrap_or("")
                                                    .trim_end_matches('\0');
                                                crate::pr_warn!(
                                                    "SLRD tgid={} tid={} comm='{}' read txn id={} code={:#x} reply={}",
                                                    tg,
                                                    thread.tid,
                                                    name,
                                                    t.id,
                                                    t.code,
                                                    t.target_node.is_none()
                                                );
                                            }
                                        }
                                    }
                                }
                            }
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
                            let sec = !is_reply && t.security_ctx != 0;
                            stage_cmd!(if is_reply {
                                BR_REPLY
                            } else if sec {
                                BR_TRANSACTION_SECCTX
                            } else {
                                BR_TRANSACTION
                            });
                            // SAFETY: BinderTransactionData is Copy + repr(C).
                            staged.extend_from_slice(core::slice::from_raw_parts(
                                &td as *const BinderTransactionData as *const u8,
                                core::mem::size_of::<BinderTransactionData>(),
                            ));
                            if sec {
                                stage_u64!(t.security_ctx);
                                // First few deliveries logged: proves the
                                // SECCTX path is live and shows the sid the
                                // receiver will parse (SA-gate forensics).
                                static SECCTX_DELIVERED: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                use core::sync::atomic::Ordering::Relaxed;
                                if SECCTX_DELIVERED.fetch_add(1, Relaxed) < 5 {
                                    crate::pr_info!(
                                        "binder: BR_TRANSACTION_SECCTX -> pid {} sid '{}' code {}",
                                        proc.pid, SECCTX_SID, td.code
                                    );
                                }
                            }
                            // The receiving side now owns the buffer.
                            if let Some(a) = t.to_proc.im().alloc.as_mut() {
                                if let Some(b) = a.blocks.iter_mut().find(|b| b.off == t.buffer_off) {
                                    b.allow_user_free = true;
                                }
                            }
                            if !is_reply && t.flags & TF_ONE_WAY == 0 {
                                // The receiver stacks it for its BC_REPLY.
                                t.set_to_tid(thread.tid);
                                // sc208 SLOW-TXN probe: server-pickup stamp.
                                unsafe {
                                    *t.pick_j.get() =
                                        crate::drivers::timer::get_jiffies() as u32;
                                }
                                let recv_id = t.id as u32;
                                thread.st().txn_stack.push(t);
                                // sc79 ROOT CAUSE (the impossible shape):
                                // END the read after delivering ONE sync
                                // transaction. OH libipc consumes ONE
                                // command per TransactWithDriver loop
                                // iteration; if a second BR_TRANSACTION
                                // (typically a from-walk redirect already
                                // sitting on this thread's todo) is staged
                                // in the SAME read, the kernel stacks both
                                // while userspace has dispatched only the
                                // first — the first handler's nested calls
                                // then make its BC_REPLY pop the WRONG
                                // stack top (the undelivered-to-userspace
                                // txn): replies cross, counters desync,
                                // and the thread strands in a read owing a
                                // reply with no outstanding nested txn
                                // (sc79 ring: TXN_RECV(12919)@11372
                                // TXN_RECV(12932)@11372 same jiffy, then
                                // the SERVER-OWING park; sc74/77/78/79
                                // binder-stall wedge family). Linux
                                // binder_thread_read returns after one
                                // transaction delivery — mirror it.
                                thread.hist(EV_TXN_RECV_END, recv_id, 1);
                                break;
                            }
                        }
                    }
                }
                // Linux done: BR_SPAWN_LOOPER rides on EVERY returning
                // read — including reads that delivered a transaction or
                // a reply (binder_thread_read's done: label in 5.15 has
                // no "no work" gate). sc329B root-cause: the old
                // `!got_transaction` gate meant the request could only be
                // issued from an idle no-work return, which by definition
                // happens only when threads are idle — under nested-call
                // load every read returns WITH work (or parks as a
                // client), so the pool never grew past its boot-time size
                // while it was being consumed; when the last reader
                // parked as a client, proc todo grew unboundedly
                // (TXN-HANG 91.5 s family into foundation pid 799).
                //
                // The pool gate mirrors the Linux done: condition exactly
                // (5.15 binder.c ~4204): requested_threads == 0, no
                // waiting worker (waiting_threads empty), and
                // requested_threads_started < max_threads — the STARTED
                // count (spawned-and-REGISTERED loopers), never
                // `threads.len()`: every client-only thread of the proc
                // owns a BinderThread entry (BC_TRANSACTION from an FFRT
                // worker, a spawned child, ...) and those must not
                // consume the service's max_threads budget. Linux never
                // decrements started on thread exit; Rux matches. The
                // returning thread must itself be a looper
                // (REGISTERED | ENTERED) as in Linux — a client thread's
                // reply-return does not request spawns there.
                {
                    let pim = pm(world, proc).im();
                    // waiting_for_proc_work now implies the full
                    // availability predicate (todo/txn_stack empty +
                    // looper flags), i.e. waiting_threads membership.
                    let ready = pim.threads.iter().any(|t| t.st().waiting_for_proc_work);
                    // sc164: forked children sharing this proc count as
                    // their OWN pool (on Linux each child IS a fresh
                    // proc): a child read must emit BR_SPAWN_LOOPER when
                    // THAT CHILD has no waiting worker — the proc-wide
                    // counters are polluted by the daemon's own pool and
                    // every other child's threads, which starved the
                    // children of reader threads (OH spawns IPCWorkThread
                    // in the child exactly on this command; without it no
                    // child thread ever blocks in a binder read and
                    // AMS->child deliveries have no reader).
                    let is_child = thread.tgid != 0 && thread.tgid != proc.pid as u32;
                    let looper_worker = |t: &Arc<BinderThread>| {
                        (t.st().looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0
                    };
                    let emit_spawn = if is_child && pim.max_threads > 0 {
                        let child_ready = pim
                            .threads
                            .iter()
                            .any(|t| t.tgid == thread.tgid && t.st().waiting_for_proc_work);
                        let child_n = pim
                            .threads
                            .iter()
                            .filter(|t| t.tgid == thread.tgid && looper_worker(t))
                            .count();
                        !child_ready && child_n < pim.max_threads as usize
                    } else {
                        pim.requested_threads == 0
                            && !ready
                            && pim.max_threads > 0
                            && pim.requested_threads_started < pim.max_threads
                            && (thread.st().looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0
                    };
                    // sc79 discipline: BR_SPAWN_LOOPER is a no-payload
                    // command — riding it AFTER the (at most one) sync
                    // transaction never stacks a second userspace-
                    // pending command, unlike the double-BR_TRANSACTION
                    // shape that crossed replies in sc79.
                    if emit_spawn && staged.len() + 4 <= read_size {
                        if !is_child {
                            pim.requested_threads += 1;
                        }
                        POOL_SPAWN_EVENTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        pool_spawn_fired = true;
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

    // POOL-SPAWN proof print (dfx=poolcensus): one line per emission,
    // rate-capped, OUTSIDE the WORLD guard. Names the proc whose read
    // carried the BR_SPAWN_LOOPER request — the live counterpart of the
    // census lines.
    if pool_spawn_fired && pool_census_on() {
        static SPAWN_PRINTED: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        use core::sync::atomic::Ordering::Relaxed;
        if SPAWN_PRINTED.fetch_add(1, Relaxed) < 40 {
            crate::pr_warn!(
                "POOL-SPAWN pid {} tid {} carried BR_SPAWN_LOOPER (events {})",
                proc.pid,
                thread.tid,
                POOL_SPAWN_EVENTS.load(Relaxed)
            );
        }
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

/// DFX: one-line-per-proc binder world snapshot for the UART magic dump.
/// IRQ context: try-lock only — if WORLD is contended the binder state is
/// mid-transition and the dump skips it (the task dump still runs).
pub fn binder_dfx_dump() {
    let Some(mut g) = WORLD.try_lock_irqsave() else {
        crate::pr_info!("binder-dfx: world busy, skipped");
        return;
    };
    let Some(world) = g.as_ref() else {
        return;
    };
    // SAFETY: read-only under WORLD (the access contract).
    unsafe {
        for p in &world.procs {
            let pim = p.im();
            crate::pr_info!(
                "binder-dfx: proc {} pid {} dead={} todo={} refs={} mgr={}",
                p.id,
                p.pid,
                pim.is_dead,
                pim.todo.len(),
                pim.refs.len(),
                (0..3).any(|c| world.mgr_node[c].as_ref().map(|m| m.proc_id).unwrap_or(0) == p.id)
            );
            for t in &pim.threads {
                let th = t.st();
                crate::pr_info!(
                    "binder-dfx:   tid {} looper={:#x} todo={} process_todo={} wfpw={} stack={} deathq={}",
                    t.tid,
                    th.looper,
                    th.todo.len(),
                    th.process_todo,
                    th.waiting_for_proc_work,
                    th.txn_stack.len(),
                    pim.todo.iter().filter(|wk| matches!(wk, Work::DeadBinder { .. })).count()
                );
                for (i, wk) in th.todo.iter().enumerate() {
                    let desc = match wk {
                        Work::TransactionComplete => "tcomplete".into(),
                        Work::Transaction(_) => "txn".into(),
                        Work::ReturnError(c, _) => alloc::format!("err#{:#x}", c),
                        Work::Node(_) => "node".into(),
                        Work::DeadBinder { desc, cookie } => alloc::format!("dead d{} c{:#x}", desc, cookie),
                        Work::ClearDeathDone { cookie } => alloc::format!("cleardone c{:#x}", cookie),
                    };
                    crate::pr_info!("binder-dfx:     thtodo[{}] {}", i, desc);
                }
            }
            for (i, wk) in pim.todo.iter().enumerate() {
                let desc = match wk {
                    Work::TransactionComplete => "tcomplete".into(),
                    Work::Transaction(_) => "txn".into(),
                    Work::ReturnError(c, _) => alloc::format!("err#{:#x}", c),
                    Work::Node(_) => "node".into(),
                    Work::DeadBinder { desc, cookie } => alloc::format!("dead d{} c{:#x}", desc, cookie),
                    Work::ClearDeathDone { cookie } => alloc::format!("cleardone c{:#x}", cookie),
                };
                crate::pr_info!("binder-dfx:     ptodo[{}] {}", i, desc);
            }
        }
    }
}
