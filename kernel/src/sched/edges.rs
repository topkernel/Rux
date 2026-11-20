//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The scheduler's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L1 (2026-11-20), per the framekernel doctrine: the
//! scheduler's POLICY code (sched.rs, fair.rs, rt.rs, cgroup.rs) is safe
//! by construction — every raw-pointer dereference it needs of the
//! scheduler's pointer currency (`*mut Task` slots/queue entries, the
//! boot-provenance statics) crosses one of the audited functions in
//! THIS file, each carrying an INV-SCE contract. One audit per edge,
//! not per call site; the aliasing discipline is exactly the one the
//! per-site SAFETY comments documented before the consolidation.
//!
//! INV registry (mirrors kernel/src/safety/ + process/task_ref.rs):
//! - INV-SCE-1 `task_mut`    — pointer→exclusive reference, scheduler
//!   ownership disciplines (GRQ hold, IRQs-off switch window, this
//!   CPU's current slot/tp, or a walker pin).
//! - INV-SCE-2 `task_shared` — pointer→shared reference, same liveness
//!   sources, diagnostic/atomic-only reads.
//! - INV-SCE-3 `grq_*`       — the MaybeUninit GRQ boot provenance:
//!   written once by `grq_init` under the GRQ_READY CAS, read only
//!   after the flag's Acquire side.

use crate::process::task::Task;

use super::fair::SchedEntity;
use super::sched::GlobalRunQueue;

// ---------------------------------------------------------------------------
// INV-SCE-4: the per-CPU idle-task storages
// ---------------------------------------------------------------------------

/// The per-CPU idle-task slots: `static mut` + MaybeUninit because
/// `Task::new_idle_at` is not const. Each slot is written exactly once
/// by `sched::init`'s idle-task construction (BSP, before secondaries
/// schedule); afterwards the slots are read-only (the b81 PER_CPU
/// scribble heal compares them as the static truth).
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; crate::config::MAX_CPUS] =
    [const { core::mem::MaybeUninit::uninit() }; crate::config::MAX_CPUS];

/// Address of cpu's idle-task slot (INV-SCE-4). Pointer-only: callers
/// keep identity semantics; the slot's address is stable for the boot's
/// lifetime. Valid as a `*mut Task` once the init pass wrote it (all
/// readers run post-init — scheduling requires it).
#[inline]
pub fn idle_task_ptr(cpu: usize) -> *mut Task {
    debug_assert!(cpu < crate::config::MAX_CPUS);
    // SAFETY: INV-SCE-4 — index in range (init writes every slot before
    // any CPU schedules); addr-of forms no reference to possibly-uninit
    // memory.
    unsafe { core::ptr::addr_of_mut!(IDLE_TASK_STORAGES[cpu]).cast::<Task>() }
}

/// Base address of the whole idle-task storage array (INV-SCE-4):
/// address arithmetic only, for the kernel-pointer range check that
/// admits the static Task storages (never their contents).
#[inline]
pub fn idle_task_storage_base() -> usize {
    // SAFETY: INV-SCE-4 — only the static's address is read.
    unsafe { core::ptr::addr_of!(IDLE_TASK_STORAGES) as usize }
}

// ---------------------------------------------------------------------------
// INV-SCE-1 / INV-SCE-2: task pointer → reference
// ---------------------------------------------------------------------------

/// Derive an exclusive task reference from a scheduler-owned pointer.
///
/// # Contract (INV-SCE-1)
/// `p` is non-null and points at a live `Task` whose slot lifetime is
/// covered by one of the scheduler's ownership disciplines:
/// (a) the GRQ hold — queue-linked tasks cannot be freed or picked
///     while the lock is held;
/// (b) this CPU's IRQs-off switch window — `prev`/`next` around the
///     context switch;
/// (c) this CPU's current slot or the hardware `tp` register — the
///     executing task;
/// (d) a walker pin — the PID-hash bucket lock or a parent reference
///     (`pid_hash_for_each_task`'s contract).
/// Mutation through the reference happens only from the context that
/// owns the task under that discipline — the same argument every
/// pre-consolidation call site made inline. Identity comparisons stay
/// on the raw pointer (`p`), never on the reference.
#[inline]
pub fn task_mut(p: *mut Task) -> &'static mut Task {
    debug_assert!(!p.is_null(), "edges::task_mut(null)");
    // SAFETY: INV-SCE-1 (caller-proved liveness/exclusion); the
    // reference is a raw alias of the pointer, like `current()` and
    // `TaskRef::DerefMut` before it.
    unsafe { &mut *p }
}

/// Derive a shared task reference (diagnostic reads, atomics) from a
/// scheduler-owned pointer.
///
/// # Contract (INV-SCE-2)
/// Same liveness sources as [task_mut]; the reference is used for
/// reads only (field reads, atomic loads, safe `&self` methods). A
/// task being read this way may concurrently execute on its owning
/// CPU — reads see mid-flight state, exactly as the raw-pointer reads
/// this edge replaces did.
#[inline]
pub fn task_shared(p: *const Task) -> &'static Task {
    debug_assert!(!p.is_null(), "edges::task_shared(null)");
    // SAFETY: INV-SCE-2.
    unsafe { &*p }
}

// ---------------------------------------------------------------------------
// INV-SCE-3: the GRQ boot provenance
// ---------------------------------------------------------------------------

/// The global run queue. `static mut` + MaybeUninit because
/// `GlobalRunQueue::new()` is not const (BTreeMap): written exactly
/// once by [`grq_init`] under the GRQ_READY CAS, read afterwards.
static mut GRQ: core::mem::MaybeUninit<GlobalRunQueue> = core::mem::MaybeUninit::uninit();

/// Publishing flag for the GRQ's one-time initialization (Acquire on
/// the readers' side pairs with the writer's stage + Release).
static GRQ_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Has the GRQ completed its one-time initialization?
#[inline]
pub fn grq_is_ready() -> bool {
    GRQ_READY.load(core::sync::atomic::Ordering::Acquire)
}

/// The initializing CPU's claim flag for the one-time GRQ setup:
/// `false` when this call won the CAS and must stage + publish.
pub fn grq_claim_init() -> bool {
    GRQ_READY.compare_exchange(
        false,
        true,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    )
    .is_err()
}

/// Writer-side staging slot (INV-SCE-3): visible only to the CAS-winning
/// initializer between [`grq_claim_init`] and [`grq_publish_init`].
/// Overwrites any prior content (early-boot re-init is not a supported
/// shape; the claim CAS serializes the single writer).
pub fn grq_stage(value: GlobalRunQueue) {
    // SAFETY: INV-SCE-3 writer side — this CPU holds the claim (the
    // GRQ_READY CAS in grq_claim_init); no reader passes the flag
    // before grq_publish_init's Release store.
    unsafe {
        GRQ = core::mem::MaybeUninit::new(value);
    }
}

/// Publish the staged GRQ (INV-SCE-3): the Release store that lets
/// every `grq_ref`/`grq_mut` reader observe the staged value. Also the
/// spinner's exit condition.
pub fn grq_publish_init() {
    GRQ_READY.store(true, core::sync::atomic::Ordering::Release);
}

/// Shared view of the initialized GRQ (INV-SCE-3 read side).
#[inline]
pub fn grq_ref() -> &'static GlobalRunQueue {
    // SAFETY: INV-SCE-3 — the caller passed the GRQ_READY Acquire gate
    // (sched::grq() checks it and spins through grq_init otherwise);
    // the MaybeUninit content is complete and never moves again.
    unsafe { GRQ.assume_init_ref() }
}

/// Exclusive view of the initialized GRQ (INV-SCE-3 mutation side).
/// Callers: the claim-winning initializer between [grq_stage] and
/// [grq_publish_init] (boot provenance fixups), or any GRQ-locked
/// path after the ready gate.
#[inline]
pub fn grq_mut() -> &'static mut GlobalRunQueue {
    // SAFETY: INV-SCE-3 — either the caller holds the init claim (the
    // content was staged by grq_stage immediately before) or passed
    // the ready gate; the reference is used under the GRQ lock or the
    // claim's write exclusion.
    unsafe { GRQ.assume_init_mut() }
}

/// Diagnostic read of the packed ownership qword `{ti_on_cpu, claim}`
/// (INV-SCE-2 diag twin): one volatile 8-byte read of the two adjacent
/// task fields, mirroring ownership_pickable's decode. Same liveness
/// sources as task_shared; the volatile read races no more than the
/// raw reads it replaces.
#[inline]
pub fn task_ownership_qword(t: *const Task) -> u64 {
    debug_assert!(!t.is_null());
    // SAFETY: INV-SCE-2 — t is a live task (GRQ-linked or walker-pinned);
    // a volatile load of two adjacent u32-class fields.
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*t).ti_on_cpu) as *const u64) }
}

// ---------------------------------------------------------------------------
// INV-SCE-5: the machine context switch
// ---------------------------------------------------------------------------

/// The machine-level context switch (INV-SCE-5): saves prev's callee
/// context through its saved-thread state and resumes through next's.
///
/// # Contract
/// Called only from [`super::sched::context_switch`] with IRQs
/// disabled on this CPU (the `__schedule` switch window) after the
/// GRQ-published pick: `prev` is the task the hardware is executing,
/// `next` was picked under the GRQ hold and its ownership qword
/// published, so neither task can be freed or double-run while the
/// switch runs. The arch routine never returns into prev's frame on
/// this CPU until prev is switched back in.
#[inline]
pub fn arch_context_switch(prev: &mut Task, next: &mut Task) {
    // SAFETY: INV-SCE-5 — the IRQs-off switch window plus the published
    // pick ownership pin both tasks for the duration of the switch.
    unsafe { crate::arch::context::context_switch(prev, next) }
}

/// Adopt a validated raw task handle as a [TaskBorrow]
/// (`process::task_ref`'s audited adoption edge, INV-SCE-2 contract):
/// syscall-context short read/write, no yield, no store.
#[inline]
pub fn task_borrow(p: *mut Task) -> Option<crate::process::task_ref::TaskBorrow> {
    // SAFETY: INV-SCE-2 — the callers' cgroup walkers hold the same
    // borrow proofs the pre-consolidation call sites documented.
    unsafe { crate::process::task_ref::TaskBorrow::from_raw(p) }
}

/// Derive a shared CgroupNode reference from a RANGE-CHECKED raw node
/// pointer (INV-SCE-8): the caller validated the address against the
/// heap/slab regions (task_cgroup's DEAD-containment) or holds an Arc
/// to the node (the subtree walk). The reference is read-only.
#[inline]
pub fn cgroup_node_ref(
    ptr: *mut super::cgroup::CgroupNode,
) -> Option<&'static super::cgroup::CgroupNode> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: INV-SCE-8 — range-checked/Arc-pinned node.
    Some(unsafe { &*ptr })
}

/// The trap frame at the top of a kernel-stack window (INV-SCE-9):
/// `top` came from `Task::kernel_stack_top_checked()` on a live task
/// (walker pin / census caller); the PtRegs-sized window below the top
/// is that task's entry frame — the same layout contract
/// `Task::pt_regs_ref` exposes. Diagnostic reads; the frame may be
/// mid-flight if the task runs concurrently (same contract as every
/// raw read this edge replaces).
#[inline]
pub fn pt_regs_at_top(top: usize) -> &'static crate::arch::pt_regs::PtRegs {
    debug_assert!(top > core::mem::size_of::<crate::arch::pt_regs::PtRegs>());
    // SAFETY: INV-SCE-9 — the fixed-offset window in a live task's
    // kernel stack (allocated with the task slot).
    unsafe {
        &*((top.wrapping_sub(core::mem::size_of::<crate::arch::pt_regs::PtRegs>()))
            as *const crate::arch::pt_regs::PtRegs)
    }
}

// ---------------------------------------------------------------------------
// INV-SCE-7: the RT priority lists' intrusive links
// ---------------------------------------------------------------------------

/// Next link of a GRQ-held intrusive list head/node (INV-SCE-7): the
/// RT runqueues' rings are only ever rewired under the GRQ; readers
/// hold the same lock (or walk a diagnostic snapshot).
#[inline]
pub fn list_next(node: *mut crate::list::ListHead) -> *mut crate::list::ListHead {
    debug_assert!(!node.is_null());
    // SAFETY: INV-SCE-7 — GRQ-held linkage; a bounded walk reads it.
    unsafe { (*node).next }
}

/// Container-of: the Task whose embedded `rt_run_list` is `pos`
/// (INV-SCE-7). Pure address arithmetic.
#[inline]
pub fn task_of_rt_list(pos: *mut crate::list::ListHead) -> *mut Task {
    let offset = core::mem::offset_of!(Task, rt_run_list);
    // SAFETY: pure pointer arithmetic, no dereference.
    unsafe { (pos as *mut u8).sub(offset) as *mut Task }
}

/// Link `task`'s RT node at a priority-list head (INV-SCE-7 write:
/// GRQ-held rewire).
#[inline]
pub fn rt_list_add(task: *mut Task, head: *mut crate::list::ListHead) {
    let t = task_mut(task);
    // SAFETY: INV-SCE-7 — the intrusive add under the GRQ.
    unsafe { t.rt_run_list.add(head) }
}

/// Tail variant of [rt_list_add].
#[inline]
pub fn rt_list_add_tail(task: *mut Task, head: *mut crate::list::ListHead) {
    let t = task_mut(task);
    // SAFETY: INV-SCE-7.
    unsafe { t.rt_run_list.add_tail(head) }
}

/// Unlink `task`'s RT node (INV-SCE-7 write).
#[inline]
pub fn rt_list_del(task: *mut Task) {
    let t = task_mut(task);
    // SAFETY: INV-SCE-7.
    unsafe { t.rt_run_list.del() }
}

/// Is `task`'s embedded RT list node linked (INV-SCE-7 read)? A freed
/// or never-enqueued node is self-linked/unset.
#[inline]
pub fn rt_list_linked(task: *mut Task) -> bool {
    debug_assert!(!task.is_null());
    // SAFETY: INV-SCE-7 — only the embedded node's link words are read,
    // under the caller's GRQ hold.
    unsafe {
        let node = core::ptr::addr_of!((*task).rt_run_list);
        let next = (*task).rt_run_list.next;
        !next.is_null() && next != node as *mut crate::list::ListHead
    }
}

// ---------------------------------------------------------------------------
// INV-SCE-6: the CFS timeline engine (moved from fair.rs, round L1)
// ---------------------------------------------------------------------------
//
// The entity-embedded intrusive red-black tree (the reference kernel's
// rb_root_cached shape). Contract: every op runs with the GRQ HELD —
// the linkage (parent/left/right/red/linked) is only ever touched by
// the GRQ-serialized enqueue/dequeue/pick paths, and the nodes
// themselves are embedded in Tasks whose queue membership pins them.
// Poison discipline (b50 #GP-guard family): every node pointer is
// range-checked (kernel_task_ptr_range_ok — a node lives inside its
// Task, so the Task range check covers it) BEFORE dereference; a node
// that fails is never unlinked or written: the op refuses, reports
// POISON-TL, and the kernel stays alive. The signatures are SAFE: the
// unsafe lives inside, one audited block per pointer family (the same
// blocks the pre-move code carried).

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct VruntimeKey {
    pub(crate) vruntime: u64,
    pub(crate) task_id: u64, // Used to distinguish tasks with same vruntime
}

impl VruntimeKey {
    pub(crate) fn new(vruntime: u64, task_id: u64) -> Self {
        Self { vruntime, task_id }
    }
}

/// grq-10ns: unit-test key constructor (tests/fair_timeline.rs drives a
/// local RbTimeline; the key type itself stays private to this module).

// ============================================================================
// grq-10ns: entity-embedded intrusive red-black timeline (the reference
// kernel's shape — struct sched_entity's rb_node + rb_root_cached).
//
// The pre-grq-10ns timeline was `alloc::collections::BTreeMap<VruntimeKey,
// *mut Task>`: every CFS enqueue/pick descended the B-tree under the GRQ
// lock with per-level key comparisons, and node split/merge could call the
// ALLOCATOR inside the critical section (alloc failure there panics the
// scheduler; R7-G1 already exiled one such allocation from pick). This
// section replaces it with the reference design: the tree NODE lives
// inside SchedEntity (`run_node`, exactly like sched_entity's embedded
// rb_node), so:
//   - insert/erase are pointer rewires — ZERO allocation under the lock
//     (BTreeMap split/merge paths gone by construction),
//   - the cached leftmost pointer (`rb_root_cached.rb_leftmost`) makes
//     pick/peek O(1),
//   - pick_next_cpu walks by SUCCESSOR (rb_next) instead of the old
//     remove-to-stash / reinsert churn — a refused candidate is simply
//     stepped past, never unlinked, so the 32-slot stash array's 768-byte
//     per-pick stack initialization and the per-skip tree mutations are
//     gone,
//   - erase does not need the sort key at all (the node's position IS
//     the linkage), so dequeue is keyless and the Leg-55 heal-scan
//     fallback disappears.
//
// Ordering keys: each entity's stored `timeline_key` (set at enqueue,
// immutable while linked — same Leg-55 discipline as before: update_curr
// mutates se.vruntime but never the sort key). All ops GRQ-HELD.
//
// Poison discipline (b50 #GP-guard family): every node pointer is
// range-checked with kernel_task_ptr_range_ok BEFORE dereference (a node
// lives inside its Task, so the Task range check covers it). A node that
// fails is NEVER unlinked or written — the op refuses, reports
// POISON-TL, and the kernel stays alive; the counters drift by the one
// entry (the old BTreeMap poison path orphaned the task too).
// ============================================================================

/// One intrusive red-black tree link, embedded in SchedEntity.
/// `linked` is the authoritative physical-linkage flag (set by insert,
/// cleared by erase — the stored key remains the ORDER key only).
#[derive(Debug)]
pub(crate) struct RbNode {
    pub(crate) parent: *mut RbNode,
    pub(crate) left: *mut RbNode,
    pub(crate) right: *mut RbNode,
    pub(crate) red: bool,
    pub(crate) linked: bool,
}

impl RbNode {
    /// Safe field read for policy code: the physical-linkage flag of a
    /// (GRQ-pinned) node, with the poison range check the engine applies.
    pub(crate) fn linked_at(node: *mut RbNode) -> bool {
        if node.is_null() || !Self::node_ok_shared(node) {
            return false;
        }
        // SAFETY: INV-SCE-6 — range-checked, GRQ-held node.
        unsafe { (*node).linked }
    }

    /// node_ok as an RbNode method (the engine's internal copy stays
    /// on RbTimeline; this is the same check for external readers).
    /// Detach a node during a timeline re-init sweep (INV-SCE-6):
    /// resets this node's link fields only — the successor was captured
    /// by the caller; the tree below/around is untouched.
    pub(crate) fn detach_at(node: *mut RbNode, nil: *mut RbNode) {
        debug_assert!(!node.is_null());
        // SAFETY: INV-SCE-6 — GRQ-held, range-checked node.
        unsafe {
            (*node).parent = nil;
            (*node).left = nil;
            (*node).right = nil;
            (*node).linked = false;
        }
    }

    pub(crate) fn node_ok_shared(node: *mut RbNode) -> bool {
        crate::sched::sched::kernel_task_ptr_range_ok(node as *const u8)
    }

    pub(crate) const fn unlinked() -> Self {
        Self {
            parent: core::ptr::null_mut(),
            left: core::ptr::null_mut(),
            right: core::ptr::null_mut(),
            red: false,
            linked: false,
        }
    }
}

/// The CLRS sentinel leaf. All child pointers that would be null point
/// here; its `parent` is transient state used by erase fixup (single
/// GRQ-held writer, consumed immediately). `red` stays false forever.
static mut RB_NIL: RbNode = RbNode::unlinked();

/// The sentinel pointer. `static mut` access discipline: taken as a raw
/// pointer once per op, never as a reference.
#[inline]
fn rb_nil() -> *mut RbNode {
    // SAFETY: address-of on a static mut; the pointer itself is stable.
    unsafe { core::ptr::addr_of_mut!(RB_NIL) }
}

/// Container-of helpers between a Task and its embedded timeline node.
/// `sched_entity` is pub(crate) in Task; run_node is the field above.
#[inline]
pub(crate) fn node_of_task(task: *mut Task) -> *mut RbNode {
    let off = core::mem::offset_of!(Task, sched_entity)
        + core::mem::offset_of!(SchedEntity, run_node);
    // SAFETY: pure pointer arithmetic, no dereference.
    unsafe { (task as *mut u8).add(off) as *mut RbNode }
}

#[inline]
pub(crate) fn task_of_node(node: *mut RbNode) -> *mut Task {
    let off = core::mem::offset_of!(Task, sched_entity)
        + core::mem::offset_of!(SchedEntity, run_node);
    // SAFETY: pure pointer arithmetic, no dereference.
    unsafe { (node as *mut u8).sub(off) as *mut Task }
}

/// The order key stored on the entity owning `node`. GRQ-held.
#[inline]
pub(crate) fn node_key(node: *mut RbNode) -> VruntimeKey {
    let off = core::mem::offset_of!(SchedEntity, run_node);
    // SAFETY: caller guarantees node is a linked node of this timeline
    // (in-range, embedded in a Task whose entity carries a Some key).
    let se = unsafe { (node as *mut u8).sub(off) as *mut SchedEntity };
    match unsafe { (*se).timeline_key() } {
        Some(k) => k,
        // Unreachable for linked nodes (insert stores the key before
        // linking); a scribbled key degrades to the maximal key so the
        // entry sorts last instead of reordering the pack.
        None => VruntimeKey::new(u64::MAX, u64::MAX),
    }
}

// ---- the tree itself -------------------------------------------------------

/// Intrusive red-black timeline root with the cached leftmost pointer
/// (the reference's `struct rb_root_cached`). pub(crate): the unit-test
/// suite drives a LOCAL instance (kernel/src/tests/fair_timeline.rs).
#[derive(Debug)]
pub(crate) struct RbTimeline {
    pub(crate) root: *mut RbNode,
    pub(crate) leftmost: *mut RbNode,
}

impl RbTimeline {
    pub(crate) const fn new() -> Self {
        Self {
            root: core::ptr::null_mut(),
            leftmost: core::ptr::null_mut(),
        }
    }

    #[inline]
    pub(crate) fn nil(&self) -> *mut RbNode {
        rb_nil()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        let nil = self.nil();
        self.root == nil || self.root.is_null()
    }

    /// Validate a node pointer before dereference (poison discipline).
    #[inline]
    pub(crate) fn node_ok(node: *mut RbNode) -> bool {
        crate::sched::sched::kernel_task_ptr_range_ok(node as *const u8)
    }

    /// Minimum (leftmost) node of the subtree rooted at `node`.
    fn subtree_min(&self, mut node: *mut RbNode) -> *mut RbNode {
        let nil = self.nil();
        // SAFETY: node range-checked by callers; hops re-checked.
        while node != nil && Self::node_ok(node) {
            let l = unsafe { (*node).left };
            if l == nil {
                return node;
            }
            node = l;
        }
        nil
    }

    /// In-order successor (rb_next). Returns NIL at the end.
    pub(crate) fn next(&self, node: *mut RbNode) -> *mut RbNode {
        let nil = self.nil();
        if !Self::node_ok(node) {
            return nil;
        }
        let r = unsafe { (*node).right };
        if r != nil {
            return unsafe { self.subtree_min(r) };
        }
        let mut n = node;
        let mut p = unsafe { (*n).parent };
        while p != nil && Self::node_ok(p) && n == unsafe { (*p).right } {
            n = p;
            p = unsafe { (*n).parent };
        }
        if p != nil && !Self::node_ok(p) {
            return nil;
        }
        p
    }

    fn rotate_left(&mut self, x: *mut RbNode) {
        let nil = self.nil();
        // SAFETY: x and y are linked, range-checked nodes (fixup only
        // touches nodes the descent validated).
        let y = unsafe { (*x).right };
        unsafe { (*x).right = (*y).left };
        if unsafe { (*y).left } != nil {
            unsafe { (*(*y).left).parent = x };
        }
        unsafe { (*y).parent = (*x).parent };
        let p = unsafe { (*x).parent };
        if p == nil {
            self.root = y;
        } else if unsafe { (*p).left } == x {
            unsafe { (*p).left = y };
        } else {
            unsafe { (*p).right = y };
        }
        unsafe { (*y).left = x };
        unsafe { (*x).parent = y };
    }

    fn rotate_right(&mut self, x: *mut RbNode) {
        let nil = self.nil();
        // SAFETY: as rotate_left.
        let y = unsafe { (*x).left };
        unsafe { (*x).left = (*y).right };
        if unsafe { (*y).right } != nil {
            unsafe { (*(*y).right).parent = x };
        }
        unsafe { (*y).parent = (*x).parent };
        let p = unsafe { (*x).parent };
        if p == nil {
            self.root = y;
        } else if unsafe { (*p).right } == x {
            unsafe { (*p).right = y };
        } else {
            unsafe { (*p).left = y };
        }
        unsafe { (*y).right = x };
        unsafe { (*x).parent = y };
    }

    /// Link `node` (whose entity key was just stored) into the tree.
    /// Allocation-free; O(log n) pointer chasing. Returns false (and
    /// touches nothing) when a descent hop lands on an out-of-range
    /// pointer — the poison discipline (caller reports).
    pub(crate) fn insert(&mut self, key: VruntimeKey, node: *mut RbNode) -> bool {
        // (pub(crate)-visible for the unit-test tree driver)
        let nil = self.nil();
        if !Self::node_ok(node) {
            return false;
        }
        unsafe {
            (*node).left = nil;
            (*node).right = nil;
            (*node).red = true;
        }
        if self.is_empty() {
            self.root = node;
            unsafe { (*node).parent = nil };
            unsafe { (*node).red = false };
            self.leftmost = node;
            unsafe { (*node).linked = true };
            return true;
        }
        let mut y = nil;
        let mut x = self.root;
        while x != nil {
            if !Self::node_ok(x) {
                return false;
            }
            y = x;
            if key < unsafe { node_key(x) } {
                x = unsafe { (*x).left };
            } else {
                x = unsafe { (*x).right };
            }
        }
        unsafe { (*node).parent = y };
        if key < unsafe { node_key(y) } {
            unsafe { (*y).left = node };
        } else {
            unsafe { (*y).right = node };
        }
        unsafe { (*node).linked = true };
        if key < unsafe { node_key(self.leftmost) } {
            self.leftmost = node;
        }
        self.insert_fixup(node);
        true
    }

    fn insert_fixup(&mut self, mut z: *mut RbNode) {
        let nil = self.nil();
        // SAFETY: fixup only touches linked, in-range nodes (guaranteed
        // by the insert descent's per-hop checks: parent chain members
        // were all on the descent path).
        unsafe {
            while (*(*z).parent).red {
                let p = (*z).parent;
                let g = (*p).parent;
                if p == (*g).left {
                    let u = (*g).right; // uncle
                    if (*u).red {
                        (*p).red = false;
                        (*u).red = false;
                        (*g).red = true;
                        z = g;
                    } else {
                        if z == (*p).right {
                            z = p;
                            self.rotate_left(z);
                        }
                        (*(*z).parent).red = false;
                        (*(*(*z).parent).parent).red = true;
                        self.rotate_right((*(*z).parent).parent);
                    }
                } else {
                    let u = (*g).left;
                    if (*u).red {
                        (*p).red = false;
                        (*u).red = false;
                        (*g).red = true;
                        z = g;
                    } else {
                        if z == (*p).left {
                            z = p;
                            self.rotate_right(z);
                        }
                        (*(*z).parent).red = false;
                        (*(*(*z).parent).parent).red = true;
                        self.rotate_left((*(*z).parent).parent);
                    }
                }
            }
            let _ = nil;
            (*self.root).red = false;
        }
    }

    /// Unlink `node` (allocation-free). The leftmost cache is advanced to
    /// the node's successor BEFORE splicing. Returns false without
    /// touching the tree when a neighbor hop is out of range.
    pub(crate) fn erase(&mut self, node: *mut RbNode) -> bool {
        let nil = self.nil();
        if !Self::node_ok(node) || !unsafe { (*node).linked } {
            return false;
        }
        unsafe {
            let node_was_black = !(*node).red;
            let left = (*node).left;
            let right = (*node).right;
            let parent = (*node).parent;
            if left != nil && !Self::node_ok(left) {
                return false;
            }
            if right != nil && !Self::node_ok(right) {
                return false;
            }
            if parent != nil && !Self::node_ok(parent) {
                return false;
            }
            if node == self.leftmost {
                self.leftmost = unsafe { self.next(node) };
            }

            if left == nil {
                // Case A: no left child — splice node out; the right
                // child (or NIL) takes its place.
                let child = right;
                if child != nil {
                    (*child).parent = parent;
                }
                if parent == nil {
                    self.root = child;
                } else if (*parent).left == node {
                    (*parent).left = child;
                } else {
                    (*parent).right = child;
                }
                if node_was_black {
                    self.erase_fixup(child, parent);
                }
            } else if right == nil {
                // Case B: no right child — the left child takes node's
                // place.
                (*left).parent = parent;
                if parent == nil {
                    self.root = left;
                } else if (*parent).left == node {
                    (*parent).left = left;
                } else {
                    (*parent).right = left;
                }
                if node_was_black {
                    self.erase_fixup(left, parent);
                }
            } else {
                // Case C: two children — the in-order successor (min of
                // the right subtree) is moved into node's structural
                // position (the reference rb_erase pointer swap; node
                // identity is the entity, so no key copying).
                let mut succ = right;
                let mut succ_parent;
                if (*succ).left == nil {
                    succ_parent = succ;
                } else {
                    loop {
                        succ_parent = succ;
                        succ = (*succ).left;
                        if !Self::node_ok(succ) {
                            return false;
                        }
                        if (*succ).left == nil {
                            break;
                        }
                    }
                    succ_parent = (*succ).parent;
                }
                let child2 = (*succ).right;
                if child2 != nil && !Self::node_ok(child2) {
                    return false;
                }
                let succ_was_black = !(*succ).red;

                if succ_parent != succ {
                    // splice succ out of its old spot first
                    if child2 != nil {
                        (*child2).parent = succ_parent;
                    }
                    (*succ_parent).left = child2;
                    // reattach node's right subtree under succ
                    (*succ).right = right;
                    (*right).parent = succ;
                }
                // succ takes node's position (parent + left subtree +
                // node's color).
                (*succ).parent = parent;
                if parent == nil {
                    self.root = succ;
                } else if (*parent).left == node {
                    (*parent).left = succ;
                } else {
                    (*parent).right = succ;
                }
                (*succ).left = left;
                (*left).parent = succ;
                (*succ).red = (*node).red;
                if succ_was_black {
                    self.erase_fixup(child2, succ_parent);
                }
            }

            (*node).parent = nil;
            (*node).left = nil;
            (*node).right = nil;
            (*node).linked = false;
        }
        true
    }

    /// CLRS RB-DELETE-FIXUP with the NIL sentinel: `x` may be NIL (its
    /// parent is carried in `xp`). Every pointer is range-checked before
    /// dereference; on a poison sighting the fixup stops early (caller
    /// reports; the tree may sit at a relaxed black-height until the
    /// census names the scribble — never a #GP).
    fn erase_fixup(&mut self, mut x: *mut RbNode, mut xp: *mut RbNode) {
        let nil = self.nil();
        // Read a validated child of a validated node; poison reads as
        // NIL so the red tests below degrade to "black" instead of
        // dereferencing a wild pointer.
        fn kid(p: *mut RbNode, left: bool, nil: *mut RbNode) -> *mut RbNode {
            // SAFETY: p is validated by the caller.
            let c = unsafe { if left { (*p).left } else { (*p).right } };
            if c != nil && RbTimeline::node_ok(c) {
                c
            } else {
                nil
            }
        }
        unsafe {
            if x != nil && !Self::node_ok(x) {
                x = nil;
            }
            while x != self.root && (x == nil || !(*x).red) {
                if xp == nil || !Self::node_ok(xp) {
                    break;
                }
                let x_is_left = x == kid(xp, true, nil);
                if x_is_left {
                    let mut w = kid(xp, false, nil);
                    if w == nil {
                        break;
                    }
                    if (*w).red {
                        (*w).red = false;
                        (*xp).red = true;
                        self.rotate_left(xp);
                        w = kid(xp, false, nil);
                        if w == nil {
                            break;
                        }
                    }
                    let wl = kid(w, true, nil);
                    let wr = kid(w, false, nil);
                    if (wl == nil || !(*wl).red) && (wr == nil || !(*wr).red) {
                        (*w).red = true;
                        x = xp;
                        xp = (*x).parent;
                    } else {
                        if wr == nil || !(*wr).red {
                            // wl is red here (valid-tree invariant)
                            (*wl).red = false;
                            (*w).red = true;
                            self.rotate_right(w);
                            w = kid(xp, false, nil);
                            if w == nil {
                                break;
                            }
                        }
                        (*w).red = (*xp).red;
                        (*xp).red = false;
                        let wr2 = kid(w, false, nil);
                        if wr2 != nil {
                            (*wr2).red = false;
                        }
                        self.rotate_left(xp);
                        x = self.root;
                    }
                } else {
                    let mut w = kid(xp, true, nil);
                    if w == nil {
                        break;
                    }
                    if (*w).red {
                        (*w).red = false;
                        (*xp).red = true;
                        self.rotate_right(xp);
                        w = kid(xp, true, nil);
                        if w == nil {
                            break;
                        }
                    }
                    let wl = kid(w, true, nil);
                    let wr = kid(w, false, nil);
                    if (wl == nil || !(*wl).red) && (wr == nil || !(*wr).red) {
                        (*w).red = true;
                        x = xp;
                        xp = (*x).parent;
                    } else {
                        if wl == nil || !(*wl).red {
                            // wr is red here (valid-tree invariant)
                            (*wr).red = false;
                            (*w).red = true;
                            self.rotate_left(w);
                            w = kid(xp, true, nil);
                            if w == nil {
                                break;
                            }
                        }
                        (*w).red = (*xp).red;
                        (*xp).red = false;
                        let wl2 = kid(w, true, nil);
                        if wl2 != nil {
                            (*wl2).red = false;
                        }
                        self.rotate_right(xp);
                        x = self.root;
                    }
                }
                if x != nil && !Self::node_ok(x) {
                    x = nil;
                }
            }
            if x != nil && Self::node_ok(x) {
                (*x).red = false;
            }
        }
    }
}

/// Shared view of a stored GRQ pointer (INV-SCE-3 twin for the lock
/// guard's held `*const GlobalRunQueue`): the guard's construction
/// passed the ready gate; read-only access through the guard.
#[inline]
pub fn grq_ptr_shared(p: *const GlobalRunQueue) -> &'static GlobalRunQueue {
    // SAFETY: INV-SCE-3 — the guard holds the initialized GRQ.
    unsafe { &*p }
}

/// Mutable view of a stored GRQ pointer (INV-SCE-3 twin): mutation is
/// serialized by the guard's own lock.
#[inline]
pub fn grq_ptr_mut(p: *const GlobalRunQueue) -> &'static mut GlobalRunQueue {
    // SAFETY: INV-SCE-3 — same gate; the caller holds the lock.
    unsafe { &mut *(p as *mut GlobalRunQueue) }
}
