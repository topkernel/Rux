//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/meminfo - Memory information

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Total physical memory in kB.
fn mem_total_kb() -> u64 {
    crate::mm::meminfo::get_basic_memory_info().mem_total as u64 / 1024
}

/// Free memory in kB.
fn mem_free_kb() -> u64 {
    crate::mm::meminfo::get_basic_memory_info().mem_free as u64 / 1024
}

/// Estimated available memory in kB.
fn mem_available_kb() -> u64 {
    crate::mm::meminfo::get_basic_memory_info().mem_available as u64 / 1024
}

/// Used memory in kB.
fn mem_used_kb() -> u64 {
    crate::mm::meminfo::get_basic_memory_info().mem_used as u64 / 1024
}

/// Page-cache pages in kB (4 KiB pages).
fn cached_kb() -> u64 {
    crate::fs::page_cache::page_cache_total_pages() as u64 * 4
}

/// LRU class sizes in kB: (active_file, inactive_file, active_anon,
/// inactive_anon).
fn lru_kb() -> (u64, u64, u64, u64) {
    crate::mm::pglist::first_online_node().map_or((0, 0, 0, 0), |node| {
        let sz = |which: usize| {
            node.lru_sizes[which].load(core::sync::atomic::Ordering::Relaxed) as u64 * 4
        };
        (
            sz(crate::mm::pglist::LRU_ACTIVE_FILE),
            sz(crate::mm::pglist::LRU_INACTIVE_FILE),
            sz(crate::mm::pglist::LRU_ACTIVE_ANON),
            sz(crate::mm::pglist::LRU_INACTIVE_ANON),
        )
    })
}

/// Swap totals in kB: (total, free).
fn swap_kb() -> (u64, u64) {
    let s = crate::mm::swap::swap_stats();
    (s.swap_total as u64 * 4, s.swap_free as u64 * 4)
}

/// Kernel buddy-heap stats in kB: (total, used, free).
fn heap_kb() -> (u64, u64, u64) {
    let h = crate::mm::buddy_allocator::buddy_stats();
    (h.heap_size as u64 / 1024, h.used_bytes as u64 / 1024, h.free_bytes as u64 / 1024)
}

/// The PageCacheOps line value (Rux extension, faultlat
/// observability): hit/miss ratio shows whether the .so working set
/// still thrashes the cache (a miss pays the ext4 big-lock + I/O path
/// on the fault). Cap is dynamic: half of managed RAM, min 32MB.
fn page_cache_ops_line() -> String {
    let (hits, misses, evictions, demotions) = crate::fs::page_cache::page_cache_stats();
    format!(
        "hits={} misses={} evictions={} demotions={} cap_pages={}",
        hits,
        misses,
        evictions,
        demotions,
        crate::fs::page_cache::page_cache_cap_pages()
    )
}

/// Sum of the file+anon LRU classes in kB: (active, inactive).
fn active_inactive_kb() -> (u64, u64) {
    let (af, if_, aa, ia) = lru_kb();
    (af + aa, if_ + ia)
}

/// Generate /proc/meminfo content.
///
/// leg-35: derived by `procfile!` (rust-design-headroom §4, track-1
/// second conversion — the first padded file). The declaration list
/// is the single artifact for the rendered bytes, `MEMINFO_FIELDS`,
/// and the shape test; `meminfo_file_tests` pins the field ORDER and
/// the Linux line shape (this conversion intentionally REPLACES the
/// old hand-written lines' four different key paddings and
/// left-flowed values with the reference kernel's exact layout:
/// `%-16s` key+colon, `%8lu` right-justified value, ` kB` — verified
/// against this host's live /proc/meminfo. Consumers parse by key, so
/// the alignment fix is safe; the `!`-marked rows render unitless,
/// matching Linux's HugePages_* rows which carry no kB suffix).
///
/// All values are in kilobytes except the HugePages_* counters (page
/// counts) and the composite PageCacheOps line.
crate::procfile! {
    /// /proc/meminfo — padded key/value stats (Linux fs/proc/meminfo.c
    /// order; Heap*/PageCacheOps are Rux extensions in the same shape).
    meminfo() fields MEMINFO_FIELDS test meminfo_shape -> Vec<u8> pad 16 unit "kB" {
        "MemTotal" => mem_total_kb(),
        "MemFree" => mem_free_kb(),
        "MemAvailable" => mem_available_kb(),
        "Buffers" => 0,
        "Cached" => cached_kb(),
        "SwapCached" => 0,
        // Kernel heap watermark (Rux extension, the soak observability
        // hook): HeapUsed must oscillate under steady-state load, not
        // climb — a monotonic climb is a kernel-side leak (see the
        // cloexec Vec leak this line was added to guard).
        "HeapTotal" => heap_kb().0,
        "HeapUsed" => heap_kb().1,
        "HeapFree" => heap_kb().2,
        "PageCacheOps!" => page_cache_ops_line(),
        "Active" => active_inactive_kb().0,
        "Inactive" => active_inactive_kb().1,
        "Active(anon)" => lru_kb().2,
        "Inactive(anon)" => lru_kb().3,
        "Active(file)" => lru_kb().0,
        "Inactive(file)" => lru_kb().1,
        "Unevictable" => 0,
        "Mlocked" => 0,
        "SwapTotal" => swap_kb().0,
        "SwapFree" => swap_kb().1,
        "Dirty" => 0,
        "Writeback" => 0,
        "AnonPages" => 0,
        "Mapped" => 0,
        "Shmem" => 0,
        "KReclaimable" => 0,
        "Slab" => 0,
        "SReclaimable" => 0,
        "SUnreclaim" => 0,
        "KernelStack" => 0,
        "PageTables" => 0,
        "NFS_Unstable" => 0,
        "Bounce" => 0,
        "WritebackTmp" => 0,
        "CommitLimit" => mem_total_kb() / 2 + swap_kb().0,
        "Committed_AS" => mem_used_kb(),
        "VmallocTotal" => 536870912u64, // 512 GB virtual
        "VmallocUsed" => 0,
        "VmallocChunk" => 0,
        "Percpu" => 0,
        "HardwareCorrupted" => 0,
        "AnonHugePages" => 0,
        "ShmemHugePages" => 0,
        "ShmemPmdMapped" => 0,
        "FileHugePages" => 0,
        "FilePmdMapped" => 0,
        // HugePages_* carry no unit in Linux (page counts, not kB).
        "HugePages_Total!" => 0,
        "HugePages_Free!" => 0,
        "HugePages_Rsvd!" => 0,
        "HugePages_Surp!" => 0,
        "Hugepagesize" => 2048,
        "Hugetlb" => 0,
        "DirectMap4k" => 4096,
        "DirectMap2M" => mem_total_kb(),
        "DirectMap1G" => 0,
    }
}

/// Backward-compatible alias (the procfs registration names
/// `meminfo::generate`).
pub fn generate() -> Vec<u8> {
    meminfo()
}

#[cfg(any(test, feature = "unit-test"))]
pub mod meminfo_file_tests {
    use alloc::format;
    use alloc::vec::Vec;

    /// The declared order IS the file: every rendered line's key, in
    /// order, equals MEMINFO_FIELDS (markers stripped), and every line
    /// carries the Linux meminfo shape — key+colon starting the line,
    /// the numeric/value column, and the ` kB` suffix unless the key
    /// is unitless (`!`-marked in the declaration). Callable body for
    /// the in-kernel unit-test suite; the #[test] wrapper covers
    /// cargo test.
    pub fn keys_shape_check() {
        let out = super::meminfo();
        let text = core::str::from_utf8(&out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let expect: Vec<&str> = super::MEMINFO_FIELDS
            .iter()
            .map(|k| k.strip_suffix('!').unwrap_or(k))
            .collect();
        assert_eq!(lines.len(), expect.len(), "line count drifted");
        for (line, key) in lines.iter().zip(expect.iter()) {
            let head = format!("{}:", key);
            assert!(
                line.starts_with(&head),
                "line {:?} does not start with declared key {:?}",
                line,
                key
            );
            let unitless = super::MEMINFO_FIELDS
                .iter()
                .any(|k| k.strip_suffix('!') == Some(*key));
            if unitless {
                assert!(!line.ends_with(" kB"), "unitless row gained kB: {:?}", line);
            } else {
                assert!(line.ends_with(" kB"), "row lost its kB unit: {:?}", line);
            }
            // The value column: at least one space after the padded
            // key field, before the value (width-16 key + >=1 space).
            let tail = &line[head.len()..];
            assert!(tail.starts_with(' '), "no gap after key field: {:?}", line);
        }
    }

    #[cfg(test)]
    #[test]
    fn keys_in_declared_order_linux_shape() {
        keys_shape_check();
    }
}
