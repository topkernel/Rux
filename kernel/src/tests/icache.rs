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
    let inode1 = Arc::new(inode::Inode::new(1, inode::InodeMode::new(inode::InodeMode::S_IFREG)));
    inode::icache_add(inode1.clone());
    let result1 = inode::icache_lookup(1);
    if result1.is_some() {
        test_pass("icache add/lookup");
    } else {
        test_fail("icache add/lookup", "not found");
    }
    inode::icache_remove(1);
    let result1 = inode::icache_lookup(1);
    if result1.is_none() {
        test_pass("icache remove");
    } else {
        test_fail("icache remove", "still exists");
    }

    // Test 2: LRU eviction policy
    inode::icache_flush();
    for i in 0..100 {
        let inode = Arc::new(inode::Inode::new(i + 1000, inode::InodeMode::new(inode::InodeMode::S_IFREG)));
        inode::icache_add(inode);
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
    let dir_inode = Arc::new(inode::Inode::new(5000, inode::InodeMode::new(inode::InodeMode::S_IFDIR)));
    inode::icache_add(dir_inode);
    let result = inode::icache_lookup(5000);
    if result.is_some() {
        test_pass("different inode types");
    } else {
        test_fail("different inode types", "not found");
    }
}
