//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内存描述符 (Memory Descriptor) - Linux mm_struct 抽象
//!
//!
//! 本模块实现与 Linux 兼容的 mm_struct 抽象，用于描述进程的地址空间。
//!
//! 参考 Linux: include/linux/mm_types.h
//!
//! 主要字段说明：
//! - pgd: 页表基址
//! - mmap: VMA 管理器
//! - start_code/end_code: 代码段范围
//! - start_data/end_data: 数据段范围
//! - start_brk/brk: 堆区域
//! - start_stack: 栈起始地址
//! - arg_start/arg_end: 命令行参数
//! - env_start/env_end: 环境变量
//! - total_vm: 总虚拟内存页数
//! - locked_vm: 锁定的内存页数
//!
//! # 架构设计
//!
//! `MmStruct` 是平台无关的数据结构，包含所有 Linux mm_struct 字段。
//! 架构特定的操作（如页表映射）通过以下方式实现：
//!
//! 1. 平台无关的方法在 `impl MmStruct` 中定义（如字段访问器）
//! 2. 架构特定的方法在 `arch/*/mm.rs` 中通过扩展 trait 或 impl 块添加
//!
//! 这种设计遵循 Linux 的分层架构：
//! - mm_struct 是通用的内存描述符
//! - 架构相关的 pte/pmd/pud/p4d/pgd 操作在 arch 目录中实现

extern crate alloc;

use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use spin::RwLock;

use crate::mm::vma::{VmaManager, Vma, VmaFlags, VmaType};
use crate::mm::page::VirtAddr;
use crate::mm::pagemap::{MapError, Perm, PageTableType};

/// 内存描述符
///
/// 与 Linux mm_struct 对应的结构体，描述进程的完整地址空间。
///
/// # Linux 对应字段
/// ```c
/// struct mm_struct {
///     struct vm_area_struct *mmap;      // VMA 链表
///     struct rb_root mm_mt;             // VMA 红黑树 (maple tree)
///     unsigned long mmap_base;          // mmap 区域基址
///     unsigned long total_vm;           // 总页数
///     unsigned long locked_vm;          // 锁定页数
///     unsigned long start_code, end_code;
///     unsigned long start_data, end_data;
///     unsigned long start_brk, brk;
///     unsigned long start_stack;
///     unsigned long arg_start, arg_end;
///     unsigned long env_start, env_end;
///     atomic_t mm_users;                // 用户计数
///     atomic_t mm_count;                // 引用计数
///     // ...
/// };
/// ```
pub struct MmStruct {
    // ==================== 页表管理 ====================
    /// 页表根节点 PPN (Page Global Directory)
    /// 对应 Linux: pgd_t *pgd
    pub pgd: u64,

    /// VMA 管理器（使用 RwLock 保护内部可变性）
    /// 对应 Linux: struct maple_tree mm_mt (或 struct rb_root mm_rb)
    vma_manager: RwLock<VmaManager>,

    /// 地址空间类型
    space_type: PageTableType,

    // ==================== 段范围（Linux 兼容） ====================
    /// 代码段起始地址
    /// 对应 Linux: mm->start_code
    start_code: AtomicUsize,

    /// 代码段结束地址
    /// 对应 Linux: mm->end_code
    end_code: AtomicUsize,

    /// 数据段起始地址
    /// 对应 Linux: mm->start_data
    start_data: AtomicUsize,

    /// 数据段结束地址
    /// 对应 Linux: mm->end_data
    end_data: AtomicUsize,

    // ==================== 堆管理 ====================
    /// 堆起始地址（brk 的最小值）
    /// 对应 Linux: mm->start_brk
    start_brk: AtomicUsize,

    /// 当前堆指针（brk 的当前值）
    /// 对应 Linux: mm->brk
    brk: AtomicUsize,

    // ==================== 栈管理 ====================
    /// 栈起始地址（栈顶）
    /// 对应 Linux: mm->start_stack
    start_stack: AtomicUsize,

    // ==================== 参数和环境变量 ====================
    /// 命令行参数起始地址
    /// 对应 Linux: mm->arg_start
    arg_start: AtomicUsize,

    /// 命令行参数结束地址
    /// 对应 Linux: mm->arg_end
    arg_end: AtomicUsize,

    /// 环境变量起始地址
    /// 对应 Linux: mm->env_start
    env_start: AtomicUsize,

    /// 环境变量结束地址
    /// 对应 Linux: mm->env_end
    env_end: AtomicUsize,

    // ==================== 虚拟内存统计 ====================
    /// 总虚拟内存页数
    /// 对应 Linux: mm->total_vm
    total_vm: AtomicU64,

    /// 锁定的内存页数
    /// 对应 Linux: mm->locked_vm
    locked_vm: AtomicU64,

    /// 固定的内存页数（pinned）
    /// 对应 Linux: atomic64_t pinned_vm
    pinned_vm: AtomicU64,

    /// 数据段页数
    /// 对应 Linux: mm->data_vm
    data_vm: AtomicU64,

    /// 执行段页数
    /// 对应 Linux: mm->exec_vm
    exec_vm: AtomicU64,

    /// 栈页数
    /// 对应 Linux: mm->stack_vm
    stack_vm: AtomicU64,

    // ==================== mmap 区域管理 ====================
    /// mmap 区域基址
    /// 对应 Linux: mm->mmap_base
    mmap_base: AtomicUsize,

    /// mmap 区域 legacy 基址
    /// 对应 Linux: mm->mmap_legacy_base
    mmap_legacy_base: AtomicUsize,

    /// 最高虚拟内存结束地址
    /// 对应 Linux: mm->highest_vm_end
    highest_vm_end: AtomicUsize,

    // ==================== 引用计数 ====================
    /// 用户计数：共享此 mm 的线程数
    /// 对应 Linux: atomic_t mm_users
    mm_users: AtomicI32,

    /// 引用计数：mm_struct 的生命期引用
    /// 对应 Linux: atomic_t mm_count
    mm_count: AtomicI32,

    // ==================== 其他字段 ====================
    /// 标志位
    /// 对应 Linux: unsigned long flags
    flags: AtomicU64,

    /// 拥有此 mm 的任务（可选）
    /// 对应 Linux: struct task_struct *owner
    owner_pid: AtomicI32,
}

/// MmStruct 标志位
pub struct MmFlags;
impl MmFlags {
    /// 正在转储核心
    pub const MMF_DUMP_CORE: u64 = 0x00000001;
    /// 跳过共享映射
    pub const MMF_DUMP_SKIP_SHARED: u64 = 0x00000002;
    /// 跳过私有映射
    pub const MMF_DUMP_SKIP_PRIVATE: u64 = 0x00000004;
    /// 已转储
    pub const MMF_DUMPED: u64 = 0x00000008;
    /// OOM 通知已禁用
    pub const MMF_OOM_DISABLE: u64 = 0x00000010;
    /// OOM 分数调整
    pub const MMF_OOM_SCORE_ADJ: u64 = 0x00000020;
}

impl MmStruct {
    /// 创建新的内存描述符
    ///
    /// # 参数
    /// - `pgd`: 页表根 PPN
    /// - `space_type`: 地址空间类型
    ///
    /// # 安全性
    /// 调用者必须确保 `pgd` 指向有效的页表
    pub unsafe fn new(pgd: u64, space_type: PageTableType) -> Self {
        use super::vma::RiscVAddressSpaceLayout;
        use super::vma::AddressSpaceLayout;

        let vma_manager = VmaManager::new();
        let brk_default = if space_type == PageTableType::User {
            super::vma::RiscVAddressSpaceLayout::heap_start()
        } else {
            0
        };

        let mmap_base = if space_type == PageTableType::User {
            // mmap 区域从堆区域之后开始
            super::vma::RiscVAddressSpaceLayout::heap_end()
        } else {
            0
        };

        Self {
            pgd,
            vma_manager: RwLock::new(vma_manager),
            space_type,
            // 段范围
            start_code: AtomicUsize::new(0),
            end_code: AtomicUsize::new(0),
            start_data: AtomicUsize::new(0),
            end_data: AtomicUsize::new(0),
            // 堆管理
            start_brk: AtomicUsize::new(brk_default),
            brk: AtomicUsize::new(brk_default),
            // 栈管理
            start_stack: AtomicUsize::new(0),
            // 参数和环境变量
            arg_start: AtomicUsize::new(0),
            arg_end: AtomicUsize::new(0),
            env_start: AtomicUsize::new(0),
            env_end: AtomicUsize::new(0),
            // 虚拟内存统计
            total_vm: AtomicU64::new(0),
            locked_vm: AtomicU64::new(0),
            pinned_vm: AtomicU64::new(0),
            data_vm: AtomicU64::new(0),
            exec_vm: AtomicU64::new(0),
            stack_vm: AtomicU64::new(0),
            // mmap 区域
            mmap_base: AtomicUsize::new(mmap_base),
            mmap_legacy_base: AtomicUsize::new(mmap_base),
            highest_vm_end: AtomicUsize::new(0),
            // 引用计数
            mm_users: AtomicI32::new(1),
            mm_count: AtomicI32::new(1),
            // 其他
            flags: AtomicU64::new(0),
            owner_pid: AtomicI32::new(-1),
        }
    }

    /// 创建共享页表的内存描述符（用于 fork）
    pub unsafe fn new_shared(pgd: u64, space_type: PageTableType, brk: VirtAddr) -> Self {
        let mut mm = Self::new(pgd, space_type);
        mm.start_brk.store(brk.as_usize(), Ordering::Release);
        mm.brk.store(brk.as_usize(), Ordering::Release);
        mm
    }

    /// 创建指定类型的内存描述符
    ///
    /// 这是 `new()` 的别名，提供与旧 `AddressSpace::new_with_type` 兼容的接口
    pub unsafe fn new_with_type(pgd: u64, space_type: PageTableType) -> Self {
        Self::new(pgd, space_type)
    }

    /// 创建内核地址空间（便捷方法）
    pub unsafe fn new_kernel(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::Kernel)
    }

    /// 创建用户地址空间（便捷方法）
    pub unsafe fn new_user(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::User)
    }

    // ==================== 基本访问器 ====================

    /// 获取页表根 PPN
    #[inline]
    pub fn pgd(&self) -> u64 {
        self.pgd
    }

    /// 获取页表根 PPN（Linux 兼容别名）
    ///
    /// 对应 Linux: pgd_t *pgd
    #[inline]
    pub fn root_ppn(&self) -> u64 {
        self.pgd
    }

    /// 获取地址空间类型
    #[inline]
    pub fn space_type(&self) -> PageTableType {
        self.space_type
    }

    // ==================== 段范围访问器 ====================

    /// 获取代码段起始地址
    #[inline]
    pub fn start_code(&self) -> usize {
        self.start_code.load(Ordering::Acquire)
    }

    /// 设置代码段起始地址
    #[inline]
    pub fn set_start_code(&self, addr: usize) {
        self.start_code.store(addr, Ordering::Release);
    }

    /// 获取代码段结束地址
    #[inline]
    pub fn end_code(&self) -> usize {
        self.end_code.load(Ordering::Acquire)
    }

    /// 设置代码段结束地址
    #[inline]
    pub fn set_end_code(&self, addr: usize) {
        self.end_code.store(addr, Ordering::Release);
    }

    /// 获取数据段起始地址
    #[inline]
    pub fn start_data(&self) -> usize {
        self.start_data.load(Ordering::Acquire)
    }

    /// 设置数据段起始地址
    #[inline]
    pub fn set_start_data(&self, addr: usize) {
        self.start_data.store(addr, Ordering::Release);
    }

    /// 获取数据段结束地址
    #[inline]
    pub fn end_data(&self) -> usize {
        self.end_data.load(Ordering::Acquire)
    }

    /// 设置数据段结束地址
    #[inline]
    pub fn set_end_data(&self, addr: usize) {
        self.end_data.store(addr, Ordering::Release);
    }

    // ==================== 堆管理 ====================

    /// 获取堆起始地址
    #[inline]
    pub fn start_brk(&self) -> usize {
        self.start_brk.load(Ordering::Acquire)
    }

    /// 设置堆起始地址
    #[inline]
    pub fn set_start_brk(&self, addr: usize) {
        self.start_brk.store(addr, Ordering::Release);
    }

    /// 获取当前 brk 值
    #[inline]
    pub fn brk(&self) -> VirtAddr {
        VirtAddr::new(self.brk.load(Ordering::Acquire))
    }

    /// 设置 brk 值
    #[inline]
    pub fn set_brk_val(&self, addr: usize) {
        self.brk.store(addr, Ordering::Release);
    }

    // ==================== 栈管理 ====================

    /// 获取栈起始地址
    #[inline]
    pub fn start_stack(&self) -> usize {
        self.start_stack.load(Ordering::Acquire)
    }

    /// 设置栈起始地址
    #[inline]
    pub fn set_start_stack(&self, addr: usize) {
        self.start_stack.store(addr, Ordering::Release);
    }

    // ==================== 参数和环境变量 ====================

    /// 获取命令行参数起始地址
    #[inline]
    pub fn arg_start(&self) -> usize {
        self.arg_start.load(Ordering::Acquire)
    }

    /// 设置命令行参数起始地址
    #[inline]
    pub fn set_arg_start(&self, addr: usize) {
        self.arg_start.store(addr, Ordering::Release);
    }

    /// 获取命令行参数结束地址
    #[inline]
    pub fn arg_end(&self) -> usize {
        self.arg_end.load(Ordering::Acquire)
    }

    /// 设置命令行参数结束地址
    #[inline]
    pub fn set_arg_end(&self, addr: usize) {
        self.arg_end.store(addr, Ordering::Release);
    }

    /// 获取环境变量起始地址
    #[inline]
    pub fn env_start(&self) -> usize {
        self.env_start.load(Ordering::Acquire)
    }

    /// 设置环境变量起始地址
    #[inline]
    pub fn set_env_start(&self, addr: usize) {
        self.env_start.store(addr, Ordering::Release);
    }

    /// 获取环境变量结束地址
    #[inline]
    pub fn env_end(&self) -> usize {
        self.env_end.load(Ordering::Acquire)
    }

    /// 设置环境变量结束地址
    #[inline]
    pub fn set_env_end(&self, addr: usize) {
        self.env_end.store(addr, Ordering::Release);
    }

    // ==================== 虚拟内存统计 ====================

    /// 获取总虚拟内存页数
    #[inline]
    pub fn total_vm(&self) -> u64 {
        self.total_vm.load(Ordering::Acquire)
    }

    /// 增加总虚拟内存页数
    #[inline]
    pub fn add_total_vm(&self, pages: u64) {
        self.total_vm.fetch_add(pages, Ordering::AcqRel);
    }

    /// 减少总虚拟内存页数
    #[inline]
    pub fn sub_total_vm(&self, pages: u64) {
        self.total_vm.fetch_sub(pages, Ordering::AcqRel);
    }

    /// 获取锁定的内存页数
    #[inline]
    pub fn locked_vm(&self) -> u64 {
        self.locked_vm.load(Ordering::Acquire)
    }

    /// 获取固定的内存页数
    #[inline]
    pub fn pinned_vm(&self) -> u64 {
        self.pinned_vm.load(Ordering::Acquire)
    }

    /// 获取数据段页数
    #[inline]
    pub fn data_vm(&self) -> u64 {
        self.data_vm.load(Ordering::Acquire)
    }

    /// 获取执行段页数
    #[inline]
    pub fn exec_vm(&self) -> u64 {
        self.exec_vm.load(Ordering::Acquire)
    }

    /// 获取栈页数
    #[inline]
    pub fn stack_vm(&self) -> u64 {
        self.stack_vm.load(Ordering::Acquire)
    }

    // ==================== mmap 区域 ====================

    /// 获取 mmap 基址
    #[inline]
    pub fn mmap_base(&self) -> usize {
        self.mmap_base.load(Ordering::Acquire)
    }

    /// 设置 mmap 基址
    #[inline]
    pub fn set_mmap_base(&self, addr: usize) {
        self.mmap_base.store(addr, Ordering::Release);
    }

    /// 获取最高虚拟内存结束地址
    #[inline]
    pub fn highest_vm_end(&self) -> usize {
        self.highest_vm_end.load(Ordering::Acquire)
    }

    /// 更新最高虚拟内存结束地址
    #[inline]
    pub fn update_highest_vm_end(&self, addr: usize) {
        let current = self.highest_vm_end.load(Ordering::Acquire);
        if addr > current {
            self.highest_vm_end.store(addr, Ordering::Release);
        }
    }

    // ==================== 引用计数 ====================

    /// 增加用户计数 (mm_users)
    /// 返回增加后的值
    #[inline]
    pub fn mm_users_inc(&self) -> i32 {
        self.mm_users.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 减少用户计数 (mm_users)
    /// 返回减少后的值
    #[inline]
    pub fn mm_users_dec(&self) -> i32 {
        self.mm_users.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 获取用户计数
    #[inline]
    pub fn mm_users(&self) -> i32 {
        self.mm_users.load(Ordering::Acquire)
    }

    /// 增加引用计数 (mm_count)
    #[inline]
    pub fn mm_count_inc(&self) -> i32 {
        self.mm_count.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 减少引用计数 (mm_count)
    #[inline]
    pub fn mm_count_dec(&self) -> i32 {
        self.mm_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 获取引用计数
    #[inline]
    pub fn mm_count(&self) -> i32 {
        self.mm_count.load(Ordering::Acquire)
    }

    // ==================== 标志位 ====================

    /// 获取标志位
    #[inline]
    pub fn flags(&self) -> u64 {
        self.flags.load(Ordering::Acquire)
    }

    /// 设置标志位
    #[inline]
    pub fn set_flags(&self, flags: u64) {
        self.flags.store(flags, Ordering::Release);
    }

    /// 检查是否设置了指定标志
    #[inline]
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags.load(Ordering::Acquire) & flag != 0
    }

    // ==================== 拥有者 ====================

    /// 获取拥有者 PID
    #[inline]
    pub fn owner_pid(&self) -> i32 {
        self.owner_pid.load(Ordering::Acquire)
    }

    /// 设置拥有者 PID
    #[inline]
    pub fn set_owner_pid(&self, pid: i32) {
        self.owner_pid.store(pid, Ordering::Release);
    }

    // ==================== VMA 操作 ====================

    /// 获取 VMA 读锁
    #[inline]
    pub fn vma_read(&self) -> spin::RwLockReadGuard<'_, VmaManager> {
        self.vma_manager.read()
    }

    /// 获取 VMA 写锁
    #[inline]
    pub fn vma_write(&self) -> spin::RwLockWriteGuard<'_, VmaManager> {
        self.vma_manager.write()
    }

    /// 查找 VMA
    pub fn find_vma(&self, addr: VirtAddr) -> Option<Vma> {
        let vma_mgr = self.vma_read();
        vma_mgr.find(addr).cloned()
    }

    /// 添加 VMA 并更新统计
    pub fn add_vma(&self, vma: Vma) -> Result<(), MapError> {
        let pages = vma.page_count() as u64;

        let mut vma_mgr = self.vma_write();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // 更新统计
        self.add_total_vm(pages);

        // 更新最高结束地址
        self.update_highest_vm_end(vma.end().as_usize());

        Ok(())
    }

    /// 删除 VMA 并更新统计
    pub fn remove_vma(&self, start: VirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();
        let vma = vma_mgr.get(start).cloned();

        if let Some(vma) = vma {
            let pages = vma.page_count() as u64;
            vma_mgr.remove(start).map_err(|_| MapError::NotMapped)?;
            self.sub_total_vm(pages);
            Ok(())
        } else {
            Err(MapError::NotMapped)
        }
    }

    // ==================== ELF 加载辅助 ====================

    /// 根据 ELF 段类型设置代码段/数据段范围
    ///
    /// 在 ELF 加载时调用，用于设置 start_code, end_code, start_data, end_data
    pub fn setup_segment_layout(
        &self,
        code_start: usize,
        code_end: usize,
        data_start: usize,
        data_end: usize,
        entry_point: usize,
    ) {
        self.set_start_code(code_start);
        self.set_end_code(code_end);
        self.set_start_data(data_start);
        self.set_end_data(data_end);

        // 更新执行段页数
        let code_pages = ((code_end - code_start + 4095) / 4096) as u64;
        self.exec_vm.store(code_pages, Ordering::Release);

        // 更新数据段页数
        let data_pages = ((data_end - data_start + 4095) / 4096) as u64;
        self.data_vm.store(data_pages, Ordering::Release);

        // 更新总页数
        self.add_total_vm(code_pages + data_pages);
    }

    /// 设置栈布局
    ///
    /// 设置栈地址和统计
    pub fn setup_stack(&self, stack_top: usize, stack_size: usize) {
        self.set_start_stack(stack_top);

        let stack_pages = ((stack_size + 4095) / 4096) as u64;
        self.stack_vm.store(stack_pages, Ordering::Release);
        self.add_total_vm(stack_pages);

        // 更新最高结束地址
        self.update_highest_vm_end(stack_top);
    }

    /// 设置命令行参数布局
    pub fn setup_argv(&self, arg_start: usize, arg_end: usize) {
        self.set_arg_start(arg_start);
        self.set_arg_end(arg_end);
    }

    /// 设置环境变量布局
    pub fn setup_envp(&self, env_start: usize, env_end: usize) {
        self.set_env_start(env_start);
        self.set_env_end(env_end);
    }
}

unsafe impl Send for MmStruct {}
unsafe impl Sync for MmStruct {}

// ============================================================================
// 辅助类型别名（向后兼容）
// ============================================================================

/// 地址空间类型别名（向后兼容）
///
/// 推荐直接使用 MmStruct
pub type AddressSpace = MmStruct;
