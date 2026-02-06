//! RISC-V 异常处理
//!
//! 处理各种异常和中断

use core::arch::asm;
use crate::println;
use crate::debug_println;

// 包含汇编代码（使用 S-mode CSR）
core::arch::global_asm!(
    r#"
.text
.align 2
.global trap_entry

trap_entry:
    // 保存调用者寄存器
    addi sp, sp, -256

    sw x1, 0(sp)
    sw x5, 4(sp)
    sw x6, 8(sp)
    sw x7, 12(sp)
    sw x10, 16(sp)
    sw x11, 20(sp)
    sw x12, 24(sp)
    sw x13, 28(sp)
    sw x14, 32(sp)
    sw x15, 36(sp)
    sw x16, 40(sp)
    sw x17, 44(sp)
    sw x18, 48(sp)
    sw x19, 52(sp)
    sw x20, 56(sp)
    sw x21, 60(sp)
    sw x22, 64(sp)
    sw x23, 68(sp)
    sw x24, 72(sp)
    sw x25, 76(sp)
    sw x26, 80(sp)
    sw x27, 84(sp)
    sw x28, 88(sp)
    sw x29, 92(sp)
    sw x30, 96(sp)
    sw x31, 100(sp)

    // 保存 sstatus, sepc, stval (S-mode CSR)
    csrrs x5, sstatus, x5
    csrrs x6, sepc, x6
    csrrs x7, stval, x7
    sw x5, 104(sp)
    sw x6, 108(sp)
    sw x7, 112(sp)

    // 调用 Rust trap 处理函数
    addi x10, sp, 0
    // 使用 tail 跳转 (直接跳转，不返回)
    tail trap_handler

    // 恢复寄存器 (trap_handler 返回后的代码)
    lw x5, 104(sp)
    lw x6, 108(sp)
    lw x7, 112(sp)
    csrrw x5, sstatus, x5
    csrrw x6, sepc, x6
    csrrw x7, stval, x7

    lw x1, 0(sp)
    lw x5, 4(sp)
    lw x6, 8(sp)
    lw x7, 12(sp)
    lw x10, 16(sp)
    lw x11, 20(sp)
    lw x12, 24(sp)
    lw x13, 28(sp)
    lw x14, 32(sp)
    lw x15, 36(sp)
    lw x16, 40(sp)
    lw x17, 44(sp)
    lw x18, 48(sp)
    lw x19, 52(sp)
    lw x20, 56(sp)
    lw x21, 60(sp)
    lw x22, 64(sp)
    lw x23, 68(sp)
    lw x24, 72(sp)
    lw x25, 76(sp)
    lw x26, 80(sp)
    lw x27, 84(sp)
    lw x28, 88(sp)
    lw x29, 92(sp)
    lw x30, 96(sp)
    lw x31, 100(sp)

    addi sp, sp, 256

    // 返回异常处理 (S-mode return)
    sret
"#
);

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
    Unknown(u64),
}

impl ExceptionCause {
    fn from_scause(scause: u64) -> Self {
        let interrupt = (scause >> 63) & 1;
        let exception_code = scause & 0x7FFFFFFFFFFFFFFF;

        if interrupt == 1 {
            // 中断 - 暂不处理
            ExceptionCause::Unknown(scause)
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
        // Note: trap_entry is defined in global_asm above
        // 使用 la 指令加载 trap_entry 的地址
        let trap_entry_addr: u64;
        asm!(
            "la {}, trap_entry",
            out(reg) trap_entry_addr,
            options(nostack)
        );

        asm!(
            "csrw stvec, {}",
            in(reg) trap_entry_addr,
            options(nostack)
        );

        // 验证 stvec
        let stvec: u64;
        asm!("csrrw {}, stvec, zero", out(reg) stvec);

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

/// 异常处理入口（从汇编调用）
#[no_mangle]
pub extern "C" fn trap_handler(frame: *mut TrapFrame) {
    unsafe {
        // 读取 scause (异常原因)
        let scause: u64;
        asm!("csrr {}, scause", out(reg) scause);

        // 读取 stval (异常相关信息)
        let stval: u64;
        asm!("csrr {}, stval", out(reg) stval);

        let exception = ExceptionCause::from_scause(scause);

        match exception {
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
