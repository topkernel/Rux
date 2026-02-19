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

/// 读取单个字符（非阻塞）
/// 如果有数据可用则返回 Some(c)，否则返回 None
pub fn getchar() -> Option<u8> {
    #[cfg(feature = "riscv64")]
    {
        const UART_BASE: usize = 0x1000_0000;
        const UART_LSR: usize = 5;  // Line Status Register

        unsafe {
            // 检查 LSR 的 bit 0 (DR - Data Ready)
            let lsr_addr = UART_BASE + UART_LSR;
            let lsr: u8;
            asm!(
                "lb t0, 0(a0)",
                in("a0") lsr_addr,
                out("t0") lsr,
                options(nostack)
            );

            if lsr & 1 == 1 {
                // 有数据可用，从 RBR 读取
                let c: u8;
                asm!(
                    "lb t0, 0(a0)",
                    in("a0") UART_BASE,
                    out("t0") c,
                    options(nostack)
                );
                Some(c)
            } else {
                None
            }
        }
    }

    #[cfg(feature = "aarch64")]
    {
        // TODO: 实现 aarch64 的 getchar
        None
    }

    #[cfg(not(any(feature = "riscv64", feature = "aarch64")))]
    {
        None
    }
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
