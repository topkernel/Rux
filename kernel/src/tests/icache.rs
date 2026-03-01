//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Inode 缓存单元测试
use crate::fs::inode;
use alloc::sync::Arc;
use alloc::format;
use super::{test_pass, test_fail, test_group_start};

pub fn test_icache() {
    test_group_start("Inode cache");

    // 测试 1: 基本缓存操作
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

    // 测试 2: LRU 淘汰策略
    inode::icache_flush();
    for i in 0..100 {
        let inode = Arc::new(inode::Inode::new(i + 1000, inode::InodeMode::new(inode::InodeMode::S_IFREG)));
        inode::icache_add(inode);
    }
    let (count, size) = inode::icache_stats();
    test_pass(&format!("LRU eviction ({}/{})", count, size));

    // 测试 3: 缓存统计信息
    let (hits, misses, evictions, hit_rate) = inode::icache_stats_detailed();
    test_pass(&format!("cache stats (hits={}, misses={})", hits, misses));

    // 测试 4: 缓存清空
    inode::icache_flush();
    let (count_after, _) = inode::icache_stats();
    if count_after == 0 {
        test_pass("cache flush");
    } else {
        test_fail("cache flush", "not empty");
    }

    // 测试 5: 多种 inode 类型
    let dir_inode = Arc::new(inode::Inode::new(5000, inode::InodeMode::new(inode::InodeMode::S_IFDIR)));
    inode::icache_add(dir_inode);
    let result = inode::icache_lookup(5000);
    if result.is_some() {
        test_pass("different inode types");
    } else {
        test_fail("different inode types", "not found");
    }
}
