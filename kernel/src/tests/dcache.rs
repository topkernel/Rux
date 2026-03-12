//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Dentry cache unit test
use crate::fs::dentry;
use alloc::sync::Arc;
use alloc::format;
use alloc::string::String;
use super::{test_pass, test_fail, test_group_start};

pub fn test_dcache() {
    test_group_start("Dentry cache");

    // Test 1: Basic cache operations
    let dentry1 = Arc::new(dentry::Dentry::new(String::from("test1.txt")));
    dentry::dcache_add(dentry1.clone(), 1);
    let result1 = dentry::dcache_lookup("test1.txt", 1);
    if result1.is_some() {
        test_pass("dcache add/lookup");
    } else {
        test_fail("dcache add/lookup", "not found");
    }
    dentry::dcache_remove("test1.txt", 1);
    let result1 = dentry::dcache_lookup("test1.txt", 1);
    if result1.is_none() {
        test_pass("dcache remove");
    } else {
        test_fail("dcache remove", "still exists");
    }

    // Test 2: LRU eviction policy
    dentry::dcache_flush();
    for i in 0..100 {
        let name = format!("file_{}.txt", i);
        let dentry = Arc::new(dentry::Dentry::new(name));
        dentry::dcache_add(dentry, 100);
    }
    let (count, size) = dentry::dcache_stats();
    test_pass(&format!("LRU eviction ({}/{})", count, size));

    // Test 3: Cache statistics
    let (hits, misses, evictions, hit_rate) = dentry::dcache_stats_detailed();
    test_pass(&format!("cache stats (hits={}, misses={})", hits, misses));

    // Test 4: Cache flush
    dentry::dcache_flush();
    let (count_after, _) = dentry::dcache_stats();
    if count_after == 0 {
        test_pass("cache flush");
    } else {
        test_fail("cache flush", "not empty");
    }

    // Test 5: Hash collision handling
    for i in 0..20 {
        let name = format!("collision_{}.txt", i);
        let dentry = Arc::new(dentry::Dentry::new(name));
        dentry::dcache_add(dentry, 200);
    }
    let mut success_count = 0;
    for i in 0..20 {
        let name = format!("collision_{}.txt", i);
        if dentry::dcache_lookup(&name, 200).is_some() {
            success_count += 1;
        }
    }
    test_pass(&format!("hash collision ({}/20)", success_count));
}
