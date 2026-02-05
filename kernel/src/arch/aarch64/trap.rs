//! ARMv8 异常处理框架
//!
//! 提供异常分发、上下文保存和恢复、中断处理等功能

use crate::println;
use crate::debug_println;
use core::arch::asm;
use core::fmt;

/// 异常类型
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExceptionType {
    // 同步异常
    SyncSP0 = 0x0,
    SyncSPx = 0x4,
    SyncEL0 = 0x8,
    SyncEL032 = 0xC,

    // IRQ中断
    IrqSP0 = 0x1,
    IrqSPx = 0x5,
    IrqEL0 = 0x9,
    IrqEL032 = 0xD,

    // FIQ快速中断
    FiqSP0 = 0x2,
    FiqSPx = 0x6,
    FiqEL0 = 0xA,
    FiqEL032 = 0xE,

    // SError系统错误
    SErrorSP0 = 0x3,
    SErrorSPx = 0x7,
    SErrorEL0 = 0xB,
    SErrorEL032 = 0xF,
}

impl ExceptionType {
    pub fn from(val: u64) -> Self {
        match val & 0xF {
            0x0 => ExceptionType::SyncSP0,
            0x1 => ExceptionType::IrqSP0,
            0x2 => ExceptionType::FiqSP0,
            0x3 => ExceptionType::SErrorSP0,
            0x4 => ExceptionType::SyncSPx,
            0x5 => ExceptionType::IrqSPx,
            0x6 => ExceptionType::FiqSPx,
            0x7 => ExceptionType::SErrorSPx,
            0x8 => ExceptionType::SyncEL0,
            0x9 => ExceptionType::IrqEL0,
            0xA => ExceptionType::FiqEL0,
            0xB => ExceptionType::SErrorEL0,
            0xC => ExceptionType::SyncEL032,
            0xD => ExceptionType::IrqEL032,
            0xE => ExceptionType::FiqEL032,
            0xF => ExceptionType::SErrorEL032,
            _ => panic!("Invalid exception type: {}", val),
        }
    }

    pub fn is_irq(&self) -> bool {
        matches!(self, ExceptionType::IrqSP0 | ExceptionType::IrqSPx |
                      ExceptionType::IrqEL0 | ExceptionType::IrqEL032)
    }

    pub fn is_sync(&self) -> bool {
        matches!(self, ExceptionType::SyncSP0 | ExceptionType::SyncSPx |
                      ExceptionType::SyncEL0 | ExceptionType::SyncEL032)
    }
}

impl fmt::Display for ExceptionType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ExceptionType::SyncSP0 => write!(f, "Sync (SP0)"),
            ExceptionType::SyncSPx => write!(f, "Sync (SPx)"),
            ExceptionType::SyncEL0 => write!(f, "Sync (EL0)"),
            ExceptionType::SyncEL032 => write!(f, "Sync (EL0, 32-bit)"),
            ExceptionType::IrqSP0 => write!(f, "IRQ (SP0)"),
            ExceptionType::IrqSPx => write!(f, "IRQ (SPx)"),
            ExceptionType::IrqEL0 => write!(f, "IRQ (EL0)"),
            ExceptionType::IrqEL032 => write!(f, "IRQ (EL0, 32-bit)"),
            ExceptionType::FiqSP0 => write!(f, "FIQ (SP0)"),
            ExceptionType::FiqSPx => write!(f, "FIQ (SPx)"),
            ExceptionType::FiqEL0 => write!(f, "FIQ (EL0)"),
            ExceptionType::FiqEL032 => write!(f, "FIQ (EL0, 32-bit)"),
            ExceptionType::SErrorSP0 => write!(f, "SError (SP0)"),
            ExceptionType::SErrorSPx => write!(f, "SError (SPx)"),
            ExceptionType::SErrorEL0 => write!(f, "SError (EL0)"),
            ExceptionType::SErrorEL032 => write!(f, "SError (EL0, 32-bit)"),
        }
    }
}

/// 异常上下文
#[repr(C)]
pub struct ExceptionContext {
    /// 异常类型
    pub exc_type: ExceptionType,
    /// 保存的程序计数器
    pub elr_el1: u64,
    /// 保存的程序状态
    pub spsr_el1: u64,
    /// 通用寄存器
    pub x: [u64; 31],
}

impl ExceptionContext {
    pub const fn zeroed() -> Self {
        Self {
            exc_type: ExceptionType::SyncSPx,
            elr_el1: 0,
            spsr_el1: 0,
            x: [0; 31],
        }
    }
}

/// 异常原因码
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ESR_EL1_EC {
    Unknown = 0x0,
    TrappedWFIWFE = 0x1,
    TrappedMCRRMRRC = 0x2,
    TrappedMRRC = 0x3,
    TrappedSystem = 0x4,
    TrappedSVE = 0x5,
    TrappedERP = 0x6,
    TrappedFP = 0x7,
    TrappedSimdFp = 0x8,
    TrappedVM = 0xC,
    TrappedEE = 0xD,
    InstructionAbortFromLowerEL = 0x20,
    InstructionAbortFromCurrentEL = 0x21,
    PCAlignmentFault = 0x22,
    DataAbortFromLowerEL = 0x24,
    DataAbortFromCurrentEL = 0x25,
    SPAlignmentFault = 0x26,
    TrappedFpException32 = 0x28,
    TrappedFpException64 = 0x2C,
    SErrorInterrupt = 0x2F,
    BreakpointFromLowerEL = 0x30,
    BreakpointFromCurrentEL = 0x31,
    SoftwareStepFromLowerEL = 0x32,
    SoftwareStepFromCurrentEL = 0x33,
    WatchpointFromLowerEL = 0x34,
    WatchpointFromCurrentEL = 0x35,
    BKPT = 0x38,
    Brk = 0x3C,
}

impl ESR_EL1_EC {
    pub fn from(esr: u64) -> Self {
        let ec = ((esr >> 26) & 0x3F) as u32;
        unsafe { core::mem::transmute(ec) }
    }
}

/// 异常处理结果
pub enum ExceptionResult {
    /// 继续执行
    Continue,
    /// 调度器需要运行
    Schedule,
}

/// 异常处理函数类型
type ExceptionHandler = fn(&mut ExceptionContext) -> ExceptionResult;

/// 异常处理器注册表
static mut EXCEPTION_HANDLERS: [Option<ExceptionHandler>; 16] = [None; 16];

/// 初始化异常处理框架
pub fn init() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"trap: Initializing exception handling...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    unsafe {
        // 设置VBAR_EL1指向异常向量表
        asm!("msr vbar_el1, {}", in(reg) vector_table, options(nomem, nostack));
        isb();
    }

    const MSG2: &[u8] = b"trap: Exception vector table installed at VBAR_EL1\n";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }

    const MSG3: &[u8] = b"trap: Exception handling [OK]\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }
}

/// 初始化系统调用支持
pub fn init_syscall() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"syscall: Initializing system call support...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    unsafe {
        // ARMv8默认支持SVC指令，无需特殊配置
        // 系统调用通过 SVC #0 指令触发
        // 在异常向量表的同步异常处理程序中分发

        const MSG2: &[u8] = b"syscall: SVC instruction support enabled\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    const MSG3: &[u8] = b"syscall: System call dispatcher ready\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }

    const MSG4: &[u8] = b"syscall: System call support [OK]\n";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
}

/// 获取VBAR_EL1寄存器的值（用于调试）
pub fn get_vbar_el1() -> u64 {
    unsafe {
        let vbar: u64;
        asm!("mrs {}, vbar_el1", out(reg) vbar, options(nomem, nostack, pure));
        vbar
    }
}


/// 注册异常处理器
pub fn register_handler(exc_type: ExceptionType, handler: ExceptionHandler) {
    let idx = exc_type as u8 as usize;
    unsafe {
        EXCEPTION_HANDLERS[idx] = Some(handler);
    }
}

/// 异常处理入口（从汇编调用）
#[no_mangle]
pub extern "C" fn trap_handler(exc_type: u64, frame: *mut u8) {
    let exc_type = ExceptionType::from(exc_type);

    match exc_type {
        ExceptionType::IrqSPx | ExceptionType::IrqEL0 |
        ExceptionType::IrqEL032 => {
            // IRQ中断处理
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"IRQ exception triggered\n";
                for &b in MSG {
                    putchar(b);
                }
            }
            handle_irq();
        }
        ExceptionType::FiqSPx | ExceptionType::FiqEL0 | ExceptionType::FiqEL032 => {
            // FIQ中断处理 - Timer 可能被路由为 FIQ
            unsafe {
                use crate::console::putchar;

                // 屏蔽中断
                let saved_daif = crate::drivers::intc::mask_irq();

                // 读取中断
                let iar = crate::drivers::intc::ack_interrupt();

                // 检查是否为 spurious
                if iar == 1023 {
                    // Spurious - 恢复并返回
                    crate::drivers::intc::restore_irq(saved_daif);
                    return;
                }

                // 处理 Timer 中断 (IRQ 30)
                if iar == 30 {
                    const MSG: &[u8] = b"FIQ: Timer interrupt (IRQ 30)\n";
                    for &b in MSG {
                        putchar(b);
                    }
                    crate::drivers::timer::restart_timer();
                } else {
                    const MSG: &[u8] = b"FIQ: Unknown IRQ ";
                    for &b in MSG {
                        putchar(b);
                    }
                    let hex = b"0123456789ABCDEF";
                    putchar(hex[((iar >> 4) & 0xF) as usize]);
                    putchar(hex[(iar & 0xF) as usize]);
                    const NL: &[u8] = b"\n";
                    for &b in NL {
                        putchar(b);
                    }
                }

                // EOI
                crate::drivers::intc::eoi_interrupt(iar);

                // 恢复中断
                crate::drivers::intc::restore_irq(saved_daif);
            }
            return;
        }
        ExceptionType::SyncSPx | ExceptionType::SyncEL0 | ExceptionType::SyncEL032 => {
            // 同步异常处理 - 检查是否为系统调用
            unsafe {
                let esr: u64;
                asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack));

                let ec = ((esr >> 26) & 0x3F) as u8;

                // 简单调试输出：打印异常类型
                use crate::console::putchar;
                const MSG_EC: &[u8] = b"[Sync EC:";
                for &b in MSG_EC {
                    putchar(b);
                }
                let hex_chars = b"0123456789ABCDEF";
                putchar(hex_chars[((ec >> 4) & 0xF) as usize]);
                putchar(hex_chars[(ec & 0xF) as usize]);
                putchar(b']');
                putchar(b'\n');

                match ec {
                    0x15 => {
                        // SVC指令 - 系统调用
                        // 调试：打印栈帧中的多个寄存器值和ELR
                        let frame_u64 = frame as *const u64;
                        let x0_val = unsafe { *frame_u64.add(0) };
                        let x8_val = unsafe { *frame_u64.add(8) };

                        // Read ELR from offset 31 (248/8 = 31)
                        let elr_val = unsafe { *frame_u64.add(31) };

                        const MSG_SVC: &[u8] = b"[x0=";
                        for &b in MSG_SVC {
                            putchar(b);
                        }
                        let hex_chars = b"0123456789ABCDEF";
                        // Print x0 as 2 hex digits
                        putchar(hex_chars[((x0_val >> 4) & 0xF) as usize]);
                        putchar(hex_chars[(x0_val & 0xF) as usize]);

                        const MSG_X8: &[u8] = b" x8=";
                        for &b in MSG_X8 {
                            putchar(b);
                        }
                        putchar(hex_chars[((x8_val >> 4) & 0xF) as usize]);
                        putchar(hex_chars[(x8_val & 0xF) as usize]);

                        const MSG_ELR: &[u8] = b" ELR=";
                        for &b in MSG_ELR {
                            putchar(b);
                        }
                        // Print ELR as 8 hex digits
                        for i in (0..64).step_by(4).rev().take(8) {
                            putchar(hex_chars[((elr_val >> i) & 0xF) as usize]);
                        }

                        const MSG_END: &[u8] = b"]\n";
                        for &b in MSG_END {
                            putchar(b);
                        }

                        let syscall_frame = unsafe { &mut *(frame as *mut crate::arch::aarch64::syscall::SyscallFrame) };
                        crate::arch::aarch64::syscall::syscall_handler(syscall_frame);
                    }
                    0x3C => {
                        // BRK指令 - 用于调试
                        debug_println!("BRK instruction hit");
                        // Advance ELR_EL1 past the BRK instruction (4 bytes) to avoid infinite loop
                        let mut elr: u64;
                        asm!("mrs {}, elr_el1", out(reg) elr, options(nomem, nostack));
                        debug_println!("ELR before advance");
                        elr += 4;  // BRK is a 32-bit instruction
                        asm!("msr elr_el1, {}", in(reg) elr, options(nomem, nostack));
                        debug_println!("ELR after advance");
                    }
                    _ => {
                        // 其他同步异常 - 打印更详细的调试信息
                        const MSG_UNKNOWN: &[u8] = b"[Unknown sync EC=";
                        for &b in MSG_UNKNOWN {
                            putchar(b);
                        }
                        let hex_chars = b"0123456789ABCDEF";
                        // Print EC as 2 hex digits
                        putchar(hex_chars[((ec >> 4) & 0xF) as usize]);
                        putchar(hex_chars[(ec & 0xF) as usize]);
                        const MSG_END: &[u8] = b"]\n";
                        for &b in MSG_END {
                            putchar(b);
                        }
                        handle_sync_from_frame(frame);
                    }
                }
            }
        }
        _ => {
            // Use putchar to debug exception type since println is broken
            use crate::console::putchar;
            const MSG: &[u8] = b"Unhandled exception type: ";
            for &b in MSG {
                unsafe { putchar(b); }
            }
            // Print exception type as raw number
            let et_val = exc_type as u8;
            let hex_chars = b"0123456789ABCDEF";
            unsafe {
                putchar(b'0');
                putchar(b'x');
                putchar(hex_chars[((et_val >> 4) & 0xF) as usize]);
                putchar(hex_chars[(et_val & 0xF) as usize]);
                putchar(b'\n');

                // Print ESR_EL1 details
                const MSG2: &[u8] = b"ESR_EL1: ";
                for &b in MSG2 {
                    putchar(b);
                }

                let esr: u64;
                asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack));
                for i in (0..64).step_by(4).rev() {
                    putchar(hex_chars[((esr >> i) & 0xF) as usize]);
                }
                putchar(b'\n');

                // Print EC (Exception Class)
                const MSG5: &[u8] = b"EC: ";
                for &b in MSG5 {
                    putchar(b);
                }
                let ec = ((esr >> 26) & 0x3F) as u8;
                putchar(hex_chars[((ec >> 4) & 0xF) as usize]);
                putchar(hex_chars[(ec & 0xF) as usize]);
                putchar(b'\n');

                // Print FAR_EL1 (fault address)
                const MSG3: &[u8] = b"FAR_EL1: ";
                for &b in MSG3 {
                    putchar(b);
                }

                let far: u64;
                asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack));
                for i in (0..64).step_by(4).rev() {
                    putchar(hex_chars[((far >> i) & 0xF) as usize]);
                }
                putchar(b'\n');

                // Print ELR_EL1 (exception return address)
                const MSG4: &[u8] = b"ELR_EL1: ";
                for &b in MSG4 {
                    putchar(b);
                }

                let elr: u64;
                asm!("mrs {}, elr_el1", out(reg) elr, options(nomem, nostack));
                for i in (0..64).step_by(4).rev() {
                    putchar(hex_chars[((elr >> i) & 0xF) as usize]);
                }
                putchar(b'\n');
            }

            loop {
                unsafe { asm!("wfi", options(nomem, nostack)); }
            }
        }
    }

    // 异常处理完成后，检查并交付待处理的信号
    // 这对应 Linux 内核的 exit_to_usermode() 中的信号检查
    crate::signal::check_and_deliver_signals();
}


/// 处理IRQ中断
fn handle_irq() {
    // 屏蔽中断，防止递归
    let saved_daif = crate::drivers::intc::mask_irq();

    // 确认中断并获取中断号
    let irq = crate::drivers::intc::ack_interrupt();

    // 调试输出：打印中断号
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"IRQ: ";
        for &b in MSG {
            putchar(b);
        }
        let hex_chars = b"0123456789ABCDEF";
        putchar(hex_chars[((irq >> 4) & 0xF) as usize]);
        putchar(hex_chars[(irq & 0xF) as usize]);
        const MSG_NL: &[u8] = b"\n";
        for &b in MSG_NL {
            putchar(b);
        }
    }

    // 检查是否为spurious interrupt (IRQ 1023)
    if irq == 1023 {
        // Spurious interrupt：恢复中断并返回
        crate::drivers::intc::restore_irq(saved_daif);
        return;
    }

    // 处理具体的中断
    match irq {
        // SGI (Software Generated Interrupt) 范围: 0-15
        // 用于 CPU 间中断 (IPI)
        0..=15 => {
            // IPI (Inter-Processor Interrupt)
            crate::arch::aarch64::ipi::handle_ipi(irq);
        }
        30 => {
            // ARMv8物理定时器中断
            // 使用底层 putchar 打印消息（避免 println! 兼容性问题）
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Timer interrupt (IRQ 30) - restarting\n";
                for &b in MSG {
                    putchar(b);
                }
            }
            crate::drivers::timer::restart_timer();
        }
        _ => {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Unhandled IRQ: ";
                for &b in MSG {
                    putchar(b);
                }
                let hex_chars = b"0123456789ABCDEF";
                putchar(hex_chars[((irq >> 4) & 0xF) as usize]);
                putchar(hex_chars[(irq & 0xF) as usize]);
                const MSG_NL: &[u8] = b"\n";
                for &b in MSG_NL {
                    putchar(b);
                }
            }
        }
    }

    // 结束中断处理
    crate::drivers::intc::eoi_interrupt(irq);

    // 恢复中断状态
    crate::drivers::intc::restore_irq(saved_daif);
}

/// 从栈帧处理同步异常
unsafe fn handle_sync_from_frame(frame: *mut u8) {
    // 从栈帧中读取寄存器
    let elr = *((frame as *const u64).offset(31));  // ELR_EL1 at offset 248/8
    let spsr = *((frame as *const u64).offset(30)); // SPSR_EL1 at offset 244/8
    let esr = *((frame as *const u64).offset(30));  // ESR_EL1 at offset 244/8

    let far: u64;
    asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack));

    let ec = ESR_EL1_EC::from(esr);
    let iss = esr & 0x1FFFFFF;

    println!("Sync Exception:");
    println!("  EC={:?}", ec);
    println!("  ISS=0x{:x}", iss);
    println!("  FAR=0x{:x}", far);
    println!("  ELR=0x{:x}", elr);
    println!("  SPSR=0x{:x}", spsr);

    loop {
        asm!("wfi", options(nomem, nostack));
    }
}

/// 外部函数声明
extern "C" {
    /// 异常向量表（定义在trap.S中）
    fn vector_table();
    /// 异常向量表结束
    fn vector_table_end();
}

/// 指令同步屏障
#[inline]
fn isb() {
    unsafe {
        asm!("isb", options(nomem, nostack));
    }
}
