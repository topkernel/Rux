//! The kernel BINARY crate (composition point, crate-flip). The entry
//! symbol rust_main lives in the LIB (boot.S's compile-time `sym`
//! reference resolves there); this bin exists to force-link the lib
//! and, as the service crates land, to own their seam registration
//! ahead of the first syscall (today registration is a no-op default).
#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]
#![recursion_limit = "4096"]
// Composition-point policy (mainline minimal-first): the bin crate is a
// linker shim — its rust_main lives in the lib (whose unsafe collects
// into kernel/src/unsafe.rs). Nothing here adds unsafe; a file-level
// forbid is not possible because the load-bearing #[unsafe(no_mangle)]
// linker marker IS an unsafe-code usage by definition. The gate
// (scripts/gate_unsafe_mainline.py) covers the keyword.

// Force the lib linkage (a bin with no references may drop the rlib).
#[allow(unused_imports)]
use rux::*;

#[cfg(not(feature = "skip_bin_link_check"))]
#[unsafe(no_mangle)]
pub extern "C" fn __bin_composition_marker() {}
