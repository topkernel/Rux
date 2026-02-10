//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

/// CPU 相关操作

/// 获取当前核心ID
#[inline]
pub fn get_core_id() -> u64 {
    let core_id: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) core_id, options(nomem, nostack, pure));
    }
    core_id & 0xFF
}

/// 获取当前线程ID
#[inline]
pub fn get_thread_id() -> u64 {
    let tpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) tpidr, options(nomem, nostack, pure));
    }
    tpidr
}

/// 设置当前线程ID
#[inline]
pub fn set_thread_id(tid: u64) {
    unsafe {
        core::arch::asm!("msr tpidr_el1, {}", in(reg) tid, options(nomem, nostack));
    }
}

/// 获取计数器频率
#[inline]
pub fn get_counter_freq() -> u64 {
    let freq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, pure));
    }
    freq
}

/// 读取虚拟计数器
#[inline]
pub fn read_counter() -> u64 {
    let cnt: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack, pure));
    }
    cnt
}

/// 使能中断
#[inline]
pub fn enable_irq() {
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
}

/// 禁用中断
#[inline]
pub fn disable_irq() {
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }
}

/// 等待中断
#[inline]
pub fn wfi() {
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
}

/// 串行化指令执行
#[inline]
pub fn isb() {
    unsafe {
        core::arch::asm!("isb", options(nomem, nostack));
    }
}

/// 数据同步屏障
#[inline]
pub fn dsb() {
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack));
    }
}

/// 数据内存屏障
#[inline]
pub fn dmb() {
    unsafe {
        core::arch::asm!("dmb sy", options(nomem, nostack));
    }
}

/// 获取中断屏蔽状态
#[inline]
pub fn get_interrupts_state() -> bool {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, pure));
    }
    // DAIF.I 位 (bit 7)
    (daif & (1 << 7)) == 0
}

/// 保存中断状态并禁用中断
#[inline]
pub fn save_and_disable_irq() -> bool {
    let state = get_interrupts_state();
    disable_irq();
    state
}

/// 恢复中断状态
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
