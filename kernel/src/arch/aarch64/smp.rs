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

    // 标记为运行中
    SmpData::mark_cpu_running(cpu_id);

    // 进入空闲循环，等待中断
    // TODO: Phase 3 会在这里添加 IPI 处理
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

/// 唤醒次核
///
/// 使用 PSCI (Power State Coordination Interface) 唤醒次核
/// 对应 Linux 的 `smp_boot_secondary_cpus`
///
/// # PSCI
/// PSCI 是 ARM 标准的电源管理接口，用于 CPU 电源控制和唤醒
/// 使用 SMC (Secure Monitor Call) 调用 PSCI 功能
///
/// # QEMU virt 机器
/// QEMU virt 机器使用 ATF (ARM Trusted Firmware) 实现 PSCI
pub fn boot_secondary_cpus() {
    unsafe {
        const MSG: &[u8] = b"[SMP: Starting CPU boot]\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // 先只启动 CPU 1，用于测试
    for cpu_id in 1..2 {
        unsafe {
            const MSG2: &[u8] = b"[SMP: Calling PSCI for CPU ";
            for &b in MSG2 {
                putchar(b);
            }
            let hex = b"0123456789ABCDEF";
            putchar(hex[cpu_id as usize]);
            const MSG3: &[u8] = b"]\n";
            for &b in MSG3 {
                putchar(b);
            }
        }

        let mpidr = cpu_id as u64; // QEMU virt 的 CPU MPIDR 就是 CPU ID

        unsafe {
            // PSCI_CPU_ON HVC call (Hypervisor Call)
            // x0 = function ID (0xC4000003 = PSCI_CPU_ON)
            // x1 = target CPU (MPIDR)
            // x2 = entry point (physical address)
            // x3 = context ID
            let mut result: u64;
            core::arch::asm!(
                "hvc #0",
                inlateout("x0") 0xC4000003u64 => result,
                in("x1") mpidr,
                in("x2") secondary_entry as u64,
                in("x3") 0u64,
                options(nomem, nostack)
            );

            unsafe {
                const MSG4: &[u8] = b"[SMP: PSCI result = ";
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
                putchar(b']');
                putchar(b'\n');
            }

            // 检查返回值 (0 = success)
            if result != 0 {
                let msg = b"[SMP: CPU boot failed]\n";
                for &b in msg {
                    putchar(b);
                }
            } else {
                let msg = b"[SMP: PSCI success]\n";
                for &b in msg {
                    putchar(b);
                }
            }
        }
    }
}

/// 外部符号声明
extern "C" {
    /// 次核启动入口点（汇编代码）
    fn secondary_entry();
}
