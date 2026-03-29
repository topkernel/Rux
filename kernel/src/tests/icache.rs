//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Inode cache unit test
use crate::fs::inode;
use alloc::sync::Arc;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_icache() {
    test_group_start("Inode cache");

    // Test 1: Basic cache operations
    let mut inode1 = inode::Inode::new(1, inode::InodeMode::new(inode::InodeMode::S_IFREG));
    inode1.fs_id = 42;
    let inode1 = Arc::new(inode1);
    inode::icache_add(inode1.clone());
    let result1 = inode::icache_lookup(1, 42);
    if result1.is_some() {
        test_pass("icache add/lookup");
    } else {
        test_fail("icache add/lookup", "not found");
    }
    inode::icache_remove(1, 42);
    let result1 = inode::icache_lookup(1, 42);
    if result1.is_none() {
        test_pass("icache remove");
    } else {
        test_fail("icache remove", "still exists");
    }

    // Test 2: LRU eviction policy
    inode::icache_flush();
    for i in 0..100 {
        let mut inode = inode::Inode::new(i + 1000, inode::InodeMode::new(inode::InodeMode::S_IFREG));
        inode.fs_id = 1;
        inode::icache_add(Arc::new(inode));
    }
    let (count, size) = inode::icache_stats();
    test_pass(&format!("LRU eviction ({}/{})", count, size));

    // Test 3: Cache statistics
    let (hits, misses, evictions, hit_rate) = inode::icache_stats_detailed();
    test_pass(&format!("cache stats (hits={}, misses={})", hits, misses));

    // Test 4: Cache flush
    inode::icache_flush();
    let (count_after, _) = inode::icache_stats();
    if count_after == 0 {
        test_pass("cache flush");
    } else {
        test_fail("cache flush", "not empty");
    }

    // Test 5: Different inode types
    let mut dir_inode = inode::Inode::new(5000, inode::InodeMode::new(inode::InodeMode::S_IFDIR));
    dir_inode.fs_id = 99;
    inode::icache_add(Arc::new(dir_inode));
    let result = inode::icache_lookup(5000, 99);
    if result.is_some() {
        test_pass("different inode types");
    } else {
        test_fail("different inode types", "not found");
    }

    // Test 6: Cross-FS isolation (same ino, different fs_id)
    let mut inode_a = inode::Inode::new(100, inode::InodeMode::new(inode::InodeMode::S_IFREG));
    inode_a.fs_id = 1;
    let mut inode_b = inode::Inode::new(100, inode::InodeMode::new(inode::InodeMode::S_IFDIR));
    inode_b.fs_id = 2;
    inode::icache_add(Arc::new(inode_a));
    inode::icache_add(Arc::new(inode_b));
    let a = inode::icache_lookup(100, 1);
    let b = inode::icache_lookup(100, 2);
    if a.is_some() && b.is_some() {
        let a_mode = a.unwrap().mode.bits();
        let b_mode = b.unwrap().mode.bits();
        if (a_mode & inode::InodeMode::S_IFMT) != (b_mode & inode::InodeMode::S_IFMT) {
            test_pass("cross-FS isolation");
        } else {
            test_fail("cross-FS isolation", "same mode returned");
        }
    } else {
        test_fail("cross-FS isolation", "not found");
    }
    inode::icache_flush();
}
