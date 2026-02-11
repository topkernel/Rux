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

/// RISC-V Trap 栈帧
///
/// 对应 trap.S 中保存的寄存器布局
/// 注意：原始 sp 保存在 TrapFrame 之前 8 字节处
#[repr(C)]
pub struct TrapFrame {
    // ABI 寄存器命名（与 RISC-V 标准一致）
    pub ra: u64,   // x1  - 返回地址
    pub t0: u64,   // x5  - 临时寄存器
    pub t1: u64,   // x6  - 临时寄存器
    pub t2: u64,   // x7  - 临时寄存器
    pub a0: u64,   // x10 - 参数/返回值
    pub a1: u64,   // x11 - 参数
    pub a2: u64,   // x12 - 参数
    pub a3: u64,   // x13 - 参数
    pub a4: u64,   // x14 - 参数
    pub a5: u64,   // x15 - 参数
    pub a6: u64,   // x16 - 参数
    pub a7: u64,   // x17 - 系统调用号
    pub s2: u64,   // x18 - 保存寄存器
    pub s3: u64,   // x19 - 保存寄存器
    pub s4: u64,   // x20 - 保存寄存器
    pub s5: u64,   // x21 - 保存寄存器
    pub s6: u64,   // x22 - 保存寄存器
    pub s7: u64,   // x23 - 保存寄存器
    pub s8: u64,   // x24 - 保存寄存器
    pub s9: u64,   // x25 - 保存寄存器
    pub s10: u64,  // x26 - 保存寄存器
    pub s11: u64,  // x27 - 保存寄存器
    pub t3: u64,   // x28 - 临时寄存器
    pub t4: u64,   // x29 - 临时寄存器
    pub t5: u64,   // x30 - 临时寄存器
    pub t6: u64,   // x31 - 临时寄存器
    // CSR 寄存器
    pub sstatus: u64,
    pub sepc: u64,
    pub stval: u64,
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

        // 注意：不覆盖 tp 寄存器
        // tp 寄存器在 boot.S 中被设置为 hart ID，用于 SMP 多核支持
        // sscratch 已经足够用于 trap 栈切换，不需要使用 tp
    }

    println!("trap: Exception handler installed");
    println!("trap: Trap stack initialized");
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

        // 设置 sstatus 寄存器：
        // - SIE 位 (bit 1) 来全局使能中断
        // - SUM 位 (bit 18) 允许 S-mode 访问用户内存
        asm!(
            "csrsi sstatus, 2",      // 设置 bit 1 (SIE = 0x2)
            "li t0, 262144",         // 加载 SUM 位的值 (2^18 = 0x40000)
            "csrs sstatus, t0",      // 设置 SUM 位
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

        // 设置 sstatus 寄存器：
        // - SIE 位 (bit 1) 来全局使能中断
        // - SUM 位 (bit 18) 允许 S-mode 访问用户内存
        asm!(
            "csrsi sstatus, 2",      // 设置 bit 1 (SIE = 0x2)
            "li t0, 262144",         // 加载 SUM 位的值 (2^18 = 0x40000)
            "csrs sstatus, t0",      // 设置 SUM 位
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
                // 将 TrapFrame 转换为 SyscallFrame 并调用 syscall_handler
                use crate::arch::riscv64::syscall::SyscallFrame;

                // 读取原始 sp（保存在 TrapFrame 之前 8 字节处）
                // trap.S: sd t0, 0(sp) 保存原始 sp
                let orig_sp = *((frame as *const u8).offset(-8) as *const u64);

                let mut syscall_frame = SyscallFrame {
                    a0: (*frame).a0,
                    a1: (*frame).a1,
                    a2: (*frame).a2,
                    a3: (*frame).a3,
                    a4: (*frame).a4,
                    a5: (*frame).a5,
                    a6: (*frame).a6,
                    a7: (*frame).a7,
                    t0: (*frame).t0,
                    t1: (*frame).t1,
                    t2: (*frame).t2,
                    t3: (*frame).t3,
                    t4: (*frame).t4,
                    t5: (*frame).t5,
                    t6: (*frame).t6,
                    s0: 0,
                    s1: 0,
                    s2: (*frame).s2,
                    s3: (*frame).s3,
                    s4: (*frame).s4,
                    s5: (*frame).s5,
                    s6: (*frame).s6,
                    s7: (*frame).s7,
                    s8: (*frame).s8,
                    s9: (*frame).s9,
                    s10: (*frame).s10,
                    s11: (*frame).s11,
                    ra: (*frame).ra,
                    sp: orig_sp,  // 正确的原始 sp
                    gp: 0,
                    tp: 0,
                    pc: (*frame).sepc,
                    status: (*frame).sstatus,
                };

                // 调用系统调用处理器
                crate::arch::riscv64::syscall::syscall_handler(&mut syscall_frame);

                // 将结果写回 TrapFrame
                (*frame).a0 = syscall_frame.a0;
                (*frame).a1 = syscall_frame.a1;
                (*frame).a2 = syscall_frame.a2;
                (*frame).a3 = syscall_frame.a3;
                (*frame).a4 = syscall_frame.a4;
                (*frame).a5 = syscall_frame.a5;

                // 跳过 ecall 指令
                (*frame).sepc += 4;
            }
            ExceptionCause::IllegalInstruction => {
                // 静默处理非法指令
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::InstructionAccessFault => {
                // 静默处理指令访问错误
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::LoadAccessFault => {
                // 静默处理加载访问错误
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::StoreAMOAccessFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;
                crate::println!("trap: Store/AMO access fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::InstructionPageFault => {
                // 静默处理指令页错误
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::LoadPageFault => {
                // 静默处理加载页错误
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::StorePageFault => {
                let is_user = (*frame).sstatus & 0x100 != 0;

                // 尝试处理 Copy-on-Write 写页错误
                if is_user {
                    if let Some(current) = crate::sched::current() {
                        if let Some(addr_space) = current.address_space() {
                            use crate::arch::riscv64::mm::{VirtAddr, handle_cow_fault, is_cow_page};
                            use crate::mm::page::VirtAddr as PageVirtAddr;

                            let fault_addr = VirtAddr::new(stval);

                            // 检查是否是 COW 页
                            if is_cow_page(addr_space.root_ppn(), fault_addr) {
                                // 尝试写时复制
                                match handle_cow_fault(addr_space.root_ppn(), fault_addr) {
                                    Some(()) => {
                                        // 不跳过指令，让进程重新执行
                                        return;
                                    }
                                    None => {
                                        // COW 失败，继续执行下面的代码
                                    }
                                }
                            }
                        }
                    }
                }

                // 静默处理存储页错误
                (*frame).sepc += 4; // 跳过错误指令
            }
            _ => {
                crate::println!("trap: Unknown exception: scause={:#x}, sepc={:#x}, stval={:#x}",
                    scause, (*frame).sepc, stval);
            }
        }
    }
}
