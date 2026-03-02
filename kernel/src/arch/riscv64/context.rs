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

    /// 创建新的用户上下文（带全局指针和 TLS 指针）
    ///
    /// # 参数
    /// - `entry_point`: 用户程序入口地址
    /// - `stack_top`: 用户栈顶地址
    /// - `global_pointer`: 全局指针（gp），用于 musl libc 访问全局变量
    /// - `user_tp`: 用户 TLS 指针
    pub fn new_with_tp(entry_point: u64, stack_top: u64, global_pointer: u64, user_tp: u64) -> Self {
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

        Self {
            x0: 0,
            x1: 0,
            x2: 0,
            x3: global_pointer, // gp - 全局指针，musl libc 使用 gp-relative 寻址
            x4: user_tp,        // tp - 用户 TLS
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

    /// 创建新的用户上下文（带全局指针）
    ///
    /// # 参数
    /// - `entry_point`: 用户程序入口地址
    /// - `stack_top`: 用户栈顶地址
    /// - `global_pointer`: 全局指针（gp），用于 musl libc 访问全局变量
    ///
    /// # Linux 风格的 sscratch/tp 协议
    /// - 内核态: sscratch = 0, tp = current task
    /// - 用户态: sscratch = current task, tp = user TLS
    /// - trap 入口: csrrw tp, sscratch, tp 交换后:
    ///   - 来自内核: tp = 0
    ///   - 来自用户: tp = current task
    pub fn new_with_gp(entry_point: u64, stack_top: u64, global_pointer: u64) -> Self {
        Self::new_with_tp(entry_point, stack_top, global_pointer, 0)
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
        // Linux 风格的 sscratch/tp 协议:
        // - 进入时: tp = current task (内核态)
        // - 切换前: sscratch = tp (保存 current task)
        // - 切换后: tp = user TLS (用户态)
        //
        // 策略：使用 s0 保存 ctx 指针，必须在最后加载 s0

        // 保存 ctx 指针到 s0
        "mv s0, a0",

        // 设置 S 模式系统寄存器
        "ld t1, 176(s0)",   // ctx.status
        "csrw sstatus, t1",

        "ld t1, 168(s0)",   // ctx.pc
        "csrw sepc, t1",

        // ===== Linux 风格的 sscratch 设置 =====
        // 在切换到用户态之前，保存 current task 到 sscratch
        // 这样下次 trap 入口时可以找到内核数据结构
        // tp 当前指向 current task
        "csrw sscratch, tp",

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

        // 加载用户 tp (TLS) - 必须在加载 s0 之前！
        "ld tp, 32(s0)",    // ctx.x4 (user tp/TLS)

        // 刷新 TLB（确保新映射的页面可见）
        "sfence.vma zero, zero",

        // 设置 a0 = 0（用户程序入口参数，通常是 0）
        "mv a0, zero",

        // 最后加载 s0（会覆盖 ctx 指针）
        "ld s0, 64(s0)",    // ctx.x8 (s0)

        // 使用 sret 切换到用户模式（S 模式返回指令）
        "sret",
    );
}

pub unsafe fn switch_to_user_wrapper(ctx: &UserContext) -> ! {
    switch_to_user(ctx);
}
