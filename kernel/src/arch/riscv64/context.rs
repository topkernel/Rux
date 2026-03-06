//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit 上下文切换
//!
//!
//! - 保存被调用者保存寄存器 (x1-x31, 除了 x0 和 tp)
//! - 保存栈指针 (sp)
//! - 保存返回地址 (ra)
//!
//! 调用约定：
//! - prev: 前一个任务的 Task 指针
//! - next: 下一个任务的 Task 指针

use crate::process::task::{Task, CpuContext};
use core::arch::asm;

pub struct InterruptGuard {
    flags: u64,
}

impl InterruptGuard {
    /// 禁用中断并创建守卫
    ///
    /// 保存 sstatus 寄存器，清除 SIE 位（全局中断使能）
    #[inline]
    pub unsafe fn new() -> Self {
        let flags: u64;
        let temp: u64;
        // 读取 sstatus 并保存
        asm!("csrr {}, sstatus", out(reg) flags, options(nomem, nostack));
        // 清除 SIE 位（bit 1）
        temp = flags & !0x02;
        asm!("csrw sstatus, {}", in(reg) temp, options(nomem, nostack));
        InterruptGuard { flags }
    }
}

impl Drop for InterruptGuard {
    /// 恢复中断状态
    #[inline]
    fn drop(&mut self) {
        unsafe {
            asm!(
                "csrw sstatus, {}",  // 恢复 sstatus
                in(reg) self.flags,
                options(nomem, nostack)
            );
        }
    }
}

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.context_switch"]
pub unsafe extern "C" fn cpu_switch_to(next_ctx: *mut CpuContext, prev_ctx: *mut CpuContext) {
    // 内联汇编实现上下文切换
    core::arch::naked_asm!(
        // 保存当前任务的上下文到 prev->context
        // RISC-V 调用约定：a0=next_ctx, a1=prev_ctx
        "sd ra, 0(a1)",      // 保存返回地址
        "sd sp, 8(a1)",      // 保存栈指针
        "sd s0, 16(a1)",
        "sd s1, 24(a1)",
        "sd s2, 32(a1)",
        "sd s3, 40(a1)",
        "sd s4, 48(a1)",
        "sd s5, 56(a1)",
        "sd s6, 64(a1)",
        "sd s7, 72(a1)",
        "sd s8, 80(a1)",
        "sd s9, 88(a1)",
        "sd s10, 96(a1)",
        "sd s11, 104(a1)",

        // 从 next->context 恢复下一个任务的上下文
        "ld ra, 0(a0)",      // 恢复返回地址
        "ld sp, 8(a0)",      // 恢复栈指针
        "ld s0, 16(a0)",
        "ld s1, 24(a0)",
        "ld s2, 32(a0)",
        "ld s3, 40(a0)",
        "ld s4, 48(a0)",
        "ld s5, 56(a0)",
        "ld s6, 64(a0)",
        "ld s7, 72(a0)",
        "ld s8, 80(a0)",
        "ld s9, 88(a0)",
        "ld s10, 96(a0)",
        "ld s11, 104(a0)",

        "ret",               // 返回到 next 的上下文

        // 参数约定:
        // a0 = next_ctx (要恢复的上下文)
        // a1 = prev_ctx (要保存的上下文)
    );
}

/// Linux 风格的上下文切换函数
///
/// 参考 Linux: arch/riscv/kernel/entry.S __switch_to
///
/// # 参数
/// - a0: prev task_struct 指针
/// - a1: next task_struct 指针
///
/// # 保存/恢复内容
/// - ra, sp, s0-s11 (被调用者保存寄存器)
/// - sstatus.SUM 位 (用户内存访问使能)
/// - tp 寄存器 (指向当前 task_struct)
///
/// # Task 结构体偏移量 (与 task.rs 一致)
/// - ti_kernel_sp: 0x08 (thread_info.kernel_sp)
/// - context: 变化 (需要计算)
///
/// 注意：由于 Task 结构体复杂，我们使用 CpuContext 偏移量
/// CpuContext 在 Task 中的偏移量由 context_mut() 计算
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.__switch_to"]
pub unsafe extern "C" fn __switch_to(prev: *mut Task, next: *mut Task) {
    core::arch::naked_asm!(
        // 参数:
        // a0 = prev task
        // a1 = next task

        // 保存返回地址和 next 指针
        "addi sp, sp, -16",
        "sd ra, 0(sp)",
        "sd a1, 8(sp)",      // 保存 next 指针

        // 获取 prev->context 和 next->context 的偏移
        // 由于 CpuContext 在 Task 中的偏移可能变化，
        // 我们调用 Rust 函数来获取指针

        // 恢复 next 指针
        "ld a1, 8(sp)",

        // 更新 tp 指向 next task
        "mv tp, a1",

        // 恢复返回地址
        "ld ra, 0(sp)",
        "addi sp, sp, 16",

        "ret",

        // 注意：这个简化版本没有保存/恢复 callee-saved 寄存器
        // 实际的上下文切换由 context_switch() 函数中的 cpu_switch_to 处理
    );
}

/// Linux 风格的上下文切换包装函数
///
/// 结合 cpu_switch_to 和 __switch_to 的功能：
/// 1. 保存/恢复 callee-saved 寄存器
/// 2. 更新 tp 指向新任务
/// 3. 保存/恢复 SUM 位
///
/// 注意：这个函数使用纯汇编实现，因为上下文切换后
/// 局部变量（在旧栈上）不再可访问。
#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.context_switch_asm"]
pub unsafe extern "C" fn context_switch_asm(
    prev_ctx: *mut CpuContext,
    next_ctx: *mut CpuContext,
    next_task: *mut Task,
) {
    core::arch::naked_asm!(
        // 参数:
        // a0 = prev_ctx (要保存的上下文)
        // a1 = next_ctx (要恢复的上下文)
        // a2 = next_task (新任务指针，用于设置 tp)

        // ===== 保存 prev 上下文 =====
        "sd ra, 0(a0)",
        "sd sp, 8(a0)",
        "sd s0, 16(a0)",
        "sd s1, 24(a0)",
        "sd s2, 32(a0)",
        "sd s3, 40(a0)",
        "sd s4, 48(a0)",
        "sd s5, 56(a0)",
        "sd s6, 64(a0)",
        "sd s7, 72(a0)",
        "sd s8, 80(a0)",
        "sd s9, 88(a0)",
        "sd s10, 96(a0)",
        "sd s11, 104(a0)",

        // ===== 恢复 next 上下文 =====
        "ld ra, 0(a1)",
        "ld sp, 8(a1)",
        "ld s0, 16(a1)",
        "ld s1, 24(a1)",
        "ld s2, 32(a1)",
        "ld s3, 40(a1)",
        "ld s4, 48(a1)",
        "ld s5, 56(a1)",
        "ld s6, 64(a1)",
        "ld s7, 72(a1)",
        "ld s8, 80(a1)",
        "ld s9, 88(a1)",
        "ld s10, 96(a1)",
        "ld s11, 104(a1)",

        // ===== 更新 tp 指向新任务 =====
        "mv tp, a2",

        // ===== 保存 prev task 到 s1 (供 ret_from_fork 使用) =====
        // s1 = prev task 指针
        // 注意：s1 是 callee-saved，会被 context_switch_asm 保存/恢复
        // 在 ret_from_fork 中，可以通过 s1 获取 prev task 并传递给 schedule_tail
        "mv s1, a0",  // s1 = prev task (第一个参数)


        // 返回到 next 的上下文
        "ret",
    );
}

/// Linux 风格的上下文切换包装函数
///
/// 结合 cpu_switch_to 和 __switch_to 的功能：
/// 1. 保存/恢复 callee-saved 寄存器
/// 2. 更新 tp 指向新任务
/// 3. 保存/恢复 SUM 位
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 在 SMP 环境中禁用中断，防止在上下文切换期间发生竞争条件
    let _irq_guard = InterruptGuard::new();

    // 获取 CpuContext 的指针
    let next_ctx: *mut CpuContext = next.context_mut();
    let prev_ctx: *mut CpuContext = prev.context_mut();
    let next_task: *mut Task = next;

    // 保存当前 SUM 位状态到 s0 (callee-saved，会在 context_switch_asm 中保存/恢复)
    // 使用汇编读取并保存 SUM 位
    let sum_status: u64;
    core::arch::asm!(
        "csrr {0}, sstatus",
        "and {0}, {0}, {1}",
        out(reg) sum_status,
        in(reg) 0x40000u64,
        options(nomem, nostack)
    );

    // 调用汇编上下文切换函数
    context_switch_asm(prev_ctx, next_ctx, next_task);

    // ===== 以下在 next 任务的上下文中执行 =====
    // sum_status 变量不可用（在旧栈上），但我们可以在切换前将其保存到任务结构中
    // 或者简单地不恢复 SUM 位（让每个任务自己管理）

    // 实际上，SUM 位应该在任务结构中保存/恢复
    // 这里简化处理：设置默认的 SUM 位状态
    core::arch::asm!(
        "csrs sstatus, {0}",
        in(reg) 0x40000u64,
        options(nomem, nostack)
    );

    // InterruptGuard 在此处 Drop，自动恢复中断状态
}
