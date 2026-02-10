//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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
        use crate::console::putchar;

        unsafe {
            // 读取计数器频率
            let freq: u64;
            asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));

            const MSG: &[u8] = b"timer: Freq = ";
            for &b in MSG {
                putchar(b);
            }
            // 简单打印频率值
            let mut f = freq;
            if f > 100000 { f = 100000; }
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if f == 0 {
                buf[pos] = b'0';
            } else {
                while f > 0 {
                    buf[pos] = b'0' + ((f % 10) as u8);
                    f /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const MSG_HZ: &[u8] = b" Hz\n";
            for &b in MSG_HZ {
                putchar(b);
            }

            // 打印 ticks_per_ms
            const MSG_TICKS: &[u8] = b"timer: ticks_per_ms = ";
            for &b in MSG_TICKS {
                putchar(b);
            }
            let mut f = self.ticks_per_ms;
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if f == 0 {
                buf[pos] = b'0';
            } else {
                while f > 0 {
                    buf[pos] = b'0' + ((f % 10) as u8);
                    f /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            const MSG_NL: &[u8] = b"\n";
            for &b in MSG_NL {
                putchar(b);
            }

            // 设置定时器为周期模式
            // bit 0: ENABLE - 使能定时器
            // bit 1: IMASK - 中断屏蔽 (0=不屏蔽，1=屏蔽)
            const MSG_CTL: &[u8] = b"timer: Enabling timer (CTL=0x1)\n";
            for &b in MSG_CTL {
                putchar(b);
            }

            // 设置控制寄存器：ENABLE=1, IMASK=0
            asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));

            // 读取并打印控制寄存器以确认
            let ctl_val: u64;
            asm!("mrs {}, cntp_ctl_el0", out(reg) ctl_val, options(nomem, nostack));
            const MSG_CTL_READ: &[u8] = b"timer: CNT_CTL_EL0 = 0x";
            for &b in MSG_CTL_READ {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            putchar(hex[((ctl_val >> 4) & 0xF) as usize]);
            putchar(hex[(ctl_val & 0xF) as usize]);
            for &b in MSG_NL {
                putchar(b);
            }

            // 设置定时器值（按照 rCore 的方法）
            // TVAL 是一个倒计时值，写入后开始倒计时
            let tval = self.ticks_per_ms * TIMER_PERIOD_MS;
            asm!("msr cntp_tval_el0, {}", in(reg) tval, options(nomem, nostack));

            // 读取并打印 TVAL 以确认
            let tval_read: u64;
            asm!("mrs {}, cntp_tval_el0", out(reg) tval_read, options(nomem, nostack));
            const MSG_TVAL: &[u8] = b"timer: CNT_TVAL_EL0 = ";
            for &b in MSG_TVAL {
                putchar(b);
            }
            let mut f = tval_read;
            let mut buf = [0u8; 20];
            let mut pos = 19;
            if f == 0 {
                buf[pos] = b'0';
            } else {
                while f > 0 {
                    buf[pos] = b'0' + ((f % 10) as u8);
                    f /= 10;
                    if pos > 0 { pos -= 1; }
                }
            }
            for &b in &buf[pos..] {
                putchar(b);
            }
            for &b in MSG_NL {
                putchar(b);
            }

            const MSG_OK: &[u8] = b"timer: Timer initialized [OK]\n";
            for &b in MSG_OK {
                putchar(b);
            }
        }
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
