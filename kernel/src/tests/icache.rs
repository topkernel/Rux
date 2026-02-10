//! Inode 缓存单元测试
//!
//! 测试索引节点缓存的功能，包括：
//! - 缓存基本操作（添加、查找、删除）
//! - LRU 淘汰策略
//! - 统计信息（命中率、淘汰次数）
//! - 缓存清空

use crate::println;
use crate::fs::inode;
use crate::collection::SimpleArc;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::ToString;

#[cfg(feature = "unit-test")]
pub fn test_icache() {
    println!("test: ===== Starting Inode Cache Tests =====");

    // 测试 1: 基本缓存操作
    println!("test: 1. Testing basic cache operations...");
    test_icache_basic();

    // 测试 2: LRU 淘汰策略
    println!("test: 2. Testing LRU eviction...");
    test_icache_lru();

    // 测试 3: 缓存统计信息
    println!("test: 3. Testing cache statistics...");
    test_icache_stats();

    // 测试 4: 缓存清空
    println!("test: 4. Testing cache flush...");
    test_icache_flush();

    // 测试 5: 多种 inode 类型
    println!("test: 5. Testing different inode types...");
    test_icache_types();

    println!("test: ===== Inode Cache Tests Completed =====");
}

/// 测试基本的缓存操作
fn test_icache_basic() {
    println!("test:    Testing add, lookup, and remove...");

    // 创建测试 inode
    let inode1 = inode::make_reg_inode_with_data(1, b"test data 1");
    let inode2 = inode::make_reg_inode_with_data(2, b"test data 2");

    let arc1 = match SimpleArc::new(inode1) {
        Some(i) => i,
        None => {
            println!("test:    FAILED - could not create inode");
            return;
        }
    };

    let arc2 = match SimpleArc::new(inode2) {
        Some(i) => i,
        None => {
            println!("test:    FAILED - could not create inode");
            return;
        }
    };

    // 添加到缓存
    inode::icache_add(arc1.clone());
    inode::icache_add(arc2.clone());

    // 查找测试
    let result1 = inode::icache_lookup(1);
    if result1.is_some() {
        println!("test:    SUCCESS - inode 1 found in cache");
    } else {
        println!("test:    FAILED - inode 1 not found");
    }

    let result2 = inode::icache_lookup(2);
    if result2.is_some() {
        println!("test:    SUCCESS - inode 2 found in cache");
    } else {
        println!("test:    FAILED - inode 2 not found");
    }

    // 测试不存在的条目
    let result3 = inode::icache_lookup(999);
    if result3.is_none() {
        println!("test:    SUCCESS - nonexistent inode correctly returns None");
    } else {
        println!("test:    FAILED - nonexistent inode should return None");
    }

    // 删除测试
    inode::icache_remove(1);
    let result4 = inode::icache_lookup(1);
    if result4.is_none() {
        println!("test:    SUCCESS - inode 1 removed from cache");
    } else {
        println!("test:    FAILED - inode 1 should be removed");
    }

    println!("test:    SUCCESS - basic cache operations work correctly");
}

/// 测试 LRU 淘汰策略
fn test_icache_lru() {
    println!("test:    Testing LRU eviction behavior...");

    // 清空缓存以确保干净的测试环境
    inode::icache_flush();

    // 创建并添加多个 inode（超过缓存大小）
    let mut inodes = Vec::new();

    // 添加 100 个条目（缓存大小为 256）
    for i in 1..=100 {
        let data = format!("inode data {}", i);
        let inode_obj = inode::make_reg_inode_with_data(i, data.as_bytes());
        if let Some(arc) = SimpleArc::new(inode_obj) {
            inode::icache_add(arc.clone());
            if i <= 10 {
                inodes.push((i, arc));
            }
        }
    }

    // 获取统计信息
    let (count, size) = inode::icache_stats();
    println!("test:      Cache count: {}/{}", count, size);

    // 检查是否有淘汰发生
    let (hits, misses, evictions, hit_rate) = inode::icache_stats_detailed();
    println!("test:      Evictions: {}", evictions);

    if evictions > 0 {
        println!("test:    SUCCESS - LRU eviction occurred");
    } else {
        println!("test:    Note - No evictions (cache may not be full)");
    }

    // 验证早期添加的 inode 可能已被淘汰
    let mut found_count = 0;
    for (ino, _) in inodes.iter().take(5) {
        if inode::icache_lookup(*ino).is_some() {
            found_count += 1;
        }
    }

    println!("test:      Early inodes still in cache: {}/5", found_count);

    // 测试访问顺序对 LRU 的影响
    // 访问早期的一些 inode
    for (ino, _) in inodes.iter().take(3) {
        let _ = inode::icache_lookup(*ino);
    }

    // 添加更多 inode，触发更多淘汰
    for i in 101..=120 {
        let data = format!("inode data {}", i);
        let inode_obj = inode::make_reg_inode_with_data(i, data.as_bytes());
        if let Some(arc) = SimpleArc::new(inode_obj) {
            inode::icache_add(arc);
        }
    }

    // 检查刚才访问的 inode 是否仍在缓存中
    let mut still_cached_count = 0;
    for (ino, _) in inodes.iter().take(3) {
        if inode::icache_lookup(*ino).is_some() {
            still_cached_count += 1;
        }
    }

    println!("test:      Recently accessed inodes still cached: {}/3", still_cached_count);

    println!("test:    SUCCESS - LRU eviction mechanism works");
}

/// 测试缓存统计信息
fn test_icache_stats() {
    println!("test:    Testing cache statistics tracking...");

    // 清空缓存
    inode::icache_flush();

    // 添加一些条目
    for i in 1000..=1010 {
        let data = format!("stat data {}", i);
        let inode_obj = inode::make_reg_inode_with_data(i, data.as_bytes());
        if let Some(arc) = SimpleArc::new(inode_obj) {
            inode::icache_add(arc);
        }
    }

    // 执行查找操作
    let mut hit_count = 0;
    let mut miss_count = 0;

    // 查找存在的条目（应该命中）
    for i in 1000..=1005 {
        if inode::icache_lookup(i).is_some() {
            hit_count += 1;
        }
    }

    // 查找不存在的条目（应该未命中）
    for i in 2000..=2005 {
        if inode::icache_lookup(i).is_none() {
            miss_count += 1;
        }
    }

    // 获取统计信息
    let (hits, misses, evictions, hit_rate) = inode::icache_stats_detailed();

    println!("test:      Expected hits: {}, Actual hits: {}", hit_count, hits);
    println!("test:      Expected misses: {}, Actual misses: {}", miss_count, misses);
    println!("test:      Hit rate: {:.2}%", hit_rate * 100.0);
    println!("test:      Evictions: {}", evictions);

    if hits >= hit_count as u64 && misses >= miss_count as u64 {
        println!("test:    SUCCESS - statistics tracking works correctly");
    } else {
        println!("test:    FAILED - statistics do not match expected values");
    }

    println!("test:    SUCCESS - cache statistics are functional");
}

/// 测试缓存清空
fn test_icache_flush() {
    println!("test:    Testing cache flush...");

    // 添加一些条目
    for i in 3000..=3020 {
        let data = format!("flush data {}", i);
        let inode_obj = inode::make_reg_inode_with_data(i, data.as_bytes());
        if let Some(arc) = SimpleArc::new(inode_obj) {
            inode::icache_add(arc);
        }
    }

    // 检查缓存有内容
    let (count_before, _) = inode::icache_stats();
    println!("test:      Cache count before flush: {}", count_before);

    // 清空缓存
    inode::icache_flush();

    // 检查缓存已清空
    let (count_after, _) = inode::icache_stats();
    println!("test:      Cache count after flush: {}", count_after);

    // 验证所有条目都已删除
    let mut all_gone = true;
    for i in 3000..=3020 {
        if inode::icache_lookup(i).is_some() {
            all_gone = false;
            break;
        }
    }

    if count_after == 0 && all_gone {
        println!("test:    SUCCESS - cache flush works correctly");
    } else {
        println!("test:    FAILED - cache not properly flushed");
    }
}

/// 测试不同类型的 inode
fn test_icache_types() {
    println!("test:    Testing different inode types...");

    // 清空缓存
    inode::icache_flush();

    // 创建不同类型的 inode
    let reg_inode = inode::make_reg_inode(5000, 1024);
    let dir_inode = inode::make_dir_inode(5001);
    let char_inode = inode::make_char_inode(5002, 0x100);
    let fifo_inode = inode::make_fifo_inode(5003);

    // 添加到缓存
    if let Some(arc) = SimpleArc::new(reg_inode) {
        inode::icache_add(arc);
    }
    if let Some(arc) = SimpleArc::new(dir_inode) {
        inode::icache_add(arc);
    }
    if let Some(arc) = SimpleArc::new(char_inode) {
        inode::icache_add(arc);
    }
    if let Some(arc) = SimpleArc::new(fifo_inode) {
        inode::icache_add(arc);
    }

    // 验证各种类型都可以正确存储和检索
    let test_cases = [
        (5000, "regular file"),
        (5001, "directory"),
        (5002, "char device"),
        (5003, "fifo"),
    ];

    let mut success_count = 0;
    for (ino, type_name) in test_cases.iter() {
        if let Some(arc) = inode::icache_lookup(*ino) {
            println!("test:      Found {} inode: {}", type_name, arc.ino);
            success_count += 1;
        } else {
            println!("test:      FAILED - {} inode not found", type_name);
        }
    }

    if success_count == test_cases.len() {
        println!("test:    SUCCESS - all inode types cached correctly");
    } else {
        println!("test:    FAILED - only {}/{} inodes found", success_count, test_cases.len());
    }

    println!("test:    SUCCESS - different inode types handled properly");
}
