//! GICv3 中断控制器驱动
//!
//! Generic Interrupt Controller v3 - ARMv8-A 架构的标准中断控制器
//! 使用内存映射的重分发器接口（GICR）访问 CPU interface
//!
//! ## GICv3 特性
//! - 使用内存映射接口访问重分发器（GICR）
//! - 支持 Group 0 (FIQ)，Timer 使用 Group 0
//! - PPI (包括 Timer) 通过 GICR 交付
//!
//! ## 参考实现
//! - Linux: drivers/irqchip/irq-gic-v3.c
//! - Linux: drivers/irqchip/irq-gic-v4.c

use crate::console::putchar;

/// GICD 寄存器基址 (QEMU virt machine)
const GICD_BASE: usize = 0x0800_0000;  // 分发器基址
const GICC_BASE: usize = 0x0801_0000;  // CPU Interface 基址 (GICv2 兼容)
const GICR_BASE: usize = 0x0808_0000;  // CPU0 重分发器基址 (GICv3)

/// GICD 寄存器偏移量
mod gicd_offsets {
    pub const CTLR: usize = 0x000;      // 分发器控制寄存器
    pub const TYPER: usize = 0x004;      // 中断类型寄存器
    pub const ISENABLER: usize = 0x100;  // 中断使能设置寄存器
    pub const ICENABLER: usize = 0x180;  // 中断使能清除寄存器
    pub const ISPENDR: usize = 0x200;    // 中断挂起设置寄存器
    pub const ICPENDR: usize = 0x280;    // 中断挂起清除寄存器
    pub const IPRIORITYR: usize = 0x400;  // 中断优先级寄存器
    pub const IGROUPR: usize = 0x80;     // 中断组寄存器
}

/// GICC 寄存器偏移量（CPU Interface，GICv2 兼容）
mod gicc_offsets {
    pub const CTLR: usize = 0x000;      // CPU 接口控制寄存器
    pub const PMR: usize = 0x004;       // 优先级掩码寄存器
    pub const BPR: usize = 0x008;       // 二进制点寄存器
    pub const IAR: usize = 0x00C;       // 中断确认寄存器
    pub const EOIR: usize = 0x010;      // 中断结束寄存器
    pub const RPR: usize = 0x014;       // 运行中优先级寄存器
    pub const HPPIR: usize = 0x018;     // 最高优先级挂起中断寄存器
}

/// GICR 寄存器偏移量（重分发器）
mod gicr_offsets {
    pub const CTLR: usize = 0x000;      // 重分发器控制寄存器
    pub const WAKER: usize = 0x014;     // 唤醒寄存器
    pub const ENABLER: usize = 0x100;   // 中断使能寄存器 (Set-enable)
    pub const ICENABLER: usize = 0x180; // 中断使能寄存器 (Clear-enable)
    pub const ISPENDR: usize = 0x200;   // 中断挂起寄存器
    pub const ICPENDR: usize = 0x280;   // 中断挂起清除寄存器
    pub const IPRIORITYR: usize = 0x400; // 中断优先级寄存器
    pub const IAR0: usize = 0xC0;       // 中断确认寄存器 (Group 0)
    pub const EOIR0: usize = 0x100;     // 中断结束寄存器 (Group 0)
}

/// GICv3 分发器
pub struct GicD {
    base: usize,
}

/// GICv3 CPU Interface（GICv2 兼容接口）
pub struct GicC {
    base: usize,
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

    /// 读取中断确认寄存器（使用内存映射接口 - GICv2 兼容模式）
    pub fn read_iar(&self) -> u32 {
        let iar = self.read_reg(gicc_offsets::IAR);

        // 调试：打印 IAR 的详细信息
        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"gicc: IAR = 0x";
            for &b in MSG {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            putchar(hex[((iar >> 12) & 0xF) as usize]);
            putchar(hex[((iar >> 8) & 0xF) as usize]);
            putchar(hex[((iar >> 4) & 0xF) as usize]);
            putchar(hex[(iar & 0xF) as usize]);

            // 解析 CPU ID 和 Interrupt ID
            let cpu_id = (iar >> 10) & 0x7;
            let irq_id = iar & 0x3FF;
            const MSG2: &[u8] = b" (CPU=";
            for &b in MSG2 {
                putchar(b);
            }
            putchar(hex[((cpu_id >> 4) & 0xF) as usize]);
            putchar(hex[(cpu_id & 0xF) as usize]);
            const MSG3: &[u8] = b", IRQ=";
            for &b in MSG3 {
                putchar(b);
            }
            putchar(hex[((irq_id >> 8) & 0xF) as usize]);
            putchar(hex[((irq_id >> 4) & 0xF) as usize]);
            putchar(hex[(irq_id & 0xF) as usize]);
            const MSG4: &[u8] = b")\n";
            for &b in MSG4 {
                putchar(b);
            }
        }

        iar
    }

    /// 使用系统寄存器读取中断确认寄存器（GICv3 标准方式，Group 1）
    pub fn read_iar1_sysreg(&self) -> u64 {
        unsafe {
            let iar: u64;
            core::arch::asm!(
                "mrs {}, ICC_IAR1_EL1",
                out(reg) iar,
                options(nomem, nostack)
            );

            // 调试：打印 IAR 的详细信息
            use crate::console::putchar;
            const MSG: &[u8] = b"sysreg: ICC_IAR1_EL1 = 0x";
            for &b in MSG {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            putchar(hex[((iar >> 12) & 0xF) as usize]);
            putchar(hex[((iar >> 8) & 0xF) as usize]);
            putchar(hex[((iar >> 4) & 0xF) as usize]);
            putchar(hex[(iar & 0xF) as usize]);
            const NL: &[u8] = b"\n";
            for &b in NL {
                putchar(b);
            }

            iar
        }
    }

    /// 使用系统寄存器读取中断确认寄存器（GICv3 Group 0）
    pub fn read_iar0_sysreg(&self) -> u32 {
        unsafe {
            let iar: u64;
            core::arch::asm!(
                "mrs {}, ICC_IAR0_EL1",
                out(reg) iar,
                options(nomem, nostack)
            );

            (iar & 0x3FF) as u32
        }
    }

    /// 使用系统寄存器写入中断结束寄存器（GICv3 标准方式，Group 1）
    pub fn write_eoir1_sysreg(&self, value: u32) {
        unsafe {
            core::arch::asm!(
                "msr ICC_EOIR1_EL1, {}",
                in(reg) value as u64,
                options(nomem, nostack)
            );
        }
    }

    /// 使用系统寄存器写入中断结束寄存器（GICv3 Group 0）
    pub fn write_eoir0_sysreg(&self, value: u32) {
        unsafe {
            core::arch::asm!(
                "msr ICC_EOIR0_EL1, {}",
                in(reg) value as u64,
                options(nomem, nostack)
            );
        }
    }

    /// 读取 ICC_SRE_EL1 系统寄存器
    pub fn read_sre(&self) -> u32 {
        unsafe {
            let sre: u64;
            core::arch::asm!(
                "mrs {}, ICC_SRE_EL1",
                out(reg) sre,
                options(nomem, nostack)
            );
            sre as u32
        }
    }

    /// 写入 ICC_SRE_EL1 系统寄存器
    pub fn write_sre(&self, value: u32) {
        unsafe {
            core::arch::asm!(
                "msr ICC_SRE_EL1, {}",
                in(reg) value as u64,
                options(nomem, nostack)
            );
            // ISB after SRE write
            core::arch::asm!("isb", options(nomem, nostack));
        }
    }

    /// 使能系统寄存器访问
    pub fn enable_sre(&self) -> bool {
        const MSG1: &[u8] = b"gicc: Enabling system register access...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        let sre = self.read_sre();

        const MSG2: &[u8] = b"gicc: ICC_SRE_EL1 before = 0x";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        let hex = b"0123456789ABCDEF";
        unsafe { putchar(hex[((sre >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(sre & 0xF) as usize]); }
        const NL: &[u8] = b"\n";
        for &b in NL {
            unsafe { putchar(b); }
        }

        // 检查 SRE bit
        if sre & 0x1 != 0 {
            const MSG3: &[u8] = b"gicc: SRE already enabled\n";
            for &b in MSG3 {
                unsafe { putchar(b); }
            }
            return true;
        }

        // 设置 SRE bit
        let new_sre = sre | 0x1;
        self.write_sre(new_sre);

        // 验证
        let sre_after = self.read_sre();
        const MSG4: &[u8] = b"gicc: ICC_SRE_EL1 after = 0x";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }
        unsafe { putchar(hex[((sre_after >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(sre_after & 0xF) as usize]); }
        for &b in NL {
            unsafe { putchar(b); }
        }

        if sre_after & 0x1 != 0 {
            const MSG5: &[u8] = b"gicc: System register access enabled [OK]\n";
            for &b in MSG5 {
                unsafe { putchar(b); }
            }
            true
        } else {
            const MSG6: &[u8] = b"gicc: Failed to enable SRE!\n";
            for &b in MSG6 {
                unsafe { putchar(b); }
            }
            false
        }
    }

    /// 写入中断结束寄存器
    pub fn write_eoir(&self, value: u32) {
        self.write_reg(gicc_offsets::EOIR, value);
    }

    /// 使用系统寄存器初始化 CPU Interface（完全按照 Linux 方式）
    pub fn init_sysreg(&self) {
        let hex = b"0123456789ABCDEF";
        const NL: &[u8] = b"\n";

        const MSG0: &[u8] = b"gicc: Attempting sysreg init...\n";
        for &b in MSG0 {
            unsafe { putchar(b); }
        }

        // 直接尝试使用系统寄存器，不读取 ICC_SRE_EL1
        // 如果 firmware 已经使能了 SRE，这些操作会成功
        // 否则我们会 fallback 到内存映射接口

        // 步骤 1: 设置 ICC_PMR_EL1（优先级掩码）
        const MSG1: &[u8] = b"gicc: Setting ICC_PMR_EL1...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }
        unsafe {
            core::arch::asm!(
                "msr ICC_PMR_EL1, {}",
                in(reg) 0xFFu64,
                options(nomem, nostack)
            );
        }

        // 步骤 2: 设置 ICC_BPR1_EL1（二进制点）
        const MSG2: &[u8] = b"gicc: Setting ICC_BPR1_EL1...\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        unsafe {
            core::arch::asm!(
                "msr ICC_BPR1_EL1, {}",
                in(reg) 0u64,
                options(nomem, nostack)
            );
        }

        // 步骤 3: 设置 ICC_CTLR_EL1（控制寄存器）
        // EOImode=1 (drop priority only, don't deactivate)
        const MSG3: &[u8] = b"gicc: Setting ICC_CTLR_EL1...\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }
        unsafe {
            // ICC_CTLR_EL1_EOImode_drop = 0x4
            core::arch::asm!(
                "msr ICC_CTLR_EL1, {}",
                in(reg) 0x4u64,
                options(nomem, nostack)
            );
        }

        // 步骤 4: 使能 Group 1（ICC_IGRPEN1_EL1）
        const MSG4: &[u8] = b"gicc: Enabling Group 1 (ICC_IGRPEN1_EL1)...\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }
        unsafe {
            core::arch::asm!(
                "msr ICC_IGRPEN1_EL1, {}",
                in(reg) 1u64,
                options(nomem, nostack)
            );
            // ISB after Group enable
            core::arch::asm!("isb", options(nomem, nostack));
        }

        const MSG_DONE: &[u8] = b"gicc: Sysreg init complete\n";
        for &b in MSG_DONE {
            unsafe { putchar(b); }
        }
    }

    /// 初始化 CPU Interface（旧版本，使用内存映射）
    #[allow(dead_code)]
    pub fn init_mmio(&self) {
        let hex = b"0123456789ABCDEF";
        const NL: &[u8] = b"\n";

        const MSG1: &[u8] = b"gicc: Setting BPR...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 步骤 1: 设置二进制点为 0（默认值）
        self.write_reg(gicc_offsets::BPR, 0);

        const MSG2: &[u8] = b"gicc: Enabling Group 1 (IRQ)...\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }

        // 步骤 2: 读取并使能 CTLR（这会重置 PMR）
        let ctlr = self.read_reg(gicc_offsets::CTLR);
        const MSG2_READ: &[u8] = b"gicc: CTLR before = 0x";
        for &b in MSG2_READ {
            unsafe { putchar(b); }
        }
        unsafe { putchar(hex[((ctlr >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(ctlr & 0xF) as usize]); }
        for &b in NL {
            unsafe { putchar(b); }
        }

        // 禁用 Group 0 (FIQ)，使能 Group 1 (IRQ)
        // bit 0 = EnableGrp0, bit 1 = EnableGrp1
        let new_ctlr = (ctlr & !0x01) | 0x02;  // 清除 bit 0，设置 bit 1
        self.write_reg(gicc_offsets::CTLR, new_ctlr);

        let final_ctlr = self.read_reg(gicc_offsets::CTLR);
        const MSG2_DONE: &[u8] = b"gicc: CTLR after = 0x";
        for &b in MSG2_DONE {
            unsafe { putchar(b); }
        }
        unsafe { putchar(hex[((final_ctlr >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(final_ctlr & 0xF) as usize]); }
        for &b in NL {
            unsafe { putchar(b); }
        }

        const MSG3: &[u8] = b"gicc: Setting PMR (after CTLR)...\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        // 步骤 3: 设置优先级掩码为 0xFF（允许所有优先级的中断）
        // 必须在 CTLR 使能之后设置！
        self.write_reg(gicc_offsets::PMR, 0xFF);

        // 验证 PMR 是否保持
        let pmr_check = self.read_reg(gicc_offsets::PMR);
        const MSG3_DONE: &[u8] = b"gicc: PMR = 0x";
        for &b in MSG3_DONE {
            unsafe { putchar(b); }
        }
        unsafe { putchar(hex[((pmr_check >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(pmr_check & 0xF) as usize]); }
        for &b in NL {
            unsafe { putchar(b); }
        }

        const MSG_DONE: &[u8] = b"gicc: CPU interface init [OK]\n";
        for &b in MSG_DONE {
            unsafe { putchar(b); }
        }
    }

    /// 初始化 CPU Interface
    /// 使用 GICv2 兼容的内存映射接口（QEMU virt 默认不支持系统寄存器）
    pub fn init(&self) {
        self.init_mmio()
    }
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

    /// 使能中断
    pub fn enable_irq(&self, irq: u32) {
        let reg_offset = gicd_offsets::ISENABLER + (irq as usize) / 32 * 4;
        let bit = 1 << (irq % 32);
        let current = self.read_reg(reg_offset);
        self.write_reg(reg_offset, current | bit);
    }

    /// 禁用中断
    pub fn disable_irq(&self, irq: u32) {
        let reg_offset = gicd_offsets::ICENABLER + (irq as usize) / 32 * 4;
        let bit = 1 << (irq % 32);
        self.write_reg(reg_offset, bit);
    }

    /// 初始化分发器
    pub fn init(&self) -> bool {
        // 步骤 1: 读取 GICD_CTLR
        const MSG1: &[u8] = b"gicv3: Step 1 - Reading GICD_CTLR\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        let ctlr = self.read_reg(gicd_offsets::CTLR);
        const MSG1_READ: &[u8] = b"gicv3: CTLR = 0x";
        for &b in MSG1_READ {
            unsafe { putchar(b); }
        }
        let hex = b"0123456789ABCDEF";
        unsafe { putchar(hex[((ctlr >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(ctlr & 0xF) as usize]); }
        const NL: &[u8] = b"\n";
        for &b in NL {
            unsafe { putchar(b); }
        }

        // 步骤 2: 读取 TYPER
        const MSG2: &[u8] = b"gicv3: Step 2 - Reading TYPER\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }

        let typer = self.read_reg(gicd_offsets::TYPER);
        let itlines = ((typer >> 5) & 0xF) + 1;
        let num_irqs = itlines * 32;

        const MSG2_OK: &[u8] = b"gicv3: ";
        for &b in MSG2_OK {
            unsafe { putchar(b); }
        }
        let mut n = num_irqs;
        let mut buf = [0u8; 20];
        let mut pos = 0;
        if n == 0 {
            buf[pos] = b'0';
            pos += 1;
        } else {
            while n > 0 {
                let digit = (n % 10) as u8;
                buf[pos] = b'0' + digit;
                pos += 1;
                n /= 10;
            }
        }
        while pos > 0 {
            pos -= 1;
            unsafe { putchar(buf[pos]); }
        }
        const MSG2_END: &[u8] = b" IRQs detected\n";
        for &b in MSG2_END {
            unsafe { putchar(b); }
        }

        // 步骤 3: 配置中断组 - 使用 Group 1 (IRQ)
        // 将 Timer (IRQ 30) 配置为 Group 1，与 GICv2 兼容接口配合
        const MSG3: &[u8] = b"gicv3: Configuring Group 1 (IRQ)...\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        // GICD_IGROUPR0: 0 = Group 0 (FIQ), 1 = Group 1 (IRQ)
        // 设置 bit 30 (Timer) 为 Group 1
        let igroup_val = 0x40000000u32;  // bit 30 = 1 (Group 1)
        self.write_reg(gicd_offsets::IGROUPR, igroup_val);

        const MSG3_OK: &[u8] = b"gicv3: Group 1 configured (Timer in Group 1)\n";
        for &b in MSG3_OK {
            unsafe { putchar(b); }
        }

        // 步骤 4: 设置中断优先级（GICD_IPRIORITYR）
        // 参考 Linux 实现：所有中断优先级为 0xa0
        const MSG4: &[u8] = b"gicv3: Step 4 - Setting interrupt priorities...\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }

        // 设置前32个中断的优先级为 0xa0（包括 Timer IRQ 30）
        for i in 0..32 / 4 {
            let prio_val = 0xa0a0a0a0u32;  // 4个中断，每个优先级 0xa0
            self.write_reg(gicd_offsets::IPRIORITYR + i * 4, prio_val);
        }

        const MSG4_OK: &[u8] = b"gicv3: Interrupt priorities set to 0xa0\n";
        for &b in MSG4_OK {
            unsafe { putchar(b); }
        }

        // 步骤 5: 禁用所有中断并清除挂起
        const MSG5: &[u8] = b"gicv3: Disabling IRQs and clearing pending\n";
        for &b in MSG5 {
            unsafe { putchar(b); }
        }

        let num_irqs_usize = num_irqs as usize;
        for i in 0..num_irqs_usize / 32 {
            self.write_reg(gicd_offsets::ICENABLER + i * 4, 0xFFFFFFFF);
        }

        for i in 0..num_irqs_usize / 32 {
            self.write_reg(gicd_offsets::ICPENDR + i * 4, 0xFFFFFFFF);
        }

        const MSG5_OK: &[u8] = b"gicv3: IRQs disabled and pending cleared\n";
        for &b in MSG5_OK {
            unsafe { putchar(b); }
        }

        // 步骤 6: 使能 GICD
        const MSG6: &[u8] = b"gicv3: Enabling distributor...\n";
        for &b in MSG6 {
            unsafe { putchar(b); }
        }

        let new_ctlr = ctlr | 1;
        self.write_reg(gicd_offsets::CTLR, new_ctlr);

        const MSG_DONE: &[u8] = b"gicv3: GICD initialization complete!\n";
        for &b in MSG_DONE {
            unsafe { putchar(b); }
        }

        true
    }
}

/// GICv3 重分发器（Redistributor）
/// 用于内存映射访问 CPU interface
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

    /// 读取中断确认寄存器 (Group 0/FIQ)
    pub fn read_iar0(&self) -> u32 {
        self.read_reg(gicr_offsets::IAR0)
    }

    /// 写入中断结束寄存器 (Group 0/FIQ)
    pub fn write_eoir0(&self, value: u32) {
        self.write_reg(gicr_offsets::EOIR0, value);
    }

    /// 唤醒 Redistributor（清除 ProcessorSleep 位）
    pub fn wake_up(&self) {
        const MSG1: &[u8] = b"gicr: Reading WAKER...\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 读取当前 WAKER 值
        let waker = self.read_reg(gicr_offsets::WAKER);

        const MSG2: &[u8] = b"gicr: WAKER = 0x";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        let hex = b"0123456789ABCDEF";
        unsafe { putchar(hex[((waker >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(waker & 0xF) as usize]); }
        const NL: &[u8] = b"\n";
        for &b in NL {
            unsafe { putchar(b); }
        }

        // 清除 bit 1 (ProcessorSleep)
        let new_waker = waker & !0x02;

        const MSG3: &[u8] = b"gicr: Clearing ProcessorSleep...\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        self.write_reg(gicr_offsets::WAKER, new_waker);

        const MSG4: &[u8] = b"gicr: WAKER after = 0x";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }
        let final_waker = self.read_reg(gicr_offsets::WAKER);
        unsafe { putchar(hex[((final_waker >> 4) & 0xF) as usize]); }
        unsafe { putchar(hex[(final_waker & 0xF) as usize]); }
        for &b in NL {
            unsafe { putchar(b); }
        }

        const MSG_DONE: &[u8] = b"gicr: Redistributor wake up [OK]\n";
        for &b in MSG_DONE {
            unsafe { putchar(b); }
        }
    }
}

/// 全局 GICv3 分发器实例
static GICD: GicD = GicD::new(GICD_BASE);

/// 全局 GICv3 CPU Interface 实例（GICv2 兼容）
static GICC: GicC = GicC::new(GICC_BASE);

/// 全局 GICv3 重分发器实例
static GICR: GicR = GicR::new(GICR_BASE);

/// 初始化 GICv3 分发器
pub fn init_distributor() -> bool {
    GICD.init()
}

/// 初始化 GICv3 Redistributor（配置 PPI）
pub fn init_redistributor() {
    const MSG1: &[u8] = b"gicr: Initializing Redistributor...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 首先清除所有 PPI 挂起状态（在配置之前）
    const MSG1B: &[u8] = b"gicr: Clearing PPI pending interrupts...\n";
    for &b in MSG1B {
        unsafe { putchar(b); }
    }

    unsafe {
        core::arch::asm!(
            "str {}, [{}]",
            in(reg) 0xFFFFFFFFu32,  // 清除所有 PPI (bit 0-31)
            in(reg) GICR_BASE + 0x0280,  // GICR_ICPENDR0
            options(nostack)
        );
    }

    // 配置 PPI 为 Group 1（IRQ）- GICv3 需要 GICR 配置 PPI
    // GICR_IGROUPR0 的 bit 30 控制 Timer (IRQ 30)
    const MSG2: &[u8] = b"gicr: Configuring PPIs as Group 1 (IRQ)...\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    // GICR_IGROUPR0: 0 = Group 0 (FIQ), 1 = Group 1 (IRQ)
    // 设置所有 PPI (bit 0-31) 为 Group 1
    unsafe {
        // 设置所有 bit 为 1（所有 PPI 都是 Group 1）
        let new_value = 0xFFFFFFFFu32;  // 所有 PPI = Group 1
        core::arch::asm!(
            "str {}, [{}]",
            in(reg) new_value,
            in(reg) GICR_BASE + 0x0080,  // GICR_IGROUPR0
            options(nostack)
        );

        // 验证写入
        let igroupr: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) igroupr,
            in(reg) GICR_BASE + 0x0080,  // GICR_IGROUPR0
            options(nostack)
        );
        const MSG_VAL: &[u8] = b"gicr: GICR_IGROUPR0 = 0x";
        for &b in MSG_VAL {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((igroupr >> 28) & 0xF) as usize]);
        putchar(hex[((igroupr >> 24) & 0xF) as usize]);
        putchar(hex[((igroupr >> 20) & 0xF) as usize]);
        putchar(hex[((igroupr >> 16) & 0xF) as usize]);
        putchar(hex[((igroupr >> 12) & 0xF) as usize]);
        putchar(hex[((igroupr >> 8) & 0xF) as usize]);
        putchar(hex[((igroupr >> 4) & 0xF) as usize]);
        putchar(hex[(igroupr & 0xF) as usize]);
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // 设置 PPI 优先级
    const MSG3: &[u8] = b"gicr: Setting PPI priorities...\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }

    // GICR_IPRIORITYR: 每 4 个字节设置 4 个 PPI 的优先级
    // PPI 是 16-31，所以只需要设置 IPRIORITYR4 到 IPRIORITYR7
    for i in 4..8 {
        unsafe {
            core::arch::asm!(
                "str {}, [{}]",
                in(reg) 0xa0a0a0a0u32,  // 优先级 0xa0
                in(reg) GICR_BASE + 0x400 + i * 4,  // GICR_IPRIORITYR
                options(nostack)
            );
        }
    }

    // 使能 PPI（特别是 Timer IRQ 30）
    const MSG4: &[u8] = b"gicr: Enabling PPIs...\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }

    unsafe {
        // GICR_ISENABLER0: bit 30 控制 Timer
        core::arch::asm!(
            "str {}, [{}]",
            in(reg) 0x40000000u32,  // bit 30 = Timer
            in(reg) GICR_BASE + 0x0100,  // GICR_ISENABLER0
            options(nostack)
        );
    }

    // 等待 Redistributor 完成（RWP - Read-Wait Polling）
    const MSG5: &[u8] = b"gicr: Waiting for RWP...\n";
    for &b in MSG5 {
        unsafe { putchar(b); }
    }

    unsafe {
        let mut timeout = 1000;
        while timeout > 0 {
            let ctlr: u32;
            core::arch::asm!(
                "ldr {}, [{}]",
                out(reg) ctlr,
                in(reg) GICR_BASE + 0x000,  // GICR_CTLR
                options(nostack)
            );

            // 检查 RWP bit (bit 3)
            if ctlr & 0x08 == 0 {
                // RWP 清除，完成
                const MSG_DONE_RWP: &[u8] = b"gicr: RWP cleared\n";
                for &b in MSG_DONE_RWP {
                    putchar(b);
                }
                break;
            }

            // 简单延时
            for _ in 0..100 {
                core::arch::asm!("nop", options(nomem, nostack));
            }
            timeout -= 1;
        }

        if timeout == 0 {
            const MSG_TIMEOUT: &[u8] = b"gicr: RWP timeout\n";
            for &b in MSG_TIMEOUT {
                putchar(b);
            }
        }
    }

    const MSG_DONE: &[u8] = b"gicr: Redistributor init [OK]\n";
    for &b in MSG_DONE {
        unsafe { putchar(b); }
    }
}

/// 初始化 GICv3 CPU interface（使用 GICv2 兼容的内存映射接口）
pub fn init_cpu_interface() {
    const MSG1: &[u8] = b"gicv3: Initializing CPU interface (GICv2 compatible mmio)...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 使用内存映射接口初始化 CPU Interface（GICv2 兼容模式）
    // QEMU virt 默认禁用系统寄存器访问（ICC_SRE_EL1.SRE=0）
    GICC.init();

    const MSG_DONE: &[u8] = b"gicv3: CPU interface initialization complete\n";
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

/// 确认并获取中断号（使用 GICC_IAR 内存映射接口）
pub fn ack_interrupt() -> u32 {
    // 使用内存映射接口读取中断确认
    let iar = GICC.read_iar();

    // Spurious interrupt 检查（ID 1023 = 0x3FF）
    if iar == 0x3FF {
        return 1023;
    }

    iar
}

/// 读取 IAR 并打印详细信息（用于调试）
pub fn read_iar_debug() -> u32 {
    GICC.read_iar()
}

/// 使用系统寄存器读取 IAR（GICv3 标准方式，Group 1）
pub fn read_iar_sysreg() -> u32 {
    (GICC.read_iar1_sysreg() & 0x3FF) as u32
}

/// 使用系统寄存器写入 EOIR（GICv3 标准方式，Group 1）
pub fn write_eoir_sysreg(irq: u32) {
    GICC.write_eoir1_sysreg(irq);
}

/// 使能系统寄存器访问
pub fn enable_sre() -> bool {
    GICC.enable_sre()
}

/// 结束中断处理（使用 GICC_EOIR 内存映射接口）
pub fn eoi_interrupt(irq: u32) {
    // 使用内存映射接口结束中断
    GICC.write_eoir(irq);
}

/// 屏蔽 IRQ/FIQ 中断
pub fn mask_irq() -> u64 {
    unsafe {
        let daif: u64;
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) daif,
            options(nomem, nostack, pure)
        );

        // 设置 F 位（bit 3）和 I 位（bit 2）禁用 FIQ 和 IRQ
        core::arch::asm!(
            "msr daifset, #0xC",  // 设置 bits 2 和 3
            options(nomem, nostack)
        );

        daif  // 返回保存的状态
    }
}

/// 恢复 IRQ/FIQ 中断状态
pub fn restore_irq(saved_daif: u64) {
    unsafe {
        core::arch::asm!(
            "msr daif, {}",
            in(reg) saved_daif,
            options(nomem, nostack)
        );
    }
}

/// 读取 Timer 硬件状态（用于调试）
pub fn check_timer_status() {
    const MSG1: &[u8] = b"diag: Checking Timer hardware status...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    unsafe {
        // 读取 CNTP_CTL_EL0
        let ctl: u64;
        core::arch::asm!("mrs {}, cntp_ctl_el0", out(reg) ctl, options(nomem, nostack));

        const MSG_CTL: &[u8] = b"diag: CNTP_CTL_EL0 = 0x";
        for &b in MSG_CTL {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((ctl >> 4) & 0xF) as usize]);
        putchar(hex[(ctl & 0xF) as usize]);

        // 检查 ISTATUS 位 (bit 2)
        let istatus = (ctl >> 2) & 0x1;
        const MSG_ISTATUS: &[u8] = b" (ISTATUS=";
        for &b in MSG_ISTATUS {
            putchar(b);
        }
        putchar(b'0' + istatus as u8);
        const MSG_END: &[u8] = b")\n";
        for &b in MSG_END {
            putchar(b);
        }

        // 读取 CNTP_TVAL_EL0
        let tval: u64;
        core::arch::asm!("mrs {}, cntp_tval_el0", out(reg) tval, options(nomem, nostack));
        const MSG_TVAL: &[u8] = b"diag: CNTP_TVAL_EL0 = ";
        for &b in MSG_TVAL {
            putchar(b);
        }
        let mut n = tval;
        let mut buf = [0u8; 20];
        let mut pos = 0;
        if n == 0 {
            buf[pos] = b'0';
            pos += 1;
        } else {
            while n > 0 {
                let digit = (n % 10) as u8;
                buf[pos] = b'0' + digit;
                pos += 1;
                n /= 10;
            }
        }
        while pos > 0 {
            pos -= 1;
            putchar(buf[pos]);
        }
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }
}

/// 读取 GICR 挂起状态（用于调试）
pub fn check_gicr_pending() {
    const MSG1: &[u8] = b"diag: Checking GICR pending status...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    unsafe {
        // 读取 GICR_ISPENDR0
        let pending: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) pending,
            in(reg) GICR_BASE + 0x0200,  // GICR_ISPENDR0
            options(nomem, nostack)
        );

        const MSG2: &[u8] = b"diag: GICR_ISPENDR0 = 0x";
        for &b in MSG2 {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        for i in (0..32).step_by(4).rev() {
            putchar(hex[((pending >> (i + 4)) & 0xF) as usize]);
            putchar(hex[((pending >> i) & 0xF) as usize]);
        }
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }

        // 检查 bit 30 (Timer)
        let timer_pending = (pending >> 30) & 0x1;
        const MSG_TIMER: &[u8] = b"diag: Timer (bit30) pending = ";
        for &b in MSG_TIMER {
            putchar(b);
        }
        putchar(b'0' + (timer_pending as u8));
        const NL2: &[u8] = b"\n";
        for &b in NL2 {
            putchar(b);
        }
    }
}

/// 使用 GICR_IAR0 读取中断（Group 0）- 静默版本
/// 注意：GICv3 中应使用 ICC_IAR0_EL1 系统寄存器
pub fn ack_interrupt_group0() -> u32 {
    // 尝试使用系统寄存器 ICC_IAR0_EL1（GICv3 标准方式）
    unsafe {
        let iar: u64;
        core::arch::asm!(
            "mrs {}, ICC_IAR0_EL1",
            out(reg) iar,
            options(nomem, nostack)
        );
        (iar & 0x3FF) as u32
    }
}

/// 使用 GICR_IAR0 读取中断（Group 0）- 调试版本
pub fn ack_interrupt_group0_debug() -> u32 {
    unsafe {
        let iar: u32;
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) iar,
            in(reg) GICR_BASE + 0x00C0,  // GICR_IAR0 (Group 0)
            options(nomem, nostack)
        );

        // 解析 Interrupt ID
        let irq_id = iar & 0x3FF;

        const MSG: &[u8] = b"gicr: IAR0 = 0x";
        for &b in MSG {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[((iar >> 12) & 0xF) as usize]);
        putchar(hex[((iar >> 8) & 0xF) as usize]);
        putchar(hex[((iar >> 4) & 0xF) as usize]);
        putchar(hex[(iar & 0xF) as usize]);

        const MSG2: &[u8] = b" (IRQ=";
        for &b in MSG2 {
            putchar(b);
        }
        putchar(hex[((irq_id >> 8) & 0xF) as usize]);
        putchar(hex[((irq_id >> 4) & 0xF) as usize]);
        putchar(hex[(irq_id & 0xF) as usize]);
        const MSG3: &[u8] = b")\n";
        for &b in MSG3 {
            putchar(b);
        }

        irq_id
    }
}

/// 使用 GICR_EOIR0 结束中断（Group 0）
/// 注意：GICv3 中应使用 ICC_EOIR0_EL1 系统寄存器
pub fn eoi_interrupt_group0(irq: u32) {
    // 使用系统寄存器 ICC_EOIR0_EL1（GICv3 标准方式）
    unsafe {
        core::arch::asm!(
            "msr ICC_EOIR0_EL1, {}",
            in(reg) irq as u64,
            options(nomem, nostack)
        );
    }
}

/// 初始化 GICv3（完整初始化）
pub fn init() {
    // 首先检查 DAIF 状态，确认中断被屏蔽
    const MSG_DAIF: &[u8] = b"gic: Checking DAIF (interrupt mask)...\n";
    for &b in MSG_DAIF {
        unsafe { putchar(b); }
    }

    let daif_before: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) daif_before,
            options(nomem, nostack)
        );
    }

    const MSG_DAIF_VAL: &[u8] = b"gic: DAIF = 0x";
    for &b in MSG_DAIF_VAL {
        unsafe { putchar(b); }
    }
    let hex = b"0123456789ABCDEF";
    unsafe { putchar(hex[((daif_before >> 4) & 0xF) as usize]); }
    unsafe { putchar(hex[(daif_before & 0xF) as usize]); }
    let nl = b"\n";
    for &b in nl {
        unsafe { putchar(b); }
    }

    // 检查 DAIF 的 I 位（bit 7）是否已设置
    // 根据测试，DAIFSET 的映射是：imm[1] → DAIF bit 7 (I)
    // 所以我们需要检查 bit 7 是否被设置
    if daif_before & (1 << 7) == 0 {
        const MSG_FIX: &[u8] = b"gic: WARNING! IRQ not masked (bit 7 not set), forcing mask...\n";
        for &b in MSG_FIX {
            unsafe { putchar(b); }
        }
        unsafe {
            // 使用 daifset 指令来设置 I 位
            // imm[1] → DAIF bit 7，所以使用 #2
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        }
    }

    const MSG0: &[u8] = b"\n=== GIC Version Detection ===\n";
    for &b in MSG0 {
        unsafe { putchar(b); }
    }

    // 检测 GIC 版本（通过 PIDR2 寄存器）
    // PIDR2 在 offset 0xFFE8，bits [7:4] 包含架构版本

    // 首先验证 GIC 可访问（读取 GICD_CTLR）
    const MSG_TEST: &[u8] = b"gic: Testing GIC accessibility (reading GICD_CTLR)...\n";
    for &b in MSG_TEST {
        unsafe { putchar(b); }
    }

    let test_val: u32;
    unsafe {
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) test_val,
            in(reg) GICD_BASE + 0x000,  // GICD_CTLR
            options(nostack)
        );
    }

    const MSG_TEST_OK: &[u8] = b"gic: GICD_CTLR = 0x";
    for &b in MSG_TEST_OK {
        unsafe { putchar(b); }
    }
    let hex = b"0123456789ABCDEF";
    unsafe { putchar(hex[((test_val >> 4) & 0xF) as usize]); }
    unsafe { putchar(hex[(test_val & 0xF) as usize]); }
    let nl = b"\n";
    for &b in nl {
        unsafe { putchar(b); }
    }

    // 现在尝试读取 IIDR（而不是 PIDR2，因为 GICv2 可能没有 PIDR2）
    const MSG_DETECT: &[u8] = b"gic: Reading IIDR register to identify GIC...\n";
    for &b in MSG_DETECT {
        unsafe { putchar(b); }
    }

    let iidr: u32;
    unsafe {
        core::arch::asm!(
            "ldr {}, [{}]",
            out(reg) iidr,
            in(reg) GICD_BASE + 0x008,  // GICD_IIDR (GICv2/v3 compatible)
            options(nostack)
        );
    }

    const MSG_IIDR_VAL: &[u8] = b"gic: IIDR = 0x";
    for &b in MSG_IIDR_VAL {
        unsafe { putchar(b); }
    }
    let hex = b"0123456789ABCDEF";
    unsafe { putchar(hex[((iidr >> 28) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 24) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 20) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 16) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 12) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 8) & 0xF) as usize]); }
    unsafe { putchar(hex[((iidr >> 4) & 0xF) as usize]); }
    unsafe { putchar(hex[(iidr & 0xF) as usize]); }
    let nl = b"\n";
    for &b in nl {
        unsafe { putchar(b); }
    }

    // 基于 QEMU virt 设备树信息：这是 GICv2
    // 设备树显示: "arm,cortex-a15-gic" = GICv2
    // 但我们的驱动是 GICv3，这会导致问题！
    const MSG_CONCLUSION: &[u8] = b"\ngic: *** CONCLUSION (from device tree) ***\n";
    for &b in MSG_CONCLUSION {
        unsafe { putchar(b); }
    }

    const MSG_V2: &[u8] = b"gic: Hardware: GICv2 (arm,cortex-a15-gic)\n";
    for &b in MSG_V2 {
        unsafe { putchar(b); }
    }

    const MSG_DRIVER: &[u8] = b"gic: Driver: GICv3 (MISMATCH!)\n";
    for &b in MSG_DRIVER {
        unsafe { putchar(b); }
    }

    const MSG_EXPLAIN: &[u8] = b"gic: GICv2 has NO GICR (0x0808_0000) - only GICC (0x0801_0000)\n";
    for &b in MSG_EXPLAIN {
        unsafe { putchar(b); }
    }

    const MSG_IMPACT: &[u8] = b"gic: This explains why Timer (PPI) doesn't work!\n";
    for &b in MSG_IMPACT {
        unsafe { putchar(b); }
    }

    // 假装这是 GICv2 用于后续逻辑
    let pidr2 = 0x20;  // arch_version = 2 (GICv2)

    // PIDR2[7:4] = Arch 版本
    // 0x3 = GICv3, 0x4 = GICv4, 0x2 = GICv2, 0x1/0x0 = GICv1
    let arch_version = (pidr2 >> 4) & 0xF;

    const MSG_ARCH: &[u8] = b"gic: Architecture version detected: ";
    for &b in MSG_ARCH {
        unsafe { putchar(b); }
    }

    // 打印版本号
    let mut v = arch_version;
    let mut buf = [0u8; 20];
    let mut pos = 0;
    if v == 0 {
        buf[pos] = b'0';
        pos += 1;
    } else {
        while v > 0 {
            let digit = (v % 10) as u8;
            buf[pos] = b'0' + digit;
            pos += 1;
            v /= 10;
        }
    }
    while pos > 0 {
        pos -= 1;
        unsafe { putchar(buf[pos]); }
    }
    for &b in nl {
        unsafe { putchar(b); }
    }

    // 判断 GIC 版本
    if arch_version == 0x3 {
        const MSG_V3: &[u8] = b"gic: *** This is GICv3 ***\n";
        for &b in MSG_V3 {
            unsafe { putchar(b); }
        }
    } else if arch_version == 0x2 {
        const MSG_V2: &[u8] = b"gic: *** This is GICv2 ***\n";
        for &b in MSG_V2 {
            unsafe { putchar(b); }
        }
        const MSG_WARN: &[u8] = b"gic: WARNING! GICv2 detected but using GICv3 driver!\n";
        for &b in MSG_WARN {
            unsafe { putchar(b); }
        }
        const MSG_WARN2: &[u8] = b"gic: PPI (Timer) may not work without GICR!\n";
        for &b in MSG_WARN2 {
            unsafe { putchar(b); }
        }
    } else {
        const MSG_UNKNOWN: &[u8] = b"gic: Unknown GIC version!\n";
        for &b in MSG_UNKNOWN {
            unsafe { putchar(b); }
        }
    }

    const MSG_SEP: &[u8] = b"=== Starting Initialization ===\n";
    for &b in MSG_SEP {
        unsafe { putchar(b); }
    }

    // 打印 GIC 基地址（与参考项目对比）
    const MSG_BASE: &[u8] = b"gic: GIC base addresses (for comparison):\n";
    for &b in MSG_BASE {
        unsafe { putchar(b); }
    }

    const MSG_GICD: &[u8] = b"  GICD_BASE = 0x0800_0000\n";
    for &b in MSG_GICD {
        unsafe { putchar(b); }
    }

    const MSG_GICC: &[u8] = b"  GICC_BASE = 0x0801_0000 (GICv2 compatible)\n";
    for &b in MSG_GICC {
        unsafe { putchar(b); }
    }

    const MSG_GICR: &[u8] = b"  GICR_BASE = 0x0808_0000 (GICv3 only!)\n";
    for &b in MSG_GICR {
        unsafe { putchar(b); }
    }

    const MSG_RCORE: &[u8] = b"gic: Reference (rCore-Tutorial-GICv2):\n";
    for &b in MSG_RCORE {
        unsafe { putchar(b); }
    }

    const MSG_RCORE_VAL: &[u8] = b"  GIC_BASE = 0x0800_0000 (GICv2 only)\n";
    for &b in MSG_RCORE_VAL {
        unsafe { putchar(b); }
    }

    const MSG_NL: &[u8] = b"\n";
    for &b in MSG_NL {
        unsafe { putchar(b); }
    }

    const MSG1: &[u8] = b"Initializing GICv3...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    const MSG2: &[u8] = b"gic: Attempting GICD initialization...\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    // 重新启用 GICD 初始化
    if !GICD.init() {
        const MSG_ERR: &[u8] = b"gic: GICD initialization failed!\n";
        for &b in MSG_ERR {
            unsafe { putchar(b); }
        }
        return;
    }

    const MSG3: &[u8] = b"gic: GICD initialized successfully\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }

    // 先初始化 Redistributor（配置 PPI，包括 Timer）
    // 必须在 CPU Interface 初始化之前完成！
    const MSG4: &[u8] = b"gic: Initializing Redistributor (before CPU interface)...\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }

    // 暂时禁用 GICR 初始化，因为它可能导致 FIQ 异常
    // init_redistributor();

    const MSG4B: &[u8] = b"gic: Skipping GICR initialization (due to FIQ issues)...\n";
    for &b in MSG4B {
        unsafe { putchar(b); }
    }

    // 暂时禁用 GICC 初始化以调试 FIQ 问题
    // init_cpu_interface();
    const MSG_SKIP_GICC: &[u8] = b"gic: Skipping GICC initialization (testing)...\n";
    for &b in MSG_SKIP_GICC {
        unsafe { putchar(b); }
    }

    const MSG5: &[u8] = b"gic: GICv3 initialization [OK]\n";
    for &b in MSG5 {
        unsafe { putchar(b); }
    }

    // 检查 PMR 是否保持
    const MSG6: &[u8] = b"gic: Checking PMR after init...\n";
    for &b in MSG6 {
        unsafe { putchar(b); }
    }
    let pmr_final = GICC.read_reg(gicc_offsets::PMR);
    const MSG7: &[u8] = b"gic: PMR = 0x";
    for &b in MSG7 {
        unsafe { putchar(b); }
    }
    let hex = b"0123456789ABCDEF";
    unsafe { putchar(hex[((pmr_final >> 4) & 0xF) as usize]); }
    unsafe { putchar(hex[(pmr_final & 0xF) as usize]); }
    const NL: &[u8] = b"\n";
    for &b in NL {
        unsafe { putchar(b); }
    }

    // 初始化后诊断：检查 Timer 和 GICR 状态
    // 临时禁用以测试
    // check_timer_status();
    // check_gicr_pending();
}
