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
    rux_syscall::register_all();
    // The mm policy rows (M1a): carve-live on x86_64, still in-tree on
    // riscv64/aarch64 (their register_in_tree fills the table inside
    // rust_main_impl; OnceSlot first-wins keeps whichever ran first).
    rux_mm::register_all();
    // Guest tests that flipped out with a service crate (pipe2 with
    // rux-fs, net_hdrs with rux-net) run via the framework's
    // SERVICE-TEST SEAM, in registration order, in the same pass/fail
    // counters as the in-framework suite.
    #[cfg(feature = "unit-test")]
    {
        rux::tests::register_service_tests(rux_fs::run_unit_tests);
        rux::tests::register_service_tests(rux_net::run_unit_tests);
    }
    rust_main_impl()
}
