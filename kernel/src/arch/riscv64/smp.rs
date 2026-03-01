//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V SMP (Symmetric Multi-Processing) 支持
//!
//! 多核启动和管理框架

use crate::println;
use crate::config::MAX_CPUS;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

pub const STACK_SIZE: usize = 65536;

pub const BOOT_HART_ID: usize = 0;

static ACTUAL_BOOT_HART: AtomicU32 = AtomicU32::new(u32::MAX);

static SMP_INIT_DONE: AtomicU32 = AtomicU32::new(0);

static CPU_STARTED: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

fn mark_cpu_started(hart_id: usize) {
    if hart_id < MAX_CPUS {
        CPU_STARTED[hart_id].store(1, Ordering::Release);
    }
}

/// 获取当前 CPU 的硬件线程 ID
///
/// Linux 兼容设计:
/// - 早期启动阶段: tp = hart_id (小数值)
/// - 调度器运行后: tp = task_struct 指针，hart_id 存储在 task_struct.ti_cpu
///
/// 通过检查 tp 的值范围来判断当前模式：
/// - 如果 tp < 0x1000，认为是 hart_id（早期启动）
/// - 否则认为是 task_struct 指针
///
/// 注意：
/// - 不能使用 mhartid CSR（M-mode 专用，S-mode 访问会触发异常）
/// - 必须确保 trap.S 正确处理 tp 寄存器
#[inline]
pub fn cpu_id() -> usize {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // 检查 tp 是否为小数值（早期启动阶段的 hart_id）
        // 有效的 task_struct 指针应该在内核地址空间 (>= 0x80000000)
        if tp_value < 0x1000 {
            // 早期启动阶段，tp 直接存储 hart_id
            tp_value as usize
        } else {
            // tp 指向 task_struct，从 ti_cpu 字段获取 hart_id
            // ti_cpu 在 Task 结构体中的偏移量是 0x18 (24 bytes)
            let ti_cpu_offset = 0x18;
            let cpu_ptr = (tp_value as usize + ti_cpu_offset) as *const core::sync::atomic::AtomicI32;
            (*cpu_ptr).load(core::sync::atomic::Ordering::Relaxed) as usize
        }
    }
}

#[inline]
pub fn is_boot_hart() -> bool {
    let actual = ACTUAL_BOOT_HART.load(Ordering::Acquire) as usize;
    if actual != u32::MAX as usize {
        cpu_id() == actual
    } else {
        // 如果还没有设置 actual boot hart，回退到检查是否为 hart 0
        cpu_id() == BOOT_HART_ID
    }
}

#[no_mangle]
pub extern "C" fn secondary_cpu_start() -> ! {
    // 从 tp 寄存器读取 hart ID（boot.S 保存的）
    let hart_id: usize = cpu_id();

    // 标记 CPU 已启动
    mark_cpu_started(hart_id);

    // 进入空闲循环（WFI）
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

pub fn init() -> bool {
    let my_hart = cpu_id();

    // 尝试成为启动核（使用 CAS 操作）
    // 只有第一个到达这里的 CPU 能成功设置 ACTUAL_BOOT_HART
    let mut is_boot_cpu = false;
    if ACTUAL_BOOT_HART.compare_exchange(
        u32::MAX,
        my_hart as u32,
        Ordering::AcqRel,
        Ordering::Acquire
    ).is_ok() {
        is_boot_cpu = true;
    }

    if is_boot_cpu {
        // 标记主核已启动
        mark_cpu_started(my_hart);

        // 唤醒其他 CPU
        let mut started_count = 0;
        for hart_id in 0..MAX_CPUS {
            if hart_id != my_hart {
                // 次核启动地址：使用内核入口点 _start（所有 CPU 都从 _start 开始）
                // external function _start from boot.S
                let start_addr: usize;
                unsafe {
                    asm!(
                        "la {}, _start",
                        out(reg) start_addr,
                        options(nomem, nostack)
                    );
                }

                // 调用 SBI hart_start
                let ret = sbi_rt::hart_start(hart_id, start_addr, 0);

                // SBI 返回值：ret.error == 0 表示成功
                if ret.error == 0 {
                    started_count += 1;
                }
            }
        }

        // 先唤醒所有次核，再设置完成标志
        // 确保次核不会在唤醒前就检查 SMP_INIT_DONE
        if started_count > 0 {
            // 稍微延迟，确保所有次核都已进入等待循环
            for _ in 0..100 {
                unsafe { asm!("nop", options(nomem, nostack)); }
            }
            // 现在设置初始化完成标志
            SMP_INIT_DONE.store(1, Ordering::Release);
        }

        is_boot_cpu
    } else {
        // 非启动核：等待初始化完成
        while SMP_INIT_DONE.load(Ordering::Acquire) == 0 {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }

        // 标记自己已启动
        mark_cpu_started(my_hart);

        false
    }
}

pub fn num_started_cpus() -> usize {
    let mut count = 0;
    for i in 0..MAX_CPUS {
        if CPU_STARTED[i].load(Ordering::Acquire) == 1 {
            count += 1;
        }
    }
    count
}
