//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tmpfs unit tests — moved here per the test-layout policy: the former
//! in-module `#[cfg(test)]` block (which never compiled in this no_std
//! crate — host-side test blocks are dead weight in kernel sources) and
//! the `run_budget_tests` unit-test function both live under
//! kernel/src/tests/ now.

use crate::fs::tmpfs::{parse_size_pages, TmpfsSuperBlock, TmpfsType, PAGE_SIZE};
use core::sync::atomic::Ordering;

// ========================================================================
// Page store (ported from the former dead #[cfg(test)] block)
// ========================================================================

fn test_page_roundtrip() {
    let sb: &'static TmpfsSuperBlock = alloc::boxed::Box::leak(alloc::boxed::Box::new(TmpfsSuperBlock::new(9001, 0)));
    let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);
    // Sparse write: page 0 and page 2, hole at page 1.
    f.write_at(0, b"hello");
    f.write_at(PAGE_SIZE * 2, b"world");
    rux::test_assert_eq!(f.file_size(), (PAGE_SIZE * 2 + 5) as u64, "sparse size");
    rux::test_assert_eq!(f.page_count(), 2, "two pages allocated");

    let mut buf = [0u8; PAGE_SIZE * 3];
    let n = f.read_at(0, &mut buf);
    rux::test_assert_eq!(n, PAGE_SIZE * 2 + 5, "read length");
    rux::test_assert_eq!(&buf[..5], b"hello", "page 0 bytes");
    // Hole reads as zeros.
    rux::test_assert!(buf[PAGE_SIZE..PAGE_SIZE * 2].iter().all(|&b| b == 0), "hole reads as zeros");
    rux::test_assert_eq!(&buf[PAGE_SIZE * 2..PAGE_SIZE * 2 + 5], b"world", "page 2 bytes");
    crate::tests::test_pass("tmpfs-page-roundtrip");
}

fn test_truncate_zero_tail() {
    let sb: &'static TmpfsSuperBlock = alloc::boxed::Box::leak(alloc::boxed::Box::new(TmpfsSuperBlock::new(9002, 0)));
    let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);
    f.write_at(0, &[0xAAu8; 64]);
    f.truncate(16);
    rux::test_assert_eq!(f.file_size(), 16, "truncated size");
    // Re-extend: the stale tail must read as zeros.
    f.truncate(64);
    let mut buf = [0u8; 64];
    rux::test_assert_eq!(f.read_at(0, &mut buf), 64, "re-extended length");
    rux::test_assert!(buf[16..].iter().all(|&b| b == 0), "stale tail zeroed");
    crate::tests::test_pass("tmpfs-truncate-tail");
}

fn test_dir_ops() {
    let sb: &'static TmpfsSuperBlock = alloc::boxed::Box::leak(alloc::boxed::Box::new(TmpfsSuperBlock::new(9003, 0)));
    let root = sb.root_node.clone();
    let d = sb.new_node(b"d".to_vec(), TmpfsType::Directory);
    root.add_child(d.clone());
    rux::test_assert!(root.find_child(b"d").is_some(), "child found after add");
    rux::test_assert!(d.list_children().is_empty(), "fresh dir is empty");
    rux::test_assert!(root.remove_child(b"d"), "remove succeeded");
    rux::test_assert!(root.find_child(b"d").is_none(), "child gone after remove");
    crate::tests::test_pass("tmpfs-dir-ops");
}

// ========================================================================
// Page budget (ported from fs::tmpfs::run_budget_tests)
// ========================================================================

fn test_budget() {
    // ---- parse_size_pages: mount option string -> page budget ----------
    rux::test_assert_eq!(parse_size_pages(None), 0, "parse: no options -> unlimited");
    rux::test_assert_eq!(parse_size_pages(Some("")), 0, "parse: empty string -> unlimited");
    rux::test_assert_eq!(parse_size_pages(Some("size=32M")), 8192, "parse: size=32M -> 8192 pages");
    rux::test_assert_eq!(parse_size_pages(Some("size=32m")), 8192, "parse: size=32m (lowercase)");
    rux::test_assert_eq!(parse_size_pages(Some("size=1G")), 256 * 1024, "parse: size=1G");
    rux::test_assert_eq!(parse_size_pages(Some("size=512k")), 128, "parse: size=512k");
    // Round UP to whole pages (Linux memparse + PAGE_ALIGN).
    rux::test_assert_eq!(parse_size_pages(Some("size=1")), 1, "parse: size=1 rounds up to 1 page");
    rux::test_assert_eq!(parse_size_pages(Some("size=4097")), 2, "parse: size=4097 rounds up to 2 pages");
    // nr_blocks counts pages.
    rux::test_assert_eq!(parse_size_pages(Some("nr_blocks=100")), 100, "parse: nr_blocks=100");
    rux::test_assert_eq!(parse_size_pages(Some("nr_blocks=10k")), 10240, "parse: nr_blocks=10k");
    // Mixed with other (ignored) options.
    rux::test_assert_eq!(parse_size_pages(Some("mode=700,size=32M")), 8192, "parse: leading ignored option");
    rux::test_assert_eq!(parse_size_pages(Some("size=32M,nr_inodes=1024")), 8192, "parse: trailing ignored option");
    // Percent of the kernel heap.
    let heap_pages = (crate::config::KERNEL_HEAP_SIZE / PAGE_SIZE) as u64;
    rux::test_assert_eq!(parse_size_pages(Some("size=50%")), heap_pages / 2, "parse: size=50% of heap");
    rux::test_assert_eq!(parse_size_pages(Some("size=200%")), heap_pages, "parse: size=200% clamps to heap");
    // Unknown/garbage values ignored (unlimited).
    rux::test_assert_eq!(parse_size_pages(Some("size=abc")), 0, "parse: garbage value ignored");
    rux::test_assert_eq!(parse_size_pages(Some("nr_inodes=100")), 0, "parse: unrelated option ignored");

    // ---- budget enforcement: short writes, ENOSPC shape -----------------
    // LTP preadv203 pattern: a 32MB-capped tmpfs fed ~118MB of writes
    // must stop granting pages, not eat the kernel heap.
    let sb: &'static TmpfsSuperBlock = alloc::boxed::Box::leak(alloc::boxed::Box::new(TmpfsSuperBlock::new(9004, 2))); // 2-page budget
    rux::test_assert_eq!(sb.page_limit_pages.load(Ordering::Relaxed), 2, "budget: limit stored");
    let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);

    // Fill the budget: two distinct pages.
    rux::test_assert_eq!(f.write_at(0, &[1u8; PAGE_SIZE]), PAGE_SIZE, "budget: page 0 stored");
    rux::test_assert_eq!(f.write_at(PAGE_SIZE, &[2u8; PAGE_SIZE]), PAGE_SIZE, "budget: page 1 stored");
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 2, "budget: both pages charged");

    // Third page: refused — short write of the bytes that DID fit
    // (none here: the write starts on a fresh page).
    rux::test_assert_eq!(f.write_at(PAGE_SIZE * 2, &[3u8; PAGE_SIZE]), 0, "budget: third page refused (0 -> ENOSPC)");
    // Partial write: the first page of the range already exists, so
    // its bytes store and the write stops at the page boundary.
    let big = [7u8; PAGE_SIZE * 2];
    let n = f.write_at(PAGE_SIZE, &big);
    rux::test_assert_eq!(n, PAGE_SIZE, "budget: partial write stops at page boundary");
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 2, "budget: still two pages charged");

    // Truncate returns pages to the budget and writes succeed again.
    f.truncate(0);
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 0, "budget: truncate returns pages");
    rux::test_assert_eq!(f.write_at(0, &[9u8; PAGE_SIZE]), PAGE_SIZE, "budget: write works after truncate");

    // Dropping the node returns everything.
    drop(f);
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 0, "budget: drop returns pages");

    // ---- budget is per-MOUNT, shared across files -----------------------
    // Two files on one 1-page tmpfs cannot each take a page.
    let sb: &'static TmpfsSuperBlock = alloc::boxed::Box::leak(alloc::boxed::Box::new(TmpfsSuperBlock::new(9005, 1)));
    let a = sb.new_node(b"a".to_vec(), TmpfsType::RegularFile);
    let b = sb.new_node(b"b".to_vec(), TmpfsType::RegularFile);
    rux::test_assert_eq!(a.write_at(0, &[1u8; 16]), 16, "shared: first file takes the page");
    rux::test_assert_eq!(b.write_at(0, &[1u8; 16]), 0, "shared: second file refused (ENOSPC)");
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 1, "shared: one page charged");
    // Hard links share one pages map — no double charge.
    let a2 = a.clone();
    rux::test_assert_eq!(a2.write_at(100, &[5u8; 16]), 16, "shared: hard link writes into same page");
    rux::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 1, "shared: no double charge for links");

    crate::tests::test_pass("tmpfs-budget");
}

pub fn test_tmpfs() {
    crate::tests::test_group_start("tmpfs");
    test_page_roundtrip();
    test_truncate_zero_tail();
    test_dir_ops();
    test_budget();
}
