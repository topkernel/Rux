//! RISC-V 异常处理
//!
//! 处理各种异常和中断

use core::arch::asm;
use crate::println;
use crate::debug_println;

#[cfg(feature = "riscv64")]
use riscv::register::{sie, sstatus, sip};

// 包含 trap.S 汇编代码 (使用 64 位指令)
#[cfg(feature = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

/// 异常上下文（保存在栈上）
#[repr(C)]
pub struct TrapFrame {
    // 通用寄存器
    x1: u64,
    x5: u64,
    x6: u64,
    x7: u64,
    x10: u64,
    x11: u64,
    x12: u64,
    x13: u64,
    x14: u64,
    x15: u64,
    x16: u64,
    x17: u64,
    x18: u64,
    x19: u64,
    x20: u64,
    x21: u64,
    x22: u64,
    x23: u64,
    x24: u64,
    x25: u64,
    x26: u64,
    x27: u64,
    x28: u64,
    x29: u64,
    x30: u64,
    x31: u64,
    // CSR 寄存器
    sstatus: u64,
    sepc: u64,
    stval: u64,
}

/// 异常原因
#[derive(Debug, Clone, Copy)]
pub enum ExceptionCause {
    InstructionAddressMisaligned,
    InstructionAccessFault,
    IllegalInstruction,
    Breakpoint,
    LoadAddressMisaligned,
    LoadAccessFault,
    StoreAMOAddressMisaligned,
    StoreAMOAccessFault,
    EnvironmentCallFromUMode,
    EnvironmentCallFromSMode,
    EnvironmentCallFromMMode,
    InstructionPageFault,
    LoadPageFault,
    StorePageFault,
    // Supervisor 中断
    SupervisorSoftwareInterrupt,
    SupervisorTimerInterrupt,
    SupervisorExternalInterrupt,
    Unknown(u64),
}

impl ExceptionCause {
    fn from_scause(scause: u64) -> Self {
        let interrupt = (scause >> 63) & 1;
        let exception_code = scause & 0x7FFFFFFFFFFFFFFF;

        if interrupt == 1 {
            // 中断
            match exception_code {
                1 => ExceptionCause::SupervisorSoftwareInterrupt,
                5 => ExceptionCause::SupervisorTimerInterrupt,
                9 => ExceptionCause::SupervisorExternalInterrupt,
                _ => ExceptionCause::Unknown(scause),
            }
        } else {
            match exception_code {
                0 => ExceptionCause::InstructionAddressMisaligned,
                1 => ExceptionCause::InstructionAccessFault,
                2 => ExceptionCause::IllegalInstruction,
                3 => ExceptionCause::Breakpoint,
                4 => ExceptionCause::LoadAddressMisaligned,
                5 => ExceptionCause::LoadAccessFault,
                6 => ExceptionCause::StoreAMOAddressMisaligned,
                7 => ExceptionCause::StoreAMOAccessFault,
                8 => ExceptionCause::EnvironmentCallFromUMode,
                9 => ExceptionCause::EnvironmentCallFromSMode,
                11 => ExceptionCause::EnvironmentCallFromMMode,
                12 => ExceptionCause::InstructionPageFault,
                13 => ExceptionCause::LoadPageFault,
                15 => ExceptionCause::StorePageFault,
                _ => ExceptionCause::Unknown(scause),
            }
        }
    }
}

/// 初始化异常处理
pub fn init() {
    println!("trap: Initializing RISC-V trap handling...");

    unsafe {
        // 设置 stvec (S-mode 异常向量表基址)
        // 使用 Direct mode (MODE=0)，所以地址必须 4 字节对齐
        // Note: trap_entry is defined in trap.S
        let trap_entry_addr: u64;
        asm!(
            "la {}, trap_entry",
            out(reg) trap_entry_addr,
            options(nostack)
        );

        // 确保 trap_entry_addr 的最后两位是 0 (Direct mode)
        let stvec_value = trap_entry_addr & !0x3;  // 清除最后两位

        asm!(
            "csrw stvec, {}",
            in(reg) stvec_value,
            options(nostack)
        );

        // 验证 stvec
        let stvec: u64;
        asm!("csrr {}, stvec", out(reg) stvec);

        println!("trap: Exception vector table installed at stvec = {:#x}", stvec);
    }

    println!("trap: RISC-V trap handling [OK]");
}

/// 初始化系统调用（兼容 main.rs 的调用）
pub fn init_syscall() {
    // RISC-V 使用 ecall 指令，在异常处理中分发
    // 这里只需要确认异常处理已经初始化
    println!("trap: System call handling initialized");
}

/// 使能 timer interrupt
pub fn enable_timer_interrupt() {
    unsafe {
        // 设置 sie 寄存器的 STIE 位 (bit 5)
        sie::set_stimer();

        // 设置 sstatus 寄存器的 SIE 位 (bit 1) 来全局使能中断
        // 使用内联汇编直接设置 sstatus.SIE 位
        asm!(
            "csrsi sstatus, 2",  // 设置 bit 1 (SIE = 0x2)
            options(nomem, nostack)
        );

        // 读取并打印 CSR 值
        let sie = sie::read();
        let sstatus = sstatus::read();
        let sip = sip::read();
        println!("trap: Timer interrupt enabled (sie = {:#x}, sstatus = {:#x}, sip = {:#x})",
               sie.bits(), sstatus.bits(), sip.bits());
    }
}

/// 异常处理入口（从汇编调用）
#[no_mangle]
pub extern "C" fn trap_handler(frame: *mut TrapFrame) {
    // 调试输出：trap_handler 被调用了
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"trap_handler: entered\n";
        for &b in MSG {
            putchar(b);
        }
    }

    unsafe {
        // 读取 scause (异常原因)
        let scause: u64;
        asm!("csrr {}, scause", out(reg) scause);

        // 读取 stval (异常相关信息)
        let stval: u64;
        asm!("csrr {}, stval", out(reg) stval);

        let exception = ExceptionCause::from_scause(scause);

        match exception {
            ExceptionCause::SupervisorTimerInterrupt => {
                crate::println!("trap: Timer interrupt!");
                // 对于 timer interrupt，需要手动增加 sepc 以跳过 WFI 指令
                // WFI 指令是 4 字节 (0x10000073)
                (*frame).sepc += 4;
                // 设置下一次定时器中断
                crate::drivers::timer::set_next_trigger();
            }
            ExceptionCause::SupervisorSoftwareInterrupt => {
                crate::println!("trap: Software interrupt");
            }
            ExceptionCause::SupervisorExternalInterrupt => {
                crate::println!("trap: External interrupt");
            }
            ExceptionCause::EnvironmentCallFromMMode => {
                // 机器模式系统调用（暂时不实现）
                crate::println!("trap: Machine-mode ECALL not supported");
            }
            ExceptionCause::EnvironmentCallFromSMode => {
                // 监管者模式系统调用（暂时不实现）
                crate::println!("trap: Supervisor-mode ECALL not supported");
            }
            ExceptionCause::EnvironmentCallFromUMode => {
                // 用户模式系统调用（暂时不实现）
                crate::println!("trap: User-mode ECALL not supported");
            }
            ExceptionCause::IllegalInstruction => {
                crate::println!("trap: Illegal instruction at mepc={:#x}", (*frame).sepc);
            }
            ExceptionCause::InstructionAccessFault => {
                crate::println!("trap: Instruction access fault at mepc={:#x}", (*frame).sepc);
            }
            ExceptionCause::LoadAccessFault => {
                crate::println!("trap: Load access fault at mepc={:#x}, addr={:#x}", (*frame).sepc, stval);
            }
            ExceptionCause::StoreAMOAccessFault => {
                crate::println!("trap: Store/AMO access fault at mepc={:#x}, addr={:#x}", (*frame).sepc, stval);
            }
            _ => {
                crate::println!("trap: Unknown exception: scause={:#x}, mepc={:#x}, stval={:#x}",
                    scause, (*frame).sepc, stval);
            }
        }
    }
}
