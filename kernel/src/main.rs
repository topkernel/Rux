//! The kernel BINARY crate — the COMPOSITION POINT (crate-flip M2).
//! Owns the `rust_main` entry symbol (x86 boot.rs binds it via its
//! extern declaration; riscv/aarch64 boot.S via linker symbols).
//!
//! M2 state: the SYSCALL SERVICE CRATE (kernel/rux-syscall) builds
//! standalone under crate-level #![forbid(unsafe_code)] with the same
//! 12 service files (zero moves — #[path] includes). The bin cannot
//! yet name it: the kernel package carrying both lib and bin cannot
//! depend on rux-syscall (which depends on the lib) without a Cargo
//! package cycle. The three-package topology (kernel lib +
//! rux-syscall + rux-bin, each its own package) is the recorded next
//! step (crate-plan.md); until then registration is the stable
//! no-op seam (syscall::seam_register_default in the lib).
#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Force the lib linkage.
#[allow(unused_imports)]
use rux::rust_main_impl;

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // (When the bin-package split lands, this gains
    // rux_syscall::register_all() ahead of the framework impl.)
    rux::syscall::seam_register_default();
    rust_main_impl()
}
