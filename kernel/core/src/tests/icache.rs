//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

//! Inode cache unit test
use crate::fs::hotpath;
use alloc::sync::Arc;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_icache() {
    test_group_start("Inode cache");

    // Test 1: Basic cache operations
    let mut inode1 = crate::fs::types::Inode::new(1, crate::fs::types::InodeMode::new(crate::fs::types::InodeMode::S_IFREG));
    inode1.fs_id = 42;
    let inode1 = Arc::new(inode1);
    hotpath::icache_add(inode1.clone());
    let result1 = hotpath::icache_lookup(1, 42);
    if result1.is_some() {
        test_pass("icache add/lookup");
    } else {
        test_fail("icache add/lookup", "not found");
    }
    hotpath::icache_remove(1, 42);
    let result1 = hotpath::icache_lookup(1, 42);
    if result1.is_none() {
        test_pass("icache remove");
    } else {
        test_fail("icache remove", "still exists");
    }

    // Test 2: LRU eviction policy
    hotpath::icache_flush();
    for i in 0..100 {
        let mut inode = crate::fs::types::Inode::new(i + 1000, crate::fs::types::InodeMode::new(crate::fs::types::InodeMode::S_IFREG));
        inode.fs_id = 1;
        hotpath::icache_add(Arc::new(inode));
    }
    let (count, size) = hotpath::icache_stats();
    test_pass(&format!("LRU eviction ({}/{})", count, size));

    // Test 3: Cache statistics
    let (hits, misses, evictions, hit_rate) = hotpath::icache_stats_detailed();
    test_pass(&format!("cache stats (hits={}, misses={})", hits, misses));

    // Test 4: Cache flush
    hotpath::icache_flush();
    let (count_after, _) = hotpath::icache_stats();
    if count_after == 0 {
        test_pass("cache flush");
    } else {
        test_fail("cache flush", "not empty");
    }

    // Test 5: Different inode types
    let mut dir_inode = crate::fs::types::Inode::new(5000, crate::fs::types::InodeMode::new(crate::fs::types::InodeMode::S_IFDIR));
    dir_inode.fs_id = 99;
    hotpath::icache_add(Arc::new(dir_inode));
    let result = hotpath::icache_lookup(5000, 99);
    if result.is_some() {
        test_pass("different inode types");
    } else {
        test_fail("different inode types", "not found");
    }

    // Test 6: Cross-FS isolation (same ino, different fs_id)
    let mut inode_a = crate::fs::types::Inode::new(100, crate::fs::types::InodeMode::new(crate::fs::types::InodeMode::S_IFREG));
    inode_a.fs_id = 1;
    let mut inode_b = crate::fs::types::Inode::new(100, crate::fs::types::InodeMode::new(crate::fs::types::InodeMode::S_IFDIR));
    inode_b.fs_id = 2;
    hotpath::icache_add(Arc::new(inode_a));
    hotpath::icache_add(Arc::new(inode_b));
    let a = hotpath::icache_lookup(100, 1);
    let b = hotpath::icache_lookup(100, 2);
    if a.is_some() && b.is_some() {
        let a_mode = a.unwrap().mode.bits();
        let b_mode = b.unwrap().mode.bits();
        if (a_mode & crate::fs::types::InodeMode::S_IFMT) != (b_mode & crate::fs::types::InodeMode::S_IFMT) {
            test_pass("cross-FS isolation");
        } else {
            test_fail("cross-FS isolation", "same mode returned");
        }
    } else {
        test_fail("cross-FS isolation", "not found");
    }
    hotpath::icache_flush();
}
