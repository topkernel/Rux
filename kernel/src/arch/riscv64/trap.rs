//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 异常处理
//!
//! 处理各种异常和中断

use core::arch::asm;
use crate::println;

#[cfg(feature = "riscv64")]
use riscv::register::{sie};

// 包含 trap.S 汇编代码 (使用 64 位指令)
#[cfg(feature = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

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

pub fn init() {
    println!("trap: Initializing RISC-V trap handling...");

    unsafe {
        // 直接设置 stvec 指向 trap_entry（不使用 trampoline）
        // 使用 Direct mode (MODE=0)，所以地址必须 4 字节对齐
        extern "C" {
            fn trap_entry();
        }

        let stvec_value = trap_entry as *const () as u64;

        asm!(
            "csrw stvec, {}",
            in(reg) stvec_value,
            options(nostack)
        );

        // 验证 stvec
        let stvec: u64;
        asm!("csrr {}, stvec", out(reg) stvec);

        println!("trap: Exception vector table installed at stvec = {:#x}", stvec);

        // 初始化 sscratch (trap 栈)
        // sscratch 用于在 trap 处理时快速切换到内核栈
        use crate::arch::riscv64::mm;
        let trap_stack = mm::get_trap_stack();
        let trap_stack_top = trap_stack;

        asm!(
            "csrw sscratch, {}",
            in(reg) trap_stack_top,
            options(nomem, nostack)
        );

        println!("trap: sscratch initialized to trap stack = {:#x}", trap_stack_top);

        // 初始化 tp 寄存器 (thread pointer) 为 trap 栈指针
        // tp 寄存器用于 trap 入口/出口的栈切换
        // 使用 tp 而不是 sscratch 可以避免从用户模式进入时的问题
        asm!(
            "mv tp, {}",
            in(reg) trap_stack_top,
            options(nomem, nostack)
        );

        println!("trap: tp register initialized to trap stack = {:#x}", trap_stack_top);
    }

    println!("trap: RISC-V trap handling [OK]");
}

pub fn init_syscall() {
    // RISC-V 使用 ecall 指令，在异常处理中分发
    // 这里只需要确认异常处理已经初始化
    println!("trap: System call handling initialized");
}

pub fn enable_timer_interrupt() {
    unsafe {
        // 设置 sie 寄存器的 STIE 位 (bit 5)
        sie::set_stimer();

        // 设置 sstatus 寄存器的 SIE 位 (bit 1) 来全局使能中断
        asm!(
            "csrsi sstatus, 2",  // 设置 bit 1 (SIE = 0x2)
            options(nomem, nostack)
        );
    }
}

pub fn disable_timer_interrupt() {
    unsafe {
        // 清除 sie 寄存器的 STIE 位 (bit 5)
        sie::clear_stimer();
    }
}

pub fn enable_external_interrupt() {
    unsafe {
        // 设置 sie 寄存器的 SEIE 位 (bit 9) - 外部中断使能
        sie::set_sext();

        // 设置 sstatus 寄存器的 SIE 位 (bit 1) 来全局使能中断
        asm!(
            "csrsi sstatus, 2",  // 设置 bit 1 (SIE = 0x2)
            options(nomem, nostack)
        );
    }
}

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
            ExceptionCause::SupervisorTimerInterrupt => {
                // Timer interrupt - 时钟中断处理
                //
                // 对应 Linux 内核的时钟中断处理流程：
                // 1. tick_sched_timer() - 更新 jiffies
                // 2. scheduler_tick() - 更新时间片，设置 need_resched
                // 3. schedule() - 如果 need_resched，触发调度

                // 1. 调用时钟中断处理函数（更新 jiffies 等）
                crate::drivers::timer::timer_interrupt_handler();

                // 2. 调度器 tick - 更新进程时间片，检查是否需要重新调度
                // 对应 Linux 内核的 scheduler_tick() (kernel/sched/fair.c)
                #[cfg(feature = "riscv64")]
                crate::sched::scheduler_tick();

                // 3. 设置下一次定时器中断
                crate::drivers::timer::set_next_trigger();

                // 4. 如果设置了 need_resched 标志，触发进程调度
                // 对应 Linux 内核的 pre_schedule() -> schedule()
                #[cfg(feature = "riscv64")]
                if crate::sched::need_resched() {
                    crate::sched::schedule();
                }
            }
            ExceptionCause::SupervisorSoftwareInterrupt => {
                // 软件中断（用于 IPI）
                let hart_id = crate::arch::riscv64::smp::cpu_id();

                // 清除软件中断
                unsafe {
                    // 清除 sip.SSIP 位
                    asm!("csrc sip, 0x2", options(nomem, nostack));
                }

                // 处理 IPI
                crate::arch::ipi::handle_software_ipi(hart_id as usize);
            }
            ExceptionCause::SupervisorExternalInterrupt => {
                // 外部中断 - 由 PLIC 处理
                let hart_id = crate::arch::riscv64::smp::cpu_id();

                // Claim 中断（获取最高优先级的待处理中断 ID）
                if let Some(irq) = crate::drivers::intc::plic::claim(hart_id as usize) {
                    match irq {
                        1 => {
                            // UART 中断（ns16550a）
                            // TODO: 实现 UART 输入处理
                            // crate::println!("IRQ: UART interrupt (IRQ 1)");
                        }
                        10..=13 => {
                            // IPI 中断（核间中断）
                            crate::arch::ipi::handle_ipi(irq, hart_id as usize);
                        }
                        _ => {
                            // 未知中断
                            crate::println!("IRQ: Unknown interrupt {} (hart {})", irq, hart_id);
                        }
                    }

                    // Complete 中断（通知 PLIC 处理完成）
                    crate::drivers::intc::plic::complete(hart_id as usize, irq);
                }
            }
            ExceptionCause::EnvironmentCallFromMMode => {
                crate::println!("trap: Machine-mode ECALL");
            }
            ExceptionCause::EnvironmentCallFromSMode => {
                crate::println!("trap: Supervisor-mode ECALL");
            }
            ExceptionCause::EnvironmentCallFromUMode => {
                // 来自用户模式的系统调用
                {
                    use crate::console::putchar;
                    const MSG1: &[u8] = b"[TRAP:ECALL]\n";
                    for &b in MSG1 { putchar(b); }
                }

                // 将 TrapFrame 转换为 SyscallFrame 并调用 syscall_handler
                use crate::arch::riscv64::syscall::SyscallFrame;

                let mut syscall_frame = SyscallFrame {
                    a0: (*frame).x10,
                    a1: (*frame).x11,
                    a2: (*frame).x12,
                    a3: (*frame).x13,
                    a4: (*frame).x14,
                    a5: (*frame).x15,
                    a6: (*frame).x16,
                    a7: (*frame).x17,
                    t0: (*frame).x5,
                    t1: (*frame).x6,
                    t2: 0,
                    t3: 0,
                    t4: 0,
                    t5: 0,
                    t6: 0,
                    s0: 0,
                    s1: 0,
                    s2: 0,
                    s3: 0,
                    s4: 0,
                    s5: 0,
                    s6: 0,
                    s7: 0,
                    s8: 0,
                    s9: 0,
                    s10: 0,
                    s11: 0,
                    ra: 0,
                    sp: (*frame).x1,
                    gp: 0,
                    tp: 0,
                    pc: (*frame).sepc,
                    status: (*frame).sstatus,
                };

                // 调用系统调用处理器
                crate::arch::riscv64::syscall::syscall_handler(&mut syscall_frame);

                // 将结果写回 TrapFrame
                (*frame).x10 = syscall_frame.a0;
                (*frame).x11 = syscall_frame.a1;
                (*frame).x12 = syscall_frame.a2;
                (*frame).x13 = syscall_frame.a3;
                (*frame).x14 = syscall_frame.a4;
                (*frame).x15 = syscall_frame.a5;

                // 跳过 ecall 指令
                (*frame).sepc += 4;

                {
                    use crate::console::putchar;
                    const MSG2: &[u8] = b"[TRAP:RETURN:";
                    for &b in MSG2 { putchar(b); }

                    // 打印 sepc 的完整 32 位十六进制值
                    let sepc = (*frame).sepc;
                    let hex_chars = b"0123456789ABCDEF";
                    for i in (0..32).step_by(4).rev() {
                        let digit = ((sepc >> i) & 0xF) as usize;
                        putchar(hex_chars[digit]);
                    }

                    const MSG3: &[u8] = b"]\n";
                    for &b in MSG3 { putchar(b); }
                }
            }
            ExceptionCause::IllegalInstruction => {
                let is_user = (*frame).sstatus & 0x100 != 0;  // 检查 SPP 位
                crate::println!("trap: Illegal instruction at sepc={:#x} ({}mode)",
                    (*frame).sepc, if is_user { "user " } else { "kernel " });
            }
            ExceptionCause::InstructionAccessFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Instruction access fault at sepc={:#x} ({}mode)",
                    (*frame).sepc, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::LoadAccessFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Load access fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::StoreAMOAccessFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Store/AMO access fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::InstructionPageFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Instruction page fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::LoadPageFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Load page fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::StorePageFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Store page fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            _ => {
                crate::println!("trap: Unknown exception: scause={:#x}, sepc={:#x}, stval={:#x}",
                    scause, (*frame).sepc, stval);
            }
        }
    }
}
