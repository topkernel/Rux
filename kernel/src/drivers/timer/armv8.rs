//! ARMv8 架构定时器驱动
//!
//! ARMv8架构包含系统计数器和定时器

use crate::println;
use core::arch::asm;

/// CNTP_CTL (EL1物理定时器控制寄存器)
const CNTP_CTL_EL0: usize = 0;
/// CNTP_CVAL (EL1物理定时器比较值寄存器)
const CNTP_CVAL_EL0: usize = 2;
/// CNTP_TVAL (EL1物理定时器定时器值寄存器)
const CNTP_TVAL_EL0: usize = 3;
/// CNTFRQ (计数器频率寄存器)
const CNTFRQ_EL0: usize = 0;
/// CNTVCT (虚拟计数器寄存器)
const CNTVCT_EL0: usize = 2;

/// 定时器频率 (Hz)
const TIMER_FREQ: u64 = 100_000; // QEMU virt机器默认频率

/// 定时器周期 (ms)
const TIMER_PERIOD_MS: u64 = 100;

/// ARMv8 系统定时器
pub struct Armv8Timer {
    ticks_per_ms: u64,
}

impl Armv8Timer {
    pub const fn new() -> Self {
        Self {
            ticks_per_ms: TIMER_FREQ / 1000,
        }
    }

    /// 初始化定时器
    pub fn init(&self) {
        unsafe {
            // 读取计数器频率
            let freq: u64;
            asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
            // println!("Timer frequency: {} Hz", freq);

            // 设置定时器为周期模式
            let ctl: u64;
            asm!("mrs {}, cntp_ctl_el0", out(reg) ctl, options(nomem, nostack));
            // bit 0: ENABLE - 使能定时器
            // bit 1: IMASK - 中断屏蔽 (1=屏蔽)
            // bit 2: ISTATUS - 定时器条件状态
            asm!("msr cntp_ctl_el0, {}", in(reg) ctl | 0x1, options(nomem, nostack));

            // 设置比较值
            let now = self.read_counter();
            let expire = now + (self.ticks_per_ms * TIMER_PERIOD_MS);
            asm!("msr cntp_cval_el0, {}", in(reg) expire, options(nomem, nostack));

            // 设置定时器值（比较值 - 当前值）
            let tval = expire - now;
            asm!("msr cntp_tval_el0, {}", in(reg) tval, options(nomem, nostack));
        }

        // println!("ARMv8 timer initialized: {}ms period", TIMER_PERIOD_MS);
    }

    /// 读取当前计数器值
    #[inline]
    pub fn read_counter(&self) -> u64 {
        unsafe {
            let cnt: u64;
            asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack));
            cnt
        }
    }

    /// 重启定时器
    #[inline]
    pub fn restart(&self) {
        unsafe {
            let now = self.read_counter();
            let expire = now + (self.ticks_per_ms * TIMER_PERIOD_MS);
            asm!("msr cntp_cval_el0, {}", in(reg) expire, options(nomem, nostack));
        }
    }
}

/// 全局定时器实例
static TIMER: Armv8Timer = Armv8Timer::new();

/// 初始化系统定时器
pub fn init() {
    TIMER.init();
}

/// 读取当前计数器值
pub fn read_counter() -> u64 {
    TIMER.read_counter()
}

/// 重启定时器
pub fn restart_timer() {
    TIMER.restart();
}
