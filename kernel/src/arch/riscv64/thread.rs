//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 线程结构体 (thread_struct)
//!
//! 参考 Linux: arch/riscv/include/asm/processor.h
//!
//! 存储架构相关的线程状态：
//! - FPU 状态
//! - 向量扩展状态
//! - 调试寄存器
//! - 其他架构特定状态

use core::arch::asm;

/// FPU 状态大小 (32 个 64 位寄存器)
const FPU_STATE_SIZE: usize = 32;

/// 线程结构体 - 存储架构相关的线程状态
///
/// 参考 Linux: struct thread_struct
#[repr(C)]
#[derive(Debug, Clone)]
pub struct ThreadStruct {
    /// FPU 状态 (f0-f31)
    ///
    /// RISC-V F 扩展: 32 个浮点寄存器
    /// 每个 64 位 (double precision)
    pub fpu: [u64; FPU_STATE_SIZE],

    /// FPU 控制状态寄存器 (fcsr)
    pub fcsr: u32,

    /// 向量扩展状态 (V 扩展)
    ///
    /// TODO: 实现 V 扩展支持
    /// struct __riscv_v_ext_state
    pub vstate_valid: bool,

    /// 线程本地存储 (TLS) 指针
    ///
    /// 由 set_tid_address 系统调用设置
    pub tp_value: u64,

    /// 当前异常帧指针（用于信号处理）
    pub exception_sp: u64,

    /// 调试标志
    pub debug_flag: bool,
}

impl ThreadStruct {
    /// 创建新的线程结构体
    pub const fn new() -> Self {
        Self {
            fpu: [0; FPU_STATE_SIZE],
            fcsr: 0,
            vstate_valid: false,
            tp_value: 0,
            exception_sp: 0,
            debug_flag: false,
        }
    }

    /// 保存 FPU 状态
    ///
    /// 参考 Linux: fstate_save()
    ///
    /// # Safety
    /// 必须在正确的上下文中调用
    #[inline]
    pub unsafe fn save_fpu(&mut self) {
        // 检查 FS 字段是否为 Initial 或 Clean
        let sstatus: u64;
        asm!("csrr {}, sstatus", out(reg) sstatus);

        let fs = (sstatus >> 13) & 0x3;
        if fs == 0 {
            // FS = Off，不需要保存
            return;
        }

        // 保存浮点寄存器 f0-f31
        asm!(
            "fsd f0, 0*8({0})",
            "fsd f1, 1*8({0})",
            "fsd f2, 2*8({0})",
            "fsd f3, 3*8({0})",
            "fsd f4, 4*8({0})",
            "fsd f5, 5*8({0})",
            "fsd f6, 6*8({0})",
            "fsd f7, 7*8({0})",
            "fsd f8, 8*8({0})",
            "fsd f9, 9*8({0})",
            "fsd f10, 10*8({0})",
            "fsd f11, 11*8({0})",
            "fsd f12, 12*8({0})",
            "fsd f13, 13*8({0})",
            "fsd f14, 14*8({0})",
            "fsd f15, 15*8({0})",
            "fsd f16, 16*8({0})",
            "fsd f17, 17*8({0})",
            "fsd f18, 18*8({0})",
            "fsd f19, 19*8({0})",
            "fsd f20, 20*8({0})",
            "fsd f21, 21*8({0})",
            "fsd f22, 22*8({0})",
            "fsd f23, 23*8({0})",
            "fsd f24, 24*8({0})",
            "fsd f25, 25*8({0})",
            "fsd f26, 26*8({0})",
            "fsd f27, 27*8({0})",
            "fsd f28, 28*8({0})",
            "fsd f29, 29*8({0})",
            "fsd f30, 30*8({0})",
            "fsd f31, 31*8({0})",
            in(reg) self.fpu.as_mut_ptr(),
            options(nostack)
        );

        // 保存 fcsr
        asm!("frcsr {0}", out(reg) self.fcsr);
    }

    /// 恢复 FPU 状态
    ///
    /// 参考 Linux: fstate_restore()
    ///
    /// # Safety
    /// 必须在正确的上下文中调用
    #[inline]
    pub unsafe fn restore_fpu(&mut self) {
        // 恢复 fcsr
        asm!("fscsr {0}", in(reg) self.fcsr);

        // 恢复浮点寄存器 f0-f31
        asm!(
            "fld f0, 0*8({0})",
            "fld f1, 1*8({0})",
            "fld f2, 2*8({0})",
            "fld f3, 3*8({0})",
            "fld f4, 4*8({0})",
            "fld f5, 5*8({0})",
            "fld f6, 6*8({0})",
            "fld f7, 7*8({0})",
            "fld f8, 8*8({0})",
            "fld f9, 9*8({0})",
            "fld f10, 10*8({0})",
            "fld f11, 11*8({0})",
            "fld f12, 12*8({0})",
            "fld f13, 13*8({0})",
            "fld f14, 14*8({0})",
            "fld f15, 15*8({0})",
            "fld f16, 16*8({0})",
            "fld f17, 17*8({0})",
            "fld f18, 18*8({0})",
            "fld f19, 19*8({0})",
            "fld f20, 20*8({0})",
            "fld f21, 21*8({0})",
            "fld f22, 22*8({0})",
            "fld f23, 23*8({0})",
            "fld f24, 24*8({0})",
            "fld f25, 25*8({0})",
            "fld f26, 26*8({0})",
            "fld f27, 27*8({0})",
            "fld f28, 28*8({0})",
            "fld f29, 29*8({0})",
            "fld f30, 30*8({0})",
            "fld f31, 31*8({0})",
            in(reg) self.fpu.as_ptr(),
            options(nostack)
        );
    }

    /// 获取 TLS 指针
    #[inline]
    pub fn tp(&self) -> u64 {
        self.tp_value
    }

    /// 设置 TLS 指针
    #[inline]
    pub fn set_tp(&mut self, tp: u64) {
        self.tp_value = tp;
    }
}

impl Default for ThreadStruct {
    fn default() -> Self {
        Self::new()
    }
}

/// 初始化 FPU
///
/// 在进程首次使用 FPU 时调用
#[inline]
pub unsafe fn fpu_init() {
    // 设置 sstatus.FS = Initial (01)
    let mut sstatus: u64;
    asm!("csrr {}, sstatus", out(reg) sstatus);
    sstatus = (sstatus & !(0x3 << 13)) | (0x1 << 13);
    asm!("csrw sstatus, {}", in(reg) sstatus);

    // 清零所有浮点寄存器
    asm!(
        "fcvt.d.l f0, zero",
        "fcvt.d.l f1, zero",
        "fcvt.d.l f2, zero",
        "fcvt.d.l f3, zero",
        "fcvt.d.l f4, zero",
        "fcvt.d.l f5, zero",
        "fcvt.d.l f6, zero",
        "fcvt.d.l f7, zero",
        "fcvt.d.l f8, zero",
        "fcvt.d.l f9, zero",
        "fcvt.d.l f10, zero",
        "fcvt.d.l f11, zero",
        "fcvt.d.l f12, zero",
        "fcvt.d.l f13, zero",
        "fcvt.d.l f14, zero",
        "fcvt.d.l f15, zero",
        "fcvt.d.l f16, zero",
        "fcvt.d.l f17, zero",
        "fcvt.d.l f18, zero",
        "fcvt.d.l f19, zero",
        "fcvt.d.l f20, zero",
        "fcvt.d.l f21, zero",
        "fcvt.d.l f22, zero",
        "fcvt.d.l f23, zero",
        "fcvt.d.l f24, zero",
        "fcvt.d.l f25, zero",
        "fcvt.d.l f26, zero",
        "fcvt.d.l f27, zero",
        "fcvt.d.l f28, zero",
        "fcvt.d.l f29, zero",
        "fcvt.d.l f30, zero",
        "fcvt.d.l f31, zero",
        options(nostack)
    );

    // 清零 fcsr
    asm!("fscsr zero");
}
