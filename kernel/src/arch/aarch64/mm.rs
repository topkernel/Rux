use core::arch::asm;
use crate::console::putchar;

// aarch64 页表配置
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

// 页表层级 - 使用两级页表简化实现
pub const PAGE_LEVELS: usize = 2;

// 页表项类型
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PageTableEntry {
    pub value: u64,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        Self { value: 0 }
    }

    pub fn is_valid(&self) -> bool {
        self.value & 1 != 0
    }

    pub fn set_valid(&mut self, valid: bool) {
        if valid {
            self.value |= 1;
        } else {
            self.value &= !1;
        }
    }

    pub fn addr(&self) -> usize {
        ((self.value >> 12) & ((1u64 << 48) - 1)) as usize
    }

    pub fn set_addr(&mut self, addr: usize) {
        self.value = (self.value & ((1 << 12) - 1)) | ((addr as u64) << 12);
    }

    // 页属性标志
    pub fn set_table(&mut self, is_table: bool) {
        if is_table {
            self.value |= 1 << 1;
        } else {
            self.value &= !(1 << 1);
        }
    }

    pub fn set_page(&mut self, is_page: bool) {
        if is_page {
            self.value |= 1 << 1;
        } else {
            self.value &= !(1 << 1);
        }
    }

    // 访问权限
    pub fn set_ap(&mut self, ap: u64) {
        self.value = (self.value & !(0b11 << 6)) | ((ap & 0b11) << 6);
    }

    // 可执行标志
    pub fn set_uxn(&mut self, uxn: bool) {
        if uxn {
            self.value |= 1 << 54;
        } else {
            self.value &= !(1 << 54);
        }
    }

    pub fn set_pxn(&mut self, pxn: bool) {
        if pxn {
            self.value |= 1 << 53;
        } else {
            self.value &= !(1 << 53);
        }
    }

    // 访问标志
    pub fn set_af(&mut self, af: bool) {
        if af {
            self.value |= 1 << 10;
        } else {
            self.value &= !(1 << 10);
        }
    }
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn zeroed() -> Self {
        Self {
            entries: [PageTableEntry::new(); 512],
        }
    }
}

// 根页表 - 使用静态存储，在BSS段中分配
// 对齐到4KB边界
#[repr(C, align(4096))]
struct BootPageTableWrapper {
    table: PageTable,
}

// 两级页表：Level 1 (512 条目，每个 8 字节) + Level 2 (512 条目，每个 8 字节)
// Level 1 表：1GB 粒度，或指向 Level 2 表
// Level 2 表：2MB 粒度，或指向 Level 3 表
static mut LEVEL1_PAGE_TABLE: BootPageTableWrapper = BootPageTableWrapper {
    table: PageTable::zeroed(),
};
static mut LEVEL2_PAGE_TABLE: BootPageTableWrapper = BootPageTableWrapper {
    table: PageTable::zeroed(),
};

pub unsafe fn init() {
    const MSG: &[u8] = b"mm: Initializing Memory Management Unit...\n";
    for &b in MSG {
        putchar(b);
    }

    // 设置两级页表
    setup_two_level_page_tables();

    // 初始化并启用 MMU
    init_mmu_registers();

    const MSG_DONE: &[u8] = b"mm: TLB invalidated\n";
    for &b in MSG_DONE {
        putchar(b);
    }

    const MSG_OK: &[u8] = b"mm: MMU enabled [OK]\n";
    for &b in MSG_OK {
        putchar(b);
    }

    const MSG_VA: &[u8] = b"mm: Virtual address: 39-bit, Page size: 4KB\n";
    for &b in MSG_VA {
        putchar(b);
    }
}

/// 设置两级页表（48位 VA, T0SZ=16）
///
/// 使用 level 1 块描述符直接映射 1GB 区域
/// - VA[47:39] 索引 level 1 表 (9 位，512 个条目)
/// - 每个 level 1 条目：1GB 块或指向 level 2 表
///
/// 设置页表（使用 level 2，2MB 块）
///
/// 改用 T0SZ=25 (39位VA)，从 level 2 开始，使用 2MB 块
/// - VA[38:30] 索引 level 2 表 (9 位，512 个条目)
/// - 每个 level 2 条目：2MB 块
///
/// 对于 0x4000_678C：
/// - level 2 索引 = 0x4000_678C >> 30 = 1
///
/// 映射策略：
/// - 条目 0: 0x0000_0000 - 0x001F_FFFF (UART 等)
/// - 条目 1: 0x4000_0000 - 0x401F_FFFF (内核)
/// - 条目 2: 0x0800_0000 - 0x081F_FFFF (GICD/GICR)
unsafe fn setup_two_level_page_tables() {
    const MSG1: &[u8] = b"MM: Setting up L2 page tables (2MB blocks)...\n";
    for &b in MSG1 {
        putchar(b);
    }

    // 使用 level 2 表
    let l2_table = &raw mut LEVEL2_PAGE_TABLE.table;

    const MSG2: &[u8] = b"MM: Clearing L2 table...\n";
    for &b in MSG2 {
        putchar(b);
    }

    // 清零 level 2 表
    for i in 0..512 {
        (*l2_table).entries[i].value = 0;
    }

    const MSG3: &[u8] = b"MM: L2 table cleared\n";
    for &b in MSG3 {
        putchar(b);
    }

    // Level 2 块描述符格式 (2MB block):
    // [47:21] 物理地址 >> 21
    // [10] AF = 1
    // [9:8] SH = 11 (Inner shareable)
    // [7:6] AP = 00 (EL1 RW)
    // [5:2] AttrIndx = 0000 (Normal) or 0001 (Device)
    // [1:0] = 01 (Block descriptor)

    // 条目 0：映射 0x0000_0000 - 0x001F_FFFF (2MB，设备区域)
    let l2_device_desc = ((0u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |
                         (3 << 8) |
                         (0 << 6) |
                         (1 << 2) |  // Device memory
                         0b01;
    (*l2_table).entries[0].value = l2_device_desc;

    const MSG4: &[u8] = b"MM: L2 entry 0 set (2MB device at 0x0000_0000)\n";
    for &b in MSG4 {
        putchar(b);
    }

    // 条目 1：映射 0x4000_0000 - 0x401F_FFFF (2MB，内核区域)
    let l2_normal_desc = ((0x4000_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |
                         (3 << 8) |
                         (0 << 6) |
                         (0 << 2) |  // Normal memory
                         0b01;
    (*l2_table).entries[1].value = l2_normal_desc;

    const MSG5: &[u8] = b"MM: L2 entry 1 set (2MB normal at 0x4000_0000)\n";
    for &b in MSG5 {
        putchar(b);
    }

    // 条目 2：映射 0x0800_0000 - 0x081F_FFFF (2MB，GIC 中断控制器)
    // GICD (Distributor) 在 0x0800_0000
    // GICR (Redistributor) 在 0x0808_0000
    let l2_gic_desc = ((0x0800_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                      (1 << 10) |
                      (3 << 8) |
                      (0 << 6) |
                      (1 << 2) |  // Device memory
                      0b01;
    (*l2_table).entries[2].value = l2_gic_desc;

    const MSG5B: &[u8] = b"MM: L2 entry 2 set (2MB device at 0x0800_0000 for GIC)\n";
    for &b in MSG5B {
        putchar(b);
    }

    // 验证条目 2 的值
    const MSG5C: &[u8] = b"MM: L2 entry 2 value = 0x";
    for &b in MSG5C {
        putchar(b);
    }
    let entry2_val = (*l2_table).entries[2].value;
    let hex_chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((entry2_val >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG5D: &[u8] = b"\n";
    for &b in MSG5D {
        putchar(b);
    }

    // 数据同步屏障
    asm!("dsb ish", options(nomem, nostack));
    const MSG6: &[u8] = b"MM: Page tables setup complete (3 L2 entries)\n";
    for &b in MSG6 {
        putchar(b);
    }
}

/// 初始化并启用MMU寄存器（使用两级页表）
///
/// 使用 T0SZ=16 (48位VA)，从 level 1 开始：
/// - VA[47:39] 索引 level 1 表 (9位，512个条目)
/// - VA[38:30] 索引 level 2 表 (9位，512个条目)
/// - 每个 level 2 条目：2MB 块
unsafe fn init_mmu_registers() {
    const MSG1: &[u8] = b"MM: Getting level 1 page table addr...\n";
    for &b in MSG1 {
        putchar(b);
    }

    // 获取 level 2 页表物理地址 (T0SZ=25 从 level 2 开始)
    let l2_table_addr = &raw mut LEVEL2_PAGE_TABLE.table as u64;

    const MSG2: &[u8] = b"MM: L2 page table addr=0x";
    for &b in MSG2 {
        putchar(b);
    }
    let hex_chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((l2_table_addr >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG2B: &[u8] = b"\n";
    for &b in MSG2B {
        putchar(b);
    }

    const MSG3: &[u8] = b"MM: Setting MAIR...\n";
    for &b in MSG3 {
        putchar(b);
    }

    // 设置MAIR_EL1 - 内存属性寄存器
    // AttrIdx 0 (MAIR[7:0]): Normal, WB-RWA-WB-RWA (0xFF)
    // AttrIdx 1 (MAIR[15:8]): Device, Dev_nGnRnE (0x00)
    let mair: u64 = (0x00 << 8) |  // AttrIdx 1: Device nGnRnE
                    (0xFF << 0);   // AttrIdx 0: Normal WB-RWA
    asm!("msr mair_el1, {}", in(reg) mair, options(nomem, nostack));

    const MSG4: &[u8] = b"MM: Setting TTBR0 to L2 table...\n";
    for &b in MSG4 {
        putchar(b);
    }

    // 设置TTBR0_EL1 - 指向 level 2 页表
    // 必须指向4KB对齐的页表
    asm!("msr ttbr0_el1, {}", in(reg) l2_table_addr, options(nomem, nostack));

    const MSG5: &[u8] = b"MM: Setting TCR (T0SZ=25, 39-bit VA, L2 start)...\n";
    for &b in MSG5 {
        putchar(b);
    }

    // 设置TCR_EL1 - 转换控制寄存器
    // T0SZ = 25 (39位虚拟地址，从 level 2 开始)
    // - VA[38:30] 索引 level 2 表 (9位，512个条目)
    // - 每个 level 2 条目：2MB 块
    //
    // 对于 0x4000_678C：level 2 索引 = 0x4000_678C >> 30 = 1
    //
    // T0SZ = 25, TG0 = 0 (4KB), 起始级别 = 2
    // IRGN0 = 1, ORGN0 = 1, SH0 = 3, EPD1 = 1
    let tcr: u64 = (25 << 0) |     // T0SZ: 39-bit VA (level 2-3)
                   (0b01 << 8) |   // IRGN0: Normal WB-WA Inner
                   (0b01 << 10) |  // ORGN0: Normal WB-WA Outer
                   (0b11 << 12) |  // SH0: Inner shareable
                   (0b00 << 14) |  // TG0: 4KB granule
                   (1 << 23);      // EPD1: 禁用TTBR1

    // Debug: 打印TCR值
    const MSG_TCR_DEBUG: &[u8] = b"MM: Computed TCR = 0x";
    for &b in MSG_TCR_DEBUG {
        putchar(b);
    }
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((tcr >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG_TCR_DEBUG2: &[u8] = b" (T0SZ=25, 39-bit VA, level 2 start)\n";
    for &b in MSG_TCR_DEBUG2 {
        putchar(b);
    }

    asm!("msr tcr_el1, {}", in(reg) tcr, options(nomem, nostack));

    const MSG7: &[u8] = b"MM: Flushing caches and TLBs...\n";
    for &b in MSG7 {
        putchar(b);
    }

    // 刷新指令缓存（在启用MMU之前）
    asm!("ic iallu", options(nomem, nostack));
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    // 刷新TLB（在启用MMU之前）
    asm!("tlbi vmalle1is", options(nomem, nostack));
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    const MSG8: &[u8] = b"MM: Setting up SCTLR...\n";
    for &b in MSG8 {
        putchar(b);
    }

    // 关键检查：在启用 MMU 之前，验证 PC 和 VBAR 是否在已映射区域
    let mut current_pc: u64;
    asm!("adr {}, #0", out(reg) current_pc, options(nomem, nostack));

    let mut vbar: u64;
    asm!("mrs {}, vbar_el1", out(reg) vbar, options(nomem, nostack));

    // 检查 PC 是否在 0x4000_0000 - 0x4FFF_FFFF 范围内
    const MSG_PC_CHECK: &[u8] = b"MM: Current PC = 0x";
    for &b in MSG_PC_CHECK {
        putchar(b);
    }
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((current_pc >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG_PC_OK: &[u8] = b"\n";
    for &b in MSG_PC_OK {
        putchar(b);
    }

    // 检查 PC 的 level 2 索引（T0SZ=25，从 level 2 开始）
    // level 2 索引 = VA[38:30]
    let pc_l2_index = (current_pc >> 30) & 0x1FF;
    const MSG_L2_IDX: &[u8] = b"MM: PC L2 index = ";
    for &b in MSG_L2_IDX {
        putchar(b);
    }
    let mut idx = pc_l2_index;
    if idx == 0 {
        putchar(b'0');
    } else {
        let mut buf = [0u8; 20];
        let mut pos = 0;
        while idx > 0 {
            buf[pos] = b'0' + ((idx % 10) as u8);
            idx /= 10;
            pos += 1;
        }
        while pos > 0 {
            pos -= 1;
            putchar(buf[pos]);
        }
    }
    const MSG_L2_NEWLINE: &[u8] = b" (should be 1 for 0x4000_0000)\n";
    for &b in MSG_L2_NEWLINE {
        putchar(b);
    }

    // 从头设置SCTLR，确保没有意外的位
    let mut sctlr: u64 = 0;
    sctlr |= (1 << 0);   // M: MMU使能
    // 缓存禁用，避免一致性问题
    // sctlr |= (1 << 2);   // C: 数据缓存使能 (禁用)
    // sctlr |= (1 << 12);  // I: 指令缓存使能 (禁用)
    // sctlr |= (1 << 3);   // SA: 栈对齐检查 (禁用)
    // 其他位保持为0：
    // - A (bit 1) = 0: 禁用严格对齐检查

    const MSG9: &[u8] = b"MM: Enabling MMU only (caches disabled)...\n";
    for &b in MSG9 {
        putchar(b);
    }

    asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nomem, nostack));

    const MSG10: &[u8] = b"MM: ISB after MMU enable...\n";
    for &b in MSG10 {
        putchar(b);
    }

    // 确保MMU使能生效
    asm!("isb", options(nomem, nostack));

    const MSG11: &[u8] = b"MM: MMU setup complete!\n";
    for &b in MSG11 {
        putchar(b);
    }

    // 验证MMU确实启用了
    let mut sctlr_check: u64;
    asm!("mrs {}, sctlr_el1", out(reg) sctlr_check, options(nomem, nostack));
    const MSG12: &[u8] = b"MM: SCTLR after enable = 0x";
    for &b in MSG12 {
        putchar(b);
    }
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((sctlr_check >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG13: &[u8] = b"\n";
    for &b in MSG13 {
        putchar(b);
    }

    // 读取VBAR_EL1的值
    let mut vbar: u64;
    asm!("mrs {}, vbar_el1", out(reg) vbar, options(nomem, nostack));
    const MSG14: &[u8] = b"MM: VBAR_EL1 = 0x";
    for &b in MSG14 {
        putchar(b);
    }
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((vbar >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG15: &[u8] = b"\n";
    for &b in MSG15 {
        putchar(b);
    }

    // 尝试读取当前PC
    let mut current_pc: u64;
    asm!("adr {}, #0", out(reg) current_pc, options(nomem, nostack));
    const MSG16: &[u8] = b"MM: Current PC = 0x";
    for &b in MSG16 {
        putchar(b);
    }
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((current_pc >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG17: &[u8] = b"\n";
    for &b in MSG17 {
        putchar(b);
    }
}

/// 刷新TLB
#[inline]
pub fn flush_tlb() {
    unsafe {
        asm!("tlbi vmalle1is", options(nomem, nostack));
        asm!("dsb ish", options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

/// 刷新特定虚拟地址的TLB
#[inline]
pub fn flush_tlb_va(vaddr: usize) {
    unsafe {
        let val = (vaddr as u64) >> 12;
        asm!("tlbi vaae1is, {}", in(reg) val, options(nomem, nostack));
        asm!("dsb ish", options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

/// 数据同步屏障
#[inline]
pub fn dsb() {
    unsafe {
        asm!("dsb sy", options(nomem, nostack));
    }
}

/// 指令同步屏障
#[inline]
pub fn isb() {
    unsafe {
        asm!("isb", options(nomem, nostack));
    }
}

/// 数据内存屏障
#[inline]
pub fn dmb() {
    unsafe {
        asm!("dmb sy", options(nomem, nostack));
    }
}
