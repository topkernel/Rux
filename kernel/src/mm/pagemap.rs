//! ARMv8 页表管理
//!
//! 遵循 Linux 内核的页表管理 (arch/arm64/include/asm/pgtable.h)
//!
//! ARMv8 使用 4 级页表：
//! - PGD (Page Global Directory) - 第 4 级
//! - PUD (Page Upper Directory) - 第 3 级
//! - PMD (Page Middle Directory) - 第 2 级
//! - PTE (Page Table Entry) - 第 1 级

use crate::mm::page::{PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE};
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

/// 页表项标志
///
/// 对应 Linux 内核的 PTE/PMD/PUD/PGD 标志
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// 有效位
    pub const VALID: u64 = 1 << 0;
    /// 表项位 (用于中间级别)
    pub const TABLE: u64 = 1 << 1;
    /// 块/页位 (块映射或页映射)
    pub const BLOCK: u64 = 1 << 1;
    /// 访问标志位
    pub const AF: u64 = 1 << 10;
    /// 可执行属性 (UXN)
    pub const UXN: u64 = 1 << 54;
    /// 可执行属性 (PXN)
    pub const PXN: u64 = 1 << 53;
    /// 连续页提示
    pub const CONTIGUOUS: u64 = 1 << 52;
    /// 特权访问异常
    pub const _DBM: u64 = 1 << 51;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & Self::VALID != 0
    }

    #[inline]
    pub fn is_table(&self) -> bool {
        self.0 & Self::TABLE != 0
    }

    #[inline]
    pub fn is_block(&self) -> bool {
        self.0 & Self::BLOCK != 0 && self.0 & Self::TABLE != 0
    }
}

impl Default for PageFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// 访问权限
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Perm {
    /// 无访问
    None = 0,
    /// 只读
    Read = 1,
    /// 读写
    ReadWrite = 2,
    /// 读写执行
    ReadWriteExec = 3,
}

/// 页表类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageTableType {
    /// 内核页表
    Kernel = 0,
    /// 用户页表
    User = 1,
}

/// 页表 (Page Table)
///
/// 包含 512 个页表项（每项 8 字节）
#[repr(C)]
#[repr(align(4096))]
pub struct PageTable {
    entries: [u64; 512],
}

impl PageTable {
    /// 创建新的页表（零初始化）
    pub fn new() -> Self {
        Self { entries: [0; 512] }
    }

    /// 获取页表项
    #[inline]
    pub fn get_entry(&self, index: usize) -> u64 {
        self.entries[index]
    }

    /// 设置页表项
    #[inline]
    pub fn set_entry(&mut self, index: usize, value: u64) {
        self.entries[index] = value;
    }

    /// 清除页表项
    #[inline]
    pub fn clear_entry(&mut self, index: usize) {
        self.entries[index] = 0;
    }
}

/// 内存映射器 (Memory Mapper)
///
/// 管理虚拟地址到物理地址的映射
pub struct MemoryMapper {
    /// 页全局目录 (PGD)
    pgd: *mut PageTable,

    /// 页表类型
    table_type: PageTableType,

    /// 映射计数
    map_count: AtomicU32,
}

impl MemoryMapper {
    /// 创建新的内存映射器
    ///
    /// # Safety
    ///
    /// pgd 必须指向有效的页表
    pub unsafe fn new(pgd: *mut PageTable, table_type: PageTableType) -> Self {
        Self {
            pgd,
            table_type,
            map_count: AtomicU32::new(0),
        }
    }

    /// 映射虚拟页面到物理页面
    ///
    /// # Arguments
    ///
    /// * `virt_addr` - 虚拟地址
    /// * `phys_frame` - 物理页帧
    /// * `perm` - 访问权限
    pub fn map(&self, virt_addr: VirtAddr, phys_frame: PhysFrame, perm: Perm) -> Result<(), MapError> {
        // 计算页表索引
        let pgd_idx = (virt_addr.as_usize() >> 39) & 0x1FF;
        let pud_idx = (virt_addr.as_usize() >> 30) & 0x1FF;
        let pmd_idx = (virt_addr.as_usize() >> 21) & 0x1FF;
        let pte_idx = (virt_addr.as_usize() >> 12) & 0x1FF;

        unsafe {
            let pgd = &mut *self.pgd;

            // 获取或创建 PUD
            let pud_entry = pgd.get_entry(pgd_idx);
            let pud = if pud_entry & 1 == 0 {
                // 需要分配新的 PUD
                let pud_frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                let pud = pud_frame.start_address().as_usize() as *mut PageTable;
                (*pud).entries = [0; 512];

                // 设置 PGD entry
                let pud_value = pud_frame.start_address().as_usize() as u64
                    | PageFlags::TABLE
                    | PageFlags::VALID;
                pgd.set_entry(pgd_idx, pud_value);

                &mut *pud
            } else {
                &mut *((pud_entry & 0x0000_ffff_f000_0000) as *mut PageTable)
            };

            // 获取或创建 PMD
            let pmd_entry = pud.get_entry(pud_idx);
            let pmd = if pmd_entry & 1 == 0 {
                // 需要分配新的 PMD
                let pmd_frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                let pmd = pmd_frame.start_address().as_usize() as *mut PageTable;
                (*pmd).entries = [0; 512];

                // 设置 PUD entry
                let pmd_value = pmd_frame.start_address().as_usize() as u64
                    | PageFlags::TABLE
                    | PageFlags::VALID;
                pud.set_entry(pud_idx, pmd_value);

                &mut *pmd
            } else {
                &mut *((pmd_entry & 0x0000_ffff_f000_0000) as *mut PageTable)
            };

            // 获取或创建 PTE
            let pte_entry = pmd.get_entry(pmd_idx);
            let pte = if pte_entry & 1 == 0 {
                // 需要分配新的 PTE
                let pte_frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                let pte = pte_frame.start_address().as_usize() as *mut PageTable;
                (*pte).entries = [0; 512];

                // 设置 PMD entry
                let pte_value = pte_frame.start_address().as_usize() as u64
                    | PageFlags::TABLE
                    | PageFlags::VALID;
                pmd.set_entry(pmd_idx, pte_value);

                &mut *pte
            } else {
                &mut *((pte_entry & 0x0000_ffff_f000_0000) as *mut PageTable)
            };

            // 设置 PTE entry（最终的页映射）
            let mut pte_value = phys_frame.start_address().as_usize() as u64;
            pte_value |= PageFlags::VALID;
            pte_value |= PageFlags::AF; // 访问标志

            // 设置权限
            match perm {
                Perm::None => {
                    pte_value |= PageFlags::PXN | PageFlags::UXN;
                }
                Perm::Read => {
                    pte_value |= PageFlags::PXN | PageFlags::UXN;
                }
                Perm::ReadWrite => {
                    // 可读写，不可执行
                    pte_value |= PageFlags::PXN | PageFlags::UXN;
                }
                Perm::ReadWriteExec => {
                    // 可读写可执行（内核页面）
                }
            }

            // 用户页面设置属性
            if self.table_type == PageTableType::User {
                // 用户页面设置 AP[1] = 0 (EL0 可访问)
                pte_value |= 1 << 6; // AP[2:1] = 01
            } else {
                // 内核页面设置 AP[1] = 1 (仅 EL1 可访问)
                pte_value |= 1 << 7; // AP[2:1] = 10
            }

            // 检查是否已经映射
            if pte.get_entry(pte_idx) & 1 != 0 {
                return Err(MapError::AlreadyMapped);
            }

            pte.set_entry(pte_idx, pte_value);
        }

        self.map_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 取消映射虚拟页面
    pub fn unmap(&self, virt_addr: VirtAddr) -> Result<(), MapError> {
        let pgd_idx = (virt_addr.as_usize() >> 39) & 0x1FF;
        let pud_idx = (virt_addr.as_usize() >> 30) & 0x1FF;
        let pmd_idx = (virt_addr.as_usize() >> 21) & 0x1FF;
        let pte_idx = (virt_addr.as_usize() >> 12) & 0x1FF;

        unsafe {
            let pgd = &*self.pgd;
            let pud_entry = pgd.get_entry(pgd_idx);

            if pud_entry & 1 == 0 {
                return Err(MapError::NotMapped);
            }

            let pud = &*((pud_entry & 0x0000_ffff_f000_0000) as *const PageTable);
            let pmd_entry = pud.get_entry(pud_idx);

            if pmd_entry & 1 == 0 {
                return Err(MapError::NotMapped);
            }

            let pmd = &*((pmd_entry & 0x0000_ffff_f000_0000) as *const PageTable);
            let pte_entry = pmd.get_entry(pmd_idx);

            if pte_entry & 1 == 0 {
                return Err(MapError::NotMapped);
            }

            let pte = &*((pte_entry & 0x0000_ffff_f000_0000) as *const PageTable);

            if pte.get_entry(pte_idx) & 1 == 0 {
                return Err(MapError::NotMapped);
            }

            // 清除 PTE
            let pte_mut = &mut *((pte_entry & 0x0000_ffff_f000_0000) as *mut PageTable);
            pte_mut.clear_entry(pte_idx);

            // 刷新 TLB
            asm!("tlbi vae1, {}", in(reg) virt_addr.as_usize(), options(nostack));
            asm!("dsb ish", options(nostack));
            asm!("isb", options(nostack));
        }

        self.map_count.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// 查找虚拟地址对应的物理地址
    pub fn translate(&self, virt_addr: VirtAddr) -> Option<PhysAddr> {
        let pgd_idx = (virt_addr.as_usize() >> 39) & 0x1FF;
        let pud_idx = (virt_addr.as_usize() >> 30) & 0x1FF;
        let pmd_idx = (virt_addr.as_usize() >> 21) & 0x1FF;
        let pte_idx = (virt_addr.as_usize() >> 12) & 0x1FF;

        unsafe {
            let pgd = &*self.pgd;
            let pud_entry = pgd.get_entry(pgd_idx);

            if pud_entry & 1 == 0 {
                return None;
            }

            let pud = &*((pud_entry & 0x0000_ffff_f000_0000) as *const PageTable);
            let pmd_entry = pud.get_entry(pud_idx);

            if pmd_entry & 1 == 0 {
                return None;
            }

            let pmd = &*((pmd_entry & 0x0000_ffff_f000_0000) as *const PageTable);
            let pte_entry = pmd.get_entry(pmd_idx);

            if pte_entry & 1 == 0 {
                return None;
            }

            let pte = &*((pte_entry & 0x0000_ffff_f000_0000) as *const PageTable);
            let pte_value = pte.get_entry(pte_idx);

            if pte_value & 1 == 0 {
                return None;
            }

            // 提取物理地址（低12位是标志）
            let phys_addr = pte_value & 0x0000_ffff_f000;
            Some(PhysAddr::new(phys_addr as usize))
        }
    }

    /// 获取映射数量
    #[inline]
    pub fn map_count(&self) -> u32 {
        self.map_count.load(Ordering::Relaxed)
    }

    /// 获取 PGD 地址
    #[inline]
    pub fn pgd_addr(&self) -> usize {
        self.pgd as usize
    }
}

unsafe impl Send for MemoryMapper {}
unsafe impl Sync for MemoryMapper {}

/// 内存映射错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// 已经映射
    AlreadyMapped,
    /// 未映射
    NotMapped,
    /// 内存不足
    OutOfMemory,
    /// 无效参数
    Invalid,
}

/// 地址空间 (Address Space)
///
/// 对应 Linux 内核的 struct mm_struct (include/linux/mm_types.h)
pub struct AddressSpace {
    /// 内存映射器
    mapper: MemoryMapper,

    /// VMA 管理器
    vma_manager: VmaManager,

    /// 地址空间类型
    space_type: PageTableType,
}

use crate::mm::vma::VmaManager;

impl AddressSpace {
    /// 创建新的地址空间
    ///
    /// # Safety
    ///
    /// pgd 必须指向有效的页表
    pub unsafe fn new(pgd: *mut PageTable, space_type: PageTableType) -> Self {
        let mapper = MemoryMapper::new(pgd, space_type);
        let vma_manager = VmaManager::new();

        Self {
            mapper,
            vma_manager,
            space_type,
        }
    }

    /// 映射虚拟内存区域
    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        // 保存VMA信息
        let start = vma.start();
        let end = vma.end();

        // 添加到 VMA 管理器
        self.vma_manager.add(vma).map_err(|_| MapError::Invalid)?;

        // 逐页映射
        let mut addr = start.as_usize();

        while addr < end.as_usize() {
            // 分配物理页帧
            let frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;

            // 映射页面
            self.mapper.map(VirtAddr::new(addr), frame, perm)?;

            addr += PAGE_SIZE;
        }

        Ok(())
    }

    /// 取消映射虚拟内存区域
    pub fn unmap_vma(&mut self, start: VirtAddr) -> Result<(), MapError> {
        // 查找 VMA
        let vma = self.vma_manager.find(start).ok_or(MapError::NotMapped)?;
        let end = vma.end();
        let mut addr = start.as_usize();

        // 逐页取消映射
        while addr < end.as_usize() {
            self.mapper.unmap(VirtAddr::new(addr))?;
            addr += PAGE_SIZE;
        }

        // 从 VMA 管理器移除
        let _ = self.vma_manager.remove(start);

        Ok(())
    }

    /// 查找包含地址的 VMA
    #[inline]
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vma_manager.find(addr)
    }

    /// 获取 VMA 迭代器
    #[inline]
    pub fn vma_iter(&self) -> impl Iterator<Item = &Vma> {
        self.vma_manager.iter()
    }

    /// 获取内存映射器
    #[inline]
    pub fn mapper(&self) -> &MemoryMapper {
        &self.mapper
    }

    /// 获取地址空间类型
    #[inline]
    pub fn space_type(&self) -> PageTableType {
        self.space_type
    }

    /// 复制地址空间（用于fork）
    ///
    /// 创建一个新的地址空间，复制所有VMA和映射
    pub fn fork(&self) -> Result<AddressSpace, MapError> {
        // 分配新的PGD页
        let new_pgd_frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
        let new_pgd = new_pgd_frame.start_address().as_usize() as *mut PageTable;

        // 初始化新的PGD
        unsafe {
            (*new_pgd).entries = [0; 512];
        }

        // 创建新的地址空间
        let new_space = unsafe { AddressSpace::new(new_pgd, self.space_type) };

        // 复制所有VMA
        for vma in self.vma_iter() {
            // 创建新的VMA（复制属性）
            let mut new_vma = Vma::new(vma.start(), vma.end(), vma.flags());
            new_vma.set_type(vma.vma_type());
            new_vma.set_offset(vma.offset());

            // 映射新VMA的页面（写时复制：暂时完全复制）
            let start = vma.start();
            let end = vma.end();
            let mut addr = start.as_usize();

            while addr < end.as_usize() {
                // 查找父进程的物理映射
                if let Some(phys_addr) = self.mapper.translate(VirtAddr::new(addr)) {
                    // 为子进程分配新的物理页并复制内容
                    let new_frame = crate::mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                    let old_frame = PhysFrame::containing_address(phys_addr);

                    // 复制页面内容
                    unsafe {
                        let src = old_frame.start_address().as_usize() as *const u8;
                        let dst = new_frame.start_address().as_usize() as *mut u8;
                        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
                    }

                    // 映射到新地址空间
                    // 从 VMA flags 推断页权限（对应 Linux 的 pgprot_create）
                    let perm = vma.flags().to_page_perm();
                    new_space.mapper.map(
                        VirtAddr::new(addr),
                        new_frame,
                        perm,
                    )?;
                }

                addr += PAGE_SIZE;
            }

            // 添加VMA到新地址空间
            new_space.vma_manager.add(new_vma).map_err(|_| MapError::Invalid)?;
        }

        Ok(new_space)
    }

    /// mmap - 创建内存映射（简化版）
    ///
    /// 对应 Linux 的 mmap 系统调用
    ///
    /// # 参数
    /// - `addr`: 建议的起始地址（0 表示自动选择）
    /// - `size`: 映射大小
    /// - `flags`: VMA 标志
    /// - `vma_type`: VMA 类型
    /// - `perm`: 访问权限
    ///
    /// # 返回
    /// 成功返回映射的起始地址，失败返回错误
    pub fn mmap(
        &mut self,
        addr: VirtAddr,
        size: usize,
        flags: crate::mm::vma::VmaFlags,
        vma_type: crate::mm::vma::VmaType,
        perm: Perm,
    ) -> Result<VirtAddr, MapError> {
        use crate::mm::vma::Vma;

        // 页对齐大小
        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        // 确定起始地址
        let start = if addr.as_usize() == 0 {
            // TODO: 实现自动地址选择
            // 简化版：从用户地址空间开始
            VirtAddr::new(0x1000_0000)
        } else {
            addr
        };

        let end = VirtAddr::new(start.as_usize() + aligned_size);

        // 创建 VMA
        let mut vma = Vma::new(start, end, flags);
        vma.set_type(vma_type);

        // 映射 VMA
        self.map_vma(vma, perm)?;

        Ok(start)
    }

    /// munmap - 取消内存映射（简化版）
    ///
    /// 对应 Linux 的 munmap 系统调用
    ///
    /// # 参数
    /// - `addr`: 起始地址
    /// - `size`: 大小
    pub fn munmap(&mut self, addr: VirtAddr, _size: usize) -> Result<(), MapError> {
        // 简化实现：调用 unmap_vma
        self.unmap_vma(addr)
    }

    /// brk - 改变数据段大小（简化版）
    ///
    /// 对应 Linux 的 brk 系统调用
    ///
    /// # 参数
    /// - `new_brk`: 新的堆顶部地址
    ///
    /// # 返回
    /// 成功返回新的堆顶部地址，失败返回错误
    pub fn brk(&mut self, _new_brk: VirtAddr) -> Result<VirtAddr, MapError> {
        // TODO: 实现堆管理
        // 当前简化实现：总是返回失败
        Err(MapError::Invalid)
    }

    /// allocate_stack - 分配用户栈（简化版）
    ///
    /// 在地址空间顶部创建栈 VMA
    ///
    /// # 参数
    /// - `size`: 栈大小（0 表示使用默认大小）
    ///
    /// # 返回
    /// 成功返回栈顶地址，失败返回错误
    pub fn allocate_stack(
        &mut self,
        size: usize,
    ) -> Result<VirtAddr, MapError> {
        use crate::mm::vma::{Vma, VmaFlags};

        let stack_size = if size == 0 {
            8 * 1024 * 1024  // 8MB 默认栈大小
        } else {
            size
        };

        // 页对齐
        let aligned_size = (stack_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        // 栈位于用户空间顶部（简化版：使用固定地址）
        let stack_top = VirtAddr::new(0x7f_ffff_f000usize & !(PAGE_SIZE - 1));
        let stack_start = VirtAddr::new(stack_top.as_usize() - aligned_size);

        // 创建栈 VMA（可读写、可向下增长）
        let mut flags = VmaFlags::new();
        flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);

        let vma = Vma::new(stack_start, stack_top, flags);
        // 从 VMA flags 推断页权限（确保一致性）
        let perm = flags.to_page_perm();
        self.map_vma(vma, perm)?;

        Ok(stack_top)
    }
}

use crate::mm::vma::Vma;

unsafe impl Send for AddressSpace {}
unsafe impl Sync for AddressSpace {}
