use core::fmt;
use core::arch::asm;

// PL011 UART 基础地址
const UART0_BASE: usize = 0x0900_0000;

/// 简单的 UART 驱动 - 专用于 QEMU virt
pub struct Uart {
    base: usize,
}

impl Uart {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    /// 写入单个字符到 UART（使用内联汇编确保正确性）
    #[inline]
    pub fn putc(&self, c: u8) {
        unsafe {
            // 使用内联汇编直接写入，和工作版本完全一致
            let addr = self.base + 0x00;  // UART_DR offset
            asm!(
                "str w1, [x0]",
                in("x0") addr,
                in("w1") c as u32,
                options(nostack, nomem)
            );
        }
    }
}

static UART: Uart = Uart::new(UART0_BASE);

/// 初始化控制台（QEMU virt 不需要初始化）
pub fn init() {
    // QEMU virt 的 UART 已经预初始化，无需操作
}

/// 写入单个字符
pub fn putchar(c: u8) {
    UART.putc(c);
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
