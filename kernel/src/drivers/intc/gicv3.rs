//! GICv3 中断控制器驱动
//!
//! Generic Interrupt Controller v3 是ARMv8-A架构的标准中断控制器

use crate::println;

/// GICv3 寄存器基址 (QEMU virt machine)
const GICD_BASE: usize = 0x0800_0000;  // 分发器基址
const GICR_BASE: usize = 0x0808_0000;  // CPU0重分发器基址

/// GICD 寄存器偏移量
mod gicd_offsets {
    pub const CTLR: usize = 0x000;     // 分发器控制寄存器
    pub const TYPER: usize = 0x004;     // 中断类型寄存器
    pub const ISENABLER: usize = 0x100; // 中断使能设置寄存器
    pub const ICENABLER: usize = 0x180; // 中断使能清除寄存器
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
            (self.base as *const u32).add(offset / 4).read_volatile()
        }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            (self.base as *mut u32).add(offset / 4).write_volatile(value);
        }
    }

    /// 初始化GICD
    pub fn init(&self) {
        use crate::console::putchar;
        const MSG1: &[u8] = b"GICD: init start\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
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

impl GicR {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read_reg(&self, offset: usize) -> u32 {
        unsafe {
            (self.base as *const u32).add(offset / 4).read_volatile()
        }
    }

    #[inline]
    fn write_reg(&self, offset: usize, value: u32) {
        unsafe {
            (self.base as *mut u32).add(offset / 4).write_volatile(value);
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

/// 全局GIC实例
static GICD: GicD = GicD::new(GICD_BASE);
static GICR: GicR = GicR::new(GICR_BASE);

/// 初始化GICv3中断控制器
///
/// 注意：GICD/GICR 内存映射访问在当前 MMU 配置下有问题，暂时跳过
/// 对于 IPI (SGI) 支持，我们主要使用 ICC_SGI1R_EL1 系统寄存器
pub fn init() {
    use crate::console::putchar;
    const MSG: &[u8] = b"GIC: Skipping full GIC initialization (MMU access issue)\n";
    for &b in MSG {
        unsafe { putchar(b); }
    }

    const MSG2: &[u8] = b"GIC: IPI uses ICC_SGI1R_EL1 system register (no GICD init needed)\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    // TODO: 修复 GICD 内存访问问题后，取消下面的注释
    /*
    const MSG_GICD: &[u8] = b"GIC: Initializing GICD (Distributor)...\n";
    for &b in MSG_GICD {
        unsafe { putchar(b); }
    }

    // 初始化 GICD (Distributor)
    GICD.init();

    const MSG_GICD_OK: &[u8] = b"GIC: GICD initialized\n";
    for &b in MSG_GICD_OK {
        unsafe { putchar(b); }
    }
    */

    const MSG_DONE: &[u8] = b"GIC: Minimal init complete (IPI ready)\n";
    for &b in MSG_DONE {
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
pub fn ack_interrupt() -> u32 {
    let iar = GICR.read_iar1();
    iar & 0x3FF  // 返回低10位（中断ID）
}

/// 结束中断处理
/// 必须在中断处理结束时调用
pub fn eoi_interrupt(irq: u32) {
    GICR.write_eoir1(irq);
}
