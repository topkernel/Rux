//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64位架构支持
//!
//! 支持 RISC-V 64位 (RV64GC) 架构

pub mod boot;
pub mod pt_regs;
pub mod trap;
pub mod context;
pub mod cpu;
// syscall 模块已移动到 kernel/src/syscall/
pub mod mm;
pub mod smp;
pub mod ipi;
pub mod process;
pub mod thread;
pub mod uaccess;

use crate::println;
use core::arch::asm;

// 包含用户模式切换汇编代码
core::arch::global_asm!(include_str!("usermode_asm.S"));



pub fn arch_init() {
    init();
}

pub fn init() {
    println!("arch: Initializing RISC-V architecture...");

    // 设置异常向量表
    trap::init();

    // 禁用中断
    unsafe {
        // RISC-V: 清除 mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus &= !(1 << 3); // Clear MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Interrupts disabled in machine mode");
    }

    // 打印 CPU 信息
    print_cpu_info();

    println!("arch: Architecture initialization [DONE]");
}

fn print_cpu_info() {
    unsafe {
        // 读取 mhartid (硬件线程 ID)
        let mhartid: u64;
        asm!("csrrw {}, mhartid, zero", out(reg) mhartid);

        // 读取 mimpid (机器实现 ID)
        let mimpid: u64;
        asm!("csrrw {}, mimpid, zero", out(reg) mimpid);

        // 读取 marchid (架构 ID)
        let marchid: u64;
        asm!("csrrw {}, marchid, zero", out(reg) marchid);

        println!("arch: mhartid (HART ID) = {}", mhartid);
        println!("arch: mimpid (Impl ID) = {:#x}", mimpid);
        println!("arch: marchid (Arch ID) = {:#x}", marchid);
    }
}

pub fn enable_interrupts() {
    unsafe {
        // 设置 mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus |= 1 << 3; // Set MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Machine-mode interrupts enabled");
    }
}

/// 获取当前 CPU (hart) ID
///
/// Linux 兼容设计:
/// - 早期启动阶段: tp = hart_id (小数值)
/// - 调度器运行后: tp = task_struct 指针，hart_id 存储在 task_struct.ti_cpu
///
/// 通过检查 tp 的值范围来判断当前模式：
/// - 如果 tp < 0x1000，认为是 hart_id（早期启动）
/// - 否则认为是 task_struct 指针
///
/// 在 S-mode 下，我们无法访问 mhartid CSR（只能从 M-mode 访问）。
pub fn cpu_id() -> u64 {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // 检查 tp 是否为小数值（早期启动阶段的 hart_id）
        // 有效的 task_struct 指针应该在内核地址空间 (>= 0x80000000)
        if tp_value < 0x1000 {
            // 早期启动阶段，tp 直接存储 hart_id
            tp_value
        } else {
            // tp 指向 task_struct，从 ti_cpu 字段获取 hart_id
            // ti_cpu 在 Task 结构体中的偏移量是 0x18 (24 bytes)
            let ti_cpu_offset = 0x18;
            let cpu_ptr = (tp_value as usize + ti_cpu_offset) as *const core::sync::atomic::AtomicI32;
            (*cpu_ptr).load(core::sync::atomic::Ordering::Relaxed) as u64
        }
    }
}
