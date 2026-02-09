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
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// 外部汇编函数（在 usermode_asm.S 中定义）
extern "C" {
    fn switch_to_user_linux_asm(entry: u64, user_stack: u64) -> !;
}

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
///
/// 放置在 .pagetables 段，避免因代码增长导致位置变化
#[link_section = ".pagetables"]
static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

/// MMU 初始化标志（确保只初始化一次）
static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 分配一个页表页面
///
/// 用户模式 trap 处理栈（每个 CPU 16KB）
/// 用于处理来自用户模式的系统调用和异常
#[link_section = ".bss"]
static mut TRAP_STACKS: [[u8; 16384]; 4] = [[0; 16384]; 4];  // 4 CPUs

/// 获取当前 CPU 的 trap 栈顶
///
/// # 安全性
/// 调用者必须确保 CPU ID 有效
pub unsafe fn get_trap_stack() -> u64 {
    let cpu_id = crate::arch::riscv64::smp::cpu_id() as usize;
    if cpu_id >= 4 {
        panic!("mm: Invalid CPU ID {}", cpu_id);
    }
    let stack_base = &mut TRAP_STACKS[cpu_id] as *mut [u8; 16384] as *mut u8;
    stack_base.add(16384) as u64  // 栈顶
}

/// # 安全性
/// 此函数使用静态分配，仅用于内核初始化
unsafe fn alloc_page_table() -> &'static mut PageTable {
    // 使用静态分配的页表（简化实现）
    // 每个页表占用一个 4KB 页面
    // 放置在 .pagetables 段，避免因代码增长导致位置变化
    #[link_section = ".pagetables"]
    static mut PAGE_TABLES: [PageTable; 256] = [PageTable::new(); 256];  // 增加到 256 个
    static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);

    let idx = NEXT_INDEX.fetch_add(1, Ordering::AcqRel);
    if idx >= PAGE_TABLES.len() {
        panic!("mm: Out of page table pages (allocated {})", idx);
    }

    // println!("mm: alloc_page_table: allocated index {}", idx);
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

    // 调试：只在 entry 地址时打印
    if virt_addr == 0x10000 {
        println!("mm: map_page: virt={:#x}, phys={:#x}, ppn={:#x}, pte_bits={:#x}",
                 virt_addr, phys_addr, ppn, pte_bits);
    }

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));
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

        // 映射内核空间（0x80200000 - 0x80A00000，8MB）
        // QEMU virt: 内核从 0x80200000 开始
        // 增加映射大小以避免代码增长导致的内存布局变化问题
        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x80200000, 0x800000, kernel_flags);

        // 映射堆空间（0x80A00000 - 0x81A00000，16MB）
        // 用于动态内存分配（Buddy System）
        let heap_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x80A00000, 0x1000000, heap_flags);

        // 映射用户物理内存区域（0x84000000 - 0x88000000，64MB）
        // 用于访问用户页表和用户程序内存
        // 使用内核权限（非用户权限），因为这是内核访问
        let user_phys_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, 0x84000000, 0x4000000, user_phys_flags);

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

        // 使用内联汇编测试代码执行（避免依赖栈）
        use crate::console::putchar;
        const MSG1: &[u8] = b"mm: After MMU enable - test 1\n";
        for &b in MSG1 { putchar(b); }

        println!("mm: RISC-V MMU [OK]");

        const MSG2: &[u8] = b"mm: After MMU OK print - test 2\n";
        for &b in MSG2 { putchar(b); }
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

// ==================== 用户地址空间管理 ====================

/// 简单的物理页分配器（bump allocator）
///
/// 用于用户程序的物理页分配
/// 从高地址向下分配
static mut USER_PHYS_ALLOCATOR: PhysAllocator = PhysAllocator::new();

/// 页表遍历辅助函数
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

/// 物理页分配器
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

/// 初始化用户物理页分配器
///
/// # 参数
/// - `start`: 起始物理地址（如 0x80000000 用于 128MB 内存）
/// - `size`: 可用内存大小
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

/// 创建用户地址空间
///
/// 分配新的根页表，用于用户进程
///
/// # 返回
/// 返回新地址空间的根页表 PPN
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

/// 复制内核映射到用户页表
///
/// 确保用户进程可以通过系统调用进入内核
///
/// # 安全性
/// 调用者必须确保物理地址已映射或使用恒等映射
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
            let is_user = pte.bits() & (1 << 4) != 0;
            println!("mm:   copied VPN2[{}] = {:#x} (U={})", i, pte.bits(), is_user);
        }
    }

    // 步骤 2：VPN2[2] 已经从内核页表复制，包含了内核代码/数据的映射
    // 不需要再映射 0x80200000 - 0x80a00000 区域
    // map_region 会覆盖我们刚刚复制的 VPN2[2] 条目，所以跳过这一步

    // 步骤 3：映射用户物理内存区域（0x84000000 - 0x88000000）
    // 这个区域包含页表分配器分配的页表
    // 使用恒等映射，权限 U=1, R=1, W=1
    println!("mm: Mapping user physical memory region (0x84000000 - 0x88000000)");

    let user_phys_flags = PageTableEntry::V | PageTableEntry::U |
                          PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x84000000, 0x4000000, user_phys_flags);

    // 步骤 3.5：映射 UART 设备（0x10000000）
    // 这样用户程序可以通过系统调用输出
    println!("mm: Mapping UART device (0x10000000) to user page table");

    let uart_flags = PageTableEntry::V | PageTableEntry::U |
                       PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::A | PageTableEntry::D;
    map_region(user_root_ppn, 0x10000000, 0x1000, uart_flags);

    copied += 1;
    println!("mm: copy_kernel_mappings: copied {} mappings from {:#x} to {:#x}",
            copied, kernel_root_ppn, user_root_ppn);
}

/// 映射用户页（非恒等映射）
///
/// # 参数
/// - `user_root_ppn`: 用户页表的根 PPN
/// - `user_virt`: 用户虚拟地址
/// - `phys`: 物理地址
/// - `flags`: 页表标志
pub unsafe fn map_user_page(user_root_ppn: u64, user_virt: VirtAddr, phys: PhysAddr, flags: u64) {
    map_page(user_root_ppn, user_virt, phys, flags);
}

/// 映射用户内存区域
///
/// # 参数
/// - `user_root_ppn`: 用户页表的根 PPN
/// - `virt_start`: 起始虚拟地址
/// - `phys_start`: 起始物理地址
/// - `size`: 区域大小
/// - `flags`: 页表标志
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

    println!("mm: map_user_region: user_root_ppn={:#x}, virt={:#x}-{:#x}, size={:#x}",
            user_root_ppn, virt_start, virt_end_val, size);

    let virt_start_addr = VirtAddr::new(virt_start);
    let phys_start_addr = PhysAddr::new(phys_start);
    let virt_end = VirtAddr::new(virt_end_val);

    let mut virt = virt_start_addr.floor();
    let end = virt_end.ceil();

    // 只在映射较小时打印详细迭代信息
    let verbose = size < 0x10000; // 小于 64KB 时打印详细信息

    let mut iteration = 0;
    while virt.bits() < end.bits() {
        if verbose {
            println!("mm:   iteration {}: virt={:#x}", iteration, virt.bits());
        }
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
        if verbose {
            println!("mm:     offset={:#x}, phys={:#x}", offset, phys.bits());
        }
        // 对于用户栈（VPN2=0 in this case），额外打印
        if !verbose && ((virt.bits() >> 30) & 0x1FF) == 0 {
            println!("mm:   iteration {}: virt={:#x}, phys={:#x}",
                    iteration, virt.bits(), phys.bits());
        }
        map_page(user_root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
        iteration += 1;
    }
}

/// 分配并映射用户内存
///
/// # 参数
/// - `user_root_ppn`: 用户页表的根 PPN
/// - `virt_addr`: 虚拟地址
/// - `size`: 大小（字节）
/// - `flags`: 页表标志
///
/// # 返回
/// 返回分配的物理地址（页对齐）
pub unsafe fn alloc_and_map_user_memory(
    user_root_ppn: u64,
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // 计算需要的页数
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    println!("mm: alloc_and_map_user_memory: virt={:#x}, size={}, pages={}",
            virt_addr, size, page_count);

    // 分配物理页
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    println!("mm:   allocated phys={:#x}", phys_addr);

    // 映射到用户地址空间
    map_user_region(user_root_ppn, virt_addr, phys_addr, size, flags);

    println!("mm:   mapping complete");
    Some(phys_addr)
}

// ==================== Linux-style Single Page Table Implementation ====================
/// 获取内核页表的物理页号
///
/// Linux使用单一页表，内核和用户程序共享同一个页表
/// 通过U-bit控制页面访问权限
pub fn get_kernel_page_table_ppn() -> u64 {
    unsafe {
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        root_ppn
    }
}

/// 分配并映射用户内存到内核页表（Linux方式）
///
/// Linux使用单一页表，用户程序直接映射到内核页表
/// 通过U-bit控制页面访问权限
///
/// # 参数
/// - `virt_addr`: 虚拟地址（用户空间）
/// - `size`: 大小（字节）
/// - `flags`: 页表标志（会自动添加U-bit）
///
/// # 返回
/// 返回分配的物理地址（页对齐）
pub unsafe fn alloc_and_map_to_kernel_table(
    virt_addr: u64,
    size: u64,
    flags: u64,
) -> Option<u64> {
    // 计算需要的页数
    let page_count = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;

    println!("mm: alloc_and_map_to_kernel_table: virt={:#x}, size={}, pages={}",
            virt_addr, size, page_count);

    // 分配物理页
    let phys_addr = USER_PHYS_ALLOCATOR.alloc_pages(page_count)?;

    println!("mm:   allocated phys={:#x}", phys_addr);

    // 获取内核页表PPN
    let kernel_ppn = get_kernel_page_table_ppn();

    // 添加U-bit（用户可访问）
    let user_flags = flags | PageTableEntry::U;

    // 映射到内核页表
    map_user_region(kernel_ppn, virt_addr, phys_addr, size, user_flags);

    println!("mm:   mapping complete");
    Some(phys_addr)
}

/// Linux风格的用户模式切换
///
/// 参考Linux的ret_from_exception()实现
/// - 使用单一页表（内核页表）
/// - 不切换satp
/// - 通过sret直接切换到用户模式
///
/// # 参数
/// - `entry`: 用户程序入口点（虚拟地址）
/// - `user_stack`: 用户栈顶（虚拟地址）
pub unsafe fn switch_to_user_linux(entry: u64, user_stack: u64) -> ! {
    // 直接调用汇编函数切换到用户模式
    switch_to_user_linux_asm(entry, user_stack);
}

