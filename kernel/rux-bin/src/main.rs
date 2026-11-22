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
    // regs tables route every dispatch from here on.
    rux_sched::register_all();
    rux_net::register_all();
    rux_syscall::register_all();
    rust_main_impl()
}
