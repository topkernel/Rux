//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V PtRegs 结构体
//!
//! 与 Linux 内核 `struct pt_regs` 完全兼容的布局
//! 参考: linux/arch/riscv/include/asm/ptrace.h
//!
//! ## 布局说明
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x00    epc         程序计数器 (sepc CSR)
//! 0x08    ra          返回地址 (x1)
//! 0x10    sp          栈指针 (x2)
//! 0x18    gp          全局指针 (x3)
//! 0x20    tp          线程指针 (x4)
//! 0x28    t0          临时寄存器 (x5)
//! 0x30    t1          临时寄存器 (x6)
//! 0x38    t2          临时寄存器 (x7)
//! 0x40    s0/fp       保存寄存器/帧指针 (x8)
//! 0x48    s1          保存寄存器 (x9)
//! 0x50    a0          参数/返回值 (x10)
//! 0x58    a1          参数 (x11)
//! 0x60    a2          参数 (x12)
//! 0x68    a3          参数 (x13)
//! 0x70    a4          参数 (x14)
//! 0x78    a5          参数 (x15)
//! 0x80    a6          参数 (x16)
//! 0x88    a7          参数/系统调用号 (x17)
//! 0x90    s2          保存寄存器 (x18)
//! 0x98    s3          保存寄存器 (x19)
//! 0xa0    s4          保存寄存器 (x20)
//! 0xa8    s5          保存寄存器 (x21)
//! 0xb0    s6          保存寄存器 (x22)
//! 0xb8    s7          保存寄存器 (x23)
//! 0xc0    s8          保存寄存器 (x24)
//! 0xc8    s9          保存寄存器 (x25)
//! 0xd0    s10         保存寄存器 (x26)
//! 0xd8    s11         保存寄存器 (x27)
//! 0xe0    t3          临时寄存器 (x28)
//! 0xe8    t4          临时寄存器 (x29)
//! 0xf0    t5          临时寄存器 (x30)
//! 0xf8    t6          临时寄存器 (x31)
//! 0x100   status      sstatus CSR
//! 0x108   badaddr     stval CSR
//! 0x110   cause       scause CSR
//! 0x118   orig_a0     原始 a0 (系统调用回滚用)
//! ```
//!
//! 总大小: 0x120 = 288 字节

use core::arch::asm;

/// RISC-V 寄存器状态结构体
///
/// 与 Linux `struct pt_regs` 布局完全一致
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct PtRegs {
    // 程序计数器
    pub epc: u64,      // 0x00 - sepc CSR

    // 通用寄存器 (按 Linux 顺序)
    pub ra: u64,       // 0x08 - x1
    pub sp: u64,       // 0x10 - x2
    pub gp: u64,       // 0x18 - x3
    pub tp: u64,       // 0x20 - x4
    pub t0: u64,       // 0x28 - x5
    pub t1: u64,       // 0x30 - x6
    pub t2: u64,       // 0x38 - x7
    pub s0: u64,       // 0x40 - x8 (also fp)
    pub s1: u64,       // 0x48 - x9
    pub a0: u64,       // 0x50 - x10
    pub a1: u64,       // 0x58 - x11
    pub a2: u64,       // 0x60 - x12
    pub a3: u64,       // 0x68 - x13
    pub a4: u64,       // 0x70 - x14
    pub a5: u64,       // 0x78 - x15
    pub a6: u64,       // 0x80 - x16
    pub a7: u64,       // 0x88 - x17
    pub s2: u64,       // 0x90 - x18
    pub s3: u64,       // 0x98 - x19
    pub s4: u64,       // 0xa0 - x20
    pub s5: u64,       // 0xa8 - x21
    pub s6: u64,       // 0xb0 - x22
    pub s7: u64,       // 0xb8 - x23
    pub s8: u64,       // 0xc0 - x24
    pub s9: u64,       // 0xc8 - x25
    pub s10: u64,      // 0xd0 - x26
    pub s11: u64,      // 0xd8 - x27
    pub t3: u64,       // 0xe0 - x28
    pub t4: u64,       // 0xe8 - x29
    pub t5: u64,       // 0xf0 - x30
    pub t6: u64,       // 0xf8 - x31

    // CSR 寄存器
    pub status: u64,   // 0x100 - sstatus CSR
    pub badaddr: u64,  // 0x108 - stval CSR
    pub cause: u64,    // 0x110 - scause CSR

    // 系统调用支持
    pub orig_a0: u64,  // 0x118 - 原始 a0，用于系统调用回滚
}

/// PtRegs 结构体大小
pub const PT_REGS_SIZE: usize = 0x120; // 288 字节

// 静态断言：确保 PtRegs 大小正确
const _: () = assert!(core::mem::size_of::<PtRegs>() == PT_REGS_SIZE);

impl PtRegs {
    /// 创建新的空 PtRegs
    pub const fn new() -> Self {
        Self {
            epc: 0, ra: 0, sp: 0, gp: 0, tp: 0,
            t0: 0, t1: 0, t2: 0, s0: 0, s1: 0,
            a0: 0, a1: 0, a2: 0, a3: 0, a4: 0,
            a5: 0, a6: 0, a7: 0, s2: 0, s3: 0,
            s4: 0, s5: 0, s6: 0, s7: 0, s8: 0,
            s9: 0, s10: 0, s11: 0, t3: 0, t4: 0,
            t5: 0, t6: 0, status: 0, badaddr: 0,
            cause: 0, orig_a0: 0,
        }
    }

    /// 检查是否来自用户模式
    ///
    /// 通过检查 sstatus.SPP 位判断
    /// SPP=0 表示来自用户模式，SPP=1 表示来自内核模式
    #[inline]
    pub fn user_mode(&self) -> bool {
        (self.status & SR_SPP) == 0
    }

    /// 检查是否来自内核模式
    #[inline]
    pub fn kernel_mode(&self) -> bool {
        !self.user_mode()
    }

    /// 获取系统调用号
    #[inline]
    pub fn syscall_nr(&self) -> i64 {
        self.a7 as i64
    }

    /// 获取系统调用参数
    ///
    /// 返回 6 个参数的数组
    #[inline]
    pub fn syscall_args(&self) -> [u64; 6] {
        [
            self.orig_a0,  // 使用 orig_a0 作为第一个参数
            self.a1,
            self.a2,
            self.a3,
            self.a4,
            self.a5,
        ]
    }

    /// 设置系统调用返回值
    #[inline]
    pub fn set_return_value(&mut self, val: i64) {
        self.a0 = val as u64;
    }

    /// 设置系统调用错误返回
    ///
    /// 如果 error 非零，返回 -error；否则返回 val
    #[inline]
    pub fn set_return_error(&mut self, error: i32, val: i64) {
        self.a0 = if error != 0 { -error as i64 as u64 } else { val as u64 };
    }

    /// 回滚系统调用
    ///
    /// 将 a0 恢复为原始值
    #[inline]
    pub fn syscall_rollback(&mut self) {
        self.a0 = self.orig_a0;
    }

    /// 获取指令指针 (PC)
    #[inline]
    pub fn instruction_pointer(&self) -> u64 {
        self.epc
    }

    /// 设置指令指针 (PC)
    #[inline]
    pub fn set_instruction_pointer(&mut self, pc: u64) {
        self.epc = pc;
    }

    /// 获取用户栈指针
    #[inline]
    pub fn user_stack_pointer(&self) -> u64 {
        self.sp
    }

    /// 设置用户栈指针
    #[inline]
    pub fn set_user_stack_pointer(&mut self, sp: u64) {
        self.sp = sp;
    }

    /// 获取帧指针
    #[inline]
    pub fn frame_pointer(&self) -> u64 {
        self.s0  // s0 也是 fp
    }

    /// 检查中断是否被禁用
    #[inline]
    pub fn irqs_disabled(&self) -> bool {
        (self.status & SR_PIE) == 0
    }
}

// ==================== CSR 位定义 ====================

/// SPP (Supervisor Previous Privilege) - bit 8
/// 表示进入 trap 前的特权级
/// 0 = User mode, 1 = Supervisor mode
pub const SR_SPP: u64 = 1 << 8;

/// SPIE (Supervisor Previous Interrupt Enable) - bit 5
/// 表示进入 trap 前中断是否使能
pub const SR_PIE: u64 = 1 << 5;

/// SIE (Supervisor Interrupt Enable) - bit 1
/// 全局中断使能
pub const SR_SIE: u64 = 1 << 1;

/// SUM (Supervisor User Memory Access) - bit 18
/// 允许 S-mode 访问用户内存
pub const SR_SUM: u64 = 1 << 18;

/// UXL (User XLEN) - bits 33:32
/// 用户模式位宽: 1 = 32-bit, 2 = 64-bit
pub const SR_UXL_32: u64 = 1 << 32;
pub const SR_UXL_64: u64 = 2 << 32;

/// FS (Floating-point Status) - bits 14:13
pub const SR_FS_OFF: u64 = 0 << 13;
pub const SR_FS_INITIAL: u64 = 1 << 13;
pub const SR_FS_CLEAN: u64 = 2 << 13;
pub const SR_FS_DIRTY: u64 = 3 << 13;

/// VS (Vector Status) - bits 10:9
pub const SR_VS_OFF: u64 = 0 << 9;
pub const SR_VS_INITIAL: u64 = 1 << 9;
pub const SR_VS_CLEAN: u64 = 2 << 9;
pub const SR_VS_DIRTY: u64 = 3 << 9;

// ==================== 异常原因码 ====================

/// 异常原因 (scause)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    // 异常 (scause 最高位为 0)
    /// 指令地址未对齐
    InstructionAddressMisaligned = 0,
    /// 指令访问错误
    InstructionAccessFault = 1,
    /// 非法指令
    IllegalInstruction = 2,
    /// 断点
    Breakpoint = 3,
    /// 加载地址未对齐
    LoadAddressMisaligned = 4,
    /// 加载访问错误
    LoadAccessFault = 5,
    /// 存储/AMO 地址未对齐
    StoreAmoAddressMisaligned = 6,
    /// 存储/AMO 访问错误
    StoreAmoAccessFault = 7,
    /// 用户态 ecall
    EcallUser = 8,
    /// 超级用户态 ecall
    EcallSupervisor = 9,
    /// 机器态 ecall
    EcallMachine = 11,
    /// 指令页错误
    InstructionPageFault = 12,
    /// 加载页错误
    LoadPageFault = 13,
    /// 存储/AMO 页错误
    StoreAmoPageFault = 15,

    // 中断 (scause 最高位为 1)
    /// 软件中断
    SupervisorSoft = 0x80000001,
    /// 定时器中断
    SupervisorTimer = 0x80000005,
    /// 外部中断
    SupervisorExternal = 0x80000009,
}

impl Cause {
    /// 从 scause 值解析
    pub fn from_cause(cause: u64) -> Self {
        match cause {
            0 => Cause::InstructionAddressMisaligned,
            1 => Cause::InstructionAccessFault,
            2 => Cause::IllegalInstruction,
            3 => Cause::Breakpoint,
            4 => Cause::LoadAddressMisaligned,
            5 => Cause::LoadAccessFault,
            6 => Cause::StoreAmoAddressMisaligned,
            7 => Cause::StoreAmoAccessFault,
            8 => Cause::EcallUser,
            9 => Cause::EcallSupervisor,
            11 => Cause::EcallMachine,
            12 => Cause::InstructionPageFault,
            13 => Cause::LoadPageFault,
            15 => Cause::StoreAmoPageFault,
            0x80000001 => Cause::SupervisorSoft,
            0x80000005 => Cause::SupervisorTimer,
            0x80000009 => Cause::SupervisorExternal,
            _ => Cause::IllegalInstruction, // 默认
        }
    }

    /// 是否为中断
    pub fn is_interrupt(&self) -> bool {
        matches!(self,
            Cause::SupervisorSoft |
            Cause::SupervisorTimer |
            Cause::SupervisorExternal)
    }

    /// 是否为异常
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// 是否为页错误
    pub fn is_page_fault(&self) -> bool {
        matches!(self,
            Cause::InstructionPageFault |
            Cause::LoadPageFault |
            Cause::StoreAmoPageFault)
    }
}

// ==================== 辅助函数 ====================

/// 判断当前是否在中断上下文
///
/// 暂时返回 false，需要后续实现抢占计数
#[inline]
pub fn in_interrupt() -> bool {
    // TODO: 实现抢占计数检查
    false
}

/// 判断当前是否在进程上下文
#[inline]
pub fn in_task() -> bool {
    !in_interrupt()
}

// ==================== 偏移量常量 (供汇编使用) ====================

/// 各字段在 PtRegs 中的偏移量
#[allow(dead_code)]
mod offsets {
    use super::*;

    pub const EPC: usize = core::mem::offset_of!(PtRegs, epc);
    pub const RA: usize = core::mem::offset_of!(PtRegs, ra);
    pub const SP: usize = core::mem::offset_of!(PtRegs, sp);
    pub const GP: usize = core::mem::offset_of!(PtRegs, gp);
    pub const TP: usize = core::mem::offset_of!(PtRegs, tp);
    pub const T0: usize = core::mem::offset_of!(PtRegs, t0);
    pub const T1: usize = core::mem::offset_of!(PtRegs, t1);
    pub const T2: usize = core::mem::offset_of!(PtRegs, t2);
    pub const S0: usize = core::mem::offset_of!(PtRegs, s0);
    pub const S1: usize = core::mem::offset_of!(PtRegs, s1);
    pub const A0: usize = core::mem::offset_of!(PtRegs, a0);
    pub const A1: usize = core::mem::offset_of!(PtRegs, a1);
    pub const A2: usize = core::mem::offset_of!(PtRegs, a2);
    pub const A3: usize = core::mem::offset_of!(PtRegs, a3);
    pub const A4: usize = core::mem::offset_of!(PtRegs, a4);
    pub const A5: usize = core::mem::offset_of!(PtRegs, a5);
    pub const A6: usize = core::mem::offset_of!(PtRegs, a6);
    pub const A7: usize = core::mem::offset_of!(PtRegs, a7);
    pub const S2: usize = core::mem::offset_of!(PtRegs, s2);
    pub const S3: usize = core::mem::offset_of!(PtRegs, s3);
    pub const S4: usize = core::mem::offset_of!(PtRegs, s4);
    pub const S5: usize = core::mem::offset_of!(PtRegs, s5);
    pub const S6: usize = core::mem::offset_of!(PtRegs, s6);
    pub const S7: usize = core::mem::offset_of!(PtRegs, s7);
    pub const S8: usize = core::mem::offset_of!(PtRegs, s8);
    pub const S9: usize = core::mem::offset_of!(PtRegs, s9);
    pub const S10: usize = core::mem::offset_of!(PtRegs, s10);
    pub const S11: usize = core::mem::offset_of!(PtRegs, s11);
    pub const T3: usize = core::mem::offset_of!(PtRegs, t3);
    pub const T4: usize = core::mem::offset_of!(PtRegs, t4);
    pub const T5: usize = core::mem::offset_of!(PtRegs, t5);
    pub const T6: usize = core::mem::offset_of!(PtRegs, t6);
    pub const STATUS: usize = core::mem::offset_of!(PtRegs, status);
    pub const BADADDR: usize = core::mem::offset_of!(PtRegs, badaddr);
    pub const CAUSE: usize = core::mem::offset_of!(PtRegs, cause);
    pub const ORIG_A0: usize = core::mem::offset_of!(PtRegs, orig_a0);
}

/// 导出偏移量常量供汇编使用
#[allow(dead_code)]
pub use offsets::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pt_regs_size() {
        assert_eq!(core::mem::size_of::<PtRegs>(), 288);
    }

    #[test]
    fn test_offsets() {
        assert_eq!(offsets::EPC, 0x00);
        assert_eq!(offsets::RA, 0x08);
        assert_eq!(offsets::SP, 0x10);
        assert_eq!(offsets::A0, 0x50);
        assert_eq!(offsets::STATUS, 0x100);
        assert_eq!(offsets::ORIG_A0, 0x118);
    }
}
