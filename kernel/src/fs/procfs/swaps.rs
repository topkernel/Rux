//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/swaps - active swap areas
//!
//! Format follows Linux proc-swaps(5): a header line, then one row per
//! active swap area. The kernel's single swap backend is a tail carve of
//! the root block device (mm/swap.rs), reported as /dev/vda with type
//! "partition" and the default priority of the first area (-2).

use alloc::string::String;
use alloc::vec::Vec;

/// Generate /proc/swaps content.
pub fn generate() -> Vec<u8> {
    let mut content = String::new();

    // Column layout mirrors Linux: name padded to col 40, type to 52,
    // size/used/priority to 60/66/-end (kB units for size/used).
    content.push_str("Filename\t\t\t\tType\t\tSize\tUsed\tPriority\n");

    if crate::mm::swap::nr_active_swap() {
        let stats = crate::mm::swap::swap_stats();
        let size_kb = stats.swap_total * 4; // pages -> kB
        let used_kb = (stats.swap_total - stats.swap_free) * 4;
        content.push_str(&alloc::format!(
            "/dev/vda                                partition\t{}\t{}\t-2\n",
            size_kb,
            used_kb,
        ));
    }

    content.into_bytes()
}
