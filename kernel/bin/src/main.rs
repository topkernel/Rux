//! The kernel BINARY package (composition point, crate-flip M3-B).
//! Owns `rust_main`: registers the service crate's dispatch tables
//! into the framework lib, then enters the framework impl. The arch
//! entries bind this symbol at link time (x86 boot.rs's extern-decl
//! sym; riscv/aarch64 boot.S linker symbols).
#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

use rux::rust_main_impl;

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // Service registration MUST precede the first syscall: the args/
    // regs tables route every dispatch from here on. Drivers first:
    // its DriverHooks rows are consumed from devfs populate onward.
    drivers::register_all();
    // Sched policy rows before the first schedule(): the SchedHooks
    // table backs every pick from scheduler start.
    sched::register_all();
    net::register_all();
    fs::register_all();
    // The ipc policy rows (sysv/posix_mq/keys/access_token) before
    // rust_main_impl: devfs populate and exit/exec/fork/sysctl paths
    // ride the IpcHooks table from their first use.
    ipc::register_all();
    syscall::register_all();
    debug::register_all();
    // The signal policy rows (the signal carve) before
    // rust_main_impl: the send path (kill/tgkill/sigqueue, SIGCHLD from
    // exit, tty ISIG) and the delivery-time default-action table ride
    // the SignalHooks table from their first use. Registered BEFORE the
    // first exit/exec: the SIGCHLD notify path consults it.
    signal::register_all();
    // The mm policy rows (M1a): carve-live on x86_64, still in-tree on
    // riscv64/aarch64 (their register_in_tree fills the table inside
    // rust_main_impl; OnceSlot first-wins keeps whichever ran first).
    mm::register_all();
    // The ext4 rows (rung 5): the ROOT filesystem driver — the hook
    // table backs the boot mount and every fs op from bio::init on;
    // the mount-table rows land at register_builtin_handlers (the
    // row body self-guards that boot point).
    ext4::register_all();
    // The process policy rows (process rung) after sched/mm — the
    // kthread/pid/lifecycle rows are consumed from devfs populate, the
    // boot exec and every kthread spawn onward.
    process::register_all();
    // The timer report rows (cold-four lane: timer): the WW
    // heartbeat's formatting half — OnceSlot-published, consumed only
    // behind the EVENT_WWCENSUS arm gate.
    timer::register_all();
    // The sync diagnostic rows (sync-upper lane): the futex cold
    // forensic family's rows — OnceSlot-published, consumed only from
    // cold sites (the do_futex/census latch sections are arm-gated).
    sync::register_all();
    // Guest tests that flipped out with a service crate (pipe2 with
    // fs, net_hdrs with net, the fair pins + timeline with
    // sched) run via the framework's SERVICE-TEST SEAM, in
    // registration order, in the same pass/fail counters as the
    // in-framework suite.
    #[cfg(feature = "unit-test")]
    {
        rux::tests::register_service_tests(fs::run_unit_tests);
        rux::tests::register_service_tests(net::run_unit_tests);
        rux::tests::register_service_tests(sched::run_unit_tests);
        rux::tests::register_service_tests(ext4::run_unit_tests);
        // signal's groups (signal handling, rt_sigprocmask) run at
        // the early seam point (their original in-suite position was
        // pre-fs_io; the procmask family needs no post-fs_io state).
        rux::tests::register_service_tests(signal::run_unit_tests);
        // ipc's entries run at the LATE seam point (post-fs_io, the
        // sysv family's historical position).
        rux::tests::register_service_tests_late(ipc::run_unit_tests);
        // process's families (fork/boundary/pid/smp_schedule) run at
        // the early seam point (their historical positions were early;
        // boundary was last in-framework but is destructive-safe there).
        rux::tests::register_service_tests(process::run_unit_tests);
        // sync's futex group (constants/flags decode) at the early
        // seam point (its historical in-suite position).
        rux::tests::register_service_tests(sync::run_unit_tests);
    }
    // The arch cold diagnostic rows (arch-cold lane): the fatal-fault
    // forensic walkers, the segv-history and ripsample report halves,
    // the remote-BT requester — cold ArchDiagHooks rows consumed only
    // by user-fault deaths, armed reports and hunter pokes (all
    // strictly post-boot).
    arch::register_all();
    rust_main_impl()
}
