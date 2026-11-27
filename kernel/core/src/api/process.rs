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
//! width bar. A fourth consumer table (NsHooks, p23-shrink-process)
//! carries the namespace-policy rows for the fs/ipc hotpath callers
//! that are still framework-side; it dissolves when those lanes land.
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must never call the wrapper that dispatches the same row.
//! Row bodies are the crate's real functions; framework call sites go
//! through these accessors only.
//!
//! Registration runs from rust_main via process::register_all —
//! publication is OnceSlot-only (no Spinlock, no per-CPU state; the
//! x86_64 unit-test kernel lesson, INV-SVCTEST-1's sibling).

#![forbid(unsafe_code)]
use crate::fs::types::Elf64Ehdr;
use crate::fs::types::File;
use crate::process::types::Task;

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
    // p23-shrink-process rows: the arch-thread bridge (boot task
    // creation — init.rs and Task::new_idle_at), the console pgid
    // reader, and the alloc-failure task-kill latch. All boot/rare.
    pub thread_set_entry: fn(&mut crate::arch::thread::ThreadStruct, u64, u64),
    pub current_pgid: fn() -> u32,
    pub memdie_latch: fn() -> bool,
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
        thread_set_entry: no_policy_thread_set_entry,
        current_pgid: no_policy_current_pgid,
        memdie_latch: no_policy_memdie_latch,
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
fn no_policy_thread_set_entry(
    _: &mut crate::arch::thread::ThreadStruct,
    _: u64,
    _: u64,
) {
    panic!("process seam: thread bridge not registered (thread_set_entry)")
}
fn no_policy_current_pgid() -> u32 {
    panic!("process seam: task bridge not registered (current_pgid)")
}
fn no_policy_memdie_latch() -> bool {
    panic!("process seam: thread flags not registered (memdie_latch)")
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

/// Seed a fresh task's context-switch state (the arch-thread bridge; the
/// body lives in the process crate's task_helpers.rs since
/// p23-shrink-process). Callers: init.rs's boot task creator,
/// Task::new_idle_at (both boot-time), the crate's fork path (direct).
#[inline]
pub fn thread_set_entry(thread: &mut crate::arch::thread::ThreadStruct, entry: u64, sp: u64) {
    (hooks().thread_set_entry)(thread, entry, sp)
}

/// The calling task's process group id (console TIOCSCTTY-family reader;
/// body in the crate's task_helpers.rs).
#[inline]
pub fn current_pgid() -> u32 {
    (hooks().current_pgid)()
}

/// Test-and-set TIF_MEMDIE on the current task for the alloc-failure
/// kill_self latch (body in the crate's thread_flags.rs).
#[inline]
pub fn memdie_latch() -> bool {
    (hooks().memdie_latch)()
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
    // p23-shrink-process: the boot-time hash init (documented no-op kept
    // for the historical lib.rs boot sequence).
    pub pid_hash_init: fn(),
}

impl PidHooks {
    pub const EMPTY: PidHooks = PidHooks {
        alloc_pid: no_policy_alloc_pid,
        free_pid: no_policy_free_pid,
        pid_max_live: no_policy_pid_max_live,
        set_pid_max_live: no_policy_set_pid_max_live,
        last_pid: no_policy_last_pid,
        set_last_pid: no_policy_set_last_pid,
        pid_hash_init: no_policy_pid_hash_init,
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
fn no_policy_pid_hash_init() {
    panic!("process seam: pid policy not registered (pid_hash_init)")
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

/// The boot-time PID hash init (lib.rs boot sequence; body in the
/// crate's pid.rs).
#[inline]
pub fn pid_hash_init() {
    (phooks().pid_hash_init)()
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

// ==================== NsHooks (namespace policy) ====================

/// The namespace rows (p23-shrink-process): the ns POLICY moved to the
/// process crate's ns.rs; the framework consumers that stay core-side
/// until their own subsystem pass (fs/ipc hotpath lanes: mount
/// bookkeeping, procfs ns-file opens, /proc/sysvipc inum, the F_SETOWN
/// pid translation) reach it through this table. Every row is a cold
/// site: mount/umount, procfs file open, kill-path translation.
///
/// This is the seam's FOURTH consumer-split table (fs/ipc lane
/// consumers); it dissolves when those lanes land and their callers
/// take the process dep directly. Signatures carry the native core
/// types (crate-plan §3a rule 3): no adapter logic.
#[derive(Clone, Copy)]
pub struct NsHooks {
    pub ns_track_vfsmount: fn(alloc::sync::Arc<crate::fs::types::VfsMountInternal>),
    pub in_init_mnt_ns: fn() -> bool,
    pub current_mnt_ns: fn() -> alloc::sync::Arc<crate::process::types::MountNamespace>,
    pub resolve_local_pid_to_global:
        fn(u32, Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>) -> Option<u32>,
    pub ns_register_mount: fn(&str, &str, &str, &str),
    pub ns_get_mounts: fn() -> alloc::vec::Vec<crate::process::types::MountEntry>,
    pub build_ns_file:
        fn(u32, crate::process::types::NsKind, u32) -> Result<alloc::sync::Arc<crate::fs::types::File>, i32>,
    pub current_ipc_ns: fn() -> alloc::sync::Arc<crate::process::types::IpcNamespace>,
    pub translate_sender_pid_ns:
        fn(u32, Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>) -> u32,
}

impl NsHooks {
    pub const EMPTY: NsHooks = NsHooks {
        ns_track_vfsmount: no_policy_ns_track_vfsmount,
        in_init_mnt_ns: no_policy_in_init_mnt_ns,
        current_mnt_ns: no_policy_current_mnt_ns,
        resolve_local_pid_to_global: no_policy_resolve_local_pid_to_global,
        ns_register_mount: no_policy_ns_register_mount,
        ns_get_mounts: no_policy_ns_get_mounts,
        build_ns_file: no_policy_build_ns_file,
        current_ipc_ns: no_policy_current_ipc_ns,
        translate_sender_pid_ns: no_policy_translate_sender_pid_ns,
    };
}

fn no_policy_ns_track_vfsmount(_: alloc::sync::Arc<crate::fs::types::VfsMountInternal>) {
    panic!("process seam: ns policy not registered (ns_track_vfsmount)")
}
fn no_policy_in_init_mnt_ns() -> bool {
    panic!("process seam: ns policy not registered (in_init_mnt_ns)")
}
fn no_policy_current_mnt_ns() -> alloc::sync::Arc<crate::process::types::MountNamespace> {
    panic!("process seam: ns policy not registered (current_mnt_ns)")
}
fn no_policy_resolve_local_pid_to_global(
    _: u32,
    _: Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>,
) -> Option<u32> {
    panic!("process seam: ns policy not registered (resolve_local_pid_to_global)")
}
fn no_policy_ns_register_mount(_: &str, _: &str, _: &str, _: &str) {
    panic!("process seam: ns policy not registered (ns_register_mount)")
}
fn no_policy_ns_get_mounts() -> alloc::vec::Vec<crate::process::types::MountEntry> {
    panic!("process seam: ns policy not registered (ns_get_mounts)")
}
fn no_policy_build_ns_file(
    _: u32,
    _: crate::process::types::NsKind,
    _: u32,
) -> Result<alloc::sync::Arc<crate::fs::types::File>, i32> {
    panic!("process seam: ns policy not registered (build_ns_file)")
}
fn no_policy_current_ipc_ns() -> alloc::sync::Arc<crate::process::types::IpcNamespace> {
    panic!("process seam: ns policy not registered (current_ipc_ns)")
}
fn no_policy_translate_sender_pid_ns(
    _: u32,
    _: Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>,
) -> u32 {
    panic!("process seam: ns policy not registered (translate_sender_pid_ns)")
}

pub static NS_HOOKS: crate::safety::slot::OnceSlot<NsHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn ns_register_hooks(hooks: NsHooks) {
    let _ = NS_HOOKS.publish(hooks);
}

#[inline]
fn nshooks() -> &'static NsHooks {
    NS_HOOKS.get().unwrap_or(&NsHooks::EMPTY)
}

/// Record a freshly attached mount in the CURRENT mount namespace
/// (sc98 teardown tracking; ns.rs body).
#[inline]
pub fn ns_track_vfsmount(mnt: alloc::sync::Arc<crate::fs::types::VfsMountInternal>) {
    (nshooks().ns_track_vfsmount)(mnt)
}

/// True when the caller runs in the init (boot) mount namespace.
#[inline]
pub fn in_init_mnt_ns() -> bool {
    (nshooks().in_init_mnt_ns)()
}

/// The calling task's mount namespace (init fallback).
#[inline]
pub fn current_mnt_ns() -> alloc::sync::Arc<crate::process::types::MountNamespace> {
    (nshooks().current_mnt_ns)()
}

/// Resolve a pid NUMBER written by a caller in `caller_ns` to the
/// global pid of the task it names (F_SETOWN / fcntl; ns.rs body).
#[inline]
pub fn resolve_local_pid_to_global(
    local: u32,
    caller_ns: Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>,
) -> Option<u32> {
    (nshooks().resolve_local_pid_to_global)(local, caller_ns)
}

/// Register a mount in the current mount namespace (fs::mount bookkeeping).
#[inline]
pub fn ns_register_mount(device: &str, mount_point: &str, fs_type: &str, flags: &str) {
    (nshooks().ns_register_mount)(device, mount_point, fs_type, flags)
}

/// Snapshot of the current namespace's mount table.
#[inline]
pub fn ns_get_mounts() -> alloc::vec::Vec<crate::process::types::MountEntry> {
    (nshooks().ns_get_mounts)()
}

/// Build the ns descriptor File for (pid, kind) without installing an fd
/// (procfs ns magic-link open path).
#[inline]
pub fn build_ns_file(
    pid: u32,
    kind: crate::process::types::NsKind,
    flags: u32,
) -> Result<alloc::sync::Arc<crate::fs::types::File>, i32> {
    (nshooks().build_ns_file)(pid, kind, flags)
}

/// The calling task's IPC namespace (stub kinds share the global objects;
/// /proc/sysvipc inum source).
#[inline]
pub fn current_ipc_ns() -> alloc::sync::Arc<crate::process::types::IpcNamespace> {
    (nshooks().current_ipc_ns)()
}

/// Translate a sender's GLOBAL pid into the pid the receiver sees
/// (siginfo si_pid under pid namespaces; ns.rs body).
#[inline]
pub fn translate_sender_pid_ns(
    sender_global_pid: u32,
    receiver_ns: Option<&alloc::sync::Arc<crate::process::types::PidNamespace>>,
) -> u32 {
    (nshooks().translate_sender_pid_ns)(sender_global_pid, receiver_ns)
}
