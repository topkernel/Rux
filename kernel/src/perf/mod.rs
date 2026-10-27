//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! perf events — software-clock sampling (`perf_event_open(2)` ABI).
//!
//! Scope of this first slice (the plan doc's perf tranche, part 1):
//! `PERF_TYPE_SOFTWARE` counters `PERF_COUNT_SW_CPU_CLOCK` (0) and
//! `PERF_COUNT_SW_TASK_CLOCK` (1) with `sample_period` (ns of counted
//! time per sample) and `PERF_SAMPLE_IP` records consumed through
//! `read(2)` on the returned fd. Everything else keeps returning
//! `EINVAL`/`ENOSYS` as before — no invented interfaces: the attr
//! layout, the open flag set, the pid/cpu scoping rules, the record
//! binary layout (`perf_event_header` + u64 ip) and the
//! `PERF_RECORD_LOST` overflow report are perf_event_open(2)'s.
//!
//! ## How samples are produced (internal design)
//!
//! The hard timer IRQ (the arch trap paths, next to the syscost tick
//! hooks) calls [`note_tick`] with the interrupted PC, the privilege
//! mode and the interrupted task's TID. That hook is LOCK-FREE: one
//! relaxed load to bail when nothing is armed, otherwise three
//! per-CPU atomic stores + a sequence bump — hard-IRQ context takes
//! no locks here.
//!
//! The Hrtimer softirq (raised every tick while any event is armed;
//! see `timer_softirq_work_pending`) calls [`softirq_tick`] on the
//! same CPU, which charges `pending_ticks * TICK_NS` to every
//! matching armed event and emits a sample whenever the accumulated
//! counter crosses `sample_period` (catch-up loop, so a burst of
//! ticks between softirq passes yields the right sample count).
//! Charging and waking readers happen in softirq context where locks
//! and wait-queue wakes are normal (the timerfd expire notify runs in
//! this same context).
//!
//! Granularity note: samples can only be produced at tick (or
//! softirq-pass) boundaries, so a `sample_period` below one tick
//! (10 ms at KERNEL_HZ=100) coalesces to tick-rate samples rather
//! than being lost. The counter arithmetic itself is exact ns.
//!
//! ## Lock order (INV: registry → event.inner)
//!
//! `REGISTRY` (irqsave) is the outer lock; an event's `inner` is
//! taken under it by the charging path. `read`/`poll` take `inner`
//! alone. All REGISTRY takes are `lock_irqsave()` uniformly, so no
//! holder can ever be interrupted into a path that wants it.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ==================== ABI constants (uapi/linux/perf_event.h) ====================

/// attr.type — software events.
pub const PERF_TYPE_SOFTWARE: u32 = 1;
/// attr.config — CPU clock (per-CPU or per-task ns).
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
/// attr.config — task clock (per-task ns).
pub const PERF_COUNT_SW_TASK_CLOCK: u64 = 1;
/// sample_type bit — record the interrupted instruction pointer.
pub const PERF_SAMPLE_IP: u64 = 1 << 0;

/// attr bitfield (u64 at offset 40) bits this slice reads.
const FLAG_DISABLED: u64 = 1 << 0;
const FLAG_INHERIT: u64 = 1 << 1;
const FLAG_EXCLUDE_USER: u64 = 1 << 4;
const FLAG_EXCLUDE_KERNEL: u64 = 1 << 5;
const FLAG_EXCLUDE_IDLE: u64 = 1 << 7;
const FLAG_FREQ: u64 = 1 << 10;

/// perf_event_open flags argument.
const PERF_FLAG_FD_NO_GROUP: u64 = 1 << 1;
const PERF_FLAG_FD_OUTPUT: u64 = 1 << 2;
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;
const PERF_FLAG_PID_CGROUP: u64 = 1 << 4;

/// Record types (perf_event_header.type).
const PERF_RECORD_LOST: u32 = 2;
const PERF_RECORD_SAMPLE: u32 = 9;
/// perf_event_header.misc cpumode bits.
const PERF_RECORD_MISC_USER: u16 = 1;
const PERF_RECORD_MISC_KERNEL: u16 = 2;

/// The mandatory head of `struct perf_event_attr` — everything this
/// slice reads lives in the first 48 bytes. `size` (the user's own
/// attr->size) gates how much of the struct the user filled in; the
/// ABI minimum (PERF_ATTR_SIZE_VER0) is 64.
#[repr(C)]
#[derive(Clone, Copy)]
struct AttrHead {
    type_: u32,   // offset 0
    size: u32,    // offset 4
    config: u64,  // offset 8
    sample_period: u64, // offset 16 (sample_freq when FLAG_FREQ)
    sample_type: u64,   // offset 24
    read_format: u64,   // offset 32
    flags1: u64,        // offset 40 (bitfield: disabled..)
}

/// ns per tick — the accounting quantum.
const TICK_NS: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

/// Pending-record byte budget per event. A sample is 16 bytes, so
/// this holds 4096 samples; beyond it new samples bump `lost` and are
/// reported via PERF_RECORD_LOST on the next read.
const RING_CAP: usize = 64 * 1024;

// ==================== Per-CPU tick stash (hard IRQ → softirq) ====================

/// Tick privilege modes for the stash.
pub const MODE_KERNEL: u8 = 0;
pub const MODE_USER: u8 = 1;
pub const MODE_IDLE: u8 = 2;

struct TickStash {
    pc: AtomicU64,
    mode: AtomicU8,
    tid: AtomicU32,
    /// Incremented by every armed tick; the softirq charges the
    /// delta since its last pass (normally 1).
    seq: AtomicU64,
    /// seq already charged by the softirq.
    charged: AtomicU64,
}

const fn new_stash() -> TickStash {
    TickStash {
        pc: AtomicU64::new(0),
        mode: AtomicU8::new(MODE_IDLE),
        tid: AtomicU32::new(0),
        seq: AtomicU64::new(0),
        charged: AtomicU64::new(0),
    }
}

static STASH: [TickStash; crate::config::MAX_CPUS] = [const { new_stash() }; crate::config::MAX_CPUS];

/// Armed-and-enabled event count: the hard-IRQ bail-out and the
/// softirq-raise gate (`timer_softirq_work_pending`) read this; open
/// and close maintain it. When 0 the subsystem costs one relaxed load
/// per tick.
static ARMED: AtomicUsize = AtomicUsize::new(0);

/// Hard-IRQ tick hook (arch trap paths). `tid` 0 = idle/no task.
/// Lock-free by contract — see the module doc.
#[inline]
pub fn note_tick(pc: u64, mode: u8, tid: u32) {
    if ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c >= crate::config::MAX_CPUS {
        return;
    }
    let s = &STASH[c];
    s.pc.store(pc, Ordering::Relaxed);
    s.mode.store(mode, Ordering::Relaxed);
    s.tid.store(tid, Ordering::Relaxed);
    s.seq.fetch_add(1, Ordering::Release);
}

/// Softirq-raise gate: is any event armed (the Hrtimer softirq must
/// run every tick so the stash gets charged)?
pub fn any_armed() -> bool {
    ARMED.load(Ordering::Acquire) > 0
}

// ==================== Event + registry ====================

/// One open perf event fd. Box'd, owned by the File's private_data;
/// the REGISTRY keeps the raw pointer until close unregisters it.
pub(crate) struct PerfEvent {
    id: u64,
    /// PERF_COUNT_SW_CPU_CLOCK / PERF_COUNT_SW_TASK_CLOCK.
    config: u64,
    sample_period: u64,
    /// TID to match (task-scoped events); 0 in per-CPU mode.
    target_tid: u32,
    /// CPU to match; -1 = task-scoped, else per-CPU.
    target_cpu: i32,
    exclude_user: bool,
    exclude_kernel: bool,
    exclude_idle: bool,
    enabled: AtomicBool,
    inner: Spinlock<PerfInner>,
    /// Readers blocked in read() waiting for records.
    wait_queue: WaitQueueHead,
}

struct PerfInner {
    /// ns accumulated since the last sample.
    counter: u64,
    /// Pending record bytes (whole records, little-endian).
    ring: VecDeque<u8>,
    /// Samples dropped because the ring was full (reported as
    /// PERF_RECORD_LOST on the next read).
    lost: u64,
}

/// id → event. Every take is lock_irqsave (module lock-order doc).
static REGISTRY: Spinlock<alloc::vec::Vec<(u64, usize)>> =
    Spinlock::new(alloc::vec::Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn register(ev: *mut PerfEvent, starts_armed: bool) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // SAFETY: caller built the box; the registry keeps the pointer
    // valid until unregister.
    unsafe {
        (*ev).id = id;
    }
    let mut reg = REGISTRY.lock_irqsave();
    reg.push((id, ev as usize));
    drop(reg);
    if starts_armed {
        ARMED.fetch_add(1, Ordering::Release);
    }
    id
}

/// Remove `id` from the registry. Returns true if it was armed (the
/// caller adjusts ARMED).
fn unregister(id: u64) -> bool {
    let mut reg = REGISTRY.lock_irqsave();
    let was_armed = match reg.iter().position(|&(i, _)| i == id) {
        Some(idx) => {
            let (_, p) = reg.swap_remove(idx);
            // SAFETY: pointer came from register, still valid.
            let armed = unsafe { (*(p as *mut PerfEvent)).enabled.load(Ordering::Acquire) };
            drop(reg);
            return armed;
        }
        None => false,
    };
    drop(reg);
    was_armed
}

/// Charge pending ticks and emit samples. Called from the Hrtimer
/// softirq on the CPU whose stash is being charged.
pub fn softirq_tick() {
    if ARMED.load(Ordering::Acquire) == 0 {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c >= crate::config::MAX_CPUS {
        return;
    }
    let s = &STASH[c];
    let seq = s.seq.load(Ordering::Acquire);
    let pending = seq.saturating_sub(s.charged.load(Ordering::Relaxed));
    if pending == 0 {
        return;
    }
    s.charged.store(seq, Ordering::Relaxed);
    let pc = s.pc.load(Ordering::Relaxed);
    let mode = s.mode.load(Ordering::Relaxed);
    let tid = s.tid.load(Ordering::Relaxed);

    let mut to_wake: alloc::vec::Vec<*mut PerfEvent> = alloc::vec::Vec::new();
    {
        let reg = REGISTRY.lock_irqsave();
        for &(_, p) in reg.iter() {
            // SAFETY: registry membership guarantees validity (close
            // unregisters before freeing).
            let ev = unsafe { &*(p as *const PerfEvent) };
            if !ev.enabled.load(Ordering::Relaxed) {
                continue;
            }
            // Scope match: per-CPU events match this CPU (idle ticks
            // included — cpu-clock counts idle time unless excluded);
            // task-scoped events match the interrupted TID only.
            match ev.target_cpu {
                cpu if cpu >= 0 => {
                    if cpu as usize != c {
                        continue;
                    }
                }
                _ => {
                    if tid == 0 || tid != ev.target_tid {
                        continue;
                    }
                }
            }
            // Privilege filter (attr exclude_* bits).
            match mode {
                MODE_USER if ev.exclude_user => continue,
                MODE_KERNEL if ev.exclude_kernel => continue,
                MODE_IDLE if ev.exclude_idle => continue,
                _ => {}
            }
            let emitted = {
                let mut inner = ev.inner.lock();
                inner.counter = inner
                    .counter
                    .saturating_add(pending as u64 * TICK_NS);
                let mut emitted = 0u64;
                while inner.counter >= ev.sample_period && ev.sample_period > 0 {
                    inner.counter -= ev.sample_period;
                    if inner.ring.len() + SAMPLE_REC_LEN <= RING_CAP {
                        push_sample(&mut inner.ring, pc, mode);
                        emitted += 1;
                    } else {
                        inner.lost += 1;
                    }
                }
                emitted
            };
            if emitted > 0 {
                to_wake.push(p as *mut PerfEvent);
            }
        }
    }
    // Wakes with no locks held (timerfd expire-notify discipline).
    for p in to_wake {
        // SAFETY: as above.
        unsafe { (*p).wait_queue.wake_up_all() };
    }
}

/// One PERF_RECORD_SAMPLE with PERF_SAMPLE_IP: header (8 B) + ip (8 B).
const SAMPLE_REC_LEN: usize = 16;
/// PERF_RECORD_LOST: header (8 B) + u64 id + u64 lost.
const LOST_REC_LEN: usize = 24;

fn push_sample(ring: &mut VecDeque<u8>, pc: u64, mode: u8) {
    let misc = if mode == MODE_USER {
        PERF_RECORD_MISC_USER
    } else {
        PERF_RECORD_MISC_KERNEL
    };
    let mut rec = [0u8; SAMPLE_REC_LEN];
    rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
    rec[4..6].copy_from_slice(&misc.to_le_bytes());
    rec[6..8].copy_from_slice(&(SAMPLE_REC_LEN as u16).to_le_bytes());
    rec[8..16].copy_from_slice(&pc.to_le_bytes());
    for b in rec {
        ring.push_back(b);
    }
}

/// PERF_RECORD_LOST record bytes for (id, lost).
pub(crate) fn lost_record_bytes(id: u64, lost: u64) -> [u8; LOST_REC_LEN] {
    let mut rec = [0u8; LOST_REC_LEN];
    rec[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
    rec[4..6].copy_from_slice(&0u16.to_le_bytes());
    rec[6..8].copy_from_slice(&(LOST_REC_LEN as u16).to_le_bytes());
    rec[8..16].copy_from_slice(&id.to_le_bytes());
    rec[16..24].copy_from_slice(&lost.to_le_bytes());
    rec
}

/// Drain whole records into `buf`. Emits the pending LOST record
/// (if any) first, then samples. Returns bytes copied, or
/// Some(0)/None semantics: Err(()) = a record is pending but does
/// not fit `buf` at all (the caller surfaces EINVAL).
fn drain_records(ev: &PerfEvent, buf: &mut [u8]) -> Result<usize, ()> {
    let mut inner = ev.inner.lock();
    let mut copied = 0usize;

    // Overflow report first, per the ABI's lost-record contract.
    if inner.lost > 0 {
        if buf.len() < LOST_REC_LEN {
            return Err(());
        }
        let rec = lost_record_bytes(ev.id, inner.lost);
        buf[..LOST_REC_LEN].copy_from_slice(&rec);
        copied += LOST_REC_LEN;
        inner.lost = 0;
    }

    while inner.ring.len() >= SAMPLE_REC_LEN && buf.len() - copied >= SAMPLE_REC_LEN {
        for b in buf[copied..copied + SAMPLE_REC_LEN].iter_mut() {
            *b = inner.ring.pop_front().unwrap_or(0);
        }
        copied += SAMPLE_REC_LEN;
    }

    if copied == 0 && !inner.ring.is_empty() {
        // A sample is pending but buf cannot hold one record.
        return Err(());
    }
    Ok(copied)
}

/// Are records pending (poll readiness)?
fn records_pending(ev: &PerfEvent) -> bool {
    let inner = ev.inner.lock();
    !inner.ring.is_empty() || inner.lost > 0
}

// ==================== File operations ====================

/// read(2): blocking record drain (timerfd read model).
fn perf_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // The smallest record we can produce is a 16-byte sample.
    if buf.len() < SAMPLE_REC_LEN {
        return -(crate::errno::constants::EINVAL as isize);
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no
    // concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(crate::errno::constants::EBADF as isize),
    };
    // SAFETY: installed by sys_perf_event_open as Box::into_raw; valid
    // while the File exists.
    let ev = unsafe { &*(ptr as *const PerfEvent) };

    if !records_pending(ev) {
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -(crate::errno::constants::EAGAIN as isize);
        }
        loop {
            let current = match crate::sched::current() {
                Some(t) => t,
                None => return -(crate::errno::constants::EAGAIN as isize),
            };
            ev.wait_queue.prepare_to_wait(current, false, true);
            if records_pending(ev) {
                ev.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                break;
            }
            if crate::signal::signal_pending() {
                ev.wait_queue.finish_wait(current);
                // SAFETY: as above.
                unsafe { crate::sched::dequeue_task(&*current); }
                return -(crate::errno::constants::EINTR) as isize;
            }
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            ev.wait_queue.finish_wait(current);
            if crate::signal::signal_pending() {
                return -(crate::errno::constants::EINTR) as isize;
            }
        }
    }

    match drain_records(ev, buf) {
        Ok(0) => -(crate::errno::constants::EAGAIN as isize), // raced with another reader
        Ok(n) => n as isize,
        Err(()) => -(crate::errno::constants::EINVAL as isize),
    }
}

fn perf_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    // SAFETY: see perf_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    if events & POLLIN != 0 && records_pending(ev) {
        ready |= POLLIN | POLLRDNORM;
    }
    ready
}

fn perf_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see perf_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    let id = ev.id;
    let was_armed = unregister(id);
    if was_armed {
        ARMED.fetch_sub(1, Ordering::Release);
    }
    // Residual race (mirrors timerfd_close): a reader still inside
    // perf_read holds &File via its own reference; close runs on the
    // last fd release, and the same one-softirq-pass window applies.
    // The registry removal above already stopped all sampler access.
    // SAFETY: ptr came from Box::into_raw in sys_perf_event_open.
    unsafe {
        let _ = Box::from_raw(ptr as *mut PerfEvent);
    }
    unsafe { *file.private_data.get() = None; }
    0
}

/// perf event file operations (timerfd model: read/poll/close).
pub(crate) static PERF_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(perf_read)
    .close(perf_close)
    .poll(perf_poll)
    .build();

/// Poll-wait registration (event-driven poll, misc dispatcher).
pub(crate) fn perf_poll_wait(
    file: &crate::fs::File,
    task: *mut crate::process::task::Task,
) -> bool {
    // SAFETY: dispatcher identity-checked the File against PERF_OPS.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return false,
    };
    // SAFETY: as above.
    let ev = unsafe { &*(ptr as *const PerfEvent) };
    ev.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

pub(crate) fn perf_poll_unwait(
    file: &crate::fs::File,
    task: *mut crate::process::task::Task,
) {
    if let Some(p) = unsafe { *file.private_data.get() } {
        // SAFETY: as above.
        let ev = unsafe { &*(p as *const PerfEvent) };
        ev.wait_queue.remove(task);
    }
}

// ==================== syscall ====================

/// sys_perf_event_open(attr, pid, cpu, group_fd, flags) — the scoped
/// software-clock slice. See the module doc for what is accepted;
/// everything unsupported returns EINVAL (validated configs we cannot
/// honor) rather than silently misbehaving.
pub fn sys_perf_event_open(args: crate::syscall::SyscallArgs) -> i64 {
    let attr_ptr = args[0] as *const AttrHead;
    let pid = args[1] as i32;
    let cpu = args[2] as i32;
    let group_fd = args[3] as i32;
    let flags = args[4];

    // 1. Flags: only the bits this slice can honor.
    if flags & !(PERF_FLAG_FD_NO_GROUP | PERF_FLAG_FD_CLOEXEC) != 0 {
        // FD_OUTPUT (ring redirect) and PID_CGROUP (cgroup scoping)
        // are real ABI features we do not implement — EINVAL.
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 2. Read the attr head. The mandatory part is 64 bytes; we only
    // decode the first 48 but require the user's size to cover VER0.
    if !crate::arch::uaccess::access_ok(attr_ptr as usize, 64) {
        return -(crate::errno::constants::EFAULT as i64);
    }
    // SAFETY: access_ok passed; get_user re-verifies page-wise.
    let attr = match unsafe {
        crate::arch::uaccess::get_user(attr_ptr as *const AttrHead)
    } {
        Some(a) => a,
        None => return -(crate::errno::constants::EFAULT as i64),
    };
    if attr.size < 64 || attr.size > 1024 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 3. Event class.
    if attr.type_ != PERF_TYPE_SOFTWARE {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if attr.config != PERF_COUNT_SW_CPU_CLOCK && attr.config != PERF_COUNT_SW_TASK_CLOCK {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 4. Sampling mode: period-based, IP-only.
    if attr.flags1 & FLAG_FREQ != 0 {
        return -(crate::errno::constants::EINVAL as i64); // freq mode not implemented
    }
    if attr.sample_type != PERF_SAMPLE_IP {
        return -(crate::errno::constants::EINVAL as i64); // this slice fills exactly IP
    }
    if attr.read_format != 0 {
        return -(crate::errno::constants::EINVAL as i64); // no group/total time read formats
    }
    if attr.flags1 & FLAG_INHERIT != 0 {
        return -(crate::errno::constants::EINVAL as i64); // no child inheritance
    }
    if attr.sample_period == 0 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    // 5. Scoping. pid: 0 = calling task, >0 = that TID, -1 = any task
    // on the given CPU (cpu-clock per-CPU form); both -1 = EINVAL.
    if group_fd != -1 {
        return -(crate::errno::constants::EINVAL as i64); // no event groups
    }
    if cpu < -1 || (cpu >= 0 && cpu as usize >= crate::config::MAX_CPUS) {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if pid < -1 {
        return -(crate::errno::constants::EINVAL as i64);
    }
    if pid == -1 && cpu == -1 {
        return -(crate::errno::constants::EINVAL as i64);
    }

    let (target_tid, target_cpu, starts_armed);
    if cpu >= 0 {
        // Per-CPU form: pid must be -1 (the reference's per-CPU
        // measurement measures the CPU, not a task).
        if pid != -1 {
            return -(crate::errno::constants::EINVAL as i64);
        }
        target_tid = 0;
        target_cpu = cpu;
    } else {
        // Task-scoped: pid 0 = self.
        let tid = if pid == 0 {
            match crate::sched::current() {
                Some(t) => unsafe { (*t).pid() },
                None => return -(crate::errno::constants::EINVAL as i64),
            }
        } else {
            pid as u32
        };
        // Existence + permission (same real uid or root, the
        // reference's perf permission rule simplified to uid).
        // SAFETY: pinned lookup keeps the Task alive across the check.
        unsafe {
            let target = crate::process::pid_hash::pid_hash_lookup_pinned(tid);
            if target.is_null() {
                return -(crate::errno::constants::ESRCH as i64);
            }
            let ok = if let Some(me) = crate::sched::current() {
                (*target).cred().uid == (*me).cred().uid || (*me).cred().uid == 0
            } else {
                false
            };
            crate::process::task::Task::task_put(target);
            if !ok {
                return -(crate::errno::constants::EACCES as i64);
            }
        }
        target_tid = tid;
        target_cpu = -1;
    }

    starts_armed = attr.flags1 & FLAG_DISABLED == 0;

    // 6. Build + install the fd.
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(crate::errno::constants::EMFILE as i64),
    };

    let ev = Box::new(PerfEvent {
        id: 0,
        config: attr.config,
        sample_period: attr.sample_period,
        target_tid,
        target_cpu,
        exclude_user: attr.flags1 & FLAG_EXCLUDE_USER != 0,
        exclude_kernel: attr.flags1 & FLAG_EXCLUDE_KERNEL != 0,
        exclude_idle: attr.flags1 & FLAG_EXCLUDE_IDLE != 0,
        enabled: AtomicBool::new(starts_armed),
        inner: Spinlock::new(PerfInner {
            counter: 0,
            ring: VecDeque::new(),
            lost: 0,
        }),
        wait_queue: WaitQueueHead::new(),
    });
    let ev_ptr = Box::into_raw(ev);
    register(ev_ptr, starts_armed);

    let file_flags = crate::fs::file::FileFlags::O_RDONLY;
    let file = Arc::new(crate::fs::File::new(
        crate::fs::file::FileFlags::new(file_flags),
    ));
    file.set_ops(&PERF_OPS);
    file.set_private_data(ev_ptr as *mut u8);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // Undo the registration (the id was stored in the event).
            let was_armed = unregister(unsafe { (*ev_ptr).id });
            if was_armed {
                ARMED.fetch_sub(1, Ordering::Release);
            }
            // SAFETY: reclaim the box.
            unsafe { let _ = Box::from_raw(ev_ptr); }
            return -(crate::errno::constants::EMFILE as i64);
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {
            if flags & PERF_FLAG_FD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            fd as i64
        }
        Err(_) => {
            let was_armed = unregister(unsafe { (*ev_ptr).id });
            if was_armed {
                ARMED.fetch_sub(1, Ordering::Release);
            }
            // SAFETY: reclaim the box.
            unsafe { let _ = Box::from_raw(ev_ptr); }
            -(crate::errno::constants::EMFILE as i64)
        }
    }
}

// ==================== unit tests ====================

#[cfg(any(test, feature = "unit-test"))]
pub mod tests {
    use super::*;

    fn sample_record(pc: u64, user: bool) -> [u8; SAMPLE_REC_LEN] {
        let mut rec = [0u8; SAMPLE_REC_LEN];
        rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
        let misc = if user { PERF_RECORD_MISC_USER } else { PERF_RECORD_MISC_KERNEL };
        rec[4..6].copy_from_slice(&misc.to_le_bytes());
        rec[6..8].copy_from_slice(&(SAMPLE_REC_LEN as u16).to_le_bytes());
        rec[8..16].copy_from_slice(&pc.to_le_bytes());
        rec
    }

    /// Record encoding: sample and lost records match the ABI layout.
    pub fn test_record_encoding() {
        let r = sample_record(0x1234_5678_9abc_def0, true);
        let expect: [u8; 16] = [
            9, 0, 0, 0, // type=PERF_RECORD_SAMPLE
            1, 0, // misc=USER
            16, 0, // size=16
            0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12, // ip LE
        ];
        if r != expect {
            crate::tests::test_fail("perf-record", "sample record bytes mismatch");
            return;
        }
        let l = lost_record_bytes(7, 3);
        let mut ok = true;
        ok &= l[0..4] == PERF_RECORD_LOST.to_le_bytes();
        ok &= l[6..8] == (LOST_REC_LEN as u16).to_le_bytes();
        ok &= l[8..16] == 7u64.to_le_bytes();
        ok &= l[16..24] == 3u64.to_le_bytes();
        if ok {
            crate::tests::test_pass("perf-record");
        } else {
            crate::tests::test_fail("perf-record", "lost record bytes mismatch");
        }
    }

    /// push_sample/drain round trip through the byte ring.
    pub fn test_ring_roundtrip() {
        let ev = PerfEvent {
            id: 42,
            config: PERF_COUNT_SW_CPU_CLOCK,
            sample_period: TICK_NS,
            target_tid: 0,
            target_cpu: 0,
            exclude_user: false,
            exclude_kernel: false,
            exclude_idle: false,
            enabled: AtomicBool::new(true),
            inner: Spinlock::new(PerfInner {
                counter: 0,
                ring: VecDeque::new(),
                lost: 0,
            }),
            wait_queue: WaitQueueHead::new(),
        };
        {
            let mut inner = ev.inner.lock();
            push_sample(&mut inner.ring, 0xaabb, MODE_KERNEL);
            push_sample(&mut inner.ring, 0xccdd, MODE_USER);
        }
        let mut buf = [0u8; 40];
        match drain_records(&ev, &mut buf) {
            Ok(n) => {
                let want0 = sample_record(0xaabb, false);
                let want1 = sample_record(0xccdd, true);
                if n == 32 && buf[..16] == want0 && buf[16..32] == want1 {
                    crate::tests::test_pass("perf-ring");
                } else {
                    crate::tests::test_fail("perf-ring", "drained bytes mismatch");
                }
            }
            Err(()) => {
                crate::tests::test_fail("perf-ring", "unexpected EINVAL");
            }
        }
        // Lost-first ordering: a lost count precedes ring samples.
        {
            let ev2 = PerfEvent {
                id: 43,
                config: PERF_COUNT_SW_CPU_CLOCK,
                sample_period: TICK_NS,
                target_tid: 0,
                target_cpu: 0,
                exclude_user: false,
                exclude_kernel: false,
                exclude_idle: false,
                enabled: AtomicBool::new(true),
                inner: Spinlock::new(PerfInner {
                    counter: 0,
                    ring: VecDeque::new(),
                    lost: 5,
                }),
                wait_queue: WaitQueueHead::new(),
            };
            let mut inner = ev2.inner.lock();
            push_sample(&mut inner.ring, 0x1, MODE_KERNEL);
            drop(inner);
            let mut b2 = [0u8; 64];
            match drain_records(&ev2, &mut b2) {
                Ok(n) => {
                    let lost = lost_record_bytes(43, 5);
                    if n == 40 && b2[..24] == lost {
                        crate::tests::test_pass("perf-lost");
                    } else {
                        crate::tests::test_fail("perf-lost", "lost record not first");
                    }
                }
                Err(()) => {
                    crate::tests::test_fail("perf-lost", "unexpected EINVAL");
                }
            }
        }
    }

    /// A buffer too small for one record is EINVAL.
    pub fn test_short_buffer() {
        let ev = PerfEvent {
            id: 44,
            config: PERF_COUNT_SW_CPU_CLOCK,
            sample_period: TICK_NS,
            target_tid: 0,
            target_cpu: 0,
            exclude_user: false,
            exclude_kernel: false,
            exclude_idle: false,
            enabled: AtomicBool::new(true),
            inner: Spinlock::new(PerfInner {
                counter: 0,
                ring: VecDeque::new(),
                lost: 0,
            }),
            wait_queue: WaitQueueHead::new(),
        };
        {
            let mut inner = ev.inner.lock();
            push_sample(&mut inner.ring, 0x99, MODE_USER);
        }
        let mut buf = [0u8; 8];
        match drain_records(&ev, &mut buf) {
            Ok(_) => crate::tests::test_fail("perf-short", "short buffer accepted"),
            Err(()) => crate::tests::test_pass("perf-short"),
        }
    }
}
