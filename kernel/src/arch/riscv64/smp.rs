//! RISC-V SMP (Symmetric Multi-Processing) 支持
//!
//! 多核启动和管理框架

use crate::println;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

/// 最大 CPU 数量（QEMU virt 默认 4 核）
pub const MAX_CPUS: usize = 4;

/// Per-CPU 栈大小（16KB）
pub const STACK_SIZE: usize = 16384;

/// 主核（启动核）的 hart ID
/// NOTE: 这个值可能在运行时改变，如果 OpenSBI 从其他 hart 启动
pub const BOOT_HART_ID: usize = 0;

/// 实际的启动 hart ID（运行时检测）
static ACTUAL_BOOT_HART: AtomicU32 = AtomicU32::new(u32::MAX);

/// SMP 初始化是否完成
static SMP_INIT_DONE: AtomicU32 = AtomicU32::new(0);

/// CPU 启动状态（0 = 未启动，1 = 已启动）
static CPU_STARTED: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 标记 CPU 已启动
fn mark_cpu_started(hart_id: usize) {
    if hart_id < MAX_CPUS {
        CPU_STARTED[hart_id].store(1, Ordering::Release);
    }
}

/// 获取当前 CPU ID (hart ID)
///
/// 从 tp (x4) 寄存器读取，boot.S 在启动时保存了 hart ID
/// OpenSBI 通过 a0 寄存器传递 hart ID 给内核
#[inline]
pub fn cpu_id() -> usize {
    unsafe {
        let hartid: u64;
        asm!("mv {}, tp", out(reg) hartid);
        hartid as usize
    }
}

/// 检查是否为启动核（主核）
///
/// 注意：这个函数只有在 SMP 初始化完成后才有效
/// 在初始化过程中，应该使用 init() 函数的返回值来判断
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

/// 次核启动入口（从 boot.S 调用）
///
/// 次核的执行流程：
/// 1. 栈已由 boot.S 设置
/// 2. 简单的启动验证
/// 3. 进入空闲循环（等待 IPI 唤醒）
#[no_mangle]
pub extern "C" fn secondary_cpu_start() -> ! {
    // 从 tp 寄存器读取 hart ID（boot.S 保存的）
    let hart_id: usize;
    unsafe {
        asm!(
            "mv {}, tp",
            out(reg) hart_id
        );
    }

    // 简单的启动验证（使用底层 putchar 避免 println 依赖）
    const MSG: &[u8] = b"sec";
    const MSG_PREFIX: &[u8] = b"\nsmp: Secondary CPU ";
    const MSG_END: &[u8] = b" starting...\n";

    // 输出前缀
    for &b in MSG_PREFIX {
        crate::console::putchar(b);
    }
    // 输出 hart ID（简单转十进制）
    if hart_id < 10 {
        crate::console::putchar(b'0' as u8 + hart_id as u8);
    } else {
        crate::console::putchar(b'1' as u8);
        crate::console::putchar(b'0' as u8 + (hart_id - 10) as u8);
    }
    // 输出后缀
    for &b in MSG_END {
        crate::console::putchar(b);
    }

    // 标记 CPU 已启动
    mark_cpu_started(hart_id);

    // 进入空闲循环（WFI）
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

/// 初始化 SMP（由所有 CPU 调用）
///
/// 执行流程：
/// 1. 第一个调用此函数的 CPU 成为启动核（主核）
/// 2. 主核执行完整初始化并唤醒其他 CPU
/// 3. 其他 CPU 等待初始化完成后返回
///
/// 返回值：true 表示启动核，false 表示次核
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
        println!("smp: Initializing RISC-V SMP...");
        println!("smp: Boot CPU (hart {}) identified", my_hart);
        println!("smp: Maximum {} CPUs supported", MAX_CPUS);
    }

    if is_boot_cpu {
        // 标记主核已启动
        mark_cpu_started(my_hart);

        // 唤醒其他 CPU
        for hart_id in 0..MAX_CPUS {
            if hart_id != my_hart {
                println!("smp: Starting secondary hart {}...", hart_id);

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
                    println!("smp: Hart {} start command sent successfully", hart_id);
                } else {
                    println!("smp: Failed to start hart {}: error={}, value={}",
                             hart_id, ret.error, ret.value);
                }
            }
        }

        // 标记初始化完成
        SMP_INIT_DONE.store(1, Ordering::Release);
        println!("smp: RISC-V SMP initialized");

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

/// 获取已启动的 CPU 数量
pub fn num_started_cpus() -> usize {
    let mut count = 0;
    for i in 0..MAX_CPUS {
        if CPU_STARTED[i].load(Ordering::Acquire) == 1 {
            count += 1;
        }
    }
    count
}
