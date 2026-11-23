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
    rux_drivers::register_all();
    // Sched policy rows before the first schedule(): the SchedHooks
    // table backs every pick from scheduler start.
    rux_sched::register_all();
    rux_net::register_all();
    rux_fs::register_all();
    // The ipc policy rows (sysv/posix_mq/keys/access_token) before
    // rust_main_impl: devfs populate and exit/exec/fork/sysctl paths
    // ride the IpcHooks table from their first use.
    rux_ipc::register_all();
    rux_syscall::register_all();
    // The signal policy rows (the rux-signal carve) before
    // rust_main_impl: the send path (kill/tgkill/sigqueue, SIGCHLD from
    // exit, tty ISIG) and the delivery-time default-action table ride
    // the SignalHooks table from their first use. Registered BEFORE the
    // first exit/exec: the SIGCHLD notify path consults it.
    rux_signal::register_all();
    // The mm policy rows (M1a): carve-live on x86_64, still in-tree on
    // riscv64/aarch64 (their register_in_tree fills the table inside
    // rust_main_impl; OnceSlot first-wins keeps whichever ran first).
    rux_mm::register_all();
    // The ext4 rows (rung 5): the ROOT filesystem driver — the hook
    // table backs the boot mount and every fs op from bio::init on;
    // the mount-table rows land at register_builtin_handlers (the
    // row body self-guards that boot point).
    rux_ext4::register_all();
    // The process policy rows (process rung) after sched/mm — the
    // kthread/pid/lifecycle rows are consumed from devfs populate, the
    // boot exec and every kthread spawn onward.
    rux_process::register_all();
    // Guest tests that flipped out with a service crate (pipe2 with
    // rux-fs, net_hdrs with rux-net, the fair pins + timeline with
    // rux-sched) run via the framework's SERVICE-TEST SEAM, in
    // registration order, in the same pass/fail counters as the
    // in-framework suite.
    #[cfg(feature = "unit-test")]
    {
        rux::tests::register_service_tests(rux_fs::run_unit_tests);
        rux::tests::register_service_tests(rux_net::run_unit_tests);
        rux::tests::register_service_tests(rux_sched::run_unit_tests);
        rux::tests::register_service_tests(rux_ext4::run_unit_tests);
        // rux-signal's groups (signal handling, rt_sigprocmask) run at
        // the early seam point (their original in-suite position was
        // pre-fs_io; the procmask family needs no post-fs_io state).
        rux::tests::register_service_tests(rux_signal::run_unit_tests);
        // rux-ipc's entries run at the LATE seam point (post-fs_io, the
        // sysv family's historical position).
        rux::tests::register_service_tests_late(rux_ipc::run_unit_tests);
        // rux-process's families (fork/boundary/pid/smp_schedule) run at
        // the early seam point (their historical positions were early;
        // boundary was last in-framework but is destructive-safe there).
        rux::tests::register_service_tests(rux_process::run_unit_tests);
    }
    rust_main_impl()
}
