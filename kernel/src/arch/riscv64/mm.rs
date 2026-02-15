//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V Sv39 虚拟内存管理
//!
//! RISC-V Sv39 分页规范：
//! - 3 级页表（512 PTE/级）
//! - 39 位虚拟地址（512GB）
//! - 4KB 页大小
//! - 页表项：10 位 PPN + 10 位标志
//!
//! 参考：
//! - RISC-V 特权架构规范 v20211203
//! - Linux arch/riscv/include/asm/pgtable.h
//! - rCore-Tutorial-v3

use crate::println;
use crate::config::MAX_PAGE_TABLES;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// 外部汇编函数（在 usermode_asm.S 中定义）
extern "C" {
    fn switch_to_user_linux_asm(entry: u64, user_stack: u64) -> !;
}

// ==================== 常量定义 ====================

pub const PAGE_SIZE: u64 = 4096;

pub const PAGE_SHIFT: u64 = 12;

pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;

pub const VA_BITS: u64 = 39;

pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

// ==================== 地址类型 ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    /// 创建虚拟地址
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr & VA_MASK)
    }

    /// 获取值
    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    /// 页对齐检查
    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    /// 向下取页
    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    /// 向上取页
    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    /// 页偏移
    #[inline]
    pub fn page_offset(&self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    /// 计算页号
    #[inline]
    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1FF
    }

    /// 获取 u64 值
    #[inline]
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    /// 获取 usize 值
    #[inline]
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// 创建物理地址
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    /// 获取值
    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    /// 页对齐检查
    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    /// 向下取页
    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    /// 向上取页
    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    /// 计算物理页号（PPN）
    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 >> PAGE_SHIFT
    }
}

// ==================== 页表项 ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// V (Valid) - 位 0
    pub const V: u64 = 1 << 0;
    /// R (Read) - 位 1
    pub const R: u64 = 1 << 1;
    /// W (Write) - 位 2
    pub const W: u64 = 1 << 2;
    /// X (Execute) - 位 3
    pub const X: u64 = 1 << 3;
    /// U (User) - 位 4
    pub const U: u64 = 1 << 4;
    /// G (Global) - 位 5
    pub const G: u64 = 1 << 5;
    /// A (Accessed) - 位 6
    pub const A: u64 = 1 << 6;
    /// D (Dirty) - 位 7
    pub const D: u64 = 1 << 7;

    /// 创建空页表项
    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    /// 从位创建
    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// 获取位值
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// 检查是否有效
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & Self::V != 0
    }

    /// 检查是否可读
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::R != 0
    }

    /// 检查是否可写
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::W != 0
    }

    /// 检查是否可执行
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::X != 0
    }

    /// 检查是否为用户页
    #[inline]
    pub fn is_user(&self) -> bool {
        self.0 & Self::U != 0
    }

    /// 获取物理页号（PPN，bits [53:10]）
    #[inline]
    pub fn ppn(&self) -> u64 {
        (self.0 >> 10) & 0x00FFFFFFFFFFFFFF
    }

    /// 创建指向下一级页表的 PTE
    #[inline]
    pub fn new_table(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V)
    }

    /// 创建指向物理页的 PTE（内核权限）
    #[inline]
    pub fn new_page_kernel(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::A | Self::D)
    }

    /// 创建指向物理页的 PTE（用户权限）
    #[inline]
    pub fn new_page_user(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::U | Self::A | Self::D)
    }

    /// 创建指向物理页的 PTE（只读）
    #[inline]
    pub fn new_page_ro(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::X | Self::A)
    }
}

impl Default for PageTableEntry {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== 页表 ====================

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// 创建新页表（清零）
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// 获取页表项
    #[inline]
    pub fn get(&self, index: usize) -> PageTableEntry {
        self.entries[index]
    }

    /// 设置页表项
    #[inline]
    pub fn set(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index] = entry;
    }

    /// 清空页表（所有 PTE 设置为 0）
    pub fn zero(&mut self) {
        for i in 0..512 {
            self.entries[i] = PageTableEntry::new();
        }
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

// ==================== satp CSR ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Satp(pub u64);

impl Satp {
    /// Bare (无地址翻译)
    pub const MODE_BARE: u64 = 0;

    /// Sv39 (39 位虚拟地址)
    pub const MODE_SV39: u64 = 8;

    /// 创建 satp 值
    #[inline]
    pub const fn new(mode: u64, asid: u16, ppn: u64) -> Self {
        Self(((mode as u64) << 60) | ((asid as u64) << 44) | (ppn & 0x0FFFFFFFFFFFFFFF))
    }

    /// 创建 Sv39 satp
    #[inline]
    pub const fn sv39(ppn: u64, asid: u16) -> Self {
        Self::new(Self::MODE_SV39, asid, ppn)
    }

    /// 获取位值
    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    /// 获取模式
    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 >> 60
    }

    /// 检查是否为 Bare 模式（MMU 禁用）
    #[inline]
    pub fn is_bare(&self) -> bool {
        self.mode() == Self::MODE_BARE
    }

    /// 检查是否为 Sv39 模式
    #[inline]
    pub fn is_sv39(&self) -> bool {
        self.mode() == Self::MODE_SV39
    }
}

// ==================== 地址空间 ====================

use crate::mm::vma::{Vma, VmaManager, VmaFlags, VmaType};
use crate::mm::pagemap::{MapError, Perm, PageTableType};
use crate::mm::page::{VirtAddr as PageVirtAddr, PhysAddr as PagePhysAddr, PAGE_SIZE as PAGE_SIZE_USIZE};

pub struct AddressSpace {
    root_ppn: u64,
    vma_manager: VmaManager,
    space_type: PageTableType,
    brk: PageVirtAddr,
}

impl AddressSpace {
    /// 创建新地址空间
    pub unsafe fn new_with_type(root_ppn: u64, space_type: PageTableType) -> Self {
        let vma_manager = VmaManager::new();
        let brk = if space_type == PageTableType::User {
            PageVirtAddr::new(0x1000_0000)
        } else {
            PageVirtAddr::new(0)
        };

        Self {
            root_ppn,
            vma_manager,
            space_type,
            brk,
        }
    }

    pub unsafe fn new(root_ppn: u64) -> Self {
        Self::new_with_type(root_ppn, PageTableType::User)
    }

    pub fn root_ppn(&self) -> u64 {
        self.root_ppn
    }

    pub fn space_type(&self) -> PageTableType {
        self.space_type
    }

    pub unsafe fn enable(&self) {
        let satp = Satp::sv39(self.root_ppn, 0);
        println!("mm: Enabling MMU (Sv39)...");
        println!("mm: satp = {:#x} (MODE={}, PPN={:#x})",
               satp.bits(), satp.mode(), satp.bits() & 0x0FFFFFFFFFFFFFFF);
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
        println!("mm: MMU enabled successfully");
    }

    pub unsafe fn disable() {
        let satp = Satp::new(Satp::MODE_BARE, 0, 0);
        println!("mm: Disabling MMU...");
        asm!("csrw satp, {}", in(reg) satp.bits());
        asm!("sfence.vma zero, zero");
        println!("mm: MMU disabled");
    }

    pub unsafe fn flush_tlb() {
        asm!("sfence.vma zero, zero");
    }

    pub unsafe fn flush_tlb_addr_page(vaddr: PageVirtAddr) {
        asm!("sfence.vma {}, zero", in(reg) vaddr.as_usize());
    }

    // ==================== VMA 操作 ====================

    pub fn map_vma(&self, vma: Vma, perm: Perm) -> Result<(), MapError> {
        use crate::mm;
        let start = vma.start();
        let end = vma.end();
        self.vma_manager.add(vma).map_err(|_| MapError::Invalid)?;

        let mut addr = start.as_usize();
        while addr < end.as_usize() {
            let frame = mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
            let flags = perm_to_flags(perm, self.space_type);
            // 转换为 RISC-V 类型并映射
            unsafe {
                map_page(
                    self.root_ppn,
                    VirtAddr::new(addr as u64),
                    PhysAddr::new(frame.start_address().as_usize() as u64),
                    flags,
                );
            }
            addr += PAGE_SIZE_USIZE;
        }
        Ok(())
    }

    pub fn unmap_vma(&mut self, start: PageVirtAddr) -> Result<(), MapError> {
        let vma = self.vma_manager.find(start).ok_or(MapError::NotMapped)?;
        let end = vma.end();
        let _ = self.vma_manager.remove(start);
        // TODO: 实际取消映射页表项
        Ok(())
    }

    pub fn find_vma(&self, addr: PageVirtAddr) -> Option<&Vma> {
        self.vma_manager.find(addr)
    }

    pub fn vma_iter(&self) -> impl Iterator<Item = &Vma> {
        self.vma_manager.iter()
    }

    pub fn vma_manager_mut(&mut self) -> &mut VmaManager {
        &mut self.vma_manager
    }

    pub fn mmap(
        &mut self,
        addr: PageVirtAddr,
        size: usize,
        flags: VmaFlags,
        vma_type: VmaType,
        perm: Perm,
    ) -> Result<PageVirtAddr, MapError> {
        let aligned_size = (size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);
        if aligned_size == 0 {
            return Err(MapError::Invalid);
        }

        let start = if addr.as_usize() == 0 {
            PageVirtAddr::new(0x1000_0000)
        } else {
            addr
        };

        let end = PageVirtAddr::new(start.as_usize() + aligned_size);
        let mut vma = Vma::new(start, end, flags);
        vma.set_type(vma_type);
        self.map_vma(vma, perm)?;
        Ok(start)
    }

    pub fn munmap(&mut self, addr: PageVirtAddr, _size: usize) -> Result<(), MapError> {
        self.unmap_vma(addr)
    }

    pub fn brk(&mut self, new_brk: PageVirtAddr) -> Result<PageVirtAddr, MapError> {
        use crate::mm;

        if new_brk.as_usize() == 0 {
            return Ok(self.brk);
        }

        if self.space_type != PageTableType::User {
            return Err(MapError::Invalid);
        }

        const HEAP_START: usize = 0x1000_0000;
        const HEAP_END: usize = 0x2000_0000;

        if new_brk.as_usize() < HEAP_START || new_brk.as_usize() > HEAP_END {
            return Ok(self.brk);
        }

        if new_brk.as_usize() < self.brk.as_usize() {
            self.brk = new_brk;
            return Ok(new_brk);
        }

        if new_brk.as_usize() > self.brk.as_usize() {
            let old_brk = self.brk;
            let old_brk_aligned = PageVirtAddr::new(old_brk.as_usize() & !(PAGE_SIZE_USIZE - 1));
            let new_brk_aligned = PageVirtAddr::new(new_brk.as_usize() & !(PAGE_SIZE_USIZE - 1));

            let mut addr = old_brk_aligned;
            while addr.as_usize() < new_brk_aligned.as_usize() {
                if unsafe { PageTableWalker::walk(self.root_ppn, addr.as_usize() as u64) }.is_none() {
                    let frame = mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                    let flags = perm_to_flags(Perm::ReadWrite, self.space_type);
                    unsafe {
                        map_page(
                            self.root_ppn,
                            VirtAddr::new(addr.as_usize() as u64),
                            PhysAddr::new(frame.start_address().as_usize() as u64),
                            flags,
                        );
                    }

                    let mut vma_flags = VmaFlags::new();
                    vma_flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSUP);
                    let vma = Vma::new(
                        addr,
                        PageVirtAddr::new(addr.as_usize() + PAGE_SIZE_USIZE),
                        vma_flags,
                    );
                    let _ = self.vma_manager.add(vma);
                }
                addr = PageVirtAddr::new(addr.as_usize() + PAGE_SIZE_USIZE);
            }

            self.brk = new_brk;
            return Ok(new_brk);
        }

        Ok(self.brk)
    }

    pub fn allocate_stack(&mut self, size: usize) -> Result<PageVirtAddr, MapError> {
        let stack_size = if size == 0 { 8 * 1024 * 1024 } else { size };
        let aligned_size = (stack_size + PAGE_SIZE_USIZE - 1) & !(PAGE_SIZE_USIZE - 1);

        let stack_top = PageVirtAddr::new(0x7fff_f000 & !(PAGE_SIZE_USIZE - 1));
        let stack_start = PageVirtAddr::new(stack_top.as_usize() - aligned_size);

        let mut flags = VmaFlags::new();
        flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);
        let vma = Vma::new(stack_start, stack_top, flags);
        self.map_vma(vma, Perm::ReadWrite)?;
        Ok(stack_top)
    }

    pub fn fork(&self) -> Result<AddressSpace, MapError> {
        use crate::mm;

        let new_root_frame = mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
        let new_root_ppn = new_root_frame.start_address().as_usize() as u64 >> PAGE_SHIFT as u64;

        unsafe {
            let new_root_table = (new_root_ppn << PAGE_SHIFT) as *mut PageTable;
            (*new_root_table).zero();
        }

        let mut new_space = unsafe { AddressSpace::new_with_type(new_root_ppn, self.space_type) };
        new_space.brk = self.brk;

        for vma in self.vma_iter() {
            let mut new_vma = Vma::new(vma.start(), vma.end(), vma.flags());
            new_vma.set_type(vma.vma_type());
            new_vma.set_offset(vma.offset());

            let start = vma.start();
            let end = vma.end();
            let mut addr = start.as_usize();

            while addr < end.as_usize() {
                let ppn = unsafe { PageTableWalker::walk(self.root_ppn, addr as u64) };
                if let Some(ppn) = ppn {
                    let new_frame = mm::alloc_frame().ok_or(MapError::OutOfMemory)?;
                    let old_phys_addr = PagePhysAddr::new((ppn << PAGE_SHIFT) as usize);

                    unsafe {
                        let src = old_phys_addr.as_usize() as *const u8;
                        let dst = new_frame.start_address().as_usize() as *mut u8;
                        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE_USIZE);
                    }

                    let perm = vma.flags().to_page_perm();
                    let flags = perm_to_flags(perm, self.space_type);
                    unsafe {
                        map_page(
                            new_root_ppn,
                            VirtAddr::new(addr as u64),
                            PhysAddr::new(new_frame.start_address().as_usize() as u64),
                            flags,
                        );
                    }
                }
                addr += PAGE_SIZE_USIZE;
            }

            new_space.vma_manager.add(new_vma).map_err(|_| MapError::Invalid)?;
        }

        Ok(new_space)
    }
}

fn perm_to_flags(perm: Perm, space_type: PageTableType) -> u64 {
    let mut flags = PageTableEntry::V | PageTableEntry::A | PageTableEntry::D;
    match perm {
        Perm::None => {}
        Perm::Read => {
            flags |= PageTableEntry::R;
        }
        Perm::ReadWrite => {
            flags |= PageTableEntry::R | PageTableEntry::W;
        }
        Perm::ReadWriteExec => {
            flags |= PageTableEntry::R | PageTableEntry::W | PageTableEntry::X;
        }
    }
    if space_type == PageTableType::User {
        flags |= PageTableEntry::U;
    }
    flags
}

// ==================== MMU 初始化 ====================

#[link_section = ".pagetables"]
static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[link_section = ".bss"]
static mut TRAP_STACKS: [[u8; 16384]; 4] = [[0; 16384]; 4];  // 4 CPUs

pub unsafe fn get_trap_stack() -> u64 {
    let cpu_id = crate::arch::riscv64::smp::cpu_id() as usize;
    if cpu_id >= 4 {
        panic!("mm: Invalid CPU ID {}", cpu_id);
    }
    let stack_base = &mut TRAP_STACKS[cpu_id] as *mut [u8; 16384] as *mut u8;
    stack_base.add(16384) as u64  // 栈顶
}

unsafe fn alloc_page_table() -> &'static mut PageTable {
    // 使用静态分配的页表（简化实现）
    // 每个页表占用一个 4KB 页面
    // 放置在 .pagetables 段，避免因代码增长导致位置变化
    #[link_section = ".pagetables"]
    static mut PAGE_TABLES: [PageTable; MAX_PAGE_TABLES] = [PageTable::new(); MAX_PAGE_TABLES];
    static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

    let idx = NEXT_INDEX.fetch_add(1, Ordering::AcqRel);
    if idx >= PAGE_TABLES.len() {
        panic!("mm: Out of page table pages (allocated {})", idx);
    }

    // println!("mm: alloc_page_table: allocated index {}", idx);
    &mut PAGE_TABLES[idx]
}

unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // 提取虚拟页号（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // 获取根页表（L2）
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = root_table_addr as *mut PageTable;

    let root = &mut *root_table;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        // 已存在 L1 页表
        pte2.ppn()
    } else {
        // 分配新的 L1 页表
        let table = alloc_page_table();
        let ppn = (table as *const PageTable as u64) >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 1 -> Level 0
    let table1 = (ppn1 << PAGE_SHIFT) as *mut PageTable;
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        // 已存在 L0 页表
        pte1.ppn()
    } else {
        // 分配新的 L0 页表
        let table = alloc_page_table();
        let ppn = (table as *const PageTable as u64) >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 0 -> 物理页
    let table0 = (ppn0 << PAGE_SHIFT) as *mut PageTable;
    let table0_ref = &mut *table0;
    let ppn = phys_addr >> PAGE_SHIFT;
    let pte_bits = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));
}

unsafe fn map_region(root_ppn: u64, start: u64, size: u64, flags: u64) {
    let virt_start = VirtAddr::new(start);
    let phys_start = PhysAddr::new(start);
    let virt_end = VirtAddr::new(start + size);

    let mut virt = virt_start.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        // 使用恒等映射：虚拟地址 = 物理地址
        let offset = virt.bits() - virt_start.bits();
        let phys = PhysAddr::new(phys_start.bits() + offset);
        map_page(root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

pub fn init() {
    unsafe {
        // 读取当前 satp 值
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);

        // 检查 MMU 是否已经使能（快速路径）
        if satp >> 60 != 0 {
            // MMU 已经使能，直接返回
            return;
        }

        // 尝试获取初始化锁（使用 CAS 操作）
        // 只有第一个到达这里的核能成功设置 false -> true
        if !MMU_INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            // 其他核正在初始化或已经初始化，等待完成
            while !MMU_INITIALIZED.load(Ordering::Acquire) {
                // 短暂延迟
                asm!("nop", options(nomem, nostack));
            }

            // 启动核已经完成页表初始化，次核现在需要使能自己的 MMU
            // 计算根页表的物理页号（与启动核使用相同的页表）
            let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

            let addr_space = AddressSpace::new(root_ppn);
            addr_space.enable();

            return;
        }

        // 只有启动核才会执行到这里
        println!("mm: Initializing RISC-V MMU (Sv39)...");

        // 初始化根页表（清零）
        ROOT_PAGE_TABLE.zero();

        // 计算根页表的物理页号
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        // 映射内核空间（0x80200000 - 0x80A00000，8MB）
        // QEMU virt: 内核从 0x80200000 开始
        // 增加映射大小以避免代码增长导致的内存布局变化问题
        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x80200000, 0x800000, kernel_flags);

        // 映射堆空间（0x80A00000 - 0x81A00000，16MB）
        // 用于动态内存分配（Buddy System）
        // 使用**恒等映射**：虚拟地址 0x80A00000 → 物理地址 0x80A00000
        // 注意：这确保了 virt_to_phys() 能正确转换 VirtQueue 的 DMA 地址
        let heap_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        let heap_virt_start = 0x80A00000u64;
        let heap_phys_start = 0x80A00000u64;  // 恒等映射
        let heap_size = 0x1000000u64;

        let virt_start = VirtAddr::new(heap_virt_start);
        let phys_start = PhysAddr::new(heap_phys_start);
        let virt_end = VirtAddr::new(heap_virt_start + heap_size);
        let mut virt = virt_start.floor();
        let end = virt_end.ceil();

        while virt.bits() < end.bits() {
            let offset = virt.bits() - virt_start.bits();
            let phys = PhysAddr::new(phys_start.bits() + offset);
            map_page(root_ppn, virt, phys, heap_flags);
            virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
        }

        // 映射用户物理内存区域（0x84000000 - 0x88000000，64MB）
        // 用于访问用户页表和用户程序内存
        // 使用内核权限（非用户权限），因为这是内核访问
        let user_phys_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x84000000, 0x4000000, user_phys_flags);

        // 映射 UART 设备（0x10000000）
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x10000000, 0x1000, device_flags);

        // 映射 VirtIO 设备 MMIO 区域（可能的位置）
        // QEMU virt 可能在以下位置放置 VirtIO 设备：
        // 1. 0x10001000-0x10009000 (传统 MMIO)
        // 映射 VirtIO MMIO 区域
        map_region(root_ppn, 0x10001000, 0x100000, device_flags);

        // 映射 PLIC（Platform-Level Interrupt Controller，0x0c000000）
        // PLIC 布局：
        // - 0x0c000000-0x0c00ffff: PRIORITY, PENDING
        // - 0x0c010000-0x0c01ffff: reserved
        // - 0x0c020000-0x0c03ffff: Hart 0 context (ENABLE, THRESHOLD, CLAIM/COMPLETE)
        // - 0x0c030000-0x0c03ffff: Hart 1 context
        // - 0x0c040000-0x0c04ffff: Hart 2 context
        // - 0x0c050000-0x0c05ffff: Hart 3 context
        // 需要 0x200000 (CONTEXT_SIZE * 4 = 0x1000 * 4 = 0x400000) 的完整映射
        map_region(root_ppn, 0x0c000000, 0x200000, device_flags);

        // 映射 CLINT（Core Local Interruptor，0x02000000）
        map_region(root_ppn, 0x02000000, 0x10000, device_flags);

        // 映射 PCIe ECAM 空间（0x30000000-0x31ffffff，用于 PCI 配置空间访问）
        // RISC-V virt 平台: PCIe ECAM 从 0x30000000 开始
        // 每个设备 4KB，最多 256 个设备，总共 1MB
        map_region(root_ppn, 0x30000000, 0x100000, device_flags);

        // 映射 PCI MMIO 空间（0x40000000-0x50000000，用于 PCI 设备 BAR 访问）
        // RISC-V virt 平台: PCI 设备的 MMIO BAR 地址范围
        // 为 PCI 设备分配的 BAR 地址映射到此区域
        map_region(root_ppn, 0x40000000, 0x10000000, device_flags);

        println!("mm: Page table mappings created");

        // 使能 MMU
        let addr_space = AddressSpace::new(root_ppn);
        addr_space.enable();

        println!("mm: RISC-V MMU [OK]");
    }
}

pub fn enable() {
    unsafe {
        // 计算根页表的物理页号
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        let addr_space = AddressSpace::new(root_ppn);
        addr_space.enable();
    }
}

pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn2 = virt.vpn(2) as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2, PageTableEntry::from_bits((ppn << 10) | flags));
    }
}

/// 映射设备内存页到用户空间
///
/// 用于将 framebuffer 等设备内存映射到用户进程的地址空间
///
/// # 参数
/// - virt: 虚拟地址 (用户空间)
/// - phys: 物理地址 (设备内存)
/// - flags: 页表项标志 (V, R, W, X, U 等)
///
/// # 注意
/// 这是一个简化的实现，使用 2MB 大页映射
pub fn map_device_page(virt: usize, phys: usize, flags: u64) {
    // 使用 2MB 大页映射
    // 对于 framebuffer，使用 2MB 页更简单
    let vpn2 = (virt >> 30) & 0x1FF;  // VPN[2] for L2 index

    // 计算 PPN (物理页号，对于 2MB 页是 PPN[2:1])
    let ppn_2m = (phys >> 21) as u64;  // 2MB 对齐的物理页号

    unsafe {
        // 创建 1GB 大页条目（L2 leaf）
        // PPN[2:1] 需要放在正确的位置
        // PTE 格式: [PPN[2] (26 bits)] [PPN[1] (9 bits)] [PPN[0] (9 bits)] [RSW] [DGBUWRXV]
        let ppn = (phys >> 12) as u64;  // 完整的物理页号
        let entry_bits = (ppn << 10) | flags;

        ROOT_PAGE_TABLE.set(vpn2 as usize, PageTableEntry::from_bits(entry_bits));
    }

    // 刷新 TLB
    unsafe {
        core::arch::asm!("sfence.vma", options(nomem, nostack));
    }
}

pub fn get_satp() -> Satp {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        Satp(satp)
    }
}

pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    // RISC-V Sv39 地址转换
    // QEMU virt 平台：内核加载在 0x80200000，使用恒等映射（虚拟地址 = 物理地址）

    const KERNEL_VIRT_BASE: u64 = 0x80200000;
    const KERNEL_VIRT_END: u64 = 0x82000000;  // 内核空间结束（堆 + 保留空间）

    // 堆空间常量（使用恒等映射）
    const HEAP_VIRT_BASE: u64 = 0x80A00000;

    let addr = virt.0;

    // 内核空间（包括代码、数据和堆）都使用**恒等映射**
    // 虚拟地址 = 物理地址
    if addr >= KERNEL_VIRT_BASE && addr < KERNEL_VIRT_END {
        // 内核代码/数据/堆空间：使用恒等映射
        // 0x80200000 → 0x80200000（代码）
        // 0x80A00000 → 0x80A00000（堆）
        PhysAddr::new(addr)
    } else if addr >= KERNEL_VIRT_BASE {
        // 内核空间但不在上述范围（不应该发生）
        PhysAddr::new(addr)
    } else {
        // 用户虚拟地址：需要查页表转换
        PhysAddr::new(addr)
    }
}

// ==================== 用户地址空间管理 ====================

static mut USER_PHYS_ALLOCATOR: PhysAllocator = PhysAllocator::new();

struct PageTableWalker;

impl PageTableWalker {
    /// 遍历页表查找虚拟地址对应的物理页号
    /// 返回 Some(ppn) 如果找到，None 如果未映射
    unsafe fn walk(user_root_ppn: u64, virt: u64) -> Option<u64> {
        let virt_addr = VirtAddr::new(virt);

        // 提取虚拟页号
        let vpn2 = virt_addr.vpn(2) as usize;
        let vpn1 = virt_addr.vpn(1) as usize;
        let vpn0 = virt_addr.vpn(0) as usize;

        // 使用物理地址访问页表（恒等映射）
        let root_table_addr = user_root_ppn << PAGE_SHIFT;
        let root_table = root_table_addr as *const PageTable;

        let pte2 = (*root_table).get(vpn2);
        if !pte2.is_valid() {
            return None;
        }

        let ppn1 = pte2.ppn();
        let table1 = (ppn1 << PAGE_SHIFT) as *const PageTable;
        let pte1 = (*table1).get(vpn1);
        if !pte1.is_valid() {
            return None;
        }

        let ppn0 = pte1.ppn();
        let table0 = (ppn0 << PAGE_SHIFT) as *const PageTable;
        let pte0 = (*table0).get(vpn0);
        if !pte0.is_valid() {
            return None;
        }

        Some(pte0.ppn())
    }
}

struct PhysAllocator {
    /// 当前分配位置（物理地址）
    current: u64,
    /// 分配限制（最低地址）
    limit: u64,
}

impl PhysAllocator {
    const fn new() -> Self {
        Self {
            current: 0,
            limit: 0,
        }
    }

    /// 初始化分配器
    ///
    /// # 参数
    /// - `start`: 起始物理地址（从高地址向下分配）
    /// - `limit`: 最低可分配地址
    unsafe fn init(&mut self, start: u64, limit: u64) {
        self.current = start;
        self.limit = limit;
    }

    /// 分配一页物理内存
    ///
    /// 返回物理页的物理地址，如果分配失败则返回 None
    unsafe fn alloc_page(&mut self) -> Option<u64> {
        if self.current < self.limit + PAGE_SIZE {
            return None;
        }

        self.current -= PAGE_SIZE;
        Some(self.current)
    }

    /// 分配多页物理内存
    unsafe fn alloc_pages(&mut self, count: usize) -> Option<u64> {
        let total_size = count as u64 * PAGE_SIZE;

        if self.current < self.limit + total_size {
            return None;
        }

        self.current -= total_size;
        Some(self.current)
    }
}

pub fn init_user_phys_allocator(start: u64, size: u64) {
    unsafe {
        // 从内存顶部向下分配，保留底部给内核
        // QEMU virt: 通常有 128MB 内存 (0x80000000 + 128MB)
        let alloc_start = start + size;
        let alloc_limit = start + 0x4000000; // 保留 64MB 给内核

        USER_PHYS_ALLOCATOR.init(alloc_start, alloc_limit);
        println!("mm: User physical allocator: {:#x} - {:#x}", alloc_limit, alloc_start);
    }
}

pub fn create_user_address_space() -> Option<u64> {
    unsafe {
        // 分配根页表（一页）
        let root_page = USER_PHYS_ALLOCATOR.alloc_page()?;

        // 初始化页表
        let root_table = root_page as *mut PageTable;
        (*root_table).zero();

        // 复制内核映射到用户页表
        // 用户页表需要能访问内核代码（用于系统调用）
        let kernel_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        // 映射内核空间到用户页表
        // 简化：直接映射整个内核区域
        let root_ppn = root_page / PAGE_SIZE;
        copy_kernel_mappings(root_ppn, kernel_ppn);

        Some(root_ppn)
    }
}

unsafe fn copy_kernel_mappings(user_root_ppn: u64, kernel_root_ppn: u64) {
    // 使用物理地址作为虚拟地址（QEMU virt 的恒等映射）
    // 注意：这依赖于 QEMU virt 平台的物理地址布局
    let kernel_virt = kernel_root_ppn * PAGE_SIZE;
    let user_virt = user_root_ppn * PAGE_SIZE;

    let kernel_table = kernel_virt as *const PageTable;
    let user_table = user_virt as *mut PageTable;

    println!("mm: copy_kernel_mappings: kernel_ppn={:#x}, user_ppn={:#x}", kernel_root_ppn, user_root_ppn);

    // 步骤 1：复制除 VPN2[0] 外的所有内核映射
    let mut copied = 0;
    for i in 0..512 {
        let pte = (*kernel_table).get(i);
        if pte.is_valid() {
            // 跳过 VPN2[0]（用户代码和栈）
            if i == 0 {
                println!("mm:   skipping VPN2[0] (user space)");
                continue;
            }

            // 复制所有其他VPN2条目，包括VPN2[2]（内核代码）
            // 这样sret指令可以从用户页表执行
            (*user_table).set(i, pte);
            copied += 1;
        }
    }

    // 步骤 2：VPN2[2] 已经从内核页表复制，包含了内核代码/数据的映射
    // 不需要再映射 0x80200000 - 0x80a00000 区域
    // map_region 会覆盖我们刚刚复制的 VPN2[2] 条目，所以跳过这一步

    // 步骤 3：映射用户物理内存区域（0x84000000 - 0x88000000）
    // 这个区域包含页表分配器分配的页表
    // 使用恒等映射，权限 U=1, R=1, W=1
    let user_phys_flags = PageTableEntry::V | PageTableEntry::U |
                          PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x84000000, 0x4000000, user_phys_flags);

    // 步骤 3.5：映射 UART 设备（0x10000000）
    // 这样用户程序可以通过系统调用输出
    let uart_flags = PageTableEntry::V | PageTableEntry::U |
                       PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x10000000, 0x1000, uart_flags);

    println!("mm: Copied {} kernel mappings to user page table", copied);
}

pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page(user_root_ppn, user_virt, phys, flags);
}

pub unsafe fn map_user_region(
    user_root_ppn: u64,
    virt_start: u64,
    phys_start: u64,
    size: u64,
    flags: u64,
) {
    // 检查溢出
    let virt_end_checked = virt_start.checked_add(size);
    if virt_end_checked.is_none() {
        panic!("map_user_region: virt_start + size overflow: virt_start={:#x}, size={:#x}",
               virt_start, size);
    }
    let virt_end_val = virt_end_checked.unwrap();

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);
    let virt_end = VirtAddr::new(virt_end_val);

    let mut virt = virt_start_addr.floor();
    let end = virt_end.ceil();

    let mut iteration = 0;
    while virt.bits() < end.bits() {
        // offset = 当前虚拟地址 - 起始虚拟地址
        // virt >= virt_start_addr 应该总是成立，因为 virt = floor(virt_start)
        let virt_bits = virt.bits();
        let virt_start_bits = virt_start_addr.bits();
        if virt_bits < virt_start_bits {
            panic!("map_user_region: virt ({:#x}) < virt_start ({:#x}), floor() failed?",
                   virt_bits, virt_start_bits);
        }
        let offset = virt_bits - virt_start_bits;
        let phys = PhysAddr::new(phys_start_addr.bits() + offset);
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
        iteration += 1;
    }

    // 只在映射较小时打印总结（用户程序内存）
    if size < 0x10000 {
        println!("mm: Mapped user memory: {:#x}-{:#x} ({} pages)", virt_start, virt_end_val, iteration);
    }
}

pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // 计算需要的页数
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    // 分配物理页
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    // 映射到用户地址空间
    map_user_region(user_root_ppn, virt_addr, phys_addr, size, flags);

    println!("mm:   mapping complete");
    Some(phys_addr)
}

// ==================== Linux-style Single Page Table Implementation ====================
pub fn get_kernel_page_table_ppn() -> u64 {
    unsafe {
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        root_ppn
    }
}

pub unsafe fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // 计算需要的页数
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    // 分配物理页
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    // 获取内核页表PPN
    let kernel_ppn = get_kernel_page_table_ppn();

    // 添加U-bit（用户可访问）
    let user_flags = flags | PageTableEntry::U;

    // 映射到内核页表
    map_user_region(kernel_ppn, virt_addr, phys_addr, size, user_flags);

    Some(phys_addr)
}

pub unsafe fn switch_to_user_linux(entry: u64, user_stack: u64) -> ! {
    // 直接调用汇编函数切换到用户模式
    switch_to_user_linux_asm(entry, user_stack);
}

// ==================== Copy-on-Write (COW) 支持 ====================

/// Copy-on-Write 标志
///
/// 用于标记页是否需要写时复制
/// 我们使用 PageTableEntry 的保留位来存储 COW 标志
/// RISC-V Sv39 中，位 [63:54] 是保留给软件使用的
pub mod cow_flags {
    /// COW 标志 - 页被标记为写时复制
    pub const COW: u64 = 1 << 8;  // 使用位 8（在 A 和 D 之后）
}

/// 复制页表（用于 fork）
///
/// 创建新页表，复制父进程的页表项，但将可写页标记为只读 + COW
///
/// # 参数
/// - parent_root_ppn: 父进程根页表的物理页号
///
/// # 返回
/// 返回子进程根页表的物理页号
///
/// # 安全性
/// 此函数是 unsafe 的，因为它直接操作原始指针和页表
pub unsafe fn copy_page_table_cow(parent_root_ppn: u64) -> Option<u64> {
    // 分配新的根页表（L2）
    let child_root_table = alloc_page_table();
    let child_root_ppn = (child_root_table as *const PageTable as u64) >> PAGE_SHIFT;

    // 复制 L2 页表项（512 项）
    let parent_root = (parent_root_ppn << PAGE_SHIFT) as *const PageTable;
    let child_root = child_root_table as *mut PageTable;

    for vpn2 in 0..512 {
        let pte2 = (*parent_root).get(vpn2);

        if !pte2.is_valid() {
            continue;  // 跳过无效项
        }

        let ppn1 = pte2.ppn();

        // 分配新的 L1 页表
        let child_table1 = alloc_page_table();
        let child_ppn1 = (child_table1 as *const PageTable as u64) >> PAGE_SHIFT;

        let child_ppn1 = (child_table1 as *const PageTable as u64) >> PAGE_SHIFT;
        (*child_root).set(vpn2, PageTableEntry::new_table(child_ppn1));

        let parent_table1 = (ppn1 << PAGE_SHIFT) as *const PageTable;
        let child_table1_ref = &mut *child_table1;

        // 复制 L1 页表项（512 项）
        for vpn1 in 0..512 {
            let pte1 = (*parent_table1).get(vpn1);

            if !pte1.is_valid() {
                continue;  // 跳过无效项
            }

            let ppn0 = pte1.ppn();

            // 分配新的 L0 页表
            let child_table0 = alloc_page_table();
            let child_ppn0 = (child_table0 as *const PageTable as u64) >> PAGE_SHIFT;

            let child_ppn0 = (child_table0 as *const PageTable as u64) >> PAGE_SHIFT;
            (*child_table1_ref).set(vpn1, PageTableEntry::new_table(child_ppn0));

            let parent_table0 = (ppn0 << PAGE_SHIFT) as *const PageTable;
            let child_table0_ref = &mut *child_table0;

            // 复制 L0 页表项（512 项）
            for vpn0 in 0..512 {
                let pte0 = (*parent_table0).get(vpn0);

                if !pte0.is_valid() {
                    continue;  // 跳过无效项
                }

                // 复制页表项，但如果是可写页，标记为只读 + COW
                let mut new_pte = pte0;

                if pte0.is_writable() {
                    // 移除 W 标志，添加 COW 标志
                    new_pte = PageTableEntry::from_bits(
                        pte0.bits() & !PageTableEntry::W | cow_flags::COW
                    );
                }

                (*child_table0_ref).set(vpn0, new_pte);
            }
        }
    }

    Some(child_root_ppn)
}

/// 处理写时复制页错误
///
/// 当进程尝试写入 COW 页时，复制该页并更新页表
///
/// # 参数
/// - root_ppn: 进程根页表的物理页号
/// - fault_addr: 触发错误的虚拟地址
///
/// # 返回
/// 成功返回 Some(())，失败返回 None
///
/// # 安全性
/// 此函数是 unsafe 的，因为它直接操作原始指针和页表
pub unsafe fn handle_cow_fault(root_ppn: u64, fault_addr: VirtAddr) -> Option<()> {
    use crate::mm::page::alloc_frame;

    let virt_addr = fault_addr.bits();

    // 提取虚拟页号（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // 获取根页表（L2）
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = root_table_addr as *mut PageTable;

    let pte2 = (*root_table).get(vpn2);
    if !pte2.is_valid() {
        println!("mm: handle_cow_fault: L2 PTE invalid");
        return None;
    }

    let ppn1 = pte2.ppn();
    let table1 = (ppn1 << PAGE_SHIFT) as *mut PageTable;

    let pte1 = (*table1).get(vpn1);
    if !pte1.is_valid() {
        println!("mm: handle_cow_fault: L1 PTE invalid");
        return None;
    }

    let ppn0 = pte1.ppn();
    let table0 = (ppn0 << PAGE_SHIFT) as *mut PageTable;

    let old_pte = (*table0).get(vpn0);
    if !old_pte.is_valid() {
        println!("mm: handle_cow_fault: L0 PTE invalid");
        return None;
    }

    // 检查是否是 COW 页
    let old_bits = old_pte.bits();
    if old_bits & cow_flags::COW == 0 {
        println!("mm: handle_cow_fault: not a COW page");
        return None;
    }

    let old_ppn = old_pte.ppn();

    // 分配新的物理页
    let new_frame = alloc_frame()?;
    let new_ppn = new_frame.start_address().as_usize() as u64 >> PAGE_SHIFT;

    let new_virt = (new_ppn << PAGE_SHIFT) as *mut u8;
    let old_virt = (old_ppn << PAGE_SHIFT) as *const u8;

    // 复制页面内容
    for i in 0..PAGE_SIZE as usize {
        *new_virt.add(i) = *old_virt.add(i);
    }

    // 更新页表项：移除 COW 标志，添加 W 标志
    let new_pte = PageTableEntry::from_bits(
        (old_bits & !cow_flags::COW) | PageTableEntry::W
    );

    // 刷新 TLB（RISC-V 使用 sfence.vma 指令）
    asm!("sfence.vma");

    // 更新页表项
    (*table0).set(vpn0, new_pte);

    println!("mm: handle_cow_fault: copied page at {:#x}, old_ppn={:#x}, new_ppn={:#x}",
             virt_addr, old_ppn, new_ppn);

    Some(())
}

/// 检查页是否为 COW 页
///
/// # 参数
/// - root_ppn: 进程根页表的物理页号
/// - addr: 虚拟地址
///
/// # 返回
/// 如果是 COW 页返回 true，否则返回 false
pub unsafe fn is_cow_page(root_ppn: u64, addr: VirtAddr) -> bool {
    let virt_addr = addr.bits();

    // 提取虚拟页号
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // 遍历页表
    let root_table = (root_ppn << PAGE_SHIFT) as *const PageTable;
    let pte2 = (*root_table).get(vpn2);

    if !pte2.is_valid() {
        return false;
    }

    let table1 = (pte2.ppn() << PAGE_SHIFT) as *const PageTable;
    let pte1 = (*table1).get(vpn1);

    if !pte1.is_valid() {
        return false;
    }

    let table0 = (pte1.ppn() << PAGE_SHIFT) as *const PageTable;
    let pte0 = (*table0).get(vpn0);

    if !pte0.is_valid() {
        return false;
    }

    // 检查 COW 标志
    (pte0.bits() & cow_flags::COW) != 0
}

