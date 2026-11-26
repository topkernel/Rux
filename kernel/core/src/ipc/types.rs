//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The ipc subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct` and `enum` of the subsystem's topic
//! files lives here, together with the constants those definitions name
//! (array lengths, and the constants those derive from); every `impl`
//! block stays in the topic file beside the functions that use it — an
//! impl is code, a definition is data layout. The audited `unsafe.rs`
//! keeps the types its own contracts are written about. The exceptions
//! are definitions local to code: a type declared inside a function or
//! inside an inline module (binder's `rtt_census` stays inline in
//! `hotpath.rs`).
//!
//! This file holds no `unsafe`; the attribute below keeps it so. A field
//! that was private to its old topic file is `pub(super)` here because
//! the code that touches it stayed in that file: the ipc directory is
//! the smallest visibility boundary Rust offers for that split.

#![forbid(unsafe_code)]

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicUsize};

use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;


// ---------------------------------------------------------------------------
// the binder UAPI records, the object model (node, ref, proc,
// thread, txn, work), the buffer allocator's block view and the
// world/guard types, with the ring length ThreadState names
// ---------------------------------------------------------------------------

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

/// struct binder_write_read (LP64). Copy: plain u64 UAPI record — the
/// uaccess capsule round-trips it (copy_in_t/copy_out_t).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BinderWriteRead {
    pub write_size: u64,
    pub write_consumed: u64,
    pub write_buffer: u64,
    pub read_size: u64,
    pub read_consumed: u64,
    pub read_buffer: u64,
}

/// struct flat_binder_object (24 bytes on LP64)
#[repr(C)]
pub struct FlatBinderObject {
    pub hdr_type: u32,
    pub flags: u32,
    pub handle: u32,
    pub _handle_pad: u32,
    pub cookie: u64,
}

/// struct binder_buffer_object (40 bytes on LP64): BINDER_TYPE_PTR —
/// points at additional data outside the parcel that the driver copies
/// into the target's sg (extra) area and rewrites to a target address.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct BinderBufferObject {
    pub(super) hdr_type: u32,
    pub(super) flags: u32,
    pub(super) buffer: u64,
    pub(super) length: u64,
    /// index into the offsets array of the parent BINDER_TYPE_PTR object
    pub(super) parent: u64,
    /// offset in the parent buffer where this buffer's address is written
    pub(super) parent_offset: u64,
}

/// struct binder_fd_array_object (32 bytes on LP64): BINDER_TYPE_FDA —
/// an array of u32 fds living inside the parent buffer object.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct BinderFdArrayObject {
    pub(super) hdr_type: u32,
    pub(super) flags: u32,
    pub(super) num_fds: u64,
    /// index into the offsets array of the parent BINDER_TYPE_PTR object
    pub(super) parent: u64,
    /// offset of the fd array within the parent's buffer
    pub(super) parent_offset: u64,
}

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

/// INV-BINDER-BLOCKVIEW (unsafe-slim r3c): the ONE audited raw window over
/// an in-flight transaction block, replacing the ~20 scattered volatile
/// derefs the payload copy + flat-object walk used to carry. A TxnBlock is
/// built after `BinderAlloc::new_buf` returns the block offset and lives
/// for the duration of THIS transaction's fixup phase.
///
/// Layout contract (same arithmetic `BinderAlloc::block_need` reserved):
/// the allocation is `alloc_len` bytes at `base` (page-aligned buddy
/// block; `base` never escapes this type except through the audited
/// `extra_copy_in`/`block_window` bodies that feed uaccess), and this
/// txn's block is
///   [data (data_size)][offsets (align8(offsets_size))][extra (align8(extra_size))]
/// starting at `block_off` (8-aligned: new_buf carves sums of align8'd
/// needs from 0). Every accessor checks BOTH the sub-layout bound AND the
/// absolute allocation bound — a slip in either contract fails the
/// transaction instead of writing out of the buffer (the sender controls
/// the offsets array, so the bounds are hostile-input territory; see the
/// wrap-proof checks the pre-conversion audit added).
///
/// Reads/writes stay volatile exactly as before the conversion (the
/// buffer is concurrently observable through the target's user mapping).
/// Misaligned object offsets were UB-shaped volatile reads before; now
/// they are a clean None -> BR_FAILED_REPLY.
pub struct TxnBlock {
    /// Kernel-side base of the whole binder buffer allocation
    /// (BinderAlloc::kvirt). Never handed out except by the audited
    /// bodies below.
    pub(super) base: *mut u8,
    /// Size of the whole allocation (npages * PAGE_SIZE).
    pub(super) alloc_len: usize,
    /// This txn's block start within the allocation (8-aligned).
    pub(super) block_off: usize,
    /// Parcel payload size (the [data] sub-area, unaligned tail is fine —
    /// only align8() sizes are reserved).
    pub(super) data_size: usize,
    /// Offsets-array size in bytes (8-aligned by the caller marshalling).
    pub(super) offsets_size: usize,
    /// Extra/sg area payload size (sg buffers + optional secctx string).
    pub(super) extra_size: usize,
    /// Target-user base of the whole allocation (BinderAlloc::user_base);
    /// the user-side addresses the sender's PTR objects carry map back
    /// against block_user_base() = user_base + block_off.
    pub(super) user_base: u64,
    /// Precomputed layout end (block_off + align8 of the three sub-areas)
    /// — span_ok runs on every accessor call, so the bound is computed
    /// once at construction, not per access.
    pub(super) layout_end: usize,
    /// Precomputed extra-area base (block-relative).
    pub(super) extra_base: usize,
    /// Precomputed offsets-array base (block-relative) — the offsets
    /// array sits right after the data sub-area.
    pub(super) offsets_base: usize,
}

/// Bound window over a resolved parent BINDER_TYPE_PTR sg copy (see
/// TxnBlock::parent_window). Carries offsets, not pointers — every
/// access re-checks against `len`.
pub(super) struct ParentWindow<'a> {
    pub(super) block: &'a TxnBlock,
    /// Absolute offset of the parent buffer start within the allocation.
    pub(super) abs: usize,
    pub(super) len: usize,
}

pub struct NodeState {
    pub(super) internal_strong_refs: i32,
    pub(super) local_strong_refs: i32,
    pub(super) local_weak_refs: i32,
    pub(super) has_strong_ref: bool,
    pub(super) has_weak_ref: bool,
    pub(super) pending_strong_ref: bool,
    pub(super) pending_weak_ref: bool,
    /// TF_ONE_WAY serialization: one async transaction in flight per node
    /// (Linux node->has_async_transaction); extra oneways park on
    /// node->async_todo until the in-flight buffer is freed.
    pub has_async_transaction: bool,
    /// Oneway transactions waiting for the node's async slot (Linux
    /// node->async_todo). Their buffers are already allocated and fixed
    /// up; delivery is just deferred.
    pub async_todo: VecDeque<Arc<BinderTxn>>,
}

pub struct BinderNode {
    pub id: u64,
    pub(super) ptr: u64,
    pub(super) cookie: u64,
    pub(super) proc_id: u64,
    /// sc157: tgid of the task that REGISTERED the node. When it differs
    /// from the owner proc's pid, the node belongs to a forked child
    /// sharing the parent's binder_proc — deliveries to such nodes must
    /// be visible to the APPTXN tracer (the AMS→child ScheduleStartAbility
    /// hop rides exactly this shape).
    pub reg_tgid: u32,
    /// flat_binder_object.flags at registration (accept_fds and
    /// txn_security_ctx bits).
    pub(super) flags: u32,
    /// sc161: the registering task group exited while the OWNER proc
    /// lives on (forked appspawn children share the daemon's proc). Set
    /// by binder_on_tgid_exit; makes node_is_dead true and fail-fasts
    /// new transactions through stale refs (BR_DEAD_REPLY), mirroring
    /// what proc teardown does for whole-process death.
    pub(super) dead: core::sync::atomic::AtomicBool,
    pub(super) st: core::cell::UnsafeCell<NodeState>,
}

pub(super) struct BinderRef {
    pub(super) desc: u32,
    pub(super) node: Arc<BinderNode>,
    pub(super) strong: i32,
    pub(super) weak: i32,
    /// Death-notification request on this handle (Linux ref->death).
    pub(super) death: Option<RefDeath>,
}

/// Death-notification request state (Linux struct binder_ref_death +
/// its work-item states).
pub(super) struct RefDeath {
    pub(super) cookie: u64,
    /// Armed: requested, node still alive. Queued: Work::DeadBinder sits
    /// on the proc todo (node dead, BR not yet read). Delivered:
    /// BR_DEAD_BINDER was read, awaiting BC_DEAD_BINDER_DONE.
    pub(super) phase: DeathPhase,
    /// BC_CLEAR_DEATH_NOTIFICATION arrived while the notification was
    /// queued or delivered: after BC_DEAD_BINDER_DONE, deliver
    /// BR_CLEAR_DEATH_NOTIFICATION_DONE and drop the request (Linux
    /// BINDER_WORK_DEAD_BINDER_AND_CLEAR).
    pub(super) clear_pending: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum DeathPhase {
    Armed,
    Queued,
    Delivered,
}

pub(crate) struct BufBlock {
    pub(crate) off: usize,
    pub(crate) len: usize,
    pub(crate) free: bool,
    pub(crate) async_block: bool,
    pub(crate) allow_user_free: bool,
    pub(crate) data_size: usize,
    pub(crate) offsets_size: usize,
    /// Per-allocation id (the reference's buffer->debug_id): assigned
    /// by new_buf when a block is carved, 0 while free. Carried by the
    /// binder_transaction_alloc_buf / buffer_release trace events.
    pub(crate) buf_id: u64,
    /// sg (extra) area size: BINDER_TYPE_PTR payload copies (layout:
    /// [data][offsets][extra], each u64-aligned).
    pub(crate) extra_size: usize,
    /// Oneway (async) buffers keep their transaction alive so the node's
    /// async slot is released when the receiver frees the buffer (Linux
    /// buffer->transaction). Sync buffers rely on the reply path instead.
    pub(crate) txn: Option<Arc<BinderTxn>>,
    /// sc243 (leg-17): the sync txn this buffer carried has been
    /// REPLIED — the request data is protocol-dead and only the
    /// receiver's contractual BC_FREE_BUFFER remains. The OHOS
    /// cc54-family services never issue that BC_FREE, so their allocs
    /// filled with delivered-but-unfreed blocks (sc243 census: 329K
    /// live BinderTxn arcs, every one pinned by such a block; at the
    /// launcher floor this is ~210MB of the 831MB heap). On alloc
    /// pressure these blocks are reclaimed (see alloc_new_buf).
    pub(crate) reply_done: bool,
}

pub(crate) struct BinderAlloc {
    pub(crate) kvirt: *mut u8,
    pub(crate) phys: usize,
    pub(crate) npages: usize,
    pub(crate) order: usize,
    pub(crate) user_base: u64,
    pub(crate) blocks: Vec<BufBlock>,
    pub(crate) free_async_space: usize,
    /// sc243 (leg-17): jiffies of the last stale-async sweep (see
    /// sweep_stale_async) — rate-limits the O(blocks) scan to one per
    /// second per allocator.
    pub(crate) last_sweep_j: u32,
    /// Monotone buffer-id source (see BufBlock::buf_id).
    pub(crate) next_buf_id: u64,
}

pub enum Work {
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

pub struct BinderTxn {
    pub id: u64,
    pub from_proc: Option<Arc<BinderProc>>,
    pub from_tid: Option<u32>,
    pub to_proc: Arc<BinderProc>,
    /// Set when the transaction is delivered (BR_TRANSACTION): the
    /// receiving thread, which will later issue BC_REPLY. Atomic with
    /// 0 = None (unsafe-slim r3: real binder tids are >= 1, so the
    /// sentinel cannot collide; Relaxed is enough — the value is a
    /// plain word, no pointer, no ordering contract).
    pub(super) to_tid: core::sync::atomic::AtomicU32,
    pub target_node: Option<Arc<BinderNode>>,
    /// Set while this txn holds the node's async slot (TF_ONE_WAY).
    /// Interior-mutable: a parked oneway claims the slot only when the
    /// in-flight buffer is freed and it graduates to the proc todo.
    pub(super) holds_async_slot: core::sync::atomic::AtomicBool,
    pub code: u32,
    pub flags: u32,
    pub(super) sender_pid: i32,
    pub(super) sender_euid: u32,
    pub(super) data_size: usize,
    pub(super) offsets_size: usize,
    pub(super) buffer_off: usize,
    /// Target-user address of the NUL-terminated secctx string in the sg
    /// area (0: the target node did not request a security context).
    pub(super) security_ctx: u64,
    /// sc208 (launcher leg 13, C-line cross-finding: SAMgr SendRequest
    /// ~30 s per call once the clock was fixed): lifecycle stamps for the
    /// SLOW-TXN probe. enq_j = jiffies at BC_TRANSACTION queue time;
    /// pick_j = jiffies at BR_TRANSACTION delivery (server pickup), 0
    /// until then. The reply pop computes queue-vs-exec and prints the
    /// split for any sync txn over 5 s — where the 30 s goes (server not
    /// picking up vs handler running long) names the next chain link.
    pub enq_j: u32,
    /// (unsafe-slim r3) all five census cells below are Relaxed atomics:
    /// every prior UnsafeCell variant trusted "single mutator under
    /// WORLD"; the enqueue-wake latch (binder_transaction) writes two of
    /// them AFTER the WORLD release, which was a bare single-writer
    /// assumption. Atomics carry the same cost (one plain word store on
    /// both targets) and drop the trust.
    pub(super) pick_j: core::sync::atomic::AtomicU32,
    /// 23rd PICK-LATE evidence: eligible proc-work readers on the target
    /// at proc-todo enqueue time (the wake_proc_one predicate count).
    /// 0 = nobody to wake — every reader busy in its own handler (a
    /// capacity signal, not a lost wake). Written under WORLD at
    /// enqueue, read in the PICK-LATE probe under WORLD.
    pub(super) enq_elig: core::sync::atomic::AtomicU8,
    /// 105th RTT census (rux:bindertrace (retired debug=bindertrace)), ns-domain twins of the
    /// jiffies stamps: enq_ns = constructor time (request: enqueue;
    /// reply: submit), pick_ns = server BR_TRANSACTION staging time, 0
    /// until then. Jiffies (10 ms) cannot resolve the 4-5 ms fast-path
    /// RTT class the 103rd measured vs the reference's ~1 ms. Written
    /// once on the txn lifecycle path under WORLD, read at the census
    /// accumulation points under WORLD.
    pub(super) enq_ns: u64,
    pub(super) pick_ns: core::sync::atomic::AtomicU64,
    /// 105th wake-dispatch census: latched right after this txn's sync
    /// wake — bits 0-3 = sched::WAKE_DISP code (1 idle-claim+IPI, 2 no-
    /// idle sync, 3 no-idle async), bits 4-11 = idle mask at wake entry,
    /// bits 12-19 = min(grq nr_running, 255). Read by the QPICK print.
    /// Latched once at the enqueue-wake point (outside WORLD, single
    /// enqueuing thread — atomic anyway), read at pickup under WORLD.
    pub(super) enq_disp: core::sync::atomic::AtomicU32,
    /// 105th: the wake-issued stamp (0 until the enqueue's wake loops
    /// ran) — q splits into sender-build (enq_ns->wake_ns) and
    /// server-wait (wake_ns->pick_ns). Same contract as enq_disp.
    pub(super) wake_ns: core::sync::atomic::AtomicU64,
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

pub struct ThreadState {
    pub looper: u32,
    pub todo: VecDeque<Work>,
    /// process_todo: a deferred TRANSACTION_COMPLETE on the todo does NOT
    /// set this — the thread keeps sleeping until real work arrives.
    pub(super) process_todo: bool,
    /// True while blocked in binder_thread_read waiting for proc work
    /// (Linux waiting_threads list, drives BR_SPAWN_LOOPER).
    pub waiting_for_proc_work: bool,
    /// Reference looper_need_return: force the thread's next read to
    /// return to userspace (binder_flush / fd close / deferred fd
    /// completion all set it; the BWR read loop breaks on it).
    pub(super) looper_need_return: bool,
    /// Park sequence stamp (65th session, reference-order parity):
    /// value of NEXT_PARK_SEQ when this thread last joined the
    /// waiting-for-proc-work set. The reference registers a parking
    /// reader with `list_add` at the HEAD of proc->waiting_threads and
    /// binder_select_thread_ilocked pops the head — i.e. the MOST
    /// RECENTLY parked looper is selected for delivery (LIFO, sticky to
    /// the thread that just finished and re-parked). We have no list;
    /// a monotonic stamp under WORLD reproduces the exact order (max
    /// stamp = head). 0 = not parked.
    pub(super) park_seq: u64,
    pub txn_stack: Vec<Arc<BinderTxn>>,
    /// 75th OH parity (CONFIG_BINDER_SENDER_INFO): thread->tokens.
    /// sender_tokenid source — the SENDER's tgid of the transaction this
    /// thread last staged to userspace (BR_TRANSACTION or BR_REPLY),
    /// stamped for EVERY delivered txn exactly like the reference's
    /// `thread->tokens.sender_tokenid = t->sender_tokenid`
    /// (linux-6.6 binder.c:4829). The GET_SENDER_INFO ioctl resolves
    /// the native access token through it. txn_stack cannot serve:
    /// oneways are never stacked (no BC_REPLY will come).
    pub(super) sender_token_pid: u32,
    /// 75th OH parity: thread->sender_pid_nr — task_tgid_nr of the
    /// sender, but ONLY when t->from was set (sync txns; the reference
    /// zeroes it for oneway/reply, binder.c:4737-4745).
    pub(super) sender_pid_nr: u32,
    /// Single-slot error dedup (94th session, hp92w4 storm root cause).
    /// Upstream binder.c 6.6 stages thread errors in TWO fixed single
    /// work slots (thread->return_error for write-path failures,
    /// thread->reply_error for async/death failures): a second error
    /// staged while one is unconsumed is DROPPED ("Unexpected reply
    /// error" — binder_send_failed_reply). Our todo-queue model had no
    /// such guard: a proc death that failed the SAME pending sync txn
    /// through two cleanup walks delivered TWO BR_DEAD_REPLY to one
    /// thread (hp92w4 rings: three threads consumed ERR_RECV(29189)
    /// twice at j4330); OH libipc's completion bookkeeping then desynced
    /// into 2-command sync BWRs and the SYNCSEND-GUARD storm aborted
    /// foundation. `err_staged` = a ReturnError work is queued and
    /// unconsumed; `err_is_write` = it came from the write path (gates
    /// the write loop, upstream's `thread->return_error.cmd == BR_OK`
    /// loop condition).
    pub(super) err_staged: bool,
    pub(super) err_is_write: bool,
    /// Transition ring (sc74 forensics); written under WORLD only.
    pub hist: [BinderEv; BHIST],
    pub hcur: usize,
    /// Live BWR ioctl shadow (r41v6): (write_size, write_consumed,
    /// read_size, first pending write cmd) of the binder_write_read this
    /// thread is currently executing — recorded at ioctl entry, consumed
    /// updated after the write leg, cleared on every exit. The
    /// bwr_park_audit reads THIS, never user memory: the audit runs on
    /// the timer heartbeat (interrupt context of an arbitrary task), so
    /// dereferencing the audited thread's user bwr pointer through the
    /// interrupted task's page tables produced the r41v4/r41v5 false-H2
    /// rows (same-VA zero pages decoded as w=0/0 rsz=0 "write-only
    /// parked"; unrelated mappings as garbage w= words).
    pub live_bwr: Option<(u64, u64, u64, u32)>,
}

pub struct BinderThread {
    pub tid: u32,
    /// Owning task's TGID (sc161 child-tgid routing): several appspawn
    /// children share ONE binder proc (the daemon's inherited fd), so the
    /// proc's thread list mixes daemon-tgid and child-tgid threads. The
    /// tgid decides which threads may consume a transaction aimed at a
    /// child-registered node (reg_tgid) — without it the daemon's OS_IPC
    /// reader threads steal the AMS→child deliveries.
    pub(super) tgid: u32,
    /// Per-thread wait queue (Linux thread->wait): replies and tcomplete
    /// wake the exact thread; proc work wakes one selected waiter.
    pub wait: WaitQueueHead,
    pub(super) st: core::cell::UnsafeCell<ThreadState>,
    /// Jiffies when this thread entered the blocking binder read wait (0
    /// while not waiting) — read atomically by the khungtaskd binder-stall
    /// scan without taking WORLD.
    pub read_wait_since: core::sync::atomic::AtomicU64,
    /// 105th QPICK census: nominal-clock stamp at the LAST return of the
    /// blocking read wait (0 before the first park). Splits a slow
    /// enqueue->pickup into wake->wait-return (scheduler) and
    /// wait-return->staging (the read path itself — WORLD acquire and
    /// drain). Relaxed atomic; diagnostic only.
    pub(super) read_wake_ns: core::sync::atomic::AtomicU64,
    /// One stall report per wait episode (avoids per-scan spam).
    pub stall_reported: core::sync::atomic::AtomicBool,
    /// One REKICK hist record per wait episode (the re-kick itself still
    /// runs every scan; only the ring entry is gated so the forensics
    /// history is not flooded by heal noise).
    pub rekicked: core::sync::atomic::AtomicBool,
    /// Kicks issued in the current wait episode — >= 2 means the thread is
    /// still parked in the SAME read wait one scan after a kick. That is
    /// usually the benign idle-reader re-park (the kick ran it; no work was
    /// readable, so the wait_event loop slept again — see wrl_last_exec_ns
    /// for the discriminator), NOT by itself a wake→run leak; the scheduler
    /// view is dumped then (WAKE-RUN-LEAK line) with the exec-delta shape.
    pub rekick_count: core::sync::atomic::AtomicU32,
    /// sc252/sc253 attribution (leg 19): outcome counters for this
    /// thread's OWN wait-queue kicks, read by the WAKE-RUN-LEAK dump —
    /// wok>=1 with the task still parked means the wake linked the task
    /// and the PICK side never ran it; wno-only means every kick was
    /// silently refused (the R82 DL-ENQ-DEFERRED / R80-phantom families).
    pub kick_wake_ok: core::sync::atomic::AtomicU32,
    pub kick_wake_refused: core::sync::atomic::AtomicU32,
    /// wrl-class lane: exec_ns the WAKE-RUN-LEAK print last sampled for
    /// this thread (per wait episode, reset with the kick counters). The
    /// census row's exec delta between scans separates the dominant
    /// benign shape — an idle reader whose kick RAN it (it re-checked
    /// thread_has_work, found none, re-parked inside the same wait_event
    /// epoch; exec advances a few µs per kick) — from the true leak
    /// shape (kicks transition the queue entry, exec never advances: the
    /// task never runs). On the lr-series boots ALL rows were re-parks;
    /// without the delta each row reads as "kicked, never ran".
    pub wrl_last_exec_ns: core::sync::atomic::AtomicU64,
}

pub struct ProcInner {
    pub(super) alloc: Option<BinderAlloc>,
    pub nodes: Vec<Arc<BinderNode>>,
    pub(super) refs: Vec<BinderRef>,
    /// Next handle descriptor to hand out. Starts at 1: desc 0 is
    /// reserved for the context manager in every process (Linux
    /// binder_get_ref_for_node starts normal refs at 1).
    pub(super) next_desc: u32,
    pub threads: Vec<Arc<BinderThread>>,
    pub todo: VecDeque<Work>,
    pub max_threads: i32,
    pub requested_threads: i32,
    pub requested_threads_started: i32,
    pub(super) is_dead: bool,
}

pub struct BinderProc {
    pub id: u64,
    pub pid: u32,
    pub(super) ctx: BinderCtx,
    pub(super) wait: WaitQueueHead,
    pub(super) inner: core::cell::UnsafeCell<ProcInner>,
}

pub struct World {
    pub(super) next_id: u64,
    pub procs: Vec<Arc<BinderProc>>,
    /// Context-manager node per device context (index by BinderCtx).
    pub(super) mgr_node: [Option<Arc<BinderNode>>; 3],
}

/// RT-R4 (WORLD per-caller attribution): world_lock's sections all
/// charge to ONE REST_SITES row (binder.rs:2092) — the 6–8 ms max could
/// not be attributed to a caller. This wrapper records each section's
/// span against the CALLER's Location (world_lock is #[track_caller]),
/// and the syscost dump prints the top rows.
pub struct WorldGuard<'a> {
    pub(super) inner: crate::sync::spinlock::SpinlockIrqGuard<'a, Option<World>>,
    pub(super) t0: u64,
    pub(super) loc: &'static core::panic::Location<'static>,
    /// LAST FIELD ON PURPOSE: fields drop in declaration order, so this
    /// drops AFTER `inner` has released WORLD — the deferred wakes run
    /// with the lock GONE (the reference's wake_q discipline; see
    /// DeferredWakes).
    pub(super) deferred: DeferredWakes,
}

/// Guard-carried drain list (see WorldGuard.deferred). Drop wakes every
/// carried thread — runs strictly after WORLD is released.
pub(super) struct DeferredWakes(pub(super) Vec<Arc<BinderThread>>);

pub(super) struct WorldSecRow {
    pub(super) loc: core::sync::atomic::AtomicU64,
    pub(super) n: core::sync::atomic::AtomicU64,
    pub(super) max: core::sync::atomic::AtomicU64,
}

/// binder_translate_fd: install a sender fd into the TARGET process's fd
/// table (fget in the sender, get_unused_fd + fd_install on the target
/// opener task). Runs in the sender's ioctl context and NEVER under WORLD
/// INV-BINDER-PINTASK (unsafe-slim r3c): RAII twin of procfs's PinnedTask
/// guard for the fd-translate paths — pid_hash_lookup_pinned takes a
/// task_refcnt the guard releases on scope end (Drop), so no early-return
/// path can leak the pin or use the task after release.
pub(super) struct PinnedTask {
    pub(super) ptr: *mut crate::process::task::Task,
}

// ---------------------------------------------------------------------------
// the SysV semaphore UAPI records and the semaphore-set object
// ---------------------------------------------------------------------------

/// struct seminfo — returned by IPC_INFO / SEM_INFO
/// Must match Linux's include/uapi/linux/sem.h. Total: 40 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SemInfoUapi {
    pub semmap: i32,  // +0
    pub semmni: i32,  // +4
    pub semmns: i32,  // +8
    pub semmnu: i32,  // +12
    pub semmsl: i32,  // +16
    pub semopm: i32,  // +20
    pub semume: i32,  // +24
    pub semusz: i32,  // +28
    pub semvmx: i32,  // +32
    pub semaem: i32,  // +36
}

/// struct semid64_ds — returned by IPC_STAT, IPC_SET
///
/// riscv64/aarch64: asm-generic/sembuf.h — perm(48) + otime + ctime +
/// nsems + 2 unused = 88 bytes, sem_nsems at offset 64.
///
/// x86_64: the x86 uapi (arch/x86/include/uapi/asm/sembuf.h) pads EACH
/// time_t with a high word, so glibc/musl `struct semid_ds` on x86_64
/// reads sem_nsems at offset 80 in a 104-byte struct (verified against
/// the musl x86_64 bits/sem.h used by the LTP build:
/// perm(48) + otime + pad + ctime + pad + nsems@80). Writing the
/// asm-generic layout put nsems where x86_64 userspace sees
/// __unused2 — LTP r21 semget01 read 0 where the kernel wrote 10
/// (`sem_nsems (0) != PSEMS (10)` from IPC_STAT). The high words are
/// pure padding on x86_64 (time_t is 64-bit), so they stay zero.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SemidDsUapi {
    pub sem_perm: IpcPermUapi,
    pub sem_otime: i64,
    /// x86 uapi high words (32-bit compat heritage): always zero on
    /// x86_64, absent from the asm-generic layout.
    #[cfg(feature = "x86_64")]
    pub __sem_otime_high: u64,
    pub sem_ctime: i64,
    #[cfg(feature = "x86_64")]
    pub __sem_ctime_high: u64,
    pub sem_nsems: u64,
    pub __unused3: u64,
    pub __unused4: u64,
}

/// struct sembuf — passed by userspace to semop/semtimedop
/// Total: 6 bytes, no padding
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SemBuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: u16,
}

/// Single semaphore within a set.
pub(super) struct SemEntry {
    /// Current semaphore value.
    pub(super) value: AtomicI32,
    /// Number of processes waiting for value to increase (sem_op < 0).
    pub(super) ncnt: AtomicUsize,
    /// Number of processes waiting for value to become zero (sem_op == 0).
    pub(super) zcnt: AtomicUsize,
    /// PID of the last process that operated on THIS semaphore (semop or
    /// SETALL) — semctl(GETPID) returns it (LTP semctl07).
    pub(super) pid: AtomicU32,
}

/// Semaphore set (the IPC object).
pub struct SemArray {
    pub perm: KernIpcPerm,
    /// Per-semaphore values and wait queues.
    pub(super) sems: Spinlock<Option<alloc::vec::Vec<SemEntry>>>,
    /// Time of last semop.
    pub(super) sem_otime: AtomicI64,
    /// Time of last semctl that changed the set.
    pub(super) sem_ctime: AtomicI64,
    /// PID of last semop.
    pub(super) sem_padid: AtomicU32,
    /// Lock for the entire semaphore set (protects sems vector existence).
    pub(super) lock: Spinlock<()>,
    /// Wait queue for processes blocked on semop.
    pub(super) wq: crate::process::wait::WaitQueueHead,
}

// ---------------------------------------------------------------------------
// the shared IPC substrate: the permission structures, the id
// registry (its slots array length named) and the task's undo entry
// ---------------------------------------------------------------------------

// Maximum number of IPC objects per type — reference IPCMNI (32768).
// The id encoding is (index << 16) | seq, so index <= 32767 keeps every
// id a positive i32; the slots arrays cost 8 bytes per entry (Option<Box>
// niche) = 256 KiB per type, zeroed .bss. The old 256-entry table capped
// msgmni below the reference default (32000): LTP msgstress01 creates
// 1000 queues by default (its -n clamp never applies without the option)
// and the parent's msgget hit ENOSPC at 257; the cleanup RMID cascade
// then turned every child's msgsnd/msgrcv into EINVAL.
pub const IPC_IDS_MAX: usize = 32768;

/// struct ipc64_perm — userspace-visible IPC permission structure.
/// Must match Linux's asm-generic/ipcbuf.h exactly for RV64.
/// Total: 48 bytes. All-POD wire fields — Copy for the typed uaccess
/// capsule.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IpcPermUapi {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u32,
    pub seq: u16,
    pub __pad2: u16,
    pub __unused1: u64,
    pub __unused2: u64,
}

/// kern_ipc_perm — kernel-internal permission structure embedded in all IPC objects.
#[derive(Clone)]
pub struct KernIpcPerm {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub seq: u32,
    /// Owning IPC namespace identity (IpcNamespace::inum). The reference
    /// kernel keeps one ipc_ids table PER ipc_namespace (ns->ids[...]);
    /// Rux keeps a single global slot table, so the namespace must ride
    /// on the object and every lookup filters by the caller's namespace
    /// (LTP msg_comm: two CLONE_NEWIPC children msgget the SAME key and
    /// each must get its OWN queue — a global key match made child1's
    /// msgrcv see child2's message, "messages leak between namespaces").
    pub ns_inum: u64,
}

/// Wrapper around each IPC object slot.
pub struct IpcObjectEntry<T> {
    pub inner: T,
    pub deleted: bool,
}

/// Generic IPC object registry. Manages allocation, lookup, and deletion
/// of IPC objects by key or ID.
pub struct IpcIds<T> {
    pub slots: Spinlock<[Option<alloc::boxed::Box<IpcObjectEntry<T>>>; IPC_IDS_MAX]>,
    pub(super) next_seq: AtomicU32,
    // pub (visibility tax of the ipc flip): sysv_shm's destroy
    // paths decrement the live-object count under the slots lock.
    pub count: AtomicUsize,
}

/// Per-process semaphore undo entry.
/// Records an adjustment made to a semaphore so it can be reversed on process exit.
/// (Type substrate of Task's sem_undo field — lives with the framework.)
#[derive(Clone, Copy)]
pub struct SemUndoEntry {
    /// Semaphore set ID.
    pub semid: i32,
    /// Semaphore number within the set.
    pub sem_num: u16,
    /// Adjustment value (the sem_op that was applied).
    pub adjustment: i32,
}
