//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit 上下文切换
//!
//! 遵循 Linux 内核的上下文切换实现 (arch/riscv/kernel/process.c)
//!
//! Linux RISC-V 的上下文切换使用 __switch_to() 函数：
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
    // 完全遵循 Linux 的 cpu_switch_to (arch/riscv/kernel/process.S)
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

pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 在 SMP 环境中禁用中断，防止在上下文切换期间发生竞争条件
    // 对应 Linux 的 local_irq_disable()
    let _irq_guard = InterruptGuard::new();

    // 获取 CpuContext 的指针
    let next_ctx: *mut CpuContext = next.context_mut();
    let prev_ctx: *mut CpuContext = prev.context_mut();

    // 调用汇编上下文切换函数
    // 注意参数顺序：a0 = next, a1 = prev
    cpu_switch_to(next_ctx, prev_ctx);

    // InterruptGuard 在此处 Drop，自动恢复中断状态
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserContext {
    /// 通用寄存器 x0-x7 (zero, ra, sp, gp, tp, t0, t1, t2)
    /// x0 = zero (硬连线为 0)
    /// x1 = ra (返回地址)
    /// x2 = sp (栈指针)
    /// x3 = gp (全局指针)
    /// x4 = tp (线程指针，用于 cpu_id())
    /// x5 = t0 (临时寄存器)
    /// x6 = t1 (临时寄存器)
    /// x7 = t2 (临时寄存器)
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    /// 被调用者保存寄存器 x8-x9 (s0-s1)
    pub x8: u64,
    pub x9: u64,
    /// 被调用者保存寄存器 x18-x27 (s2-s11)
    pub x18: u64,
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    /// 用户栈指针
    pub sp: u64,
    /// 程序计数器 (入口点)
    pub pc: u64,
    /// 程序状态寄存器
    pub status: u64,
}

impl UserContext {
    /// 创建新的用户上下文
    ///
    /// # 参数
    /// - `entry_point`: 用户程序入口地址
    /// - `stack_top`: 用户栈顶地址
    pub fn new(entry_point: u64, stack_top: u64) -> Self {
        Self::new_with_gp(entry_point, stack_top, 0)
    }

    /// 创建新的用户上下文（带全局指针）
    ///
    /// # 参数
    /// - `entry_point`: 用户程序入口地址
    /// - `stack_top`: 用户栈顶地址
    /// - `global_pointer`: 全局指针（gp），用于 musl libc 访问全局变量
    pub fn new_with_gp(entry_point: u64, stack_top: u64, global_pointer: u64) -> Self {
        // 读取当前 sstatus（我们在 S 模式，不是 M 模式）
        let mut sstatus_value: u64;
        unsafe {
            asm!("csrr {}, sstatus", out(reg) sstatus_value);
        }

        // 配置 sstatus（RISC-V S 模式状态寄存器）:
        // - SPP (bit 8) = 0: 从 S-Mode 返回到 U-Mode
        // - SPIE (bit 5) = 1: 在 U-Mode 中使能中断
        // - SUM (bit 18) = 1: 允许 S 模式访问用户内存
        sstatus_value &= !(1 << 8);   // Clear SPP (返回到 U 模式)
        sstatus_value |= 1 << 5;    // Set SPIE (U 模式中使能中断)
        sstatus_value |= 1 << 18;   // Set SUM (S 模式可访问用户内存)

        // 读取当前 tp 寄存器（包含 hart ID）
        let tp_value: u64;
        unsafe {
            asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));
        }

        Self {
            x0: 0,
            x1: 0,
            x2: 0,
            x3: global_pointer, // gp - 全局指针，musl libc 使用 gp-relative 寻址
            x4: tp_value, // tp - hart ID，用于 cpu_id()
            x5: 0,
            x6: 0,
            x7: 0,
            x8: 0,
            x9: 0,
            x18: 0,
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            sp: stack_top,
            pc: entry_point,
            status: sstatus_value,
        }
    }
}

#[unsafe(naked)]
#[no_mangle]
#[link_section = ".text.switch_to_user"]
pub unsafe extern "C" fn switch_to_user(ctx: *const UserContext) -> ! {
    core::arch::naked_asm!(
        // UserContext 指针通过 a0 传递
        // UserContext 布局 (每个字段 8 字节):
        // x0(zero)=0, x1(ra)=8, x2(sp)=16, x3(gp)=24, x4(tp)=32
        // x5(t0)=40, x6(t1)=48, x7(t2)=56
        // x8(s0)=64, x9(s1)=72
        // x18(s2)=80, x19(s3)=88, x20(s4)=96, x21(s5)=104
        // x22(s6)=112, x23(s7)=120, x24(s8)=128, x25(s9)=136
        // x26(s10)=144, x27(s11)=152
        // sp=160, pc=168, status=176
        //
        // 策略：使用 s0 保存 ctx 指针，最后加载 s0

        // 保存 ctx 指针到 s0
        "mv s0, a0",

        // 设置 S 模式系统寄存器
        "ld t1, 176(s0)",   // ctx.status
        "csrw sstatus, t1",

        "ld t1, 168(s0)",   // ctx.pc
        "csrw sepc, t1",

        // 加载 tp (hart ID) 并设置 sscratch
        // 这必须在加载其他寄存器之前完成
        "ld tp, 32(s0)",    // ctx.x4 (tp/hart ID)
        "addi t1, tp, 1",   // sscratch = tp + 1
        "csrw sscratch, t1",

        // 加载被调用者保存寄存器 (s1-s11)，除了 s0
        "ld s1, 72(s0)",    // ctx.x9 (s1)
        "ld s2, 80(s0)",    // ctx.x18 (s2)
        "ld s3, 88(s0)",    // ctx.x19 (s3)
        "ld s4, 96(s0)",    // ctx.x20 (s4)
        "ld s5, 104(s0)",   // ctx.x21 (s5)
        "ld s6, 112(s0)",   // ctx.x22 (s6)
        "ld s7, 120(s0)",   // ctx.x23 (s7)
        "ld s8, 128(s0)",   // ctx.x24 (s8)
        "ld s9, 136(s0)",   // ctx.x25 (s9)
        "ld s10, 144(s0)",  // ctx.x26 (s10)
        "ld s11, 152(s0)",  // ctx.x27 (s11)

        // 设置用户栈指针
        "ld sp, 160(s0)",   // ctx.sp

        // 加载 gp (全局指针)
        "ld gp, 24(s0)",    // ctx.x3 (gp)

        // 加载 ra (返回地址)
        "ld ra, 8(s0)",     // ctx.x1 (ra)

        // 加载临时寄存器 t0, t1, t2
        "ld t0, 40(s0)",    // ctx.x5 (t0)
        "ld t1, 48(s0)",    // ctx.x6 (t1)
        "ld t2, 56(s0)",    // ctx.x7 (t2)

        // 最后加载 s0（会覆盖 ctx 指针）
        "ld s0, 64(s0)",    // ctx.x8 (s0)

        // 设置 a0 = 0（用户程序入口参数，通常是 0）
        // UserContext.x0 总是 0，我们直接清零 a0
        "mv a0, zero",

        // 使用 sret 切换到用户模式（S 模式返回指令）
        "sret",
    );
}

pub unsafe fn switch_to_user_wrapper(ctx: &UserContext) -> ! {
    // 简化的调试输出
    use crate::console::putchar;
    const MSG1: &[u8] = b"Switching to user mode (U-mode)...\n";
    for &b in MSG1 {
        putchar(b);
    }

    // 打印上下文信息
    crate::println!("  ctx.pc={:#x}, ctx.sp={:#x}", ctx.pc, ctx.sp);

    switch_to_user(ctx);
}
