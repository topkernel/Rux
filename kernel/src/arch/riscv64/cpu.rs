/// CPU 相关操作 (RISC-V 64-bit)
use core::arch::asm;

/// 获取当前核心ID (hart ID)
#[inline]
pub fn get_core_id() -> u64 {
    let hart_id: u64;
    unsafe {
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id, options(nomem, nostack, pure));
    }
    hart_id
}

/// 获取当前线程ID
#[inline]
pub fn get_thread_id() -> u64 {
    // RISC-V 使用 tp 寄存器 (x4) 存储线程指针
    let tp: u64;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
    }
    tp
}

/// 设置当前线程ID
#[inline]
pub fn set_thread_id(tid: u64) {
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) tid, options(nomem, nostack));
    }
}

/// 获取计数器频率 (RISC-V 使用 time CSR)
#[inline]
pub fn get_counter_freq() -> u64 {
    // QEMU virt 平台默认频率: 10 MHz
    10_000_000
}

/// 读取计数器 (time CSR)
#[inline]
pub fn read_counter() -> u64 {
    let time: u64;
    unsafe {
        core::arch::asm!("csrrw {}, time, zero", out(reg) time, options(nomem, nostack, pure));
    }
    time
}

/// 使能中断
#[inline]
pub fn enable_irq() {
    unsafe {
        // 设置 mstatus.MIE (Machine Interrupt Enable) 位
        let mut mstatus: u64;
        asm!("csrrs {}, mstatus, zero", out(reg) mstatus);
        mstatus |= 1 << 3; // MIE bit
        asm!("csrw mstatus, {}", in(reg) mstatus);
    }
}

/// 禁用中断
#[inline]
pub fn disable_irq() {
    unsafe {
        // 清除 mstatus.MIE (Machine Interrupt Enable) 位
        let mut mstatus: u64;
        asm!("csrrs {}, mstatus, zero", out(reg) mstatus);
        mstatus &= !(1 << 3); // MIE bit
        asm!("csrw mstatus, {}", in(reg) mstatus);
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
        core::arch::asm!("fence.i", options(nomem, nostack));
    }
}

/// 数据同步屏障
#[inline]
pub fn dsb() {
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// 数据内存屏障
#[inline]
pub fn dmb() {
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// 获取中断屏蔽状态
#[inline]
pub fn get_interrupts_state() -> bool {
    let mstatus: u64;
    unsafe {
        asm!("csrrs {}, mstatus, zero", out(reg) mstatus, options(nomem, nostack, pure));
    }
    // mstatus.MIE 位 (bit 3)
    (mstatus & (1 << 3)) != 0
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
