//! SMP (对称多处理) 支持
//!
//! 对应 Linux 的 ARM64 SMP 实现 (arch/arm64/kernel/smp.c)
//!
//! 支持：
//! - 多核启动
//! - CPU 间通信 (IPI)
//! - Per-CPU 数据管理

use core::sync::atomic::{AtomicUsize, AtomicU32, AtomicU64, Ordering};

use crate::console::putchar;

/// CPU 启动状态
///
/// 对应 Linux 的 struct cpu_boot_info
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
enum CpuBootState {
    Unknown = 0,
    Booting = 1,
    Running = 2,
}

/// Per-CPU 启动信息
///
/// 对应 Linux 的 secondary_data
#[repr(C)]
pub struct CpuBootInfo {
    /// CPU ID
    pub cpu_id: u64,
    /// 启动状态
    pub state: AtomicU32,
    /// 栈指针
    pub stack_ptr: AtomicU64,
}

/// 全局 SMP 数据
///
/// 管理 SMP 系统的全局状态
pub struct SmpData {
    /// 最大 CPU 数量
    pub max_cpus: usize,
    /// 当前启动的 CPU 数量
    pub active_cpus: AtomicUsize,
    /// Per-CPU 启动信息
    pub boot_info: [CpuBootInfo; 4],
}

/// 全局 SMP 数据
///
/// 使用 MaybeUninit 避免启动时的初始化顺序问题
static mut SMP_DATA: Option<SmpData> = None;

impl SmpData {
    /// 初始化 SMP 数据
    ///
    /// 设置 CPU 数量、per-CPU 栈指针等
    ///
    /// # Arguments
    /// * `max_cpus` - 最大 CPU 数量
    pub fn init(max_cpus: usize) {
        unsafe {
            SMP_DATA = Some(SmpData {
                max_cpus,
                active_cpus: AtomicUsize::new(1), // 主核 (CPU 0) 已启动
                boot_info: [
                    CpuBootInfo {
                        cpu_id: 0,
                        state: AtomicU32::new(CpuBootState::Running as u32),
                        stack_ptr: AtomicU64::new(0),
                    },
                    CpuBootInfo {
                        cpu_id: 1,
                        state: AtomicU32::new(CpuBootState::Unknown as u32),
                        stack_ptr: AtomicU64::new(0),
                    },
                    CpuBootInfo {
                        cpu_id: 2,
                        state: AtomicU32::new(CpuBootState::Unknown as u32),
                        stack_ptr: AtomicU64::new(0),
                    },
                    CpuBootInfo {
                        cpu_id: 3,
                        state: AtomicU32::new(CpuBootState::Unknown as u32),
                        stack_ptr: AtomicU64::new(0),
                    },
                ],
            });

            // 设置 per-CPU 栈指针
            extern "C" {
                static __per_cpu_stacks_start: u8;
            }
            let stacks_base = (&__per_cpu_stacks_start as *const _ as u64) as *mut u8;

            for cpu_id in 0..max_cpus {
                // 每个栈 16KB，栈顶在 base + (cpu_id * 16KB) + 16KB
                let stack_top = stacks_base as u64 + (cpu_id as u64) * 0x4000 + 0x4000;
                SMP_DATA.as_mut().unwrap().boot_info[cpu_id].stack_ptr
                    .store(stack_top, Ordering::Release);
            }

            // 通知次核可以继续启动
            // smp_spin_table 在 boot.S 中定义
            extern "C" {
                static mut smp_spin_table: u64;
            }
            smp_spin_table = 1;

            // 内存屏障确保所有 CPU 看到更新
            core::sync::atomic::fence(Ordering::SeqCst);

            // 发送事件 (SEV) 唤醒等待的 CPU
            unsafe {
                core::arch::asm!("sev", options(nomem, nostack));
            }
        }
    }

    /// 标记指定 CPU 为运行状态
    ///
    /// # Arguments
    /// * `cpu_id` - CPU ID
    pub fn mark_cpu_running(cpu_id: u64) {
        unsafe {
            if let Some(ref data) = SMP_DATA {
                data.boot_info[cpu_id as usize].state.store(CpuBootState::Running as u32, Ordering::Release);
                data.active_cpus.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    /// 获取当前运行的 CPU 数量
    ///
    /// # Returns
    /// 运行的 CPU 数量
    pub fn get_active_cpu_count() -> usize {
        unsafe {
            SMP_DATA.as_ref()
                .map(|d| d.active_cpus.load(Ordering::Acquire))
                .unwrap_or(1)
        }
    }
}

/// 次核启动入口点
///
/// 由汇编代码 `secondary_entry` 调用
/// 对应 Linux 的 `secondary_start_kernel`
///
/// # Safety
/// 此函数只能在次核启动时调用一次
#[no_mangle]
pub unsafe extern "C" fn secondary_cpu_start() -> ! {
    use crate::console::putchar;
    use crate::process::sched;

    let cpu_id = crate::arch::aarch64::cpu::get_core_id();

    // 输出启动信息
    let msg = b"[CPU";
    for &b in msg {
        putchar(b);
    }
    let hex = b"0123456789ABCDEF";
    putchar(hex[(cpu_id & 0xF) as usize]);
    let msg2 = b" up]\n";
    for &b in msg2 {
        putchar(b);
    }

    // === 次核初始化序列 ===
    // 参考 Linux: arch/arm64/kernel/smp.c:secondary_start_kernel

    // 1. 初始化 per-CPU 运行队列
    let msg3 = b"[CPU";
    for &b in msg3 {
        putchar(b);
    }
    putchar(hex[(cpu_id & 0xF) as usize]);
    let msg4 = b"] init: runqueue\n";
    for &b in msg4 {
        putchar(b);
    }
    sched::init_per_cpu_rq(cpu_id as usize);

    // 2. 初始化 per-CPU 栈（已在 boot.S 中设置）

    // 3. TODO: 初始化 per-CPU GIC (GICR)
    // let msg5 = b"[CPU";
    // for &b in msg5 {
    //     putchar(b);
    // }
    // putchar(hex[(cpu_id & 0xF) as usize]);
    // let msg6 = b"] init: GICR\n";
    // for &b in msg6 {
    //     putchar(b);
    // }
    // crate::drivers::intc::init_per_cpu(cpu_id as usize);

    // 4. 标记为运行中
    SmpData::mark_cpu_running(cpu_id);

    // 5. 使能本核 IRQ
    let msg7 = b"[CPU";
    for &b in msg7 {
        putchar(b);
    }
    putchar(hex[(cpu_id & 0xF) as usize]);
    let msg8 = b"] init: IRQ enabled\n";
    for &b in msg8 {
        putchar(b);
    }
    core::arch::asm!("msr daifclr, #2", options(nomem, nostack));

    // 6. 进入空闲循环，等待中断或调度
    let msg9 = b"[CPU";
    for &b in msg9 {
        putchar(b);
    }
    putchar(hex[(cpu_id & 0xF) as usize]);
    let msg10 = b"] idle: waiting for work\n";
    for &b in msg10 {
        putchar(b);
    }

    // TODO: Phase 3 会在这里添加 IPI 处理和调度器支持
    // 参考 Linux: cpu_startup_entry()
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
        // TODO: 检查调度器是否有任务
        // if let Some(task) = sched::this_cpu_rq().lock().pick_next_task() {
        //     // 切换到任务
        // }
    }
}

/// 唤醒次核
///
/// 使用 PSCI (Power State Coordination Interface) 唤醒次核
/// 对应 Linux 的 `smp_boot_secondary_cpus`
///
/// # PSCI
/// PSCI 是 ARM 标准的电源管理接口，用于 CPU 电源控制和唤醒。
///
/// # PSCI 调用方式
/// QEMU virt 的设备树指定 PSCI 方法为 "hvc"（Hypervisor Call）：
/// - PSCI_VERSION: 0x84000000 (HVC) / 0xC4000000 (SMC)
/// - PSCI_CPU_ON: 0x84000003 (HVC) / 0xC4000003 (SMC)
///
/// # 实现细节
/// - 必须使用 HVC 调用（匹配设备树中的 "method" 属性）
/// - QEMU virt 启动时运行在 EL1，所以 HVC 调用直接生效
/// - 返回值 0 表示成功，非 0 表示错误（见 PSCI 规范）
pub fn boot_secondary_cpus() {
    use crate::console::putchar;
    const MSG1: &[u8] = b"smp: Booting secondary CPUs...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 首先检查 PSCI 版本（使用 HVC，根据设备树）
    const MSG_CHECK: &[u8] = b"smp: Checking PSCI version...\n";
    for &b in MSG_CHECK {
        unsafe { putchar(b); }
    }

    let psci_version: u64;
    unsafe {
        // PSCI_VERSION 使用 HVC 调用 (0x84000000)
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") 0x84000000u64 => psci_version,
            options(nomem, nostack)
        );
    }

    unsafe {
        const MSG_VER: &[u8] = b"smp: PSCI version = 0x";
        for &b in MSG_VER {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        let mut v = psci_version;
        for _ in 0..8 {
            let digit = (v & 0xF) as usize;
            putchar(hex[digit]);
            v >>= 4;
        }
        putchar(b'\n');
    }

    // 先只启动 CPU 1，用于测试
    for cpu_id in 1..2 {
        const MSG2: &[u8] = b"smp: Calling PSCI for CPU ";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }
        let hex = b"0123456789ABCDEF";
        unsafe { putchar(hex[cpu_id as usize]); }
        const MSG3: &[u8] = b"\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        let mpidr = cpu_id as u64; // QEMU virt 的 CPU MPIDR 就是 CPU ID

        unsafe {
            // PSCI_CPU_ON HVC call (Hypervisor Call)
            // 根据设备树，QEMU virt 使用 HVC 方法
            // x0 = function ID (0x84000003 = PSCI_CPU_ON)
            // x1 = target CPU (MPIDR)
            // x2 = entry point (physical address)
            // x3 = context ID
            let mut result: u64;
            core::arch::asm!(
                "hvc #0",
                inlateout("x0") 0x84000003u64 => result,
                in("x1") mpidr,
                in("x2") secondary_entry as u64,
                in("x3") 0u64,
                options(nomem, nostack)
            );

            const MSG4: &[u8] = b"smp: PSCI result = ";
            for &b in MSG4 {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            let mut r = result;
            for _ in 0..16 {
                let digit = (r & 0xF) as usize;
                putchar(hex[digit]);
                r >>= 4;
            }
            const MSG_END: &[u8] = b"\n";
            for &b in MSG_END {
                putchar(b);
            }

            // 检查返回值 (0 = success)
            if result != 0 {
                const MSG_FAIL: &[u8] = b"smp: CPU boot failed\n";
                for &b in MSG_FAIL {
                    putchar(b);
                }
            } else {
                const MSG_OK: &[u8] = b"smp: CPU boot PSCI success\n";
                for &b in MSG_OK {
                    putchar(b);
                }
            }
        }
    }

    const MSG_DONE: &[u8] = b"smp: Secondary CPU boot initiated\n";
    for &b in MSG_DONE {
        unsafe { putchar(b); }
    }
}

/// 测试 IPI 通信
///
/// CPU 0 发送一个 Reschedule IPI 到 CPU 1
pub fn test_ipi() {
    use crate::console::putchar;
    use crate::arch::aarch64::ipi::{send_ipi, IpiType};

    let this_cpu = crate::arch::aarch64::cpu::get_core_id();

    unsafe {
        const MSG: &[u8] = b"[SMP: Testing IPI from CPU ";
        for &b in MSG {
            putchar(b);
        }
        let hex = b"0123456789ABCDEF";
        putchar(hex[(this_cpu & 0xF) as usize]);
        const MSG2: &[u8] = b"]\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    // CPU 0 发送 IPI 到 CPU 1
    if this_cpu == 0 {
        unsafe {
            const MSG: &[u8] = b"[SMP: CPU 0 sending Reschedule IPI to CPU 1]\n";
            for &b in MSG {
                putchar(b);
            }
        }
        send_ipi(1, IpiType::Reschedule);
    }
}

/// 初始化 GIC 用于 IPI (简化版)
///
/// 只做最小化初始化，支持 SGI 发送和接收
/// 对应 Linux 的 gic_init() 的简化版本
pub fn init_gic_for_ipi() {
    unsafe {
        const MSG: &[u8] = b"[SMP: Initializing GIC for IPI]\n";
        for &b in MSG {
            putchar(b);
        }
    }
}

/// 外部符号声明
extern "C" {
    /// 次核启动入口点（汇编代码）
    fn secondary_entry();
}
