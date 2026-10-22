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
    /// sg (extra) area size: BINDER_TYPE_PTR payload copies (layout:
    /// [data][offsets][extra], each u64-aligned).
    extra_size: usize,
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
            self.blocks.push(BufBlock {
                off: cand,
                len: need,
                free: false,
                async_block: true,
                allow_user_free: false,
                data_size,
                offsets_size,
                extra_size,
                txn: None,
            });
            self.blocks.sort_by_key(|b| b.off);
            self.free_async_space -= need;
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
        if let Some(b) = self.blocks.iter_mut().find(|b| b.off == o) {
            b.free = false;
            b.async_block = false;
            b.allow_user_free = false;
            b.data_size = data_size;
            b.offsets_size = offsets_size;
            b.extra_size = extra_size;
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
                    txn: None,
                };
                b.len = need;
                self.blocks.push(rest);
                self.blocks.sort_by_key(|b| b.off);
            }
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
        let es = self.blocks[idx].extra_size;
        let txn = self.blocks[idx].txn.take();
        self.blocks[idx].free = true;
        self.blocks[idx].allow_user_free = false;
        if was_async {
            self.free_async_space += Self::block_need(ds, os, es);
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
    /// BINDER_WORK_DEAD_BINDER(_AND_CLEAR): (ref desc, cookie). The node
    /// died; deliver BR_DEAD_BINDER.
    DeadBinder { desc: u32, cookie: u64 },
    /// BINDER_WORK_CLEAR_DEATH_NOTIFICATION: deliver
    /// BR_CLEAR_DEATH_NOTIFICATION_DONE (cookie).
    ClearDeathDone { cookie: u64 },
}

// sc140 (Session Q): registry of forked appspawn child pids (written by
// the clone census at each appspawn fork, first 128 per boot). The
// APPTXN tracer prints any binder transaction whose TARGET is one of
// these — the AMS→child Schedule* delivery is the suspected missing hop
// behind the launcher-family park.
static APP_CHILD_PIDS: [core::sync::atomic::AtomicU32; 128] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 128];

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
    t.wait.wake_up(crate::process::wait::WakeUpHint::Normal, 0);
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
    !world.procs.iter().any(|p| p.id == node.proc_id)
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
    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    {
        let mut w = world_lock();
        let Some(world) = w.as_mut() else { return };
        // SAFETY: under WORLD.
        unsafe {
            // Membership: OH opens /dev/binder on the group leader
            // (proc.pid == tgid); a non-leader opener is matched by its
            // own pid so its personal pool entry is still reaped.
            let proc = world
                .procs
                .iter()
                .find(|p| p.pid == dying_tgid || p.pid == dying_pid)
                .cloned();
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
        let vma = Vma::new(
            crate::mm::page::VirtAddr::new(b),
            crate::mm::page::VirtAddr::new(b + size),
            vma_flags,
        );
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
        txn: None,
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
    let top = tk.kernel_stack_top();
    #[cfg(feature = "x86_64")]
    let (rip, sys, a0, a1) = {
        let regs = (top.wrapping_sub(
            crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
        )) as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's own entry
        // frame; the kernel stack stays mapped while the Task
        // lives, and the frame slot is written on every entry.
        unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi) }
    };
    #[cfg(feature = "riscv64")]
    let (rip, sys, a0, a1) = {
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
                        let otop = ot.kernel_stack_top();
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
            let top = tk.kernel_stack_top();
            #[cfg(feature = "x86_64")]
            let (rip, sys, a0, a1) = {
                let regs = (top.wrapping_sub(
                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's own entry frame;
                // the kernel stack stays mapped while the Task lives.
                unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi) }
            };
            #[cfg(feature = "riscv64")]
            let (rip, sys, a0, a1) = {
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
            crate::pr_info!(
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
    let task = crate::sched::find_task_by_pid(t.tid);
    if task.is_null() {
        return;
    }
    let tk = &*task;
    let Some(aspace) = tk.address_space() else { return };
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
    let top = tk.kernel_stack_top();
    #[cfg(feature = "x86_64")]
    let (rsp, rip) = {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of the holder's entry frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract, riscv64.
        unsafe { ((*regs).sp, (*regs).epc) }
    };
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    let (rsp, rip) = (0u64, 0u64);
    crate::pr_warn!(
        "binder-stall:   PARK-STACK tid {} rip={:#x} rsp={:#x}",
        t.tid,
        rip,
        rsp
    );
    // 16 candidate frame words above rsp; each that lands in a mapped
    // exec-looking VMA prints with the file offset for offline symbolize.
    let vmas = aspace.vma_read();
    for i in 0..16u64 {
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
                "binder-stall:   ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} bytes={:#018x}",
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
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts are only used
        // to reach each live proc's thread list).
        unsafe {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
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
                                let _ = writeln!(
                                    out,
                                    "binder-stall: WAKE-RUN-LEAK tid {} kicked {}x age {}s state={:#x} cfs_linked={} on_cpu={} pick_state={} cpu={}",
                                    t.tid,
                                    kicks + 1,
                                    age / hz,
                                    tk.state().bits(),
                                    linked,
                                    tk.on_cpu(),
                                    tk.pick_state(),
                                    tk.ti_cpu()
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
    for p in retodo {
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
                    let t = Arc::new(BinderThread::new(tid, crate::process::current_pid() as u32));
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
                    let mut drain_wake: Option<Arc<BinderProc>> = None;
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
                                    if t.holds_async_slot() {
                                        let mut next: Option<Arc<BinderTxn>> = None;
                                        if let Some(node) = &t.target_node {
                                            let ns = node.st();
                                            next = ns.async_todo.pop_front();
                                            if next.is_none() {
                                                ns.has_async_transaction = false;
                                            }
                                        }
                                        if let Some(h) = next {
                                            h.set_holds_async_slot(true);
                                            let tp = h.to_proc.clone();
                                            let tid = h.id;
                                            let tpid = tp.pid;
                                            pm(world, &tp).im().todo.push_back(Work::Transaction(h));
                                            drain_wake = Some(tp);
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
                    if let Some(tp) = drain_wake {
                        wake_proc_one(&tp, 0);
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
            let off = pm(world, tp)
                .im()
                .alloc
                .as_mut()
                .unwrap()
                .new_buf(data_size, offsets_size, sg_total, is_async);
            let Some(off) = off else {
                *err_cmd = BR_FAILED_REPLY;
                crate::pr_info!(
                    "binder: buffer alloc failed (need {}+{}+{})",
                    align8(data_size),
                    align8(offsets_size),
                    align8(sg_size)
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
    });

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
        // The samgr interface token starts UTF-16 "oh" at parcel offset
        // 12 (after the 8-byte OH parcel header + u32 string length).
        static SAREG_SAMGR_PID: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static SAREG_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let mut is_samgr_req = false;
        if !reply
            && matches!(tr.code, SAMGR_ADD | SAMGR_REMOVE | SAMGR_ADD_ONDEMAND | SAMGR_UNLOAD)
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
            }
        }
        let reply_to_mutation = reply
            && in_reply_to
                .as_ref()
                .map(|t| {
                    matches!(
                        t.code,
                        SAMGR_ADD | SAMGR_REMOVE | SAMGR_ADD_ONDEMAND | SAMGR_UNLOAD
                    )
                })
                .unwrap_or(false)
            && proc.pid as u32
                == SAREG_SAMGR_PID.load(core::sync::atomic::Ordering::Relaxed);
        if is_samgr_req || reply_to_mutation {
            if SAREG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 600 {
                let comm = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let mut pb = [0u8; 64];
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
                    "SAREG {} code={} from pid={} tgid={} comm='{}' -> pid={} dlen={} parcel={}",
                    if reply { "reply" } else { "send " },
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

    let mut wakes: Vec<Arc<BinderProc>> = Vec::new();
    // sc161: child-routed wakes — (proc, child tgid) for transactions
    // aimed at forked-child nodes; wake_proc_one prefers that child's
    // threads so the delivery is not eaten by the daemon's readers.
    let mut wakes_child: Vec<(Arc<BinderProc>, u32)> = Vec::new();
    // Threads to wake directly (thread-targeted delivery).
    let mut wake_ths: Vec<Arc<BinderThread>> = Vec::new();
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
                    } else {
                        ns.has_async_transaction = true;
                    }
                }
                if !parked {
                    // SAFETY: under WORLD.
                    unsafe { txn.set_holds_async_slot(true) };
                    pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                    thread.hist(EV_TXN_PQUEUED, txn.id as u32, target.pid);
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
                                    "APPTXN id={} from pid={} comm='{}' -> pid={} code={:#x} oneway=1 proc child={}",
                                    txn.id,
                                    proc.pid,
                                    comm,
                                    target.pid,
                                    txn.code,
                                    target_node.as_ref().map(|n| n.reg_tgid).unwrap_or(0)
                                );
                            }
                        }
                    }
                    // sc161: oneway to a child-registered node — route the
                    // wake to the child's own threads.
                    let pref = target_node
                        .as_ref()
                        .map(|n| n.reg_tgid)
                        .filter(|&g| g != 0 && g != target.pid)
                        .unwrap_or(0);
                    if pref != 0 {
                        wakes_child.push((target.clone(), pref));
                    } else {
                        wakes.push(target.clone());
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
                                "APPTXN id={} from pid={} comm='{}' -> pid={} code={:#x} oneway={} {} child={}",
                                txn.id,
                                proc.pid,
                                comm,
                                target.pid,
                                txn.code,
                                oneway as u8,
                                if t_thread.is_some() { "thr" } else { "proc" },
                                target_node.as_ref().map(|n| n.reg_tgid).unwrap_or(0)
                            );
                        }
                    }
                }
                if let Some(tt) = t_thread {
                    let tth = tt.st();
                    tth.todo.push_back(Work::Transaction(txn.clone()));
                    tth.process_todo = true;
                    tt.hist(EV_TXN_TQUEUED, txn.id as u32, tt.tid);
                    wake_ths.push(tt);
                } else {
                    pm(world, &target).im().todo.push_back(Work::Transaction(txn.clone()));
                    thread.hist(EV_TXN_PQUEUED, txn.id as u32, target.pid);
                    // sc161: sync txn queued on the proc todo — if the
                    // target node belongs to a forked child, prefer that
                    // child's threads for the wake.
                    let pref = target_node
                        .as_ref()
                        .map(|n| n.reg_tgid)
                        .filter(|&g| g != 0 && g != target.pid)
                        .unwrap_or(0);
                    if pref != 0 {
                        wakes_child.push((target.clone(), pref));
                    } else {
                        wakes.push(target.clone());
                    }
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
                    let node = {
                        let sp = pm(world, sender);
                        match sp.im().nodes.iter().find(|n| n.ptr == ptr).cloned() {
                            Some(n) => n,
                            None => {
                                let n = Arc::new(BinderNode::new(
                                    world.next_id,
                                    ptr,
                                    obj.cookie,
                                    sender.id,
                                    crate::process::current_pid() as u32,
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
                    // waiting_threads membership: only a thread that can
                    // actually take proc work registers for wakeups
                    // (binder_available_for_proc_work_ilocked). A reader
                    // without looper flags or with a stacked txn must not
                    // — it would eat wake_proc_one's single pick.
                    let th = thread.st();
                    th.waiting_for_proc_work = th.todo.is_empty()
                        && th.txn_stack.is_empty()
                        && (th.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
                    thread.hist(EV_PARK, th.txn_stack.len() as u32, 0);
                }
            }
            thread
                .read_wait_since
                .store(crate::drivers::timer::get_jiffies(), core::sync::atomic::Ordering::Release);
            thread.stall_reported.store(false, core::sync::atomic::Ordering::Release);
            thread.rekicked.store(false, core::sync::atomic::Ordering::Release);
            thread.rekick_count.store(0, core::sync::atomic::Ordering::Release);
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
                            got_transaction = true;
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
                // Linux done: BR_SPAWN_LOOPER when no transaction was read
                // and more threads are allowed and none are ready.
                if !got_transaction {
                    let pim = pm(world, proc).im();
                    // waiting_for_proc_work now implies the full
                    // availability predicate (todo/txn_stack empty +
                    // looper flags), i.e. waiting_threads membership.
                    let ready = pim.threads.iter().any(|t| t.st().waiting_for_proc_work);
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
