//! 任务控制块 (Task Control Block)
//!
//! 遵循 Linux 内核的 `struct task_struct` 定义 (include/linux/sched.h)
//!
//! 关键设计要点：
//! 1. 进程状态必须与 Linux 完全一致
//! 2. 调度相关字段与 Linux 对齐
//! 3. PID/TGID 遵循 Linux 的命名和语义

use core::sync::atomic::{AtomicU32, Ordering};
use crate::mm::pagemap::AddressSpace;
use crate::fs::FdTable;
use crate::signal::{SignalStruct, SigPending};
use alloc::boxed::Box;

/// 进程状态 - 必须与 Linux 完全一致
///
/// 对应 Linux 内核的 task_state_t (include/linux/sched.h)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TaskState {
    /// 可运行状态 (TASK_RUNNING)
    /// 进程在 CPU 上运行或在运行队列中等待
    Running = 0,

    /// 可中断睡眠 (TASK_INTERRUPTIBLE)
    /// 进程在等待某个事件，可被信号唤醒
    Interruptible = 1,

    /// 不可中断睡眠 (TASK_UNINTERRUPTIBLE)
    /// 进程在等待某个事件，不能被信号唤醒
    Uninterruptible = 2,

    /// 僵死状态 (EXIT_ZOMBIE)
    /// 进程已退出，但父进程尚未等待 (wait)
    Zombie = 4,

    /// 停止状态 (TASK_STOPPED)
    /// 进程被信号停止 (SIGSTOP, SIGTSTP, etc.)
    Stopped = 8,

    /// 死亡状态 (EXIT_DEAD)
    /// 进程最终状态，将被回收
    Dead = 16,
}

/// 调度策略 - 必须与 Linux 完全一致
///
/// 对应 Linux 内核的调度策略 (include/linux/sched.h)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SchedPolicy {
    /// 普通分时调度 (SCHED_NORMAL)
    Normal = 0,

    /// FIFO 实时调度 (SCHED_FIFO)
    Fifo = 1,

    /// RR 实时调度 (SCHED_RR)
    Rr = 2,

    /// 批处理调度 (SCHED_BATCH)
    Batch = 3,

    /// 空闲调度 (SCHED_IDLE)
    Idle = 5,

    /// deadline 调度 (SCHED_DEADLINE)
    Deadline = 6,
}

/// 任务标志 (task flags)
///
/// 对应 Linux 内核的 task_struct::flags
pub mod task_flags {
    use bitflags::bitflags;

    bitflags! {
        /// PF_* flags from Linux (include/linux/sched.h)
        pub struct TaskFlags: u32 {
            const PF_KTHREAD     = 0x00200000; /* I am a kernel thread */
            const PF_EXITING     = 0x00000004; /* Getting shut down */
            const PF_VCPU        = 0x00000010; /* I'm a virtual CPU */
            const PF_WQ_WORKER   = 0x00000020; /* I'm a workqueue worker */
        }
    }
}

/// CPU 上下文 - 进程切换时保存/恢复的寄存器
///
/// 对应 Linux 内核的 struct pt_regs (arch/arm64/include/asm/ptrace.h)
/// 以及进程切换时的 cpu_context (arch/arm64/kernel/process.c)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct CpuContext {
    /// 通用寄存器 x19-x28 (被调用者保存)
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,

    /// 帧指针 (x29)
    pub fp: u64,

    /// 链接寄存器 (x30)
    pub sp: u64,

    /// 程序计数器 (PC) - 进程恢复执行的位置
    pub pc: u64,

    // 信号处理需要的额外寄存器
    /// 参数寄存器 x0-x7 (用于信号处理函数参数)
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,

    /// 用户栈指针 (SP_EL0)
    pub user_sp: u64,

    /// 用户程序状态寄存器 (SPSR_EL0 保存值)
    pub user_spsr: u64,
}

impl Default for CpuContext {
    fn default() -> Self {
        Self {
            x19: 0, x20: 0, x21: 0, x22: 0,
            x23: 0, x24: 0, x25: 0, x26: 0,
            x27: 0, x28: 0, fp: 0, sp: 0, pc: 0,
            x0: 0, x1: 0, x2: 0, x3: 0,
            x4: 0, x5: 0, x6: 0, x7: 0,
            user_sp: 0, user_spsr: 0,
        }
    }
}

/// 进程标识符 (PID 类型)
///
/// 遵循 Linux 内核的 pid_t 类型
pub type Pid = u32;

/// 任务控制块 (Task Control Block)
///
/// 完全遵循 Linux 内核的 `struct task_struct` (include/linux/sched.h)
///
/// 核心字段对应关系：
/// - state: task_struct::state
/// - pid: task_struct::pid
/// - tgid: task_struct::tgid (线程组 ID)
/// - prio: task_struct::prio (动态优先级)
/// - static_prio: task_struct::static_prio (静态优先级)
/// - normal_prio: task_struct::normal_prio
/// - policy: task_struct::policy
/// - context: cpu_context (arch/arm64/kernel/process.c)
/// - mm: task_struct::mm (内存描述符)
/// - files: task_struct::files (文件描述符表)
/// - signal: task_struct::signal (信号处理)
pub struct Task {
    /// 进程状态 (volatile, 多核可见)
    state: AtomicU32,

    /// 进程 ID
    pid: Pid,

    /// 线程组 ID (线程的主进程 PID)
    /// 单线程进程: tgid == pid
    tgid: Pid,

    /// 调度策略
    policy: SchedPolicy,

    /// 动态优先级 (0-139, 数值越大优先级越低)
    /// - 0-99: 实时进程
    /// - 100-139: 普通进程
    prio: i32,

    /// 静态优先级 (120 是普通进程的默认值)
    static_prio: i32,

    /// normal_prio: 基于 static_prio 和调度策略计算的优先级
    normal_prio: i32,

    /// 时间片剩余
    time_slice: u32,

    /// CPU 上下文
    context: CpuContext,

    /// 内核栈
    /// TODO: 实现内核栈分配
    kernel_stack: Option<*mut u8>,

    /// 地址空间 (mm_struct)
    /// 内核线程为 None，用户进程为 Some
    address_space: Option<AddressSpace>,

    /// 文件描述符表 (files_struct)
    /// 使用 Box 以减少 Task 的大小
    fdtable: Option<Box<FdTable>>,

    /// 信号处理结构 (signal_struct)
    /// 使用 Box 以减少 Task 的大小
    pub signal: Option<Box<SignalStruct>>,

    /// 待处理信号 (pending)
    pub pending: SigPending,

    /// 信号栈 (sigaltstack)
    pub sigstack: crate::signal::SignalStack,

    /// 父进程
    parent: Option<*const Task>,

    /// 退出码 (Zombie 状态时有效)
    exit_code: i32,

    // 子进程列表 (TODO: 实现链表)
    // children: ListHead,

    // 兄弟进程列表 (TODO: 实现链表)
    // sibling: ListHead,
}

impl Task {
    /// 创建新任务
    ///
    /// 对应 Linux 内核的 fork() / copy_process()
    pub fn new(pid: Pid, policy: SchedPolicy) -> Self {
        use crate::console::putchar;
        const MSG1: &[u8] = b"Task::new: start\n";
        for &b in MSG1 {
            unsafe { putchar(b); }
        }

        // 根据 Linux 内核的调度优先级计算
        // PRIO_TO_PRIO: static_prio 120 -> prio 120
        let static_prio = 120; // DEFAULT_PRIO (include/linux/sched/prio.h)
        let normal_prio = static_prio; // SCHED_NORMAL 时 normal_prio == static_prio
        let prio = normal_prio;

        const MSG2: &[u8] = b"Task::new: before fdtable check\n";
        for &b in MSG2 {
            unsafe { putchar(b); }
        }

        // Idle 任务不需要文件描述符表和信号处理
        // 暂时禁用 FdTable 和 Signal 创建，避免堆分配问题
        let (fdtable, signal) = (None, None);

        const MSG3: &[u8] = b"Task::new: before Self construction\n";
        for &b in MSG3 {
            unsafe { putchar(b); }
        }

        const MSG4: &[u8] = b"Task::new: creating task struct\n";
        for &b in MSG4 {
            unsafe { putchar(b); }
        }

        const MSG4a: &[u8] = b"Task::new: before AtomicU32::new\n";
        for &b in MSG4a {
            unsafe { putchar(b); }
        }
        let state = AtomicU32::new(TaskState::Running as u32);

        const MSG4b: &[u8] = b"Task::new: before CpuContext::default\n";
        for &b in MSG4b {
            unsafe { putchar(b); }
        }
        let context = CpuContext::default();

        const MSG4c: &[u8] = b"Task::new: before SigPending::new\n";
        for &b in MSG4c {
            unsafe { putchar(b); }
        }
        let pending = SigPending::new();

        const MSG4c2: &[u8] = b"Task::new: before SignalStack::new\n";
        for &b in MSG4c2 {
            unsafe { putchar(b); }
        }
        let sigstack = crate::signal::SignalStack::new();

        const MSG4d: &[u8] = b"Task::new: before struct construction\n";
        for &b in MSG4d {
            unsafe { putchar(b); }
        }

        let task = Self {
            state,
            pid,
            tgid: pid, // 单线程进程 tgid == pid
            policy,
            prio,
            static_prio,
            normal_prio,
            time_slice: HZ, // 默认时间片 (100Hz -> 10ms)
            context,
            kernel_stack: None,
            address_space: None,
            fdtable,
            signal,
            pending,
            sigstack,
            parent: None,
            exit_code: 0,
        };

        const MSG5: &[u8] = b"Task::new: done\n";
        for &b in MSG5 {
            unsafe { putchar(b); }
        }

        task
    }

    /// 在指定内存位置构造 idle task
    ///
    /// 这个函数避免在栈上创建大对象，直接在给定地址构造 Task
    ///
    /// # Safety
    ///
    /// ptr 必须是对齐且足够大的内存块
    pub unsafe fn new_idle_at(ptr: *mut Task) {
        use crate::console::putchar;
        const MSG: &[u8] = b"Task::new_idle_at: start\n";
        for &b in MSG {
            putchar(b);
        }

        // 手动初始化每个字段，避免在栈上创建 Task
        const MSG2: &[u8] = b"Task::new_idle_at: writing fields\n";
        for &b in MSG2 {
            putchar(b);
        }

        // 写入各个字段
        core::ptr::addr_of_mut!((*ptr).state).write(AtomicU32::new(TaskState::Running as u32));
        core::ptr::addr_of_mut!((*ptr).pid).write(0);
        core::ptr::addr_of_mut!((*ptr).tgid).write(0);
        core::ptr::addr_of_mut!((*ptr).policy).write(SchedPolicy::Idle);
        core::ptr::addr_of_mut!((*ptr).prio).write(120);
        core::ptr::addr_of_mut!((*ptr).static_prio).write(120);
        core::ptr::addr_of_mut!((*ptr).normal_prio).write(120);
        core::ptr::addr_of_mut!((*ptr).time_slice).write(100);
        core::ptr::addr_of_mut!((*ptr).context).write(CpuContext::default());
        core::ptr::addr_of_mut!((*ptr).kernel_stack).write(None);
        core::ptr::addr_of_mut!((*ptr).address_space).write(None);
        core::ptr::addr_of_mut!((*ptr).fdtable).write(None);  // Idle task 不需要 fdtable
        core::ptr::addr_of_mut!((*ptr).signal).write(None);  // Idle task 不需要 signal
        core::ptr::addr_of_mut!((*ptr).pending).write(SigPending::new());
        core::ptr::addr_of_mut!((*ptr).sigstack).write(crate::signal::SignalStack::new());
        core::ptr::addr_of_mut!((*ptr).parent).write(None);
        core::ptr::addr_of_mut!((*ptr).exit_code).write(0);

        const MSG3: &[u8] = b"Task::new_idle_at: done\n";
        for &b in MSG3 {
            putchar(b);
        }
    }

    /// 在指定内存位置构造普通 task
    ///
    /// 这个函数避免在栈上创建大对象，直接在给定地址构造 Task
    ///
    /// # Safety
    ///
    /// ptr 必须是对齐且足够大的内存块
    pub unsafe fn new_task_at(ptr: *mut Task, pid: Pid, policy: SchedPolicy) {
        use crate::console::putchar;
        const MSG: &[u8] = b"Task::new_task_at: start\n";
        for &b in MSG {
            putchar(b);
        }

        // 根据 Linux 内核的调度优先级计算
        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio;
        let prio = normal_prio;

        // 手动初始化每个字段，避免在栈上创建 Task
        const MSG2: &[u8] = b"Task::new_task_at: writing fields\n";
        for &b in MSG2 {
            putchar(b);
        }

        // 写入各个字段
        core::ptr::addr_of_mut!((*ptr).state).write(AtomicU32::new(TaskState::Running as u32));
        core::ptr::addr_of_mut!((*ptr).pid).write(pid);
        core::ptr::addr_of_mut!((*ptr).tgid).write(pid);
        core::ptr::addr_of_mut!((*ptr).policy).write(policy);
        core::ptr::addr_of_mut!((*ptr).prio).write(prio);
        core::ptr::addr_of_mut!((*ptr).static_prio).write(static_prio);
        core::ptr::addr_of_mut!((*ptr).normal_prio).write(normal_prio);
        core::ptr::addr_of_mut!((*ptr).time_slice).write(HZ);
        core::ptr::addr_of_mut!((*ptr).context).write(CpuContext::default());
        core::ptr::addr_of_mut!((*ptr).kernel_stack).write(None);
        core::ptr::addr_of_mut!((*ptr).address_space).write(None);
        core::ptr::addr_of_mut!((*ptr).fdtable).write(None);  // 暂时不分配 fdtable
        core::ptr::addr_of_mut!((*ptr).signal).write(None);  // 暂时不分配 signal
        core::ptr::addr_of_mut!((*ptr).pending).write(SigPending::new());
        core::ptr::addr_of_mut!((*ptr).sigstack).write(crate::signal::SignalStack::new());
        core::ptr::addr_of_mut!((*ptr).parent).write(None);
        core::ptr::addr_of_mut!((*ptr).exit_code).write(0);

        const MSG3: &[u8] = b"Task::new_task_at: done\n";
        for &b in MSG3 {
            putchar(b);
        }
    }

    /// 获取进程状态
    #[inline]
    pub fn state(&self) -> TaskState {
        match self.state.load(Ordering::Relaxed) {
            0 => TaskState::Running,
            1 => TaskState::Interruptible,
            2 => TaskState::Uninterruptible,
            4 => TaskState::Zombie,
            8 => TaskState::Stopped,
            16 => TaskState::Dead,
            _ => TaskState::Running, // 默认
        }
    }

    /// 设置进程状态
    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state as u32, Ordering::Release);
    }

    /// 获取 PID
    #[inline]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// 获取父进程 PID (PPID)
    #[inline]
    pub fn ppid(&self) -> Pid {
        match self.parent {
            Some(parent_ptr) => unsafe { (*parent_ptr).pid },
            None => 0, // 没有父进程，返回 0
        }
    }

    /// 获取 TGID
    #[inline]
    pub fn tgid(&self) -> Pid {
        self.tgid
    }

    /// 获取 CPU 上下文的可变引用
    pub fn context_mut(&mut self) -> &mut CpuContext {
        &mut self.context
    }

    /// 获取 CPU 上下文的引用
    pub fn context(&self) -> &CpuContext {
        &self.context
    }

    /// 获取地址空间的可变引用
    pub fn address_space_mut(&mut self) -> Option<&mut AddressSpace> {
        self.address_space.as_mut()
    }

    /// 获取地址空间的引用
    pub fn address_space(&self) -> Option<&AddressSpace> {
        self.address_space.as_ref()
    }

    /// 设置地址空间
    pub fn set_address_space(&mut self, addr_space: Option<AddressSpace>) {
        self.address_space = addr_space;
    }

    /// 是否有地址空间（用户进程）
    #[inline]
    pub fn has_address_space(&self) -> bool {
        self.address_space.is_some()
    }

    /// 检查是否有文件描述符表
    #[inline]
    pub fn has_fdtable(&self) -> bool {
        self.fdtable.is_some()
    }

    /// 获取文件描述符表 (Option 版本)
    #[inline]
    pub fn try_fdtable(&self) -> Option<&FdTable> {
        self.fdtable.as_ref().map(|b| b.as_ref())
    }

    /// 获取文件描述符表
    #[inline]
    pub fn fdtable(&self) -> &FdTable {
        self.fdtable.as_ref().expect("FdTable not initialized")
    }

    /// 获取文件描述符表的可变引用 (Option 版本)
    #[inline]
    pub fn try_fdtable_mut(&mut self) -> Option<&mut FdTable> {
        self.fdtable.as_mut().map(|b| b.as_mut())
    }

    /// 获取文件描述符表的可变引用
    #[inline]
    pub fn fdtable_mut(&mut self) -> &mut FdTable {
        self.fdtable.as_mut().expect("FdTable not initialized")
    }

    /// 设置父进程
    pub fn set_parent(&mut self, parent: *const Task) {
        self.parent = Some(parent);
    }

    /// 获取父进程指针
    #[inline]
    pub fn parent_ptr(&self) -> Option<*const Task> {
        self.parent
    }

    /// 获取退出码
    #[inline]
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// 设置退出码
    #[inline]
    pub fn set_exit_code(&mut self, code: i32) {
        self.exit_code = code;
    }

    /// 获取待处理信号队列的引用
    #[inline]
    pub fn pending(&self) -> &crate::signal::SigPending {
        &self.pending
    }
}

/// HZ: 时钟频率 (与 Linux 内核一致)
///
/// Linux 默认 CONFIG_HZ=100 (每秒 100 次时钟中断)
/// 可选: 100, 250, 300, 1000
const HZ: u32 = 100;
