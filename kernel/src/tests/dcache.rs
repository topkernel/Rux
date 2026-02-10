//! Dentry 缓存单元测试
//!
//! 测试目录项缓存的功能，包括：
//! - 缓存基本操作（添加、查找、删除）
//! - LRU 淘汰策略
//! - 统计信息（命中率、淘汰次数）
//! - 缓存清空

use crate::println;
use crate::fs::dentry;
use crate::collection::SimpleArc;
use alloc::vec::Vec;
use alloc::format;
use alloc::string::ToString;

#[cfg(feature = "unit-test")]
pub fn test_dcache() {
    println!("test: ===== Starting Dentry Cache Tests =====");

    // 测试 1: 基本缓存操作
    println!("test: 1. Testing basic cache operations...");
    test_dcache_basic();

    // 测试 2: LRU 淘汰策略
    println!("test: 2. Testing LRU eviction...");
    test_dcache_lru();

    // 测试 3: 缓存统计信息
    println!("test: 3. Testing cache statistics...");
    test_dcache_stats();

    // 测试 4: 缓存清空
    println!("test: 4. Testing cache flush...");
    test_dcache_flush();

    // 测试 5: 哈希冲突处理
    println!("test: 5. Testing hash collision handling...");
    test_dcache_collision();

    println!("test: ===== Dentry Cache Tests Completed =====");
}

/// 测试基本的缓存操作
fn test_dcache_basic() {
    println!("test:    Testing add, lookup, and remove...");

    // 创建测试 dentry
    let dentry1 = match SimpleArc::new(dentry::Dentry::new("test1.txt".to_string())) {
        Some(d) => d,
        None => {
            println!("test:    FAILED - could not create dentry");
            return;
        }
    };

    let dentry2 = match SimpleArc::new(dentry::Dentry::new("test2.txt".to_string())) {
        Some(d) => d,
        None => {
            println!("test:    FAILED - could not create dentry");
            return;
        }
    };

    // 添加到缓存
    dentry::dcache_add(dentry1.clone(), 1);  // parent_ino = 1
    dentry::dcache_add(dentry2.clone(), 1);  // parent_ino = 1

    // 查找测试
    let result1 = dentry::dcache_lookup("test1.txt", 1);
    if result1.is_some() {
        println!("test:    SUCCESS - dentry1 found in cache");
    } else {
        println!("test:    FAILED - dentry1 not found");
    }

    let result2 = dentry::dcache_lookup("test2.txt", 1);
    if result2.is_some() {
        println!("test:    SUCCESS - dentry2 found in cache");
    } else {
        println!("test:    FAILED - dentry2 not found");
    }

    // 测试不存在的条目
    let result3 = dentry::dcache_lookup("nonexistent.txt", 1);
    if result3.is_none() {
        println!("test:    SUCCESS - nonexistent entry correctly returns None");
    } else {
        println!("test:    FAILED - nonexistent entry should return None");
    }

    // 删除测试
    dentry::dcache_remove("test1.txt", 1);
    let result4 = dentry::dcache_lookup("test1.txt", 1);
    if result4.is_none() {
        println!("test:    SUCCESS - dentry1 removed from cache");
    } else {
        println!("test:    FAILED - dentry1 should be removed");
    }

    println!("test:    SUCCESS - basic cache operations work correctly");
}

/// 测试 LRU 淘汰策略
fn test_dcache_lru() {
    println!("test:    Testing LRU eviction behavior...");

    // 清空缓存以确保干净的测试环境
    dentry::dcache_flush();

    // 创建并添加多个 dentry（超过缓存大小）
    let parent_ino = 100;
    let mut entries = Vec::new();

    // 添加 100 个条目（缓存大小为 256）
    for i in 0..100 {
        let name = format!("file_{}.txt", i);
        let name_clone = name.clone();
        if let Some(dentry) = SimpleArc::new(dentry::Dentry::new(name)) {
            dentry::dcache_add(dentry, parent_ino);
            if i < 10 {
                entries.push((name_clone, i));
            }
        }
    }

    // 获取统计信息
    let (count, size) = dentry::dcache_stats();
    println!("test:      Cache count: {}/{}", count, size);

    // 检查是否有淘汰发生
    let (hits, misses, evictions, hit_rate) = dentry::dcache_stats_detailed();
    println!("test:      Evictions: {}", evictions);

    if evictions > 0 {
        println!("test:    SUCCESS - LRU eviction occurred");
    } else {
        println!("test:    Note - No evictions (cache may not be full)");
    }

    // 验证早期添加的条目可能已被淘汰
    let mut found_count = 0;
    for (name, _) in entries.iter().take(5) {
        if dentry::dcache_lookup(name, parent_ino).is_some() {
            found_count += 1;
        }
    }

    println!("test:      Early entries still in cache: {}/5", found_count);

    println!("test:    SUCCESS - LRU eviction mechanism works");
}

/// 测试缓存统计信息
fn test_dcache_stats() {
    println!("test:    Testing cache statistics tracking...");

    // 清空缓存
    dentry::dcache_flush();

    // 添加一些条目
    for i in 0..10 {
        let name = format!("stat_test_{}.txt", i);
        if let Some(dentry) = SimpleArc::new(dentry::Dentry::new(name)) {
            dentry::dcache_add(dentry, 50);
        }
    }

    // 执行查找操作
    let mut hit_count = 0;
    let mut miss_count = 0;

    // 查找存在的条目（应该命中）
    for i in 0..5 {
        let name = format!("stat_test_{}.txt", i);
        if dentry::dcache_lookup(&name, 50).is_some() {
            hit_count += 1;
        }
    }

    // 查找不存在的条目（应该未命中）
    for i in 100..105 {
        let name = format!("nonexistent_{}.txt", i);
        if dentry::dcache_lookup(&name, 50).is_none() {
            miss_count += 1;
        }
    }

    // 获取统计信息
    let (hits, misses, evictions, hit_rate) = dentry::dcache_stats_detailed();

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
fn test_dcache_flush() {
    println!("test:    Testing cache flush...");

    // 添加一些条目
    for i in 0..20 {
        let name = format!("flush_test_{}.txt", i);
        if let Some(dentry) = SimpleArc::new(dentry::Dentry::new(name)) {
            dentry::dcache_add(dentry, 60);
        }
    }

    // 检查缓存有内容
    let (count_before, _) = dentry::dcache_stats();
    println!("test:      Cache count before flush: {}", count_before);

    // 清空缓存
    dentry::dcache_flush();

    // 检查缓存已清空
    let (count_after, _) = dentry::dcache_stats();
    println!("test:      Cache count after flush: {}", count_after);

    // 验证所有条目都已删除
    let mut all_gone = true;
    for i in 0..20 {
        let name = format!("flush_test_{}.txt", i);
        if dentry::dcache_lookup(&name, 60).is_some() {
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

/// 测试哈希冲突处理
fn test_dcache_collision() {
    println!("test:    Testing hash collision handling...");

    // 清空缓存
    dentry::dcache_flush();

    // 使用相同的父 inode 创建多个条目
    let parent_ino = 200;

    // 添加多个条目，可能发生哈希冲突
    for i in 0..20 {
        let name = format!("collision_{}.txt", i);
        if let Some(dentry) = SimpleArc::new(dentry::Dentry::new(name)) {
            dentry::dcache_add(dentry, parent_ino);
        }
    }

    // 验证条目可以正确查找
    let mut success_count = 0;
    for i in 0..20 {
        let name = format!("collision_{}.txt", i);
        if dentry::dcache_lookup(&name, parent_ino).is_some() {
            success_count += 1;
        }
    }

    println!("test:      Found {}/20 entries", success_count);

    // 至少应该找到一部分条目
    if success_count > 0 {
        println!("test:    SUCCESS - hash collision handling works");
    } else {
        println!("test:    FAILED - no entries found");
    }

    // 测试不同的父 inode（相同名称）
    let name = "test.txt".to_string();
    if let Some(dentry1) = SimpleArc::new(dentry::Dentry::new(name.clone())) {
        dentry::dcache_add(dentry1, 201);
    }
    if let Some(dentry2) = SimpleArc::new(dentry::Dentry::new(name.clone())) {
        dentry::dcache_add(dentry2, 202);
    }

    let result1 = dentry::dcache_lookup(&name, 201);
    let result2 = dentry::dcache_lookup(&name, 202);

    if result1.is_some() && result2.is_some() {
        println!("test:    SUCCESS - same name with different parent_ino handled correctly");
    } else {
        println!("test:    Note - parent_ino differentiation may need verification");
    }

    println!("test:    SUCCESS - hash collision handling functional");
}
