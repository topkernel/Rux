//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
use core::fmt;
use core::arch::asm;
use spin::Mutex;

// UART 基础地址 - 根据架构选择
#[cfg(feature = "aarch64")]
const UART0_BASE: usize = 0x0900_0000;  // ARM PL011 UART

#[cfg(feature = "riscv64")]
const UART0_BASE: usize = 0x1000_0000;  // RISC-V ns16550a UART

/// 简单的 UART 驱动 - 专用于 QEMU virt
pub struct Uart {
    base: usize,
}

impl Uart {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    /// 写入单个字符到 UART（使用内联汇编确保正确性）
    #[inline(never)]
    pub fn putc(&self, c: u8) {
        #[cfg(feature = "aarch64")]
        unsafe {
            let addr = self.base + 0x00;  // UART_DR offset
            asm!(
                "str w1, [x0]",
                in("x0") addr,
                in("w1") c as u32,
                options(nostack, nomem)
            );
        }

        #[cfg(feature = "riscv64")]
        unsafe {
            let addr = self.base;  // UART_THR offset ( Transmit Holding Register)
            asm!(
                "sb t1, 0(a0)",
                in("a0") addr,
                in("t1") c,
                options(nostack, nomem)
            );
        }
    }
}

/// 全局 UART 控制台（使用自旋锁保护，SMP 安全）
static UART: Mutex<Uart> = Mutex::new(Uart::new(UART0_BASE));

/// 初始化控制台（QEMU virt 不需要初始化）
pub fn init() {
    // QEMU virt 的 UART 已经预初始化，无需操作
}

/// 写入单个字符（SMP 安全）
pub fn putchar(c: u8) {
    // 使用自旋锁保护 UART 访问
    let uart = UART.lock();
    uart.putc(c);
}

/// 写入字符串（SMP 安全，只获取一次锁）
pub fn puts(s: &str) {
    let uart = UART.lock();
    for b in s.bytes() {
        uart.putc(b);
    }
}

/// 获取 UART 锁（用于批量输出）
///
/// 返回锁守卫，调用者可以在其作用域内安全地调用 putc
pub fn lock() -> spin::MutexGuard<'static, Uart> {
    UART.lock()
}

/// 中断安全的字符输出（不获取锁，直接写入UART）
///
/// 仅在中断处理程序中使用
/// 注意：如果多个CPU同时调用此函数，输出可能交错
pub fn putchar_no_lock(c: u8) {
    let uart = Uart::new(UART0_BASE);
    uart.putc(c);
}

/// 中断安全的字符串输出（不获取锁）
///
/// 仅在中断处理程序中使用
pub fn puts_no_lock(s: &str) {
    let uart = Uart::new(UART0_BASE);
    for b in s.bytes() {
        uart.putc(b);
    }
}

/// 读取单个字符（未实现）
pub fn getchar() -> Option<u8> {
    None
}

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
        Ok(())
    }
}
