//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内核内存统计
//!
//! 提供类似 /proc/meminfo 的内存统计功能，跟踪系统内存使用情况。
//!
//! # 统计内容
//! - 物理内存使用（Frame Allocator）
//! - 堆内存使用（Buddy Allocator）
//! - Slab 分配器使用
//! - Per-CPU Pages 缓存
//! - 页描述符状态
//!
//! 参考：

use super::page::frame_stats;
use super::buddy_allocator::buddy_stats;
use super::slab::slab_stats;
use super::pcp::pcp_stats;
use super::page_desc::page_desc_stats;
use super::PAGE_SIZE;

/// 内存统计信息（类似 /proc/meminfo）
#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    // ========== 物理内存 ==========
    /// 总物理内存（字节）
    pub mem_total: usize,
    /// 空闲物理内存（字节）
    pub mem_free: usize,
    /// 可用物理内存（字节）= mem_free + 可回收内存
    pub mem_available: usize,
    /// 已使用物理内存（字节）
    pub mem_used: usize,

    // ========== 堆内存（Buddy Allocator） ==========
    /// 堆总大小（字节）
    pub heap_total: usize,
    /// 堆已使用（字节）
    pub heap_used: usize,
    /// 堆空闲（字节）
    pub heap_free: usize,

    // ========== Slab 分配器 ==========
    /// Slab 页数
    pub slab_pages: usize,
    /// Slab 分配次数
    pub slab_allocs: usize,
    /// Slab 释放次数
    pub slab_frees: usize,

    // ========== Per-CPU Pages ==========
    /// 各 CPU 的 PCP 页数
    pub pcp_pages: [usize; 4],

    // ========== 页描述符统计 ==========
    /// 空闲页数
    pub pages_free: usize,
    /// 使用中页数
    pub pages_used: usize,
    /// 保留页数
    pub pages_reserved: usize,
    /// 已映射页数
    pub pages_mapped: usize,
    /// 脏页数
    pub pages_dirty: usize,
    /// COW 页数
    pub pages_cow: usize,
    /// 匿名页数
    pub pages_anon: usize,
}

impl Default for MemoryInfo {
    fn default() -> Self {
        Self {
            mem_total: 0,
            mem_free: 0,
            mem_available: 0,
            mem_used: 0,
            heap_total: 0,
            heap_used: 0,
            heap_free: 0,
            slab_pages: 0,
            slab_allocs: 0,
            slab_frees: 0,
            pcp_pages: [0; 4],
            pages_free: 0,
            pages_used: 0,
            pages_reserved: 0,
            pages_mapped: 0,
            pages_dirty: 0,
            pages_cow: 0,
            pages_anon: 0,
        }
    }
}

impl MemoryInfo {
    /// 格式化为人类可读字符串
    pub fn format(&self) -> MemoryInfoFormatter {
        MemoryInfoFormatter { info: self }
    }
}

/// 内存信息格式化器
pub struct MemoryInfoFormatter<'a> {
    info: &'a MemoryInfo,
}

impl<'a> core::fmt::Display for MemoryInfoFormatter<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "Memory Info:")?;
        writeln!(f, "  MemTotal:       {:>10} kB ({} MB)", self.info.mem_total / 1024, self.info.mem_total / 1024 / 1024)?;
        writeln!(f, "  MemFree:        {:>10} kB ({} MB)", self.info.mem_free / 1024, self.info.mem_free / 1024 / 1024)?;
        writeln!(f, "  MemAvailable:   {:>10} kB ({} MB)", self.info.mem_available / 1024, self.info.mem_available / 1024 / 1024)?;
        writeln!(f, "  MemUsed:        {:>10} kB ({} MB)", self.info.mem_used / 1024, self.info.mem_used / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  HeapTotal:      {:>10} kB ({} MB)", self.info.heap_total / 1024, self.info.heap_total / 1024 / 1024)?;
        writeln!(f, "  HeapUsed:       {:>10} kB ({} MB)", self.info.heap_used / 1024, self.info.heap_used / 1024 / 1024)?;
        writeln!(f, "  HeapFree:       {:>10} kB ({} MB)", self.info.heap_free / 1024, self.info.heap_free / 1024 / 1024)?;
        writeln!(f)?;
        writeln!(f, "  SlabPages:      {:>10} pages", self.info.slab_pages)?;
        writeln!(f, "  SlabAllocs:     {:>10}", self.info.slab_allocs)?;
        writeln!(f, "  SlabFrees:      {:>10}", self.info.slab_frees)?;
        writeln!(f)?;
        writeln!(f, "  PCP Pages:      CPU0={} CPU1={} CPU2={} CPU3={}",
            self.info.pcp_pages[0], self.info.pcp_pages[1],
            self.info.pcp_pages[2], self.info.pcp_pages[3])?;
        writeln!(f)?;
        writeln!(f, "  PagesFree:      {:>10}", self.info.pages_free)?;
        writeln!(f, "  PagesUsed:      {:>10}", self.info.pages_used)?;
        writeln!(f, "  PagesReserved:  {:>10}", self.info.pages_reserved)?;
        writeln!(f, "  PagesMapped:    {:>10}", self.info.pages_mapped)?;
        writeln!(f, "  PagesDirty:     {:>10}", self.info.pages_dirty)?;
        writeln!(f, "  PagesCOW:       {:>10}", self.info.pages_cow)?;
        writeln!(f, "  PagesAnon:      {:>10}", self.info.pages_anon)
    }
}

/// 获取完整的内存统计信息
pub fn get_memory_info() -> MemoryInfo {
    let mut info = MemoryInfo::default();

    // 物理内存统计
    let frame_stats = frame_stats();
    info.mem_total = frame_stats.total_bytes;
    info.mem_free = frame_stats.free_bytes;
    info.mem_used = frame_stats.allocated_bytes;
    info.mem_available = frame_stats.free_bytes; // 简化：等于空闲内存

    // 堆内存统计
    let buddy_stats = buddy_stats();
    info.heap_total = buddy_stats.heap_size;
    info.heap_used = buddy_stats.used_bytes;
    info.heap_free = buddy_stats.free_bytes;

    // Slab 统计
    let slab_stats = slab_stats();
    info.slab_pages = slab_stats.total_pages;
    info.slab_allocs = slab_stats.cache_stats.iter().map(|c| c.alloc_count).sum();
    info.slab_frees = slab_stats.cache_stats.iter().map(|c| c.free_count).sum();

    // Per-CPU Pages 统计
    let pcp_stats = pcp_stats();
    for (i, cpu_stat) in pcp_stats.cpu_stats.iter().enumerate() {
        if i < 4 && cpu_stat.initialized {
            info.pcp_pages[i] = cpu_stat.counts.iter().sum();
        }
    }

    // 页描述符统计
    let page_stats = page_desc_stats();
    info.pages_free = page_stats.free_pages;
    info.pages_used = page_stats.used_pages;
    info.pages_reserved = page_stats.reserved_pages;
    info.pages_mapped = page_stats.mapped_pages;
    info.pages_dirty = page_stats.dirty_pages;
    info.pages_cow = page_stats.cow_pages;
    info.pages_anon = page_stats.anonymous_pages;

    info
}

/// 打印内存统计信息
pub fn print_memory_info() {
    let info = get_memory_info();
    crate::println!("{}", info.format());
}

/// 内存使用摘要（用于快速检查）
#[derive(Debug, Clone, Copy, Default)]
pub struct MemorySummary {
    /// 总物理内存（MB）
    pub total_mb: usize,
    /// 已使用物理内存（MB）
    pub used_mb: usize,
    /// 空闲物理内存（MB）
    pub free_mb: usize,
    /// 堆使用率（百分比）
    pub heap_usage_percent: usize,
}

/// 获取内存使用摘要
pub fn get_memory_summary() -> MemorySummary {
    let info = get_memory_info();

    let total_mb = info.mem_total / 1024 / 1024;
    let used_mb = info.mem_used / 1024 / 1024;
    let free_mb = info.mem_free / 1024 / 1024;

    let heap_usage_percent = if info.heap_total > 0 {
        info.heap_used * 100 / info.heap_total
    } else {
        0
    };

    MemorySummary {
        total_mb,
        used_mb,
        free_mb,
        heap_usage_percent,
    }
}

/// 检查内存是否紧张（用于 OOM 预警）
pub fn is_memory_low() -> bool {
    let info = get_memory_info();

    // 如果空闲内存少于总内存的 5%，认为内存紧张
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 5
    } else {
        false
    }
}

/// 检查是否应该触发 OOM
pub fn should_trigger_oom() -> bool {
    let info = get_memory_info();

    // 如果空闲内存少于总内存的 1%，触发 OOM
    if info.mem_total > 0 {
        info.mem_free * 100 / info.mem_total < 1
    } else {
        false
    }
}
