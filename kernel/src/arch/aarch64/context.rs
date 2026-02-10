//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ARM64 上下文切换
//!
//! 完全遵循 Linux 内核的上下文切换实现 (arch/arm64/kernel/process.c)
//!
//! Linux ARM64 的上下文切换使用 __switch_to() 函数：
//! - 保存被调用者保存寄存器 (x19-x28)
//! - 保存帧指针 (x29/fp)
//! - 保存栈指针 (sp)
//! - 保存程序计数器 (x30/lr)
//!
//! 调用约定：
//! - prev: 前一个任务的 Task 指针
//! - next: 下一个任务的 Task 指针

use crate::process::task::{Task, CpuContext};
use core::arch::asm;

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.context_switch"]
pub unsafe extern "C" fn cpu_switch_to(next_ctx: *mut CpuContext, prev_ctx: *mut CpuContext) {
    // 内联汇编实现上下文切换
    // 完全遵循 Linux 的 cpu_switch_to (arch/arm64/kernel/process.S)
    core::arch::naked_asm!(
        // 保存当前任务的上下文到 prev->context
        "stp x19, x20, [x1], #16",
        "stp x21, x22, [x1], #16",
        "stp x23, x24, [x1], #16",
        "stp x25, x26, [x1], #16",
        "stp x27, x28, [x1], #16",
        "stp x29, x30, [x1], #16",
        "str x18, [x1], #8",     // 保存 platform specific 寄存器
        "mov x8, sp",
        "str x8, [x1], #8",      // 保存 sp

        // 从 next->context 恢复下一个任务的上下文
        "ldp x19, x20, [x0], #16",
        "ldp x21, x22, [x0], #16",
        "ldp x23, x24, [x0], #16",
        "ldp x25, x26, [x0], #16",
        "ldp x27, x28, [x0], #16",
        "ldp x29, x30, [x0], #16",
        "ldr x18, [x0], #8",     // 恢复 platform specific 寄存器
        "ldr x8, [x0], #8",      // 获取新的 sp
        "mov sp, x8",            // 切换到新的栈

        "ret",                   // 返回到 next 的上下文

        // 参数约定:
        // x0 = &next->context (要恢复的上下文)
        // x1 = &prev->context (要保存的上下文)
    );
}

pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 获取 CpuContext 的指针
    let next_ctx: *mut CpuContext = next.context_mut();
    let prev_ctx: *mut CpuContext = prev.context_mut();

    // 调用汇编上下文切换函数
    // 注意参数顺序：x0 = next, x1 = prev
    cpu_switch_to(next_ctx, prev_ctx);
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /// 用户寄存器
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,
    /// 用户栈指针
    pub sp: u64,
    /// 程序计数器 (入口点)
    pub elr: u64,
    /// 程序状态寄存器
    pub spsr: u64,
}

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.switch_to_user"]
pub unsafe extern "C" fn switch_to_user(ctx: *const UserContext) -> ! {
    core::arch::naked_asm!(
        // UserContext 指针通过 x0 传递
        // UserContext 布局 (每个字段 8 字节):
        // x0(0), x1(1), x2(2), x3(3), x4(4), x5(5), x6(6), x7(7), x8(8),
        // x19(9), x20(10), x21(11), x22(12), x23(13), x24(14), x25(15), x26(16), x27(17), x28(18), x29(19),
        // sp(20), elr(21), spsr(22)

        // 先保存指针到 x15
        "mov x15, x0",

        // 先设置系统寄存器 (必须使用正确的偏移)
        "ldr x13, [x15, #160]",  // ctx.sp (offset 20*8 = 160)
        "msr sp_el0, x13",       // 设置用户栈指针

        "ldr x14, [x15, #168]",  // ctx.elr (offset 21*8 = 168)
        "msr elr_el1, x14",      // 设置入口点

        "ldr x16, [x15, #176]",  // ctx.spsr (offset 22*8 = 176)
        "msr spsr_el1, x16",     // 设置程序状态

        // 现在加载通用寄存器
        "ldr x0, [x15, #0]",     // ctx.x0
        "ldr x1, [x15, #8]",     // ctx.x1
        "ldr x2, [x15, #16]",    // ctx.x2
        "ldr x3, [x15, #24]",    // ctx.x3
        "ldr x4, [x15, #32]",    // ctx.x4
        "ldr x5, [x15, #40]",    // ctx.x5
        "ldr x6, [x15, #48]",    // ctx.x6
        "ldr x7, [x15, #56]",    // ctx.x7
        "ldr x8, [x15, #64]",    // ctx.x8

        // 加载被调用者保存寄存器
        "ldr x19, [x15, #72]",   // ctx.x19 (offset 9*8)
        "ldr x20, [x15, #80]",   // ctx.x20 (offset 10*8)
        "ldr x21, [x15, #88]",   // ctx.x21 (offset 11*8)
        "ldr x22, [x15, #96]",   // ctx.x22 (offset 12*8)
        "ldr x23, [x15, #104]",  // ctx.x23 (offset 13*8)
        "ldr x24, [x15, #112]",  // ctx.x24 (offset 14*8)
        "ldr x25, [x15, #120]",  // ctx.x25 (offset 15*8)
        "ldr x26, [x15, #128]",  // ctx.x26 (offset 16*8)
        "ldr x27, [x15, #136]",  // ctx.x27 (offset 17*8)
        "ldr x28, [x15, #144]",  // ctx.x28 (offset 18*8)
        "ldr x29, [x15, #152]",  // ctx.x29 (offset 19*8)

        // 清空临时寄存器
        "mov x13, xzr",
        "mov x14, xzr",
        "mov x16, xzr",
        "mov x15, xzr",

        // DEBUG: Simple indicator before eret
        "mov x13, #0x09000000",
        "mov w14, #'>'",
        "str w14, [x13]",

        // Instruction synchronization barrier - ensure all system register writes complete
        "isb",

        // 使用 eret 切换到用户模式
        "eret",
    );
}

pub unsafe fn switch_to_user_wrapper(ctx: &UserContext) -> ! {
    // 简化的调试输出
    use crate::console::putchar;
    const MSG: &[u8] = b"Switching to user mode (EL0)...\n";
    for &b in MSG {
        putchar(b);
    }

    switch_to_user(ctx);
}
