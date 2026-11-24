//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! PROCESS FLIP SEAM (crate-flip rung: process): the three hook
//! tables the FRAMEWORK uses to reach the process service crate's
//! cold-path rows. The hot framework callers (arch traps' do_exit /
//! do_exit_group, signal delivery's exit paths, the scheduler reap
//! path) keep calling the MECHANISM that stayed framework-side in
//! process/exit.rs, process/task.rs, process/pid_hash.rs — no fn
//! pointer sits between them and the callee. Everything routed here
//! was measured cold: kthread spawn/stop/name (loop-park sites),
//! pid allocator/sysctl knobs, ptrace stop/write (flag-gated arms in
//! the trap/signal paths), coredump (rare), reboot latches, the boot
//! exec, and the wait4 park audit.
//!
//! Three tables, not one (crate-plan §3a rule 1): lifecycle rows
//! (ProcessHooks), kernel-thread rows (KthreadHooks), pid-allocator
//! rows (PidHooks) — split by consumer, each well under the ~20-row
//! width bar.
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must never call the wrapper that dispatches the same row.
//! Row bodies are the crate's real functions; framework call sites go
//! through these accessors only.
//!
//! Registration runs from rust_main via process::register_all —
//! publication is OnceSlot-only (no Spinlock, no per-CPU state; the
//! x86_64 unit-test kernel lesson, INV-SVCTEST-1's sibling).

use crate::fs::elf::Elf64Ehdr;
use crate::fs::file::File;
use crate::process::task::Task;

// ==================== ProcessHooks (lifecycle) ====================

/// The cold lifecycle rows: exec (the boot-time exec), the wait4 park
/// audit, coredump, ptrace stop/write, and the reboot latches. Signatures
/// carry the native types (crate-plan §3a rule 3): no adapter logic.
#[derive(Clone, Copy)]
pub struct ProcessHooks {
    pub do_execve_elf: fn(
        &mut Task,
        &[u8],
        &[alloc::string::String],
        &[alloc::string::String],
        u64,
        usize,
        &Elf64Ehdr,
        &str,
        Option<&[u8]>,
        bool,
        Option<&alloc::sync::Arc<File>>,
        Option<&alloc::sync::Arc<File>>,
    ) -> Result<(), i32>,
    pub wait4_park_audit: fn(),
    pub signal_makes_core: fn(i32) -> bool,
    pub do_coredump: fn(&Task, i32) -> bool,
    pub panic_reboot: fn() -> !,
    pub force_reboot: fn() -> !,
    pub force_power_off: fn() -> !,
    pub ctrl_alt_del_latch: fn(),
    pub cad_deliver_pending: fn(),
    pub init_exited_fallback_cascade: fn(),
}

/// An unfilled row is a kernel misbuild, not a degraded mode (the
/// MmHooks::EMPTY shape): name it loudly instead of failing softly.
impl ProcessHooks {
    pub const EMPTY: ProcessHooks = ProcessHooks {
        do_execve_elf: no_policy_do_execve_elf,
        wait4_park_audit: no_policy_wait4_park_audit,
        signal_makes_core: no_policy_signal_makes_core,
        do_coredump: no_policy_do_coredump,
        panic_reboot: no_policy_panic_reboot,
        force_reboot: no_policy_force_reboot,
        force_power_off: no_policy_force_power_off,
        ctrl_alt_del_latch: no_policy_ctrl_alt_del_latch,
        cad_deliver_pending: no_policy_cad_deliver_pending,
        init_exited_fallback_cascade: no_policy_init_exited_fallback_cascade,
    };
}

fn no_policy_do_execve_elf(
    _: &mut Task,
    _: &[u8],
    _: &[alloc::string::String],
    _: &[alloc::string::String],
    _: u64,
    _: usize,
    _: &Elf64Ehdr,
    _: &str,
    _: Option<&[u8]>,
    _: bool,
    _: Option<&alloc::sync::Arc<File>>,
    _: Option<&alloc::sync::Arc<File>>,
) -> Result<(), i32> {
    panic!("process seam: exec policy not registered (do_execve_elf)")
}
fn no_policy_wait4_park_audit() {
    panic!("process seam: wait policy not registered (wait4_park_audit)")
}
fn no_policy_signal_makes_core(_: i32) -> bool {
    panic!("process seam: coredump policy not registered (signal_makes_core)")
}
fn no_policy_do_coredump(_: &Task, _: i32) -> bool {
    panic!("process seam: coredump policy not registered (do_coredump)")
}
fn no_policy_panic_reboot() -> ! {
    panic!("process seam: reboot policy not registered (panic_reboot)")
}
fn no_policy_force_reboot() -> ! {
    panic!("process seam: reboot policy not registered (force_reboot)")
}
fn no_policy_force_power_off() -> ! {
    panic!("process seam: reboot policy not registered (force_power_off)")
}
fn no_policy_ctrl_alt_del_latch() {
    panic!("process seam: reboot policy not registered (ctrl_alt_del_latch)")
}
fn no_policy_cad_deliver_pending() {
    panic!("process seam: reboot policy not registered (cad_deliver_pending)")
}
fn no_policy_init_exited_fallback_cascade() {
    panic!("process seam: reboot policy not registered (init_exited_fallback_cascade)")
}

pub static PROCESS_HOOKS: crate::safety::slot::OnceSlot<ProcessHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins —
/// the composition bin's `process::register_all` runs before any
/// consumer).
pub fn process_register_hooks(hooks: ProcessHooks) {
    let _ = PROCESS_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static ProcessHooks {
    PROCESS_HOOKS.get().unwrap_or(&ProcessHooks::EMPTY)
}

// ---- accessors: the framework's cold door into process policy ----

#[inline]
pub fn do_execve_elf(
    task: &mut Task,
    program_data: &[u8],
    argv: &[alloc::string::String],
    envp: &[alloc::string::String],
    entry: u64,
    phdr_count: usize,
    ehdr: &Elf64Ehdr,
    pathname: &str,
    interp_data: Option<&[u8]>,
    secure_exec: bool,
    exe_file: Option<&alloc::sync::Arc<File>>,
    interp_file: Option<&alloc::sync::Arc<File>>,
) -> Result<(), i32> {
    (hooks().do_execve_elf)(
        task,
        program_data,
        argv,
        envp,
        entry,
        phdr_count,
        ehdr,
        pathname,
        interp_data,
        secure_exec,
        exe_file,
        interp_file,
    )
}

#[inline]
pub fn wait4_park_audit() {
    (hooks().wait4_park_audit)()
}

#[inline]
pub fn signal_makes_core(sig: i32) -> bool {
    (hooks().signal_makes_core)(sig)
}

#[inline]
pub fn do_coredump(task: &Task, sig: i32) -> bool {
    (hooks().do_coredump)(task, sig)
}

#[inline]
pub fn panic_reboot() -> ! {
    (hooks().panic_reboot)()
}

#[inline]
pub fn force_reboot() -> ! {
    (hooks().force_reboot)()
}

#[inline]
pub fn force_power_off() -> ! {
    (hooks().force_power_off)()
}

#[inline]
pub fn ctrl_alt_del_latch() {
    (hooks().ctrl_alt_del_latch)()
}

#[inline]
pub fn cad_deliver_pending() {
    (hooks().cad_deliver_pending)()
}

#[inline]
pub fn init_exited_fallback_cascade() {
    (hooks().init_exited_fallback_cascade)()
}

// ==================== KthreadHooks ====================

/// The kernel-thread rows: spawn/stop/name sites in framework kthread
/// LOOP bodies (parked pollers) and boot-time spawners — cold by
/// measurement (each site runs per kthread iteration at worst, on
/// loops that sleep).
#[derive(Clone, Copy)]
pub struct KthreadHooks {
    pub kthread_run: fn(
        extern "C" fn(*mut core::ffi::c_void) -> i32,
        *mut core::ffi::c_void,
        &'static str,
    ) -> Option<&'static mut Task>,
    pub kthread_run_on_cpu: fn(
        extern "C" fn(*mut core::ffi::c_void) -> i32,
        *mut core::ffi::c_void,
        &'static str,
        usize,
    ) -> Option<&'static mut Task>,
    pub kthread_should_stop: fn() -> bool,
    pub kthread_stop: fn(&mut Task) -> i32,
    pub kthread_name: fn(u32) -> Option<&'static str>,
}

impl KthreadHooks {
    pub const EMPTY: KthreadHooks = KthreadHooks {
        kthread_run: no_policy_kthread_run,
        kthread_run_on_cpu: no_policy_kthread_run_on_cpu,
        kthread_should_stop: no_policy_kthread_should_stop,
        kthread_stop: no_policy_kthread_stop,
        kthread_name: no_policy_kthread_name,
    };
}

fn no_policy_kthread_run(
    _: extern "C" fn(*mut core::ffi::c_void) -> i32,
    _: *mut core::ffi::c_void,
    _: &'static str,
) -> Option<&'static mut Task> {
    panic!("process seam: kthread policy not registered (kthread_run)")
}
fn no_policy_kthread_run_on_cpu(
    _: extern "C" fn(*mut core::ffi::c_void) -> i32,
    _: *mut core::ffi::c_void,
    _: &'static str,
    _: usize,
) -> Option<&'static mut Task> {
    panic!("process seam: kthread policy not registered (kthread_run_on_cpu)")
}
fn no_policy_kthread_should_stop() -> bool {
    panic!("process seam: kthread policy not registered (kthread_should_stop)")
}
fn no_policy_kthread_stop(_: &mut Task) -> i32 {
    panic!("process seam: kthread policy not registered (kthread_stop)")
}
fn no_policy_kthread_name(_: u32) -> Option<&'static str> {
    panic!("process seam: kthread policy not registered (kthread_name)")
}

pub static KTHREAD_HOOKS: crate::safety::slot::OnceSlot<KthreadHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn kthread_register_hooks(hooks: KthreadHooks) {
    let _ = KTHREAD_HOOKS.publish(hooks);
}

#[inline]
fn khooks() -> &'static KthreadHooks {
    KTHREAD_HOOKS.get().unwrap_or(&KthreadHooks::EMPTY)
}

#[inline]
pub fn kthread_run(
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    arg: *mut core::ffi::c_void,
    name: &'static str,
) -> Option<&'static mut Task> {
    (khooks().kthread_run)(fn_ptr, arg, name)
}

#[inline]
pub fn kthread_run_on_cpu(
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    arg: *mut core::ffi::c_void,
    name: &'static str,
    cpu: usize,
) -> Option<&'static mut Task> {
    (khooks().kthread_run_on_cpu)(fn_ptr, arg, name, cpu)
}

#[inline]
pub fn kthread_should_stop() -> bool {
    (khooks().kthread_should_stop)()
}

#[inline]
pub fn kthread_stop(task: &mut Task) -> i32 {
    (khooks().kthread_stop)(task)
}

#[inline]
pub fn kthread_name(pid: u32) -> Option<&'static str> {
    (khooks().kthread_name)(pid)
}

// ==================== PidHooks ====================

/// The pid-allocator rows: the framework's idle-task boot allocation,
/// the task-free path's free_pid, and the procfs sysctl knobs
/// (pid_max / last_pid). All once-per-event sites, cold by
/// measurement.
#[derive(Clone, Copy)]
pub struct PidHooks {
    pub alloc_pid: fn() -> Option<u32>,
    pub free_pid: fn(u32),
    pub pid_max_live: fn() -> u32,
    pub set_pid_max_live: fn(u32),
    pub last_pid: fn() -> u32,
    pub set_last_pid: fn(u32) -> bool,
}

impl PidHooks {
    pub const EMPTY: PidHooks = PidHooks {
        alloc_pid: no_policy_alloc_pid,
        free_pid: no_policy_free_pid,
        pid_max_live: no_policy_pid_max_live,
        set_pid_max_live: no_policy_set_pid_max_live,
        last_pid: no_policy_last_pid,
        set_last_pid: no_policy_set_last_pid,
    };
}

fn no_policy_alloc_pid() -> Option<u32> {
    panic!("process seam: pid policy not registered (alloc_pid)")
}
fn no_policy_free_pid(_: u32) {
    panic!("process seam: pid policy not registered (free_pid)")
}
fn no_policy_pid_max_live() -> u32 {
    panic!("process seam: pid policy not registered (pid_max_live)")
}
fn no_policy_set_pid_max_live(_: u32) {
    panic!("process seam: pid policy not registered (set_pid_max_live)")
}
fn no_policy_last_pid() -> u32 {
    panic!("process seam: pid policy not registered (last_pid)")
}
fn no_policy_set_last_pid(_: u32) -> bool {
    panic!("process seam: pid policy not registered (set_last_pid)")
}

pub static PID_HOOKS: crate::safety::slot::OnceSlot<PidHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn pid_register_hooks(hooks: PidHooks) {
    let _ = PID_HOOKS.publish(hooks);
}

#[inline]
fn phooks() -> &'static PidHooks {
    PID_HOOKS.get().unwrap_or(&PidHooks::EMPTY)
}

#[inline]
pub fn alloc_pid() -> Option<u32> {
    (phooks().alloc_pid)()
}

#[inline]
pub fn free_pid(pid: u32) {
    (phooks().free_pid)(pid)
}

#[inline]
pub fn pid_max_live() -> u32 {
    (phooks().pid_max_live)()
}

#[inline]
pub fn set_pid_max_live(v: u32) {
    (phooks().set_pid_max_live)(v)
}

#[inline]
pub fn last_pid() -> u32 {
    (phooks().last_pid)()
}

#[inline]
pub fn set_last_pid(v: u32) -> bool {
    (phooks().set_last_pid)(v)
}

// ==================== ExitHooks ====================

/// The exit TEARDOWN rows: the functions do_exit / do_exit_group
/// (core-side, hot-name-called) reach in the process crate's
/// exit_teardown family. Each runs ONCE per task death — cold by the
/// call-direction scan; an indirect call here is off every hot path.
#[derive(Clone, Copy)]
pub struct ExitHooks {
    pub exit_robust_list: fn(&Task),
    pub exit_clear_child_tid: fn(&mut Task),
    pub kill_other_threads: fn(&Task),
    pub wait_for_thread_group_death: fn(&Task, bool) -> bool,
    pub sweep_dead_threads: fn(*mut Task),
    pub reparent_children_to_init: fn(&Task),
}

impl ExitHooks {
    pub const EMPTY: ExitHooks = ExitHooks {
        exit_robust_list: no_policy_exit_robust_list,
        exit_clear_child_tid: no_policy_exit_clear_child_tid,
        kill_other_threads: no_policy_kill_other_threads,
        wait_for_thread_group_death: no_policy_wait_for_thread_group_death,
        sweep_dead_threads: no_policy_sweep_dead_threads,
        reparent_children_to_init: no_policy_reparent_children_to_init,
    };
}

fn no_policy_exit_robust_list(_: &Task) {
    panic!("process seam: exit teardown not registered (exit_robust_list)")
}
fn no_policy_exit_clear_child_tid(_: &mut Task) {
    panic!("process seam: exit teardown not registered (exit_clear_child_tid)")
}
fn no_policy_kill_other_threads(_: &Task) {
    panic!("process seam: exit teardown not registered (kill_other_threads)")
}
fn no_policy_wait_for_thread_group_death(_: &Task, _: bool) -> bool {
    panic!("process seam: exit teardown not registered (wait_for_thread_group_death)")
}
fn no_policy_sweep_dead_threads(_: *mut Task) {
    panic!("process seam: exit teardown not registered (sweep_dead_threads)")
}
fn no_policy_reparent_children_to_init(_: &Task) {
    panic!("process seam: exit teardown not registered (reparent_children_to_init)")
}

pub static EXIT_HOOKS: crate::safety::slot::OnceSlot<ExitHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn exit_register_hooks(hooks: ExitHooks) {
    let _ = EXIT_HOOKS.publish(hooks);
}

#[inline]
fn ehooks() -> &'static ExitHooks {
    EXIT_HOOKS.get().unwrap_or(&ExitHooks::EMPTY)
}

#[inline]
pub fn exit_robust_list(t: &Task) {
    (ehooks().exit_robust_list)(t)
}
#[inline]
pub fn exit_clear_child_tid(t: &mut Task) {
    (ehooks().exit_clear_child_tid)(t)
}
#[inline]
pub fn kill_other_threads(t: &Task) {
    (ehooks().kill_other_threads)(t)
}
#[inline]
pub fn wait_for_thread_group_death(t: &Task, killable: bool) -> bool {
    (ehooks().wait_for_thread_group_death)(t, killable)
}
#[inline]
pub fn sweep_dead_threads(t: *mut Task) {
    (ehooks().sweep_dead_threads)(t)
}
#[inline]
pub fn reparent_children_to_init(t: &Task) {
    (ehooks().reparent_children_to_init)(t)
}
