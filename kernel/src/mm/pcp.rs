//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Per-CPU Pages (PCP) - 每CPU页缓存
//!
//! 减少全局页分配器的锁竞争，提高多核性能。
//!
//! 参考：
//! - Linux mm/percpu.c, mm/page_alloc.c
//!
//! # 设计
//! - 每个 CPU 维护独立的页缓存
//! - 分配时优先从本地缓存获取（无锁）
//! - 本地缓存空时批量从全局分配器获取
//! - 本地缓存满时批量归还给全局分配器
//!
//! # 迁移类型 (MigrateType)
//! - Unmovable: 不可移动（内核使用的页）
//! - Movable: 可移动（用户空间页，可迁移）
//! - Reclaimable: 可回收（可被换出）

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::config::MAX_CPUS;
use super::page::{PhysFrame, PAGE_SIZE, alloc_frame, dealloc_frame};
use super::page_desc::{pfn_to_page_mut, PageFlag};

/// 迁移类型数量
pub const MIGRATE_TYPES: usize = 3;

/// 每种迁移类型的页链表最大长度
pub const PCP_HIGH: usize = 64;      // 高水位：超过时归还页面
pub const PCP_LOW: usize = 16;       // 低水位：低于时从全局分配器获取
pub const PCP_BATCH: usize = 16;     // 批量操作数量

/// 迁移类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum MigrateType {
    /// 不可移动
    Unmovable = 0,
    /// 可移动
    Movable = 1,
    /// 可回收
    Reclaimable = 2,
}

/// Per-CPU 页缓存
///
/// 每个 CPU 维护的本地页缓存
#[repr(C)]
pub struct PerCpuPages {
    /// 每种迁移类型的页链表
    /// 存储物理页号 (PPN)，0 表示空
    lists: [usize; MIGRATE_TYPES],
    /// 每种迁移类型的页数
    counts: [usize; MIGRATE_TYPES],
    /// 高水位（超过时归还）
    high: usize,
    /// 批量操作数量
    batch: usize,
    /// 初始化标志
    initialized: bool,
}

impl PerCpuPages {
    /// 创建未初始化的 PerCpuPages
    pub const fn new() -> Self {
        Self {
            lists: [0; MIGRATE_TYPES],
            counts: [0; MIGRATE_TYPES],
            high: PCP_HIGH,
            batch: PCP_BATCH,
            initialized: false,
        }
    }

    /// 初始化 PerCpuPages
    pub fn init(&mut self) {
        self.lists = [0; MIGRATE_TYPES];
        self.counts = [0; MIGRATE_TYPES];
        self.high = PCP_HIGH;
        self.batch = PCP_BATCH;
        self.initialized = true;
    }

    /// 从指定迁移类型分配一个页
    pub fn alloc(&mut self, migratetype: MigrateType) -> Option<PhysFrame> {
        let mt = migratetype as usize;

        // 检查本地缓存是否有页
        if self.counts[mt] > 0 {
            // 从链表头取出一个页
            let pfn = self.lists[mt];
            if pfn == 0 {
                return None;
            }

            // 获取下一个页
            let next = self.get_next_free(pfn);
            self.lists[mt] = next;
            self.counts[mt] -= 1;

            // 清除页的空闲链表指针
            self.clear_next_free(pfn);

            return Some(PhysFrame::new(pfn));
        }

        // 本地缓存为空，从全局分配器批量获取
        self.refill(migratetype)?;

        // 再次尝试分配
        self.alloc(migratetype)
    }

    /// 释放一个页到本地缓存
    pub fn free(&mut self, frame: PhysFrame, migratetype: MigrateType) {
        let mt = migratetype as usize;
        let pfn = frame.number;

        // 将页添加到链表头
        self.set_next_free(pfn, self.lists[mt]);
        self.lists[mt] = pfn;
        self.counts[mt] += 1;

        // 检查是否超过高水位
        if self.counts[mt] >= self.high {
            // 批量归还给全局分配器
            self.drain(migratetype);
        }
    }

    /// 从全局分配器批量获取页
    fn refill(&mut self, migratetype: MigrateType) -> Option<()> {
        let mt = migratetype as usize;
        let batch = self.batch;

        for _ in 0..batch {
            match alloc_frame() {
                Some(frame) => {
                    let pfn = frame.number;
                    // 添加到链表头
                    self.set_next_free(pfn, self.lists[mt]);
                    self.lists[mt] = pfn;
                    self.counts[mt] += 1;
                }
                None => break,  // 全局分配器无可用页
            }
        }

        if self.counts[mt] > 0 {
            Some(())
        } else {
            None
        }
    }

    /// 批量归还页给全局分配器
    fn drain(&mut self, migratetype: MigrateType) {
        let mt = migratetype as usize;
        let batch = self.batch;

        // 保留低水位的页
        while self.counts[mt] > PCP_LOW && self.counts[mt] > batch {
            // 从链表头取出 batch 个页
            for _ in 0..batch {
                let pfn = self.lists[mt];
                if pfn == 0 {
                    break;
                }

                let next = self.get_next_free(pfn);
                self.lists[mt] = next;
                self.counts[mt] -= 1;

                // 清除空闲链表指针
                self.clear_next_free(pfn);

                // 归还给全局分配器
                dealloc_frame(PhysFrame::new(pfn));
            }
        }
    }

    /// 获取页的下一个空闲页指针
    fn get_next_free(&self, pfn: usize) -> usize {
        let page = super::page_desc::pfn_to_page(pfn);
        if page.is_null() {
            return 0;
        }
        unsafe { (*page).next_free() }
    }

    /// 设置页的下一个空闲页指针
    fn set_next_free(&self, pfn: usize, next: usize) {
        let page = super::page_desc::pfn_to_page_mut(pfn);
        if page.is_null() {
            return;
        }
        unsafe {
            (*page).set_next_free(next);
        }
    }

    /// 清除页的空闲链表指针
    fn clear_next_free(&self, pfn: usize) {
        let page = super::page_desc::pfn_to_page_mut(pfn);
        if page.is_null() {
            return;
        }
        unsafe {
            (*page).set_next_free(0);
        }
    }

    /// 获取页数统计
    pub fn count(&self, migratetype: MigrateType) -> usize {
        self.counts[migratetype as usize]
    }

    /// 获取总页数
    pub fn total_count(&self) -> usize {
        self.counts.iter().sum()
    }
}

/// 全局 Per-CPU Pages 数组
///
/// 使用静态数组存储每个 CPU 的页缓存
static mut PER_CPU_PAGES: [PerCpuPages; MAX_CPUS] = [
    PerCpuPages::new(),
    PerCpuPages::new(),
    PerCpuPages::new(),
    PerCpuPages::new(),
];

/// 初始化 Per-CPU Pages
///
/// 在每个 CPU 启动时调用
pub fn init_percpu_pages(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    unsafe {
        PER_CPU_PAGES[cpu_id].init();
    }
}

/// 获取当前 CPU 的 Per-CPU Pages
///
/// # 安全性
/// 调用者必须确保 cpu_id 有效
fn this_cpu_pcp() -> Option<&'static mut PerCpuPages> {
    let cpu_id = crate::arch::cpu_id() as usize;
    if cpu_id >= MAX_CPUS {
        return None;
    }

    unsafe {
        if !PER_CPU_PAGES[cpu_id].initialized {
            return None;
        }
        Some(&mut PER_CPU_PAGES[cpu_id])
    }
}

/// 从 Per-CPU 缓存分配一个页
///
/// 优先从本地 CPU 缓存分配（无锁）
/// 失败时回退到全局分配器
pub fn alloc_page_pcp(migratetype: MigrateType) -> Option<PhysFrame> {
    // 尝试从 Per-CPU 缓存分配
    if let Some(pcp) = this_cpu_pcp() {
        if let Some(frame) = pcp.alloc(migratetype) {
            return Some(frame);
        }
    }

    // 回退到全局分配器
    alloc_frame()
}

/// 释放一个页到 Per-CPU 缓存
///
/// 优先释放到本地 CPU 缓存（无锁）
/// 失败时回退到全局分配器
pub fn free_page_pcp(frame: PhysFrame, migratetype: MigrateType) {
    // 尝试释放到 Per-CPU 缓存
    if let Some(pcp) = this_cpu_pcp() {
        pcp.free(frame, migratetype);
        return;
    }

    // 回退到全局分配器
    dealloc_frame(frame);
}

/// 获取 Per-CPU 缓存统计信息
pub fn pcp_stats() -> PcpStats {
    let mut stats = PcpStats::default();

    unsafe {
        for cpu_id in 0..MAX_CPUS {
            if PER_CPU_PAGES[cpu_id].initialized {
                stats.cpu_stats[cpu_id].initialized = true;
                for mt in 0..MIGRATE_TYPES {
                    stats.cpu_stats[cpu_id].counts[mt] = PER_CPU_PAGES[cpu_id].counts[mt];
                }
            }
        }
    }

    stats
}

/// 单个 CPU 的 Per-CPU 缓存统计
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuPcpStats {
    pub initialized: bool,
    pub counts: [usize; MIGRATE_TYPES],
}

/// 全局 Per-CPU 缓存统计
#[derive(Debug, Clone, Copy, Default)]
pub struct PcpStats {
    pub cpu_stats: [CpuPcpStats; MAX_CPUS],
}

/// 根据分配标志确定迁移类型
pub fn gfp_to_migratetype(gfp_flags: u32) -> MigrateType {
    // 简化实现：默认返回 Movable
    // 完整实现应根据 GFP 标志判断
    if gfp_flags & GFP_KERNEL != 0 {
        MigrateType::Unmovable
    } else {
        MigrateType::Movable
    }
}

/// GFP 标志（Get Free Pages）
pub const GFP_KERNEL: u32 = 0x01;      // 内核分配（不可移动）
pub const GFP_USER: u32 = 0x02;        // 用户分配（可移动）
pub const GFP_ATOMIC: u32 = 0x04;      // 原子分配（不能睡眠）
pub const GFP_HIGHUSER: u32 = 0x08;    // 高端用户内存
pub const GFP_DMA: u32 = 0x10;         // DMA 内存
pub const GFP_NOWAIT: u32 = 0x20;      // 不等待

/// 便捷函数：分配内核页
pub fn alloc_kernel_page() -> Option<PhysFrame> {
    alloc_page_pcp(MigrateType::Unmovable)
}

/// 便捷函数：分配用户页
pub fn alloc_user_page() -> Option<PhysFrame> {
    alloc_page_pcp(MigrateType::Movable)
}

/// 便捷函数：释放内核页
pub fn free_kernel_page(frame: PhysFrame) {
    free_page_pcp(frame, MigrateType::Unmovable);
}

/// 便捷函数：释放用户页
pub fn free_user_page(frame: PhysFrame) {
    free_page_pcp(frame, MigrateType::Movable);
}
