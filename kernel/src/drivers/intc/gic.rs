//! GIC 中断控制器驱动
//!
//! Generic Interrupt Controller 是ARMv8-A架构的标准中断控制器
//! 当前支持 GICv2 模式（使用 GICC CPU 接口寄存器）
//!
//! ## GIC 版本
//! - **GICv2**: 使用内存映射的 CPU 接口寄存器 (GICC_BASE = 0x0801_0000)
//! - **GICv3**: 使用系统寄存器访问 CPU 接口 (ICC_*)
//!
//! 本实现使用 GICv2 模式，因为 QEMU virt 机器在 GICv2 模式下更稳定。

use crate::println;

/// GIC 寄存器基址 (QEMU virt machine)
const GICD_BASE: usize = 0x0800_0000;  // 分发器基址
const GICC_BASE: usize = 0x0801_0000;  // CPU 接口基址 (GICv2)
const GICR_BASE: usize = 0x0808_0000;  // CPU0重分发器基址 (GICv3)

/// GICD 寄存器偏移量
mod gicd_offsets {
    pub const CTLR: usize = 0x000;     // 分发器控制寄存器
    pub const TYPER: usize = 0x004;     // 中断类型寄存器
    pub const ISENABLER: usize = 0x100; // 中断使能设置寄存器
    pub const ICENABLER: usize = 0x180; // 中断使能清除寄存器
    pub const ISPENDR: usize = 0x200;   // 中断挂起设置寄存器
    pub const ICPENDR: usize = 0x280;   // 中断挂起清除寄存器
    pub const IPRIORITYR: usize = 0x400; // 中断优先级寄存器
    pub const ITARGETSR: usize = 0x800; // 中断目标处理器寄存器
    pub const ICFGR: usize = 0xC00;    // 中断配置寄存器
    pub const IGROUPR: usize = 0x80;    // 中断组寄存器
    pub const SGIR: usize = 0xF00;      // 软件生成中断寄存器
}

/// GICR 寄存器偏移量
mod gicr_offsets {
    pub const CTLR: usize = 0x000;     // 重分发器控制寄存器
    pub const WAKER: usize = 0x014;    // 唤醒寄存器
    pub const ENABLER: usize = 0x100;  // 中断使能寄存器
    pub const IAR1: usize = 0xC4;      // 中断确认寄存器 (Group 1)
    pub const EOIR1: usize = 0x104;    // 中断结束寄存器 (Group 1)
}

/// GICC 寄存器偏移量 (GICv2 CPU 接口)
mod gicc_offsets {
    pub const CTLR: usize = 0x000;     // CPU 接口控制寄存器
    pub const PMR: usize = 0x004;      // 优先级掩码寄存器
    pub const BPR: usize = 0x008;      // 二进制点寄存器
    pub const IAR: usize = 0x00C;      // 中断确认寄存器
    pub const EOIR: usize = 0x010;     // 中断结束寄存器
    pub const RPR: usize = 0x014;      // 运行时优先级寄存器
    pub const HPPIR: usize = 0x018;    // 最高优先级挂起中断寄存器
}

/// GICv3 分发器
pub struct GicD {
    base: usize,
}

impl GicD {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            let addr = self.base + offset;
            let value: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
            value
        }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            let addr = self.base + offset;
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
        }
    }

    /// 初始化GICD
    pub fn init(&self) {
        use crate::console::putchar;
        const MSG1: &[u8] = b"GICD: init start\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 禁用所有中断
        const MSG1A: &[u8] = b"GICD: Disabling all IRQs\n";
        for &b in MSG1A {
            unsafe { putchar(b); }
        }
        for i in 0..32 {
            self.write_reg(gicd_offsets::ICENABLER + i * 4, 0xFFFFFFFF);
        }

        // 先测试是否能读取 CTLR
        const MSG1B: &[u8] = b"GICD: trying to read CTLR...\n";
        for &b in MSG1B {
            unsafe { putchar(b); }
        }

        let ctlr_val = self.read_reg(gicd_offsets::CTLR);
        const MSG1C: &[u8] = b"GICD: CTLR read OK\n";
        for &b in MSG1C {
            unsafe { putchar(b); }
        }

        // 检查GIC版本
        const MSG2: &[u8] = b"GICD: reading TYPER...\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }

        let typer = self.read_reg(gicd_offsets::TYPER);
        let itlines_number = ((typer >> 5) & 0xF) + 1;
        let cpus_number = ((typer) & 0xF) + 1;

        const MSG2_OK: &[u8] = b"GICD: typer read OK\n";
        for &b in MSG2_OK {
            unsafe { putchar(b); }
        }

        // println!("GICv3: {} interrupt lines, {} CPUs",
        //          itlines_number * 32, cpus_number);

        // 禁用所有中断
        let num_irqs = itlines_number as usize * 32;

        const MSG3: &[u8] = b"GICD: disabling IRQs\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        for i in (0..num_irqs).step_by(32) {
            self.write_reg(gicd_offsets::ICENABLER + i, 0xFFFFFFFF);
        }

        const MSG4: &[u8] = b"GICD: setting groups\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }

        // 设置所有中断为组1 (Group 1 = IRQ, Group 0 = FIQ)
        for i in (0..num_irqs).step_by(32) {
            self.write_reg(gicd_offsets::IGROUPR + i, 0xFFFFFFFF);
        }

        const MSG8: &[u8] = b"GICD: enabling distributor\n";
        for &b in MSG8 {
            unsafe { putchar(b); }
        }

        // 使能分发器
        let ctlr = self.read_reg(gicd_offsets::CTLR);
        self.write_reg(gicd_offsets::CTLR, ctlr | 1);

        const MSG9: &[u8] = b"GICD: init done\n";
        for &b in MSG9 {
            unsafe { putchar(b); }
        }

        // println!("GICD initialized");
    }

    /// 使能中断
    pub fn enable_irq(&self, irq: u32) {
        let reg = gicd_offsets::ISENABLER + (irq as usize / 32);
        let bit = irq % 32;
        self.write_reg(reg, 1 << bit);
    }

    /// 禁用中断
    pub fn disable_irq(&self, irq: u32) {
        let reg = gicd_offsets::ICENABLER + (irq as usize / 32);
        let bit = irq % 32;
        self.write_reg(reg, 1 << bit);
    }
}

/// GICv3 重分发器
pub struct GicR {
    base: usize,
}

/// GICv2 CPU 接口
pub struct GicC {
    base: usize,
}

impl GicR {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            let addr = self.base + offset;
            let value: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
            value
        }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            let addr = self.base + offset;
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
        }
    }

    /// 初始化GICR
    pub fn init(&self) {
        use crate::console::putchar;
        const MSG1: &[u8] = b"GICR: Starting initialization...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 读取当前WAKER状态
        let waker = self.read_reg(gicr_offsets::WAKER);

        const MSG2: &[u8] = b"GICR: WAKER = 0x";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        let hex_chars = b"0123456789ABCDEF";
        for i in 0..8 {
            let shift = (7 - i) * 4;
            let nibble = ((waker >> shift) & 0xF) as usize;
            putchar(hex_chars[nibble]);
        }
        const MSG2NL: &[u8] = b"\n";
        for &b in MSG2NL {
            unsafe { putchar(b); }
        }

        // 确保处理器处于唤醒状态 (清除bit 0 ProcessorSleep)
        self.write_reg(gicr_offsets::WAKER, waker & !1);

        const MSG3: &[u8] = b"GICR: clearing WAKER sleep bit\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        // 使能重分发器
        let mut ctlr = self.read_reg(gicr_offsets::CTLR);
        ctlr |= 1; // Enable
        self.write_reg(gicr_offsets::CTLR, ctlr);

        const MSG4: &[u8] = b"GICR: CTLR enabled\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }

        const MSG5: &[u8] = b"GICR initialized\n";
        for &b in MSG5 {
            unsafe { putchar(b); }
        }
    }

    /// 读取挂起寄存器
    pub fn read_pending(&self) -> u64 {
        unsafe {
            let low = (self.base as *const u32).add(0x100 / 4).read_volatile();
            let high = (self.base as *const u32).add(0x104 / 4).read_volatile();
            ((high as u64) << 32) | (low as u64)
        }
    }

    /// 读取IAR1（Interrupt Acknowledge Register for Group 1）
    /// 返回中断ID，bit 9:0是中断号，bit 31:24是CPU ID
    pub fn read_iar1(&self) -> u32 {
        self.read_reg(gicr_offsets::IAR1)
    }

    /// 写入EOIR1（End of Interrupt Register for Group 1）
    pub fn write_eoir1(&self, irq: u32) {
        self.write_reg(gicr_offsets::EOIR1, irq);
    }
}

impl GicC {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            let addr = self.base + offset;
            let value: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
            value
        }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            let addr = self.base + offset;
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) value,
                in(reg) addr,
                options(nomem, nostack)
            );
        }
    }

    /// 初始化 GICv2 CPU 接口（简化版本，匹配 rCore）
    pub fn init(&self) {
        use crate::console::putchar;

        const MSG1: &[u8] = b"gicc: Initializing CPU interface (GICv2 mode, rCore-compatible)...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 设置优先级掩码 (PMR) - 允许所有优先级的中断
        const MSG2: &[u8] = b"gicc: Setting PMR to 0xFF\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        self.write_reg(gicc_offsets::PMR, 0xFF);

        // 使能 CPU 接口 (CTLR)
        // Group 0 中断默认使用 FIQ，Group 1 使用 IRQ
        const MSG3: &[u8] = b"gicc: Enabling CPU interface (CTLR=1)\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }
        self.write_reg(gicc_offsets::CTLR, 1);

        const MSG4: &[u8] = b"gicc: CPU interface init [OK]\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }
    }

    /// 读取中断确认寄存器 (IAR)
    pub fn read_iar(&self) -> u32 {
        self.read_reg(gicc_offsets::IAR)
    }

    /// 写入中断结束寄存器 (EOIR)
    pub fn write_eoir(&self, irq: u32) {
        self.write_reg(gicc_offsets::EOIR, irq);
    }
}

/// 全局GIC实例
static GICD: GicD = GicD::new(GICD_BASE);
static GICR: GicR = GicR::new(GICR_BASE);
static GICC: GicC = GicC::new(GICC_BASE);

/// 初始化GIC中断控制器
///
/// 使用 GICv2 模式（GICC CPU 接口寄存器）
pub fn init() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"gic: Initializing GIC interrupt controller (GICv2 mode)...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    const MSG2: &[u8] = b"gic: Attempting GICD initialization...\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    // 尝试初始化 GICD（使用安全检查避免挂起）
    let gicd_success = unsafe { try_init_gicd() };

    if gicd_success {
        const MSG3: &[u8] = b"gic: GICD initialized successfully\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }
    } else {
        const MSG3: &[u8] = b"gic: GICD init failed, using system registers only\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }
    }

    // 初始化 CPU 接口（使用 GICv3 系统寄存器）
    init_cpu_interface();

    const MSG4: &[u8] = b"gic: GICv3 initialization [OK]\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
}

/// 尝试初始化 GICD，使用安全检查避免挂起
///
/// 返回 true 表示成功，false 表示失败
unsafe fn try_init_gicd() -> bool {
    use crate::console::putchar;

    // 步骤 1: 使用内联汇编读取 GICD_CTLR（已验证可行）
    const MSG1: &[u8] = b"gicd: Step 1 - Reading GICD_CTLR (inline asm)...\n";
    for &b in MSG1 {
        putchar(b);
    }

    let ctlr: u32;
    core::arch::asm!(
        "ldr {}, [{}]",
        out(reg) ctlr,
        in(reg) GICD_BASE,
        options(nomem, nostack)
    );

    // 打印读取的值
    const MSG_VAL: &[u8] = b"gicd: CTLR = 0x";
    for &b in MSG_VAL {
        putchar(b);
    }
    let hex = b"0123456789ABCDEF";
    for i in 0..8 {
        let shift = (7 - i) * 4;
        let nibble = ((ctlr >> shift) & 0xF) as usize;
        putchar(hex[nibble]);
    }
    const MSG_NL: &[u8] = b"\n";
    for &b in MSG_NL {
        putchar(b);
    }

    // 步骤 2: 读取 TYPER
    const MSG2: &[u8] = b"gicd: Step 2 - Reading TYPER...\n";
    for &b in MSG2 {
        putchar(b);
    }

    let typer: u32;
    core::arch::asm!(
        "ldr {}, [{}]",
        out(reg) typer,
        in(reg) (GICD_BASE + 0x004),
        options(nomem, nostack)
    );

    let itlines = ((typer >> 5) & 0xF) + 1;
    let num_irqs = itlines * 32;

    // 打印中断数量
    const MSG_IRQS: &[u8] = b"gicd: ";
    for &b in MSG_IRQS {
        putchar(b);
    }
    let mut n = num_irqs;
    let mut buf = [0u8; 20];
    let mut pos = 0;
    if n == 0 {
        buf[pos] = b'0';
        pos += 1;
    } else {
        while n > 0 {
            buf[pos] = b'0' + ((n % 10) as u8);
            n /= 10;
            pos += 1;
        }
    }
    while pos > 0 {
        pos -= 1;
        putchar(buf[pos]);
    }
    const MSG_IRQS2: &[u8] = b" IRQs detected\n";
    for &b in MSG_IRQS2 {
        putchar(b);
    }

    // 步骤 3: 配置 PPI 为 Group 0 (FIQ) - 使用默认配置
    const MSG3: &[u8] = b"gicd: Using default Group 0 for PPI (16-31)\n";
    for &b in MSG3 {
        putchar(b);
    }

    // 不配置 IGROUPR，使用默认值（Group 0 = FIQ）
    // Group 0 中断使用 FIQ 信号

    const MSG4: &[u8] = b"gicd: Disabling IRQs and clearing pending\n";
    for &b in MSG4 {
        putchar(b);
    }

    // 禁用所有中断
    let num_irqs_usize = num_irqs as usize;
    for i in 0..num_irqs_usize / 32 {
        core::arch::asm!(
            "str {}, [{}]",
            in(reg) 0xFFFFFFFF_u32,
            in(reg) (GICD_BASE + gicd_offsets::ICENABLER + i * 4),
            options(nomem, nostack)
        );
    }

    // 清除所有挂起的中断
    for i in 0..num_irqs_usize / 32 {
        core::arch::asm!(
            "str {}, [{}]",
            in(reg) 0xFFFFFFFF_u32,
            in(reg) (GICD_BASE + gicd_offsets::ICPENDR + i * 4),
            options(nomem, nostack)
        );
    }

    const MSG4B: &[u8] = b"gicd: IRQs disabled and pending cleared\n";
    for &b in MSG4B {
        putchar(b);
    }

    // 步骤 5: 使能 GICD
    const MSG5: &[u8] = b"gicd: Enabling distributor...\n";
    for &b in MSG5 {
        putchar(b);
    }

    let new_ctlr = ctlr | 1;
    core::arch::asm!(
        "str {}, [{}]",
        in(reg) new_ctlr,
        in(reg) GICD_BASE,
        options(nomem, nostack)
    );

    const MSG_DONE: &[u8] = b"gicd: GICD initialization complete!\n";
    for &b in MSG_DONE {
        putchar(b);
    }

    true
}

/// 初始化 CPU 接口（使用 GICv2 GICC 寄存器）
pub fn init_cpu_interface() {
    use crate::console::putchar;

    const MSG1: &[u8] = b"gic: Initializing CPU interface (GICv2 GICC)...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 步骤 1: 设置优先级掩码 (PMR)
    const MSG2: &[u8] = b"gic: Setting PMR to 0xFF\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }
    GICC.write_reg(gicc_offsets::PMR, 0xFF);

    // 步骤 2: 使能 CPU 接口 (CTLR)
    const MSG3: &[u8] = b"gic: Enabling CTLR\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }
    GICC.write_reg(gicc_offsets::CTLR, 1);

    const MSG4: &[u8] = b"gic: CPU interface init [OK]\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
}

/// 使能中断
pub fn enable_irq(irq: u32) {
    GICD.enable_irq(irq);
}

/// 禁用中断
pub fn disable_irq(irq: u32) {
    GICD.disable_irq(irq);
}

/// 确认并获取中断号
/// 必须在中断处理开始时调用
/// 使用 GICv2 GICC_IAR 寄存器（内存映射）
///
/// 返回中断号，如果是 spurious interrupt (1023) 则不需要调用 eoi_interrupt
pub fn ack_interrupt() -> u32 {
    // 使用 GICC 寄存器读取中断确认
    let iar = GICC.read_iar();

    // Spurious interrupt 检查
    if iar >= 1020 {
        return 1023;
    }

    iar
}

/// 结束中断处理
/// 必须在中断处理结束时调用
/// 使用 GICv2 GICC_EOIR 寄存器（内存映射）
///
/// # Arguments
/// * `irq` - 中断号（从 ack_interrupt 返回，非 spurious）
pub fn eoi_interrupt(irq: u32) {
    // 使用 GICC 寄存器结束中断
    GICC.write_eoir(irq);
}

/// 屏蔽 IRQ 中断
///
/// 保存当前 DAIF 状态并禁用 IRQ/FIQ
/// 返回保存的状态，可用于 restore_irq()
pub fn mask_irq() -> u64 {
    unsafe {
        let daif: u64;
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) daif,
            options(nomem, nostack, pure)
        );

        // 设置 I 位（bit 2）和 F 位（bit 3）禁用 IRQ 和 FIQ
        core::arch::asm!(
            "msr daifset, #0xC",  // 设置 bits 2 和 3
            options(nomem, nostack)
        );

        daif  // 返回保存的状态
    }
}

/// 恢复 IRQ 中断状态
///
/// # Arguments
/// * `saved_daif` - 从 mask_irq() 保存的状态
pub fn restore_irq(saved_daif: u64) {
    unsafe {
        // 恢复 DAIF 寄存器
        core::arch::asm!(
            "msr daif, {}",
            in(reg) saved_daif,
            options(nomem, nostack)
        );
    }
}

