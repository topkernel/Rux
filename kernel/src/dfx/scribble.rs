//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Byte-level scribble hunter (x86 fork/exec storm corruption).
//!
//! Tracks a shadow copy of the scheduling-critical fields of every live
//! Task — `thread.{fs_base, gs_base, sp, callee.ret_addr}` plus the
//! parked `pt_regs` control words (rip/cs/rflags/rsp/ss) — and flags any
//! change that does not flow through a legitimate write point:
//!
//! - **switch-out quiesce check** (`quiesce` called from
//!   `x86_switch_publish`): the task is fully saved at that instant, so
//!   any field that differs from its shadow was scribbled WHILE the
//!   task was running (or between its last quiesce and now).
//! - **periodic verify** (`verify` called from the timer tick): catches
//!   scribbles landing on quiescent sleeping tasks — the observed
//!   `gs_base <- small int` family, where the garbage value is loaded
//!   into MSR_KERNEL_GS_BASE at the task's NEXT switch-in.
//!
//! All reporting goes through the raw console path (`taskdump_*` /
//! `put_hex`) so it works with locks held or state half-torn.
//!
//! GDB hook: on the first confirmed hit the detector calls
//! `scribble_park(field_addr)` (a `#[no_mangle]` no-inline function that
//! just returns). Under `qemu -s` a gdb script breaks on that symbol,
//! reads `$rdi` (the scribbled field's address) and arms QEMU-watchpoints
//! (Z2/Z4 — honored by TCG, unlike guest DR registers) on the victim
//! neighborhood to catch the writer's next store with its RIP.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::process::task::Task;

pub const SCRIBBLE_SLOTS: usize = 512;

const OFF_THREAD: usize = core::mem::offset_of!(Task, thread);
const OFF_FS: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, fs_base);
const OFF_GS: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, gs_base);
const OFF_SP: usize = OFF_THREAD + core::mem::offset_of!(crate::arch::thread::ThreadStruct, sp);
const OFF_RET: usize =
    OFF_THREAD + core::mem::offset_of!(crate::arch::thread::CalleeSaved, ret_addr);

struct Slot {
    /// Task pointer this slot tracks (0 = free).
    task: AtomicU64,
    /// 0 = pre-first-run (init writes not yet absorbed), 1 = armed.
    armed: AtomicU32,
    /// Seqlock: odd while quiesce() is rewriting the shadows, bumped on
    /// every re-sync. verify() only trusts a read set bracketed by an
    /// unchanged even generation.
    gen: AtomicU32,
    sh_fs: AtomicU64,
    sh_gs: AtomicU64,
    sh_sp: AtomicU64,
    sh_ret: AtomicU64,
    sh_rip: AtomicU64,
    sh_rsp: AtomicU64,
    sh_cs: AtomicU64,
    /// Anomaly-class rate-limit bitmask for verify_consistency.
    armed_swapped: AtomicU32,
}

static SLOTS: [Slot; SCRIBBLE_SLOTS] = [const {
    Slot {
        task: AtomicU64::new(0),
        armed: AtomicU32::new(0),
        gen: AtomicU32::new(0),
        sh_fs: AtomicU64::new(0),
        sh_gs: AtomicU64::new(0),
        sh_sp: AtomicU64::new(0),
        sh_ret: AtomicU64::new(0),
        sh_rip: AtomicU64::new(0),
        sh_rsp: AtomicU64::new(0),
        sh_cs: AtomicU64::new(0),
        armed_swapped: AtomicU32::new(0),
    }
}; SCRIBBLE_SLOTS];

pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static HITS: AtomicU32 = AtomicU32::new(0);
/// Park for the gdb breakpoint after the first hit (dfx=scribblepark).
pub static PARK: AtomicBool = AtomicBool::new(false);
/// Park when pid 1 registers (dfx=scribblepid1) so gdb can arm
/// watchpoints on the long-lived init task's Task struct + kernel
/// stack at their boot-fixed addresses.
pub static PARK_PID1: AtomicBool = AtomicBool::new(false);
/// Park when the 64th task registers (early in the fork/exec storm) so
/// gdb can arm quiet-region watchpoints (stack bottoms, tp_value,
/// exception_sp) on the whole live set before the scribbler strikes.
pub static PARK_SWEEP: AtomicBool = AtomicBool::new(false);
static REG_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static VERIFY_TICK: AtomicU32 = AtomicU32::new(0);
static DIVERGE: AtomicU32 = AtomicU32::new(0);
static GS_FIX_ANNOUNCED: AtomicBool = AtomicBool::new(false);

// ---- pick/publish event ring (dfx=scribble) -------------------------
// Every mark_picked_on_cpu and x86_switch_publish appends here; the CONS
// report dumps the tail, showing the last picks/switches around an
// abandoned pick (on_cpu=1, claim=-1, re-linked, owned by no slot).
pub struct RingEntry {
    pub stamp: AtomicU64, // jiffies at append
    pub cpu: AtomicU32,
    pub kind: AtomicU32, // 1 = mark_picked(next), 2 = publish(prev,next)
    pub a: AtomicU64,    // marked/prev task
    pub b: AtomicU64,    // next task (publish) / 0
}

pub static RING: [RingEntry; 128] = [const {
    RingEntry {
        stamp: AtomicU64::new(0),
        cpu: AtomicU32::new(0),
        kind: AtomicU32::new(0),
        a: AtomicU64::new(0),
        b: AtomicU64::new(0),
    }
}; 128];

static RING_CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Append a pick/publish event (lock-free, best-effort ordering).
pub fn ring_log(cpu: usize, kind: u32, a: u64, b: u64) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let i = RING_CUR.fetch_add(1, Ordering::Relaxed) % 128;
    let e = &RING[i];
    e.stamp.store(crate::drivers::timer::read_time(), Ordering::Relaxed);
    e.cpu.store(cpu as u32, Ordering::Relaxed);
    e.kind.store(kind, Ordering::Relaxed);
    e.a.store(a, Ordering::Relaxed);
    e.b.store(b, Ordering::Relaxed);
}

/// Dump the ring tail (raw console).
pub fn ring_dump() {
    let cur = RING_CUR.load(Ordering::Relaxed);
    let start = cur.saturating_sub(40);
    puts(b"SCRIBBLE-RING cur=");
    put_dec(cur as u64);
    puts(b"\n");
    for i in start..cur {
        let e = &RING[i % 128];
        let kind = e.kind.load(Ordering::Relaxed);
        if kind == 0 {
            continue;
        }
        put_dec((i % 128) as u64);
        puts(b": j=");
        put_dec(e.stamp.load(Ordering::Relaxed));
        puts(b" cpu=");
        put_dec(e.cpu.load(Ordering::Relaxed) as u64);
        match kind {
            1 => puts(b" PICK "),
            2 => puts(b" PUB  "),
            3 => puts(b" ENQ  "),
            _ => puts(b" ??   "),
        }
        put_hex(e.a.load(Ordering::Relaxed));
        puts(b" ");
        put_hex(e.b.load(Ordering::Relaxed));
        puts(b"\n");
    }
}
/// Verify cadence: every N timer ticks per CPU.
const VERIFY_EVERY: u32 = 4;

// ---------------------------------------------------------------------------
// Raw console helpers (lock-free, usable from any context)
// ---------------------------------------------------------------------------

fn putc(c: u8) {
    // While a report is being formatted, mirror every byte into the
    // static report buffer (gdb-readable copy immune to serial
    // interleaving).
    if REPORTING.load(Ordering::Relaxed) {
        let p = REPORT_POS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: diagnostic buffer, bounded by len check.
        let buf = unsafe { &mut *REPORT_BUF.0.get() };
        if p < buf.len() {
            buf[p] = c;
        }
    }
    // SAFETY: raw console byte, no locking, no allocation.
    unsafe { crate::console::putchar_no_lock(c) };
}

fn puts(s: &[u8]) {
    for &b in s {
        putc(b);
    }
}

fn put_hex(v: u64) {
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nibble = (v >> shift) & 0xF;
        let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
        putc(c);
    }
}

fn put_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if v == 0 {
        putc(b'0');
        return;
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        putc(b);
    }
}

/// Hex-dump `n` qwords at `base`, 4 per line, tagged.
fn dump_qwords(tag: &[u8], base: usize, n: usize) {
    puts(tag);
    puts(b" @");
    put_hex(base as u64);
    puts(b"\n");
    for i in 0..n {
        if i % 4 == 0 {
            puts(b"  ");
            put_hex((base + i * 8) as u64);
            puts(b":");
        }
        puts(b" ");
        // SAFETY: diagnostic read of kernel memory.
        let v = unsafe { core::ptr::read_volatile((base + i * 8) as *const u64) };
        put_hex(v);
        if i % 4 == 3 {
            puts(b"\n");
        }
    }
    if n % 4 != 0 {
        puts(b"\n");
    }
}

// ---------------------------------------------------------------------------
// Slot table
// ---------------------------------------------------------------------------

fn hash_of(task: usize) -> usize {
    // Task allocations are at least page aligned.
    (task >> 12) & (SCRIBBLE_SLOTS - 1)
}

fn slot_of(task: *mut Task) -> Option<usize> {
    let h = hash_of(task as usize);
    for i in 0..SCRIBBLE_SLOTS {
        let idx = (h + i) & (SCRIBBLE_SLOTS - 1);
        let cur = SLOTS[idx].task.load(Ordering::Acquire);
        if cur == task as u64 {
            return Some(idx);
        }
        if cur == 0 {
            return None;
        }
    }
    None
}

#[inline]
fn rd(task: *mut Task, off: usize) -> u64 {
    // SAFETY: diagnostic read of a live Task field.
    unsafe { core::ptr::read_volatile((task as usize + off) as *const u64) }
}

#[inline]
fn gen_of(idx: usize) -> u32 {
    SLOTS[idx].gen.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Register a freshly built Task (called at the end of `new_task_at`).
pub fn register(task: *mut Task) {
    if !ENABLED.load(Ordering::Relaxed) || task.is_null() {
        return;
    }
    // Reclaimed-page guard: a Task page freed without its unregister
    // landing (exit raced the table-full path, or the free came from an
    // error path that bypassed free_task_slot) leaves a STALE armed slot
    // for this address; the slot_of probe would then hand verify() a
    // half-old shadow for the NEW occupant (the TASK_NEW + zeroed-thread
    // HIT family on reused pages, u-series). Reset any such slot first.
    if let Some(idx) = slot_of(task) {
        SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
        SLOTS[idx].task.store(0, Ordering::Release);
        SLOTS[idx].armed.store(0, Ordering::Release);
    }
    let h = hash_of(task as usize);
    for i in 0..SCRIBBLE_SLOTS {
        let idx = (h + i) & (SCRIBBLE_SLOTS - 1);
        if SLOTS[idx]
            .task
            .compare_exchange(0, task as u64, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
            SLOTS[idx].armed.store(0, Ordering::Release);
            SLOTS[idx].sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
            SLOTS[idx].sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
            SLOTS[idx].sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
            SLOTS[idx].sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
            SLOTS[idx].sh_rip.store(0, Ordering::Relaxed);
            SLOTS[idx].sh_rsp.store(0, Ordering::Relaxed);
            SLOTS[idx].sh_cs.store(0, Ordering::Relaxed);
            // Sweep park: once the live set is large (boot tasks ~25,
            // LTP storm children after), hand gdb the addresses so it
            // can arm quiet-region mines before the scribbler strikes.
            if PARK_SWEEP.load(Ordering::Relaxed) {
                let n = REG_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                if n == 30 {
                    puts(b"SCRIBBLE-SWEEPPARK");
                    puts(b" task=");
                    put_hex(task as u64);
                    puts(b" gs=");
                    put_hex((task as usize + OFF_GS) as u64);
                    puts(b" fs=");
                    put_hex((task as usize + OFF_FS) as u64);
                    puts(b" sp=");
                    put_hex((task as usize + OFF_SP) as u64);
                    puts(b" ret=");
                    put_hex((task as usize + OFF_RET) as u64);
                    puts(b" thread=");
                    put_hex((task as usize + OFF_THREAD) as u64);
                    puts(b"\n");
                    scribble_park(task as u64);
                }
            }
            // pid1 park: the init task outlives the whole storm; its
            // Task struct and kernel stack sit at boot-fixed addresses.
            // gdb breaks on scribble_park, parses this line from the
            // serial log, and arms QEMU watchpoints on the listed
            // addresses (the x86 link strips DWARF, so the kernel hands
            // gdb the exact field addresses).
            if PARK_PID1.load(Ordering::Relaxed) {
                // SAFETY: read-only pid probe of the just-built task.
                let pid = unsafe { (*task).pid() };
                if pid == 1 {
                    puts(b"SCRIBBLE-PID1");
                    puts(b" task=");
                    put_hex(task as u64);
                    if let Some(kstack_bottom) = unsafe {
                        (*task).get_kernel_stack().map(|top| {
                            top as usize
                                - crate::config::KERNEL_STACK_SIZE
                        })
                    } {
                        puts(b" kstack=");
                        put_hex(kstack_bottom as u64);
                    }
                    puts(b" gs=");
                    put_hex((task as usize + OFF_GS) as u64);
                    puts(b" fs=");
                    put_hex((task as usize + OFF_FS) as u64);
                    puts(b" sp=");
                    put_hex((task as usize + OFF_SP) as u64);
                    puts(b" ret=");
                    put_hex((task as usize + OFF_RET) as u64);
                    puts(b" state=");
                    put_hex((task as usize + crate::process::task::TASK_STATE) as u64);
                    puts(b"\n");
                    scribble_park(task as u64);
                }
            }
            return;
        }
    }
    // Table full: silently untracked (512 live tasks is already a flood).
}

/// Unregister a Task about to be freed (`free_task_slot`, before drop).
pub fn unregister(task: *mut Task) {
    if !ENABLED.load(Ordering::Relaxed) || task.is_null() {
        return;
    }
    if let Some(idx) = slot_of(task) {
        SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
        SLOTS[idx].task.store(0, Ordering::Release);
    }
}

/// Absorb a legitimate in-run write to the thread fields (prctl
/// ARCH_SET_GS/SET_FS, exec reset, fork child init) into the shadow.
pub fn note_thread_write(task: *mut Task) {
    if !ENABLED.load(Ordering::Relaxed) || task.is_null() {
        return;
    }
    if let Some(idx) = slot_of(task) {
        SLOTS[idx].sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
        SLOTS[idx].sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
        SLOTS[idx].gen.fetch_add(1, Ordering::AcqRel);
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Switch-out quiesce point: the task's context is fully saved. Any
/// tracked field that differs from its shadow changed outside the
/// legitimate write points — scribbled while running.
pub fn quiesce(task: *mut Task) {
    if !ENABLED.load(Ordering::Relaxed) || task.is_null() {
        return;
    }
    let Some(idx) = slot_of(task) else { return };
    let s = &SLOTS[idx];
    if s.armed.load(Ordering::Acquire) != 0 {
        let live_gs = rd(task, OFF_GS);
        let live_fs = rd(task, OFF_FS);
        if live_gs != s.sh_gs.load(Ordering::Relaxed)
            || live_fs != s.sh_fs.load(Ordering::Relaxed)
        {
            report(task, idx, b"run-window");
        }
    }
    // Absorb this run's saved state into the shadow (sp/ret/fs/gs were
    // just rewritten by the switch path itself). Seqlock discipline:
    // odd generation while the shadows are being rewritten.
    let g = s.gen.fetch_add(1, Ordering::AcqRel);
    let _ = g;
    s.sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
    s.sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
    s.sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
    s.sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
    // Snapshot the parked pt_regs (last complete trap frame at the
    // kernel stack top).
    if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
        let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
        const OFF_RIP: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, rip);
        const OFF_CS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, cs);
        const OFF_RSP: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, rsp);
        // SAFETY: diagnostic reads of the task's kernel stack top.
        unsafe {
            s.sh_rip
                .store(core::ptr::read_volatile((pr + OFF_RIP) as *const u64), Ordering::Relaxed);
            s.sh_rsp
                .store(core::ptr::read_volatile((pr + OFF_RSP) as *const u64), Ordering::Relaxed);
            s.sh_cs
                .store(core::ptr::read_volatile((pr + OFF_CS) as *const u64), Ordering::Relaxed);
        }
    }
    s.gen.fetch_add(1, Ordering::AcqRel); // even again
    s.armed.store(1, Ordering::Release);
}

/// Periodic verify from the timer tick: scan tasks and compare against
/// the shadows using seqlock discipline (a read set is trusted only if
/// the generation stayed even and unchanged across it).
pub fn verify() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let n = VERIFY_TICK.fetch_add(1, Ordering::Relaxed);
    if n % VERIFY_EVERY != 0 {
        return;
    }
    let cur = crate::sched::current().map(|t| t as *mut Task);
    for idx in 0..SCRIBBLE_SLOTS {
        let t = SLOTS[idx].task.load(Ordering::Acquire);
        if t == 0 {
            continue;
        }
        let task = t as *mut Task;
        if cur == Some(task) {
            continue;
        }
        // A task running on ANOTHER CPU also moves its sp/fpu state
        // legitimately between our read and its next quiesce — the
        // sp-only WEAK reports on cross-CPU running tasks were pure
        // detector noise (u-series logs). Skip anything a CPU actually
        // runs or is picking right now; only fully quiescent sleepers
        // can be judged.
        if unsafe { (*task).on_cpu() }
            || (0..crate::config::MAX_CPUS)
                .any(|c| crate::sched::sched::cpu_state(c).current == task)
        {
            continue;
        }
        // Recycled-page race: unregister (free_task_slot) bumps gen and
        // clears the task pointer, but verify may already hold the old
        // pointer while the page is re-born as a fork child — TASK_NEW
        // (0x80, half-built thread fields) or ZOMBIE/DEAD (exit path
        // rewriting them). Neither is a judgeable quiescent sleeper;
        // both produced the state=0x80/empty-comm HIT spam (u5, v1).
        {
            let st = unsafe { (*task).state().bits() };
            if st
                & (crate::process::task::TaskState::TASK_NEW
                    | crate::process::task::TaskState::ZOMBIE
                    | crate::process::task::TaskState::DEAD)
                != 0
            {
                continue;
            }
        }
        let s = &SLOTS[idx];
        if s.armed.load(Ordering::Acquire) == 0 {
            continue;
        }
        // Stable read set (retry a couple of times when a concurrent
        // quiesce bumps the generation mid-read).
        let mut live_gs = 0;
        let mut live_fs = 0;
        let mut live_sp = 0;
        let mut live_ret = 0;
        let mut sh_gs = 0;
        let mut sh_fs = 0;
        let mut sh_sp = 0;
        let mut sh_ret = 0;
        let mut stable = false;
        for _ in 0..3 {
            let g0 = gen_of(idx);
            if g0 % 2 == 1 {
                core::hint::spin_loop();
                continue;
            }
            live_gs = rd(task, OFF_GS);
            live_fs = rd(task, OFF_FS);
            live_sp = rd(task, OFF_SP);
            live_ret = rd(task, OFF_RET);
            sh_gs = s.sh_gs.load(Ordering::Relaxed);
            sh_fs = s.sh_fs.load(Ordering::Relaxed);
            sh_sp = s.sh_sp.load(Ordering::Relaxed);
            sh_ret = s.sh_ret.load(Ordering::Relaxed);
            if gen_of(idx) == g0 {
                stable = true;
                break;
            }
        }
        if !stable {
            continue;
        }
        let mut mism = 0;
        if live_gs != sh_gs {
            mism += 1;
        }
        if live_fs != sh_fs {
            mism += 1;
        }
        if live_sp != sh_sp {
            mism += 1;
        }
        if live_ret != sh_ret {
            mism += 1;
        }
        if mism == 0 {
            continue;
        }
        // pt_regs control words for tasks asleep off-CPU: a garbage cs
        // (not __USER_CS 0x33 / __KERNEL_CS 0x08) is a frame scribble.
        let mut frame_bad = false;
        // SAFETY: atomic state probe of a live task.
        let st = unsafe { (*task).state().bits() };
        let on_cpu = unsafe { (*task).on_cpu() };
        if !on_cpu
            && st != crate::process::task::TaskState::RUNNING
            && (st == crate::process::task::TaskState::INTERRUPTIBLE
                || st == crate::process::task::TaskState::UNINTERRUPTIBLE)
        {
            if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
                let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
                const OFF_CS: usize = core::mem::offset_of!(crate::arch::pt_regs::PtRegs, cs);
                // SAFETY: diagnostic read of the task's parked frame.
                let cs = unsafe { core::ptr::read_volatile((pr + OFF_CS) as *const u64) };
                if cs != 0x33 && cs != 0x08 {
                    frame_bad = true;
                }
            }
        }
        // Single-field sp drifts are the weakest signal (deep stack
        // depth changes between runs read as mismatch only when the
        // quiesce hook raced); report but do not park on them.
        if mism == 1 && live_sp != sh_sp && !frame_bad {
            puts(b"SCRIBBLE-WEAK sp-only task=");
            put_hex(task as u64);
            puts(b" sh=");
            put_hex(sh_sp);
            puts(b" live=");
            put_hex(live_sp);
            puts(b"\n");
            s.sh_sp.store(live_sp, Ordering::Relaxed);
            s.gen.fetch_add(2, Ordering::AcqRel);
            continue;
        }
        report(task, idx, b"quiescent");
        // Resync so repeated scribbles on the same field re-trip.
        let g = s.gen.fetch_add(1, Ordering::AcqRel);
        let _ = g;
        s.sh_gs.store(rd(task, OFF_GS), Ordering::Relaxed);
        s.sh_fs.store(rd(task, OFF_FS), Ordering::Relaxed);
        s.sh_sp.store(rd(task, OFF_SP), Ordering::Relaxed);
        s.sh_ret.store(rd(task, OFF_RET), Ordering::Relaxed);
        s.gen.fetch_add(1, Ordering::AcqRel);
    }
}

/// Periodic GS sanity tripwire: with the per-CPU GS pairing intact, the
/// ACTIVE GS base points at PER_CPU[cpu] whose first u32 is a small cpu
/// number. A trap/syscall running with a wrong ACTIVE GS (0 / user value)
/// reads the low IVT page instead — a huge garbage "cpu id" that then
/// indexes PER_CPU/CURRENT_PT_REGS wildly (the byte-scribble family:
/// register-frame data, IVT patterns 0xf000ff53, user pointers landing
/// in Task pages). Runs once per timer tick per CPU.
pub fn verify_gs() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // GS-pairing self-heal counter (trap.S guards): non-zero proves
    // kernel execution with a mis-paired GS base occurred and was
    // neutralized. Announce the first occurrence.
    let fixes = scribble_gs_fixups.load(Ordering::Relaxed);
    if fixes != 0 && !GS_FIX_ANNOUNCED.swap(true, Ordering::AcqRel) {
        puts(b"SCRIBBLE-GS-FIXUPS n=");
        put_dec(fixes as u64);
        puts(b" (kernel ran with a wild GS base; stub guard healed)\n");
    }
    // SAFETY: one %gs-relative u32 read; MSR reads are data-safe.
    let (gscpu, gsbase, kgsbase) = unsafe {
        let id: u32;
        core::arch::asm!(
            "mov {0:e}, dword ptr gs:[0]",
            out(reg) id,
            options(nomem, nostack, preserves_flags)
        );
        let b1 = crate::arch::x86_64::cpu::rdmsr(0xC000_0101);
        let b2 = crate::arch::x86_64::cpu::rdmsr(0xC000_0102);
        (id, b1, b2)
    };
    if gscpu as usize >= crate::config::MAX_CPUS {
        let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
        puts(b"\nSCRIBBLE-GS-BAD #");
        put_dec(n as u64);
        puts(b" cpu=");
        put_dec(crate::arch::cpu_id() as u64);
        puts(b" gscpu_read=");
        put_hex(gscpu as u64);
        puts(b" gs_base=");
        put_hex(gsbase);
        puts(b" kernel_gs_base=");
        put_hex(kgsbase);
        puts(b"\n");
        if PARK.load(Ordering::Relaxed) && n == 1 {
            scribble_park(gsbase);
        }
    }
}

/// Scheduler-consistency scan: run under the GRQ lock (trylock — skip
/// the round when contended) so scheduler transients are excluded, and
/// require the anomaly to persist across two consecutive scans before
/// reporting. A task that is BOTH linked on a class queue AND on-CPU is
/// a double-run seed — the second CPU that picks it executes on the
/// same kernel stack (register-frame scribbles, interleaved trap
/// frames, sp/ret drift).
pub fn verify_consistency() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // Try the GRQ lock; skip this round if the scheduler holds it.
    let Some(_grq_guard) = crate::sched::sched::try_grq_lock() else {
        return;
    };
    let cur = crate::sched::current().map(|t| t as *mut Task);
    for idx in 0..SCRIBBLE_SLOTS {
        let t = SLOTS[idx].task.load(Ordering::Acquire);
        if t == 0 {
            continue;
        }
        let task = t as *mut Task;
        // SAFETY: read-only probes of live task scheduling fields; the
        // GRQ lock is held, so pick/dequeue/publish windows are closed.
        let (on_rq, on_cpu, state) = unsafe {
            ((*task).sched_entity().is_on_rq(), (*task).on_cpu(), (*task).state().bits())
        };
        let running = state == crate::process::task::TaskState::RUNNING;
        const CLS_A: &[u8] = b"onrq+oncpu";
        const CLS_B: &[u8] = b"onrq+sleeping";
        const CLS_C: &[u8] = b"phantom-run";
        // Under the GRQ lock the legitimate states are:
        //   (linked ∧ RUNNING ∧ ¬on_cpu ∧ ¬current)   — queued
        //   (¬linked ∧ RUNNING ∧ on_cpu)              — running/picked
        //   (¬linked ∧ sleeping ∧ ¬on_cpu)            — blocked
        //   current (this CPU's or another's pick is excluded by the lock)
        // Anything else that persists across two scans is an invariant
        // break worth naming. (onrq+sleeping is also the pre-schedule
        // window ONLY when the sleeper itself is current on a CPU —
        // excluded via the current-slot scan.)
        let curr_any = cur == Some(task)
            || (0..crate::config::MAX_CPUS)
                .any(|c| crate::sched::sched::cpu_state(c).current == task);
        let anomaly: &[u8] = if curr_any {
            continue;
        } else if on_rq && on_cpu {
            CLS_A
        } else if on_rq && !running {
            CLS_B
        } else if running && !on_rq && !on_cpu {
            CLS_C
        } else {
            // Consistent again: re-arm the rate-limit for this slot.
            SLOTS[idx].armed_swapped.store(0, Ordering::Relaxed);
            continue;
        };
        let key = if core::ptr::eq(anomaly.as_ptr(), CLS_A.as_ptr()) {
            1u32
        } else if core::ptr::eq(anomaly.as_ptr(), CLS_B.as_ptr()) {
            2
        } else {
            4
        };
        // Persistence filter: first sighting only latches the class;
        // report on the SECOND consecutive scan.
        let seen = SLOTS[idx].armed_swapped.fetch_or(key, Ordering::AcqRel);
        if seen & key == 0 {
            // Latch: freeze the event-ring tail NOW (the report-side
            // dump comes 40ms later — causal entries scroll out).
            puts(b"SCRIBBLE-LATCH ");
            put_hex(task as u64);
            puts(b" cls=");
            puts(anomaly);
            puts(b"\n");
            ring_dump();
            continue;
        }
        puts(b"SCRIBBLE-CONS ");
        put_hex(task as u64);
        // SAFETY: diagnostic read of the task header.
        unsafe {
            puts(b" pid=");
            put_dec((*task).pid() as u64);
        }
        puts(b" cls=");
        puts(anomaly);
        puts(b" onrq=");
        put_dec(on_rq as u64);
        puts(b" oncpu=");
        put_dec(on_cpu as u64);
        puts(b" st=0x");
        put_hex(state as u64);
        puts(b" cpu=");
        put_dec(crate::arch::cpu_id() as u64);
        // Full ownership picture: which CPU's slots claim this task,
        // the task's own ti_cpu + running_on_cpu claim, policy.
        // SAFETY: diagnostic reads of scheduling fields.
        unsafe {
            puts(b" ticpu=");
            put_dec((*task).ti_cpu() as i64 as u64);
            puts(b" claim=");
            put_dec((*task).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
            for c in 0..crate::config::MAX_CPUS {
                puts(b" s");
                put_dec(c as u64);
                puts(b"=");
                put_hex(crate::sched::sched::cpu_state(c).current as u64);
                puts(b"/");
                put_hex(
                    crate::arch::smp::PER_CPU[c]
                        .current_task
                        .load(Ordering::Relaxed),
                );
            }
        }
        puts(b"\n");
        if key == 1 || key == 2 {
            ring_dump();
            let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
            if PARK.load(Ordering::Relaxed) && n == 1 {
                puts(b"SCRIBBLE-CONSPARK task=");
                put_hex(task as u64);
                puts(b" gs=");
                put_hex((task as usize + OFF_GS) as u64);
                puts(b" fs=");
                put_hex((task as usize + OFF_FS) as u64);
                puts(b" sp=");
                put_hex((task as usize + OFF_SP) as u64);
                puts(b" ret=");
                put_hex((task as usize + OFF_RET) as u64);
                puts(b" thread=");
                put_hex((task as usize + OFF_THREAD) as u64);
                puts(b"\n");
                scribble_park((task as usize + OFF_THREAD) as u64);
            }
        }
    }
}

/// R55 tripwire: __schedule observed slot-prev != hardware-current (tp).
/// Names the divergence the moment the scheduler first sees it — before
/// any heal, with every slot's claim printed.
pub fn slot_divergence(cpu: usize, tp: *mut Task, slot_prev: *mut Task) {
    let n = DIVERGE.fetch_add(1, Ordering::AcqRel) + 1;
    if n > 64 {
        return; // rate limit
    }
    puts(b"SCRIBBLE-DIVERGE #");
    put_dec(n as u64);
    puts(b" cpu=");
    put_dec(cpu as u64);
    puts(b" tp=");
    put_hex(tp as u64);
    // SAFETY: diagnostic header reads; the pages are live task structs.
    unsafe {
        if !tp.is_null() {
            puts(b"(pid=");
            put_dec((*tp).pid() as u64);
            puts(b")");
        }
        puts(b" slot=");
        put_hex(slot_prev as u64);
        if !slot_prev.is_null() {
            puts(b"(pid=");
            put_dec((*slot_prev).pid() as u64);
            puts(b")");
        }
        puts(b" slots:");
        for c in 0..crate::config::MAX_CPUS {
            put_dec(c as u64);
            puts(b"=");
            put_hex(crate::sched::sched::cpu_state(c).current as u64);
        }
        if !tp.is_null() {
            puts(b" tponcpu=");
            put_dec((*tp).on_cpu() as u64);
            puts(b" tpclaim=");
            put_dec((*tp).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
            puts(b" tponrq=");
            put_dec((*tp).sched_entity().is_on_rq() as u64);
            puts(b" tpst=0x");
            put_hex((*tp).state().bits() as u64);
        }
    }
    puts(b"\n");
}

/// Double-run report: the running_on_cpu claim swap in
/// x86_switch_publish detected a second switch-into a task whose
/// continuation is still claimed by another CPU. This runs INSIDE the
/// switch path — no polling races — and the two CPUs are both live.
pub fn double_run(pid: u64, newcpu: u64, oldcpu: u64, prevpid: u64, next: u64) {
    // Bounded spin on the report lock (a concurrent panic path may hold
    // it; never wedge the switch).
    let mut spins = 0u32;
    while REPORT_LOCK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
        && spins < 200_000
    {
        spins += 1;
        core::hint::spin_loop();
    }
    REPORT_POS.store(0, Ordering::Relaxed);
    REPORTING.store(true, Ordering::Relaxed);

    puts(b"\nDOUBLE-RUN pid=");
    put_dec(pid);
    puts(b" newcpu=");
    put_dec(newcpu);
    puts(b" oldcpu=");
    put_dec(oldcpu);
    puts(b" prevpid=");
    put_dec(prevpid);
    puts(b" next=");
    put_hex(next);
    puts(b"\n");
    ring_dump();

    REPORTING.store(false, Ordering::Relaxed);
    REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
    REPORT_LOCK.store(false, Ordering::Release);

    if PARK.load(Ordering::Relaxed) {
        puts(b"SCRIBBLE-DBLPARK task=");
        put_hex(next);
        puts(b" gs=");
        put_hex((next as usize + OFF_GS) as u64);
        puts(b" fs=");
        put_hex((next as usize + OFF_FS) as u64);
        puts(b" sp=");
        put_hex((next as usize + OFF_SP) as u64);
        puts(b" ret=");
        put_hex((next as usize + OFF_RET) as u64);
        puts(b" thread=");
        put_hex((next as usize + OFF_THREAD) as u64);
        puts(b"\n");
        scribble_park((next as usize + OFF_THREAD) as u64);
    }
}

// ---------------------------------------------------------------------------
// Reporting + gdb park
// ---------------------------------------------------------------------------

/// Count of GS-pairing self-heals performed by the trap stubs and
/// syscall entry (trap.S reads this symbol from asm). Non-zero during
/// a run proves kernel code executed with a mis-paired (user/wild) GS
/// base — the scribble engine — and that the guard neutralized it.
#[no_mangle]
pub static scribble_gs_fixups: AtomicU32 = AtomicU32::new(0);

/// GDB park hook: `break scribble_park` under `qemu -s`; `$rdi` holds
/// the most diagnostic scribbled field address to watch.
#[no_mangle]
#[inline(never)]
pub extern "C" fn scribble_park(_field_addr: u64) {}

/// Full text of the last HIT report (also readable via gdb after the
/// run dies: serial interleaving cannot destroy this copy).
pub struct ReportBuf(core::cell::UnsafeCell<[u8; 4096]>);
unsafe impl Sync for ReportBuf {}
/// Symbol gdb dumps after a run: `dump binary memory file &REPORT_BUF ..`
pub static REPORT_BUF: ReportBuf = ReportBuf(core::cell::UnsafeCell::new([0; 4096]));
pub static REPORT_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

static REPORTING: AtomicBool = AtomicBool::new(false);
static REPORT_POS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static REPORT_LOCK: AtomicBool = AtomicBool::new(false);

/// R63 crash capture: on a kernel-mode fatal trap (#GP/#UD/#PF-to-panic),
/// serialize a full picture BEFORE the (4-CPU-interleaving) panic printer
/// runs: the faulting pt_regs verbatim, the per-CPU ownership picture
/// (sched current slots, arch hardware current_task, switch-out marks),
/// the current task identity, and the pick/publish ring tail. One CPU at
/// a time via REPORT_LOCK, so at least one clean copy reaches the serial
/// log even when several CPUs die together.
pub fn crash_report(vector: &[u8], regs: &crate::arch::pt_regs::PtRegs) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // Bounded spin (a concurrent panic path may hold the lock; never
    // wedge the crash report itself).
    let mut spins = 0u32;
    while REPORT_LOCK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
        && spins < 200_000
    {
        spins += 1;
        core::hint::spin_loop();
    }
    REPORT_POS.store(0, Ordering::Relaxed);
    REPORTING.store(true, Ordering::Relaxed);

    puts(b"\n=== SCRIBBLE-CRASH ");
    puts(vector);
    puts(b" rip=");
    put_hex(regs.rip);
    puts(b" cs=");
    put_hex(regs.cs);
    puts(b" err=");
    put_hex(regs.orig_rax);
    puts(b" cr2-ish rsp=");
    put_hex(regs.rsp);
    puts(b"\n");
    // Full pt_regs (fault frame verbatim).
    dump_qwords(b"PTREGS", regs as *const _ as usize, 21);

    // Stack window around the faulting rsp — the recursion signature:
    // a repeated return address across this band names the function
    // that burned the stack (R64: the overflow-into-neighbor engine).
    {
        let rsp = regs.rsp as usize;
        let lo = rsp.saturating_sub(0x40) & !0xF;
        let hi = rsp + 0x380;
        dump_qwords(b"STACKWIN", lo, (hi - lo) / 8);
    }

    // Ownership picture.
    let tp = crate::arch::cpu::get_thread_id() as *mut Task;
    puts(b"OWNER cpu=");
    put_dec(crate::arch::cpu_id() as u64);
    puts(b" tp=");
    put_hex(tp as u64);
    if !tp.is_null() {
        // SAFETY: diagnostic header reads of the task the hardware runs.
        unsafe {
            puts(b" pid=");
            put_dec((*tp).pid() as u64);
            puts(b" st=0x");
            put_hex((*tp).state().bits() as u64);
            puts(b" oncpu=");
            put_dec((*tp).on_cpu() as u64);
            puts(b" claim=");
            put_dec((*tp).running_on_cpu.load(Ordering::Relaxed) as i64 as u64);
            puts(b" onrq=");
            put_dec((*tp).sched_entity().is_on_rq() as u64);
            if let Some(kstack) = (*tp).get_kernel_stack() {
                puts(b" kstack=");
                put_hex(kstack as u64);
            }
        }
    }
    puts(b"\nSLOTS:");
    for c in 0..crate::config::MAX_CPUS {
        puts(b" c");
        put_dec(c as u64);
        puts(b"=");
        put_hex(crate::sched::sched::cpu_state(c).current as u64);
        puts(b"/");
        put_hex(
            crate::arch::smp::PER_CPU[c]
                .current_task
                .load(Ordering::Relaxed),
        );
    }
    puts(b"\n");

    // Victim task header + thread struct (thread.sp / kernel_stack /
    // callee.ret_addr live here — a torn value names which field the
    // scribbler rewrote to route this CPU onto a foreign stack).
    if !tp.is_null() {
        dump_qwords(b"TPHDR", tp as usize, 16);
        dump_qwords(b"THRD", (tp as usize) + OFF_THREAD, 76);
    }

    // Pick/publish ring tail — the scheduling history into the crash.
    ring_dump();

    REPORTING.store(false, Ordering::Relaxed);
    REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
    REPORT_LOCK.store(false, Ordering::Release);
}

fn report(task: *mut Task, idx: usize, phase: &[u8]) {
    // One writer at a time (bounded spin — never deadlock a panic path).
    let mut spins = 0u32;
    while REPORT_LOCK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
        && spins < 200_000
    {
        spins += 1;
        core::hint::spin_loop();
    }
    REPORT_POS.store(0, Ordering::Relaxed);
    REPORTING.store(true, Ordering::Relaxed);

    let n = HITS.fetch_add(1, Ordering::AcqRel) + 1;
    let s = &SLOTS[idx];
    puts(b"\nSCRIBBLE-HIT #");
    put_dec(n as u64);
    puts(b" phase=");
    puts(phase);
    puts(b" cpu=");
    put_dec(crate::arch::cpu_id() as u64);
    puts(b" task=");
    put_hex(task as u64);
    // SAFETY: diagnostic reads of stable Task header fields.
    unsafe {
        puts(b" pid=");
        put_dec((*task).pid() as u64);
        puts(b" comm=");
        let comm = (*task).comm();
        for &b in comm.iter() {
            if b == 0 {
                break;
            }
            putc(b);
        }
        puts(b" state=0x");
        put_hex((*task).state().bits() as u64);
    }
    puts(b"\n");

    puts("  gs  sh=".as_bytes());
    put_hex(s.sh_gs.load(Ordering::Relaxed));
    puts(b" live=");
    put_hex(rd(task, OFF_GS));
    puts(b"\n  fs  sh=");
    put_hex(s.sh_fs.load(Ordering::Relaxed));
    puts(b" live=");
    put_hex(rd(task, OFF_FS));
    puts(b"\n  sp  sh=");
    put_hex(s.sh_sp.load(Ordering::Relaxed));
    puts(b" live=");
    put_hex(rd(task, OFF_SP));
    puts(b"\n  ret sh=");
    put_hex(s.sh_ret.load(Ordering::Relaxed));
    puts(b" live=");
    put_hex(rd(task, OFF_RET));
    puts(b"\n");

    // Full ThreadStruct dump — the damage SHAPE (contiguous 512-byte
    // block == fxsave-looking; register-frame runs == trap-push on a
    // wild rsp) names the writer family.
    dump_qwords(b"THREAD", (task as usize) + OFF_THREAD, 76);
    // Task header (thread_info fields + state/pid + early list links).
    dump_qwords(b"TASKHDR", task as usize, 16);
    // Parked pt_regs at the stack top.
    if let Some(stack_top) = unsafe { (*task).get_kernel_stack() } {
        let pr = stack_top as usize - core::mem::size_of::<crate::arch::pt_regs::PtRegs>();
        dump_qwords(b"PTREGS", pr, 21);
    }

    REPORTING.store(false, Ordering::Relaxed);
    REPORT_LEN.store(REPORT_POS.load(Ordering::Relaxed), Ordering::Release);
    REPORT_LOCK.store(false, Ordering::Release);

    if PARK.load(Ordering::Relaxed) {
        // Park the FIRST hit: gdb arms watchpoints on the neighborhood.
        if n == 1 {
            puts(b"SCRIBBLE-HITPARK");
            puts(b" task=");
            put_hex(task as u64);
            puts(b" gs=");
            put_hex((task as usize + OFF_GS) as u64);
            puts(b" fs=");
            put_hex((task as usize + OFF_FS) as u64);
            puts(b" sp=");
            put_hex((task as usize + OFF_SP) as u64);
            puts(b" ret=");
            put_hex((task as usize + OFF_RET) as u64);
            puts(b" thread=");
            put_hex((task as usize + OFF_THREAD) as u64);
            puts(b"\n");
            scribble_park((task as usize + OFF_GS) as u64);
        }
    }
}
