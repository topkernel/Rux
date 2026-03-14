//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/meminfo - Memory information

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Generate /proc/meminfo content
///
/// All values are in kilobytes.
pub fn generate() -> Vec<u8> {
    use crate::mm::meminfo::get_memory_info;

    let info = get_memory_info();
    let mut content = String::new();

    // Convert to KB
    let mem_total_kb = info.mem_total / 1024;
    let mem_free_kb = info.mem_free / 1024;
    let mem_available_kb = info.mem_available / 1024;
    let mem_used_kb = info.mem_used / 1024;

    // Main memory info
    content.push_str(&format!("MemTotal:       {} kB\n", mem_total_kb));
    content.push_str(&format!("MemFree:        {} kB\n", mem_free_kb));
    content.push_str(&format!("MemAvailable:   {} kB\n", mem_available_kb));
    content.push_str("Buffers:               0 kB\n");
    content.push_str("Cached:                0 kB\n");
    content.push_str("SwapCached:            0 kB\n");

    // Active/Inactive memory
    content.push_str(&format!("Active:          {} kB\n", mem_used_kb));
    content.push_str("Inactive:              0 kB\n");
    content.push_str(&format!("Active(anon):    {} kB\n", mem_used_kb));
    content.push_str("Inactive(anon):        0 kB\n");
    content.push_str("Active(file):          0 kB\n");
    content.push_str("Inactive(file):        0 kB\n");
    content.push_str("Unevictable:           0 kB\n");
    content.push_str("Mlocked:               0 kB\n");

    // Swap info
    content.push_str("SwapTotal:             0 kB\n");
    content.push_str("SwapFree:              0 kB\n");

    // Dirty/writeback pages
    content.push_str("Dirty:                 0 kB\n");
    content.push_str("Writeback:             0 kB\n");
    content.push_str("AnonPages:             0 kB\n");
    content.push_str("Mapped:                0 kB\n");
    content.push_str("Shmem:                 0 kB\n");

    // Kernel memory
    content.push_str("KReclaimable:          0 kB\n");
    content.push_str("Slab:                  0 kB\n");
    content.push_str("SReclaimable:          0 kB\n");
    content.push_str("SUnreclaim:            0 kB\n");
    content.push_str("KernelStack:           0 kB\n");
    content.push_str("PageTables:            0 kB\n");

    // NFS (not applicable)
    content.push_str("NFS_Unstable:          0 kB\n");
    content.push_str("Bounce:                0 kB\n");
    content.push_str("WritebackTmp:          0 kB\n");

    // Commit limit
    content.push_str(&format!("CommitLimit:    {} kB\n", mem_total_kb / 2));
    content.push_str(&format!("Committed_AS:    {} kB\n", mem_used_kb));

    // Virtual memory
    content.push_str("VmallocTotal:  536870912 kB\n");  // 512 GB virtual
    content.push_str("VmallocUsed:           0 kB\n");
    content.push_str("VmallocChunk:          0 kB\n");
    content.push_str("Percpu:                0 kB\n");

    // Hardware info
    content.push_str("HardwareCorrupted:     0 kB\n");

    // Huge pages
    content.push_str("AnonHugePages:         0 kB\n");
    content.push_str("ShmemHugePages:        0 kB\n");
    content.push_str("ShmemPmdMapped:        0 kB\n");
    content.push_str("FileHugePages:         0 kB\n");
    content.push_str("FilePmdMapped:         0 kB\n");
    content.push_str("HugePages_Total:       0\n");
    content.push_str("HugePages_Free:        0\n");
    content.push_str("HugePages_Rsvd:        0\n");
    content.push_str("HugePages_Surp:        0\n");
    content.push_str("Hugepagesize:       2048 kB\n");
    content.push_str("Hugetlb:               0 kB\n");

    // Direct mapping
    content.push_str("DirectMap4k:       4096 kB\n");
    content.push_str(&format!("DirectMap2M:     {} kB\n", mem_total_kb));
    content.push_str("DirectMap1G:           0 kB\n");

    content.into_bytes()
}
