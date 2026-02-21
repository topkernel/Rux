//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 异常处理
//!
//! 处理各种异常和中断

use core::arch::asm;

#[cfg(feature = "riscv64")]
use riscv::register::{sie};

// 包含 trap.S 汇编代码 (使用 64 位指令)
#[cfg(feature = "riscv64")]
core::arch::global_asm!(include_str!("trap.S"));

/// RISC-V Trap 栈帧
///
/// 对应 trap.S 中保存的寄存器布局
/// 注意：
///   - 用户 tp 保存在 TrapFrame 之前 16 字节处 (sp+0)
///   - 原始 sp 保存在 TrapFrame 之前 8 字节处 (sp+8)
///   - TrapFrame 从 sp+16 开始（ra）
#[repr(C)]
pub struct TrapFrame {
    // 调用者保存寄存器（trap.S 从 sp+16 开始保存）
    // TrapFrame 指针 = sp + 16
    pub ra: u64,   // x1  - 返回地址 (frame+0 = sp+16)
    pub t0: u64,   // x5  - 临时寄存器 (frame+8 = sp+24)
    pub t1: u64,   // x6  - 临时寄存器 (frame+16 = sp+32)
    pub t2: u64,   // x7  - 临时寄存器 (frame+24 = sp+40)
    pub a0: u64,   // x10 - 参数/返回值 (frame+32 = sp+48)
    pub a1: u64,   // x11 - 参数 (frame+40 = sp+56)
    pub a2: u64,   // x12 - 参数 (frame+48 = sp+64)
    pub a3: u64,   // x13 - 参数 (frame+56 = sp+72)
    pub a4: u64,   // x14 - 参数 (frame+64 = sp+80)
    pub a5: u64,   // x15 - 参数 (frame+72 = sp+88)
    pub a6: u64,   // x16 - 参数 (frame+80 = sp+96)
    pub a7: u64,   // x17 - 系统调用号 (frame+88 = sp+104)
    pub t3: u64,   // x28 - 临时寄存器 (frame+96 = sp+112)
    pub t4: u64,   // x29 - 临时寄存器 (frame+104 = sp+120)
    pub t5: u64,   // x30 - 临时寄存器 (frame+112 = sp+128)
    pub t6: u64,   // x31 - 临时寄存器 (frame+120 = sp+136)
    // 被调用者保存寄存器（s2-s11）
    pub s2: u64,   // x18 - 保存寄存器 (frame+128 = sp+144)
    pub s3: u64,   // x19 - 保存寄存器 (frame+136 = sp+152)
    pub s4: u64,   // x20 - 保存寄存器 (frame+144 = sp+160)
    pub s5: u64,   // x21 - 保存寄存器 (frame+152 = sp+168)
    pub s6: u64,   // x22 - 保存寄存器 (frame+160 = sp+176)
    pub s7: u64,   // x23 - 保存寄存器 (frame+168 = sp+184)
    pub s8: u64,   // x24 - 保存寄存器 (frame+176 = sp+192)
    pub s9: u64,   // x25 - 保存寄存器 (frame+184 = sp+200)
    pub s10: u64,  // x26 - 保存寄存器 (frame+192 = sp+208)
    pub s11: u64,  // x27 - 保存寄存器 (frame+200 = sp+216)
    // 填充: frame+208 到 frame+224 (trap.S 没有保存这些位置)
    // 需要 2 个 u64 (16 字节) 使 sstatus 到达 frame+224 = sp+240
    pub _pad: [u64; 2], // frame+208, 216
    // CSR 寄存器
    pub sstatus: u64,  // frame+224 = sp+240
    pub sepc: u64,     // frame+232 = sp+248
    pub stval: u64,    // frame+240 = sp+256
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
        let _stvec: u64;
        asm!("csrr {}, stvec", out(reg) _stvec);

        // 初始化 sscratch 为 hart_id + 1
        // 这对于 trap.S 正确处理第一个用户态 trap 至关重要
        // trap.S 期望 sscratch = hart_id + 1，这样：
        //   csrrw tp, sscratch, tp  交换后 tp = hart_id + 1
        //   addi tp, tp, -1         tp = hart_id
        // 注意：+1 是为了避免 hart_id = 0 时的歧义（0 也可能是未初始化）
        let hart_id: u64;
        asm!(
            "mv {}, tp",
            out(reg) hart_id,
            options(nomem, nostack, pure)
        );
        let sscratch_value = hart_id + 1;

        asm!(
            "csrw sscratch, {}",
            in(reg) sscratch_value,
            options(nomem, nostack)
        )
    }
}

pub fn init_syscall() {
    // RISC-V 使用 ecall 指令，在异常处理中分发
    // 这里只需要确认异常处理已经初始化
}

pub fn enable_timer_interrupt() {
    unsafe {
        // 设置 sie 寄存器的 STIE 位 (bit 5) - 定时器中断使能
        // 2^5 = 0x20 = 32
        asm!(
            "li t0, 32",           // 加载 STIE 位的值 (2^5)
            "csrw sie, t0",         // 设置 sie 寄存器
            options(nomem, nostack)
        );

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
        // 2^9 = 0x200 = 512 (注意: csrsi 只支持 5-bit 立即数，需要用 li 加载)
        asm!(
            "li t0, 512",          // 加载 SEIE 位的值 (2^9)
            "csrw sie, t0",         // 设置 sie 寄存器
            options(nomem, nostack)
        );

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

/// 当前 CPU 的 TrapFrame 指针（用于 fork）
/// 在 trap 入口时设置，在 trap 出口时清除
static CURRENT_TRAP_FRAME: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// 获取当前的 TrapFrame 指针
/// 用于 fork 复制父进程的 trap 状态
pub fn current_trap_frame() -> *const TrapFrame {
    CURRENT_TRAP_FRAME.load(core::sync::atomic::Ordering::Relaxed) as *const TrapFrame
}

#[no_mangle]
pub extern "C" fn trap_handler(frame: *mut TrapFrame) {
    unsafe {
        // 保存当前 TrapFrame 指针（用于 fork）
        CURRENT_TRAP_FRAME.store(frame as u64, core::sync::atomic::Ordering::Relaxed);

        // 读取 scause (异常原因)
        let scause: u64;
        asm!("csrr {}, scause", out(reg) scause);

        // 读取 stval (异常相关信息)
        let stval: u64;
        asm!("csrr {}, stval", out(reg) stval);

        let exception = ExceptionCause::from_scause(scause);

        // 调试输出（可选）
        // if !matches!(exception, ExceptionCause::SupervisorTimerInterrupt) {
        //     crate::println!("TRAP: {:?} sepc={:#x} stval={:#x}", exception, (*frame).sepc, stval);
        // }

        match exception {
            ExceptionCause::SupervisorTimerInterrupt => {
                // Timer interrupt - 时钟中断处理
                //
                // 1. tick_sched_timer() - 更新 jiffies
                // 2. scheduler_tick() - 更新时间片，设置 need_resched
                // 3. schedule() - 如果 need_resched，触发调度

                // 1. 调用时钟中断处理函数（更新 jiffies 等）
                crate::drivers::timer::timer_interrupt_handler();

                // 2. 调度器 tick - 更新进程时间片，检查是否需要重新调度
                #[cfg(feature = "riscv64")]
                crate::sched::scheduler_tick();

                // 3. 设置下一次定时器中断
                crate::drivers::timer::set_next_trigger();

                // 4. 如果设置了 need_resched 标志，触发进程调度
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
                        1..=8 => {
                            // VirtIO MMIO 设备中断（VirtIO slot 0-7）
                            // QEMU RISC-V virt: IRQ 1-8 对应 VirtIO 设备槽位 0-7
                            crate::drivers::virtio::interrupt_handler();
                        }
                        32..=127 => {
                            // VirtIO PCI 设备中断
                            // QEMU RISC-V virt: IRQ 32+ 对应 PCI 设备
                            // IRQ = 32 + (PCI slot * 4) + (INT_PIN - 1)
                            crate::println!("trap: PCI VirtIO interrupt detected (IRQ {})", irq);
                            crate::drivers::virtio::interrupt_handler_pci(irq as usize);
                        }
                        10 => {
                            // UART 中断（ns16550a）- QEMU RISC-V virt 使用 IRQ 10
                            // TODO: 实现 UART 输入处理
                            // crate::println!("IRQ: UART interrupt (IRQ 10)");
                        }
                        11..=13 => {
                            // IPI 中断（核间中断）
                            crate::arch::ipi::handle_ipi(irq, hart_id as usize);
                        }
                        _ => {
                            // 未知中断 - 静默忽略
                        }
                    }

                    // Complete 中断（通知 PLIC 处理完成）
                    crate::drivers::intc::plic::complete(hart_id as usize, irq);
                }
            }
            ExceptionCause::EnvironmentCallFromMMode => {
                // Machine-mode ecall - 不应该发生
            }
            ExceptionCause::EnvironmentCallFromSMode => {
                // Supervisor-mode ecall - 不应该发生
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
                // SPP bit (8): 0 = from U-mode, 1 = from S-mode
                let is_user = (*frame).sstatus & 0x100 == 0;
                crate::println!("trap: Store/AMO access fault at sepc={:#x}, addr={:#x} ({}mode)",
                    (*frame).sepc, stval, if is_user { "user " } else { "kernel " });
                (*frame).sepc += 4; // 跳过错误指令
            }
            ExceptionCause::InstructionPageFault => {
                // SPP bit (8): 0 = from U-mode, 1 = from S-mode
                let is_user = (*frame).sstatus & 0x100 == 0;

                if is_user {
                    if let Some(current) = crate::sched::current() {
                        if let Some(addr_space) = current.address_space() {
                            use crate::arch::riscv64::mm::{
                                VirtAddr, handle_mm_fault, handle_cow_fault,
                                FaultFlags, MmFaultResult
                            };

                            let fault_addr = VirtAddr::new(stval);
                            let flags = FaultFlags::EXEC | FaultFlags::USER;

                            match handle_mm_fault(&addr_space, fault_addr, flags) {
                                MmFaultResult::Handled => {
                                    // 页面已映射，重新执行指令
                                    return;
                                }
                                MmFaultResult::CowPending => {
                                    // COW 不适用于执行错误
                                }
                                MmFaultResult::AlreadyMapped => {
                                    // 已映射但可能是权限问题
                                }
                                MmFaultResult::Segfault => {
                                    crate::println!("trap: Segfault at {:#x} (exec)", stval);
                                }
                                MmFaultResult::PermissionDenied => {
                                    crate::println!("trap: Permission denied at {:#x} (exec)", stval);
                                }
                                MmFaultResult::OutOfMemory => {
                                    crate::println!("trap: Out of memory at {:#x} (exec)", stval);
                                }
                            }
                        }
                    }
                }

                // 无法处理，跳过指令
                (*frame).sepc += 4;
            }
            ExceptionCause::LoadPageFault => {
                // SPP bit (8): 0 = from U-mode, 1 = from S-mode
                let is_user = (*frame).sstatus & 0x100 == 0;

                if is_user {
                    if let Some(current) = crate::sched::current() {
                        if let Some(addr_space) = current.address_space() {
                            use crate::arch::riscv64::mm::{
                                VirtAddr, handle_mm_fault,
                                FaultFlags, MmFaultResult
                            };

                            let fault_addr = VirtAddr::new(stval);
                            let flags = FaultFlags::READ | FaultFlags::USER;

                            match handle_mm_fault(&addr_space, fault_addr, flags) {
                                MmFaultResult::Handled => {
                                    // 页面已映射，重新执行指令
                                    return;
                                }
                                MmFaultResult::AlreadyMapped => {
                                    // 已映射，可能需要其他处理
                                }
                                MmFaultResult::Segfault => {
                                    crate::println!("trap: Segfault at {:#x} (read), sepc={:#x}", stval, (*frame).sepc);
                                }
                                MmFaultResult::PermissionDenied => {
                                    crate::println!("trap: Permission denied at {:#x} (read)", stval);
                                }
                                MmFaultResult::OutOfMemory => {
                                    crate::println!("trap: Out of memory at {:#x} (read)", stval);
                                }
                                MmFaultResult::CowPending => {
                                    // 读操作不需要 COW
                                }
                            }
                        }
                    }
                }

                // 无法处理，跳过指令
                (*frame).sepc += 4;
            }
            ExceptionCause::StorePageFault => {
                // SPP bit (8): 0 = from U-mode, 1 = from S-mode
                let is_user = (*frame).sstatus & 0x100 == 0;

                if is_user {
                    if let Some(current) = crate::sched::current() {
                        if let Some(addr_space) = current.address_space() {
                            use crate::arch::riscv64::mm::{
                                VirtAddr, handle_mm_fault, handle_cow_fault,
                                FaultFlags, MmFaultResult
                            };

                            let fault_addr = VirtAddr::new(stval);
                            let flags = FaultFlags::WRITE | FaultFlags::USER;

                            // 首先尝试 handle_mm_fault
                            match handle_mm_fault(&addr_space, fault_addr, flags) {
                                MmFaultResult::Handled => {
                                    // 页面已映射，重新执行指令
                                    return;
                                }
                                MmFaultResult::CowPending => {
                                    // COW 页面，尝试写时复制
                                    match handle_cow_fault(addr_space.root_ppn(), fault_addr) {
                                        Some(()) => {
                                            // COW 成功，重新执行指令
                                            return;
                                        }
                                        None => {
                                            crate::println!("trap: COW failed at {:#x}", stval);
                                        }
                                    }
                                }
                                MmFaultResult::AlreadyMapped => {
                                    // 已映射但可能不是 COW 页
                                    crate::println!("trap: Store fault on non-COW page at {:#x}", stval);
                                }
                                MmFaultResult::Segfault => {
                                    crate::println!("trap: Segfault at {:#x} (write)", stval);
                                }
                                MmFaultResult::PermissionDenied => {
                                    crate::println!("trap: Permission denied at {:#x} (write)", stval);
                                }
                                MmFaultResult::OutOfMemory => {
                                    crate::println!("trap: Out of memory at {:#x} (write)", stval);
                                }
                            }
                        }
                    }
                }

                // 无法处理，跳过指令
                (*frame).sepc += 4;
            }
            _ => {
                crate::println!("trap: Unknown exception: scause={:#x}, sepc={:#x}, stval={:#x}",
                    scause, (*frame).sepc, stval);
            }
        }

        // 清除当前 TrapFrame 指针
        CURRENT_TRAP_FRAME.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}
