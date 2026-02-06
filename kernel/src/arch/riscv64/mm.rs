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

use crate::println;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ==================== 常量定义 ====================

/// 页大小（4KB）
pub const PAGE_SIZE: u64 = 4096;

/// 页偏移位数（12 位，4KB 页）
pub const PAGE_SHIFT: u64 = 12;

/// 页内偏移掩码
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;

/// Sv39 虚拟地址位数（39 位）
pub const VA_BITS: u64 = 39;

/// Sv39 虚拟地址掩码
pub const VA_MASK: u64 = (1 << VA_BITS) - 1;

// ==================== 地址类型 ====================

/// 虚拟地址
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
}

/// 物理地址
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

/// RISC-V Sv39 页表项（PTE）
///
/// 格式：[63:54] RSW, [53:10] PPN, [9:8] RSW, [7:0] 标志
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

/// 页表（512 个 PTE）
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

/// satp CSR (Supervisor Address Translation and Protection)
///
/// 格式：[63:60] MODE, [59:44] ASID, [43:0] PPN
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

/// 地址空间（简单的包装，用于管理页表）
pub struct AddressSpace {
    root_ppn: u64,
}

impl AddressSpace {
    /// 创建新地址空间
    ///
    /// # 参数
    /// - `root_ppn`: 根页表的物理页号
    pub unsafe fn new(root_ppn: u64) -> Self {
        Self { root_ppn }
    }

    /// 获取根页表的物理页号
    pub fn root_ppn(&self) -> u64 {
        self.root_ppn
    }

    /// 使能 MMU（设置 satp CSR）
    ///
    /// 将 satp 设置为 Sv39 模式，并刷新 TLB
    pub unsafe fn enable(&self) {
        let satp = Satp::sv39(self.root_ppn, 0);

        println!("mm: Enabling MMU (Sv39)...");
        println!("mm: satp = {:#x} (MODE={}, PPN={:#x})",
               satp.bits(), satp.mode(), satp.bits() & 0x0FFFFFFFFFFFFFFF);

        // 设置 satp CSR
        asm!("csrw satp, {}", in(reg) satp.bits());

        // 刷新 TLB（所有地址空间）
        asm!("sfence.vma zero, zero");

        println!("mm: MMU enabled successfully");
    }

    /// 禁用 MMU（设置 satp 为 Bare 模式）
    pub unsafe fn disable() {
        let satp = Satp::new(Satp::MODE_BARE, 0, 0);

        println!("mm: Disabling MMU...");

        // 设置 satp 为 Bare 模式
        asm!("csrw satp, {}", in(reg) satp.bits());

        // 刷新 TLB
        asm!("sfence.vma zero, zero");

        println!("mm: MMU disabled");
    }

    /// 刷新 TLB
    pub unsafe fn flush_tlb() {
        asm!("sfence.vma zero, zero");
    }

    /// 刷新指定虚拟地址的 TLB
    pub unsafe fn flush_tlb_addr(vaddr: VirtAddr) {
        asm!("sfence.vma {}, zero", in(reg) vaddr.0);
    }
}

// ==================== MMU 初始化 ====================

/// 静态根页表（L2，512 GB 大页映射）
///
/// 映射整个内核地址空间 0x80000000 - 0xFFFFFFFFF
/// 使用大页映射（每个 PTE 映射 1GB）
static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// MMU 初始化标志（确保只初始化一次）
static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 分配一个页表页面
///
/// # 安全性
/// 此函数使用静态分配，仅用于内核初始化
unsafe fn alloc_page_table() -> &'static mut PageTable {
    // 使用静态分配的页表（简化实现）
    // 每个页表占用一个 4KB 页面
    static mut PAGE_TABLES: [PageTable; 64] = [PageTable::new(); 64];
    static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

    let idx = NEXT_INDEX.fetch_add(1, Ordering::AcqRel);
    if idx >= PAGE_TABLES.len() {
        panic!("mm: Out of page table pages");
    }

    &mut PAGE_TABLES[idx]
}

/// 映射一个虚拟页到物理页
///
/// # 参数
/// - `root_ppn`: 根页表的物理页号
/// - `virt`: 虚拟地址
/// - `phys`: 物理地址
/// - `flags`: 页表标志（V/R/W/X/U/G/A/D）
///
/// # 安全性
/// 调用者必须确保：
/// - 虚拟地址页对齐
/// - 物理地址页对齐
/// - 根页表有效
unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // 提取虚拟页号（VPN2, VPN1, VPN0）
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // 获取根页表（L2）
    let root_table = (root_ppn << PAGE_SHIFT) as *mut PageTable;
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
    table0_ref.set(vpn0, PageTableEntry::from_bits((ppn << 10) | flags));
}

/// 映射一个内存区域（恒等映射）
///
/// # 参数
/// - `root_ppn`: 根页表的物理页号
/// - `start`: 起始虚拟地址（也是物理地址）
/// - `size`: 区域大小
/// - `flags`: 页表标志
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

/// 初始化 MMU
///
/// 1. 创建根页表
/// 2. 映射内核代码和数据段
/// 3. 映射设备内存
/// 4. 使能 MMU
///
/// **线程安全**：使用 CAS 操作确保只有一个核执行初始化
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
        println!("mm: Current satp = {:#x} (MODE={})", satp, satp >> 60);

        // 初始化根页表（清零）
        ROOT_PAGE_TABLE.zero();

        // 计算根页表的物理页号
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        println!("mm: Root page table at PPN = {:#x}", root_ppn);

        // 映射内核空间（0x80200000 - 0x80400000，2MB）
        // QEMU virt: 内核从 0x80200000 开始
        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x80200000, 0x200000, kernel_flags);

        // 映射 UART 设备（0x10000000）
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x10000000, 0x1000, device_flags);

        // 映射 PLIC（Platform-Level Interrupt Controller，0x0c000000）
        map_region(root_ppn, 0x0c000000, 0x400000, device_flags);

        // 映射 CLINT（Core Local Interruptor，0x02000000）
        map_region(root_ppn, 0x02000000, 0x10000, device_flags);

        println!("mm: Page table mappings created");

        // 使能 MMU
        let addr_space = AddressSpace::new(root_ppn);
        addr_space.enable();

        println!("mm: RISC-V MMU [OK]");
    }
}

/// 使能 MMU（在页表设置完成后调用）
pub fn enable() {
    unsafe {
        // 计算根页表的物理页号
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        let addr_space = AddressSpace::new(root_ppn);
        addr_space.enable();
    }
}

/// 简单的恒等映射（用于调试）
///
/// 将虚拟地址直接映射到相同的物理地址
/// 适用于 QEMU virt 平台
pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn2 = virt.vpn(2) as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2, PageTableEntry::from_bits((ppn << 10) | flags));
    }
}

/// 获取当前 satp 值
pub fn get_satp() -> Satp {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        Satp(satp)
    }
}

/// 将虚拟地址转换为物理地址
///
/// 注意：MMU 未启用时使用简单的物理地址转换
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    // QEMU virt 平台：内核加载在 0x80200000
    // 使用简单的地址转换
    PhysAddr::new(virt.0)
}
