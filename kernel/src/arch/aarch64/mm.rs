use core::arch::asm;
use crate::console::putchar;

// aarch64 页表配置
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

// 页表层级 - 使用4级页表 (48位虚拟地址)
pub const PAGE_LEVELS: usize = 4;

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

static mut BOOT_PAGE_TABLE: BootPageTableWrapper = BootPageTableWrapper {
    table: PageTable::zeroed(),
};

pub unsafe fn init() {
    const MSG: &[u8] = b"MM: MMU enablement temporarily disabled\n";
    for &b in MSG {
        putchar(b);
    }

    // MMU启用问题调查结果：
    // 1. 页表描述符格式已修复（AP、SH、AF字段）
    // 2. T0SZ值已修正（使用T0SZ=0，64位VA）
    // 3. 但仍发生指令翻译错误（ESR_EL1=0x86000004, Level 3）
    // 4. 递归异常：异常处理程序(0x200)本身无法访问

    // 可能的根本原因：
    // - 64位VA的页表索引计算复杂
    // - 异常向量表地址映射问题
    // - 需要进一步研究ARMv8 MMU规范

    // 暂时禁用MMU，先实现其他功能
    let mut sctlr: u64;
    asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
    sctlr &= !(1 << 0);  // 清除M位 - 禁用MMU
    asm!("isb", options(nomem, nostack));
    asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    const MSG2: &[u8] = b"MM: MMU disabled (investigating translation fault issue)\n";
    for &b in MSG2 {
        putchar(b);
    }
}

/// 设置恒等映射页表
/// 将物理内存 1:1 映射到相同的虚拟地址
unsafe fn setup_identity_page_table() {
    const MSG1: &[u8] = b"MM: Got page table addr...\n";
    for &b in MSG1 {
        putchar(b);
    }

    // 使用静态分配的页表
    let page_table = &raw mut BOOT_PAGE_TABLE.table;

    const MSG2: &[u8] = b"MM: Clearing page table...\n";
    for &b in MSG2 {
        putchar(b);
    }

    // 清零页表 - 逐个清零而不是使用write_bytes
    for i in 0..512 {
        (*page_table).entries[i].value = 0;
    }

    const MSG2B: &[u8] = b"MM: Page table cleared\n";
    for &b in MSG2B {
        putchar(b);
    }

    const MSG3: &[u8] = b"MM: Setting entries...\n";
    for &b in MSG3 {
        putchar(b);
    }

    // 创建1GB block描述符的辅助函数
    // ARMv8 Block Descriptor format (4KB granule, level 0):
    // [47:12] - Physical address (aligned to 4KB)
    // [10]    - AF (Access Flag)
    // [9:8]   - SH[1:0] (Shareability)
    // [7:6]   - AP[2:1] (Access Permissions)
    // [54]    - UXN (Unprivileged Execute Never)
    // [53]    - PXN (Privileged Execute Never)
    // [4:2]   - AttrIndx (Memory Attribute Index from MAIR)
    // [1]     - Block descriptor (valid=1)
    let make_block_desc = |pa: u64, ap: u64, uxn: u64, pxn: u64, attr: u64, sh: u64| -> u64 {
        (pa & 0x0000_FFFF_FFFF_F000)  // 物理地址对齐到4KB
        | 0x1                          // Valid bit + Block descriptor type
        | ((ap & 0x3) << 6)           // AP[2:1] at bits [7:6]
        | ((sh & 0x3) << 8)           // SH[1:0] at bits [9:8]
        | ((uxn & 0x1) << 54)         // UXN bit
        | ((pxn & 0x1) << 53)         // PXN bit
        | ((attr & 0x7) << 2)         // AttrIndx[2:0] at bits [4:2]
        | (1 << 10)                   // AF bit
    };

    // MAIR setup: AttrIdx 0 = Normal memory (0xFF), AttrIdx 1 = Device memory (0x00)
    // SH values: 0=Non-shareable, 1=Outer shareable, 2=Reserved, 3=Inner shareable

    // 映射 0x0000_0000 - 0x3FFF_FFFF (1GB, 设备/UART等)
    // AP=0b00: EL1 only, RW (device memory should not be executable)
    // AttrIdx=1: Device memory (nGnRnE)
    // SH=0: Non-shareable (device memory)
    (*page_table).entries[0].value = make_block_desc(0x0000_0000, 0b00, 1, 1, 1, 0);

    const MSG4: &[u8] = b"MM: Entry 0 done\n";
    for &b in MSG4 {
        putchar(b);
    }

    // 映射 0x4000_0000 - 0x7FFF_FFFF (1GB, 内核代码+数据)
    // 正确的1GB块描述符格式：
    // [47:12] = 物理地址 >> 12 (块起始地址)
    // [10] = 1 (AF)
    // [9:8] = 11 (SH = 3, Inner shareable)
    // [7:6] = 00 (AP = EL1 only, RW)
    // [1] = 1 (valid)
    //
    // 对于0x4000_0000的1GB块：
    // - 物理地址字段应该是 0x4000_0000 >> 12 = 0x40000
    // - 完整值 = (0x40000 << 12) | 0x701 = 0x4000_0000_0701
    (*page_table).entries[1].value = 0x4000_0000_0701;

    const MSG5: &[u8] = b"MM: Entry 1 done, value=0x4000_0000_0701 (PA field=0x40000)\n";
    for &b in MSG5 {
        putchar(b);
    }

    // 映射 0x8000_0000 - 0xBFFF_FFFF (1GB)
    // AttrIdx=0: Normal memory, SH=3: Inner shareable
    (*page_table).entries[2].value = make_block_desc(0x8000_0000, 0b00, 0, 0, 0, 3);

    // 映射 0xC000_0000 - 0xFFFF_FFFF (1GB)
    // AttrIdx=0: Normal memory, SH=3: Inner shareable
    (*page_table).entries[3].value = make_block_desc(0xC000_0000, 0b00, 0, 0, 0, 3);

    const MSG6: &[u8] = b"MM: All entries done\n";
    for &b in MSG6 {
        putchar(b);
    }

    // 数据同步屏障
    asm!("dsb ish", options(nomem, nostack));
}

/// 初始化并启用MMU寄存器
unsafe fn init_mmu_registers() {
    const MSG1: &[u8] = b"MM: Getting page table addr...\n";
    for &b in MSG1 {
        putchar(b);
    }

    // 获取页表物理地址
    let page_table_addr = &raw mut BOOT_PAGE_TABLE.table as u64;

    const MSG2: &[u8] = b"MM: Page table addr=0x";
    for &b in MSG2 {
        putchar(b);
    }
    let hex_chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((page_table_addr >> shift) & 0xF) as usize;
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

    const MSG4: &[u8] = b"MM: Setting TTBR0...\n";
    for &b in MSG4 {
        putchar(b);
    }

    // 设置TTBR0_EL1 - 页表基址
    // 必须指向4KB对齐的页表
    asm!("msr ttbr0_el1, {}", in(reg) page_table_addr, options(nomem, nostack));

    const MSG5: &[u8] = b"MM: Setting TCR...\n";
    for &b in MSG5 {
        putchar(b);
    }

    // 设置TCR_EL1 - 转换控制寄存器
    // T0SZ = 0 (64位虚拟地址，使用level 0 only，可以包含block描述符)
    // IRGN0 = 0 (Normal WB-WA, Inner Write-Back Write-Allocate)
    // ORGN0 = 0 (Normal WB-WA, Outer Write-Back Write-Allocate)
    // SH0 = 3 (Inner shareable)
    // TG0 = 0 (4KB granule)
    // EPD1 = 1 (禁用TTBR1_EL1)
    let tcr: u64 = (0 << 0) |     // T0SZ: 64-bit VA (level 0 only)
                   (0b00 << 8) |  // IRGN0: Normal WB-WA
                   (0b00 << 10) | // ORGN0: Normal WB-WA
                   (0b11 << 12) | // SH0: Inner shareable
                   (0b00 << 14) | // TG0: 4KB granule
                   (1 << 23);     // EPD1: 禁用TTBR1

    // Debug: 打印TCR值
    const MSG_TCR_DEBUG: &[u8] = b"MM: Computed TCR = 0x";
    for &b in MSG_TCR_DEBUG {
        putchar(b);
    }
    let hex_chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let nibble = ((tcr >> shift) & 0xF) as usize;
        putchar(hex_chars[nibble]);
    }
    const MSG_TCR_DEBUG2: &[u8] = b" (T0SZ=0, 64-bit VA)\n";
    for &b in MSG_TCR_DEBUG2 {
        putchar(b);
    }

    asm!("msr tcr_el1, {}", in(reg) tcr, options(nomem, nostack));

    const MSG6: &[u8] = b"MM: Reading SCTLR...\n";
    for &b in MSG6 {
        putchar(b);
    }

    // 设置SCTLR_EL1 - 系统控制寄存器
    // M = 1: 启用MMU
    // C = 0: 禁用数据缓存（先测试MMU，缓存稍后启用）
    // I = 0: 禁用指令缓存
    // A = 0: 禁用严格对齐检查
    const MSG7: &[u8] = b"MM: Flushing TLBs...\n";
    for &b in MSG7 {
        putchar(b);
    }

    // 刷新TLB（在启用MMU之前）
    asm!("tlbi vmalle1is", options(nomem, nostack));
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    const MSG8: &[u8] = b"MM: Setting up SCTLR...\n";
    for &b in MSG8 {
        putchar(b);
    }

    // 从头设置SCTLR，确保没有意外的位
    let mut sctlr: u64 = 0;
    sctlr |= (1 << 0);   // M: MMU使能
    // 其他位保持为0：
    // - A (bit 1) = 0: 禁用严格对齐检查
    // - C (bit 2) = 0: 禁用数据缓存
    // - I (bit 12) = 0: 禁用指令缓存

    const MSG9: &[u8] = b"MM: Enabling MMU...\n";
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
    let hex_chars = b"0123456789ABCDEF";
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
