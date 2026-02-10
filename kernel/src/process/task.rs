//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 任务控制块 (Task Control Block)
//!
//! 遵循 Linux 内核的 `struct task_struct` 定义 (include/linux/sched.h)
//!
//! 关键设计要点：
//! 1. 进程状态必须与 Linux 完全一致
//! 2. 调度相关字段与 Linux 对齐
//! 3. PID/TGID 遵循 Linux 的命名和语义

use core::sync::atomic::{AtomicU32, Ordering};
use core::ptr;
use crate::mm::pagemap::AddressSpace;
use crate::fs::FdTable;
use crate::signal::{SignalStruct, SigPending};
use alloc::boxed::Box;
use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::mem::offset_of;
use super::list::ListHead;

/// 内核栈大小 (16KB = 4 个页面)
///
/// 对应 Linux 内核的 THREAD_SIZE (arch/riscv64/include/asm/thread_info.h)
/// RISC-V 通常使用 16KB 内核栈
const KERNEL_STACK_SIZE: usize = 16384;  // 16KB

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

    /// 信号掩码 (blocked)
    ///
    /// 对应 Linux 的 blocked 信号集 (sigset_t)
    /// 用于 sigprocmask 系统调用
    pub sigmask: u64,

    /// 信号栈 (sigaltstack)
    pub sigstack: crate::signal::SignalStack,

    /// 信号帧地址（在用户空间）
    pub sigframe_addr: u64,

    /// 信号帧（内核空间备份）
    pub sigframe: Option<crate::signal::SignalFrame>,

    /// 父进程
    parent: Option<*const Task>,

    /// 退出码 (Zombie 状态时有效)
    exit_code: i32,

    /// 子进程列表
    ///
    /// 对应 Linux 的 `task_struct::children` (include/linux/sched.h)
    /// 这是一个链表头，所有子进程通过各自的 sibling 字段链接到此
    pub children: ListHead,

    /// 兄弟进程链表节点
    ///
    /// 对应 Linux 的 `task_struct::sibling` (include/linux/sched.h)
    /// 用于将此进程链接到父进程的 children 链表中
    pub sibling: ListHead,

    /// 父进程的 children 链表头指针（用于 next_sibling 边界检测）
    ///
    /// 当进程添加到父进程时，保存父进程 children 的地址
    /// 用于 next_sibling() 判断是否到达链表末尾
    parent_children_head: *mut ListHead,
}

impl Task {
    /// 创建新任务
    ///
    /// 对应 Linux 内核的 fork() / copy_process()
    pub fn new(pid: Pid, policy: SchedPolicy) -> Self {
        // 根据 Linux 内核的调度优先级计算
        // PRIO_TO_PRIO: static_prio 120 -> prio 120
        let static_prio = 120; // DEFAULT_PRIO (include/linux/sched/prio.h)
        let normal_prio = static_prio; // SCHED_NORMAL 时 normal_prio == static_prio
        let prio = normal_prio;

        // Idle 任务不需要文件描述符表和信号处理
        // 暂时禁用 FdTable 和 Signal 创建，避免堆分配问题
        let (fdtable, signal) = (None, None);

        let state = AtomicU32::new(TaskState::Running as u32);
        let context = CpuContext::default();
        let pending = SigPending::new();
        let sigstack = crate::signal::SignalStack::new();

        let mut task = Self {
            state,
            pid,
            tgid: pid, // 单线程进程 tgid == pid
            policy,
            prio,
            static_prio,
            normal_prio,
            time_slice: DEFAULT_TIME_SLICE, // 默认时间片 (10 个时钟中断 = 100ms)
            context,
            kernel_stack: None,
            address_space: None,
            fdtable,
            signal,
            pending,
            sigmask: 0,  // 初始信号掩码为空
            sigstack,
            sigframe_addr: 0,
            sigframe: None,
            parent: None,
            exit_code: 0,
            children: ListHead::new(),
            sibling: ListHead::new(),
            parent_children_head: ptr::null_mut(),
        };

        // 初始化 children 和 sibling 链表（必须在结构体构造后）
        task.children.init();
        task.sibling.init();

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
        use core::ptr;
        use core::mem::offset_of;

        // 使用 ptr::write 和 offset_of 来安全地初始化每个字段
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::Running as u32),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pid)) as *mut Pid,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, tgid)) as *mut Pid,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, policy)) as *mut SchedPolicy,
            SchedPolicy::Idle,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, static_prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, normal_prio)) as *mut i32,
            120,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, time_slice)) as *mut u32,
            100,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, context)) as *mut CpuContext,
            CpuContext::default(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, kernel_stack)) as *mut Option<*mut u8>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fdtable)) as *mut Option<Box<FdTable>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, signal)) as *mut Option<Box<SignalStruct>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pending)) as *mut SigPending,
            SigPending::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigmask)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigstack)) as *mut crate::signal::SignalStack,
            crate::signal::SignalStack::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe_addr)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe)) as *mut Option<crate::signal::SignalFrame>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent)) as *mut Option<*mut Task>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, exit_code)) as *mut i32,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent_children_head)) as *mut *mut ListHead,
            ptr::null_mut(),
        );

        // 初始化 children 和 sibling 链表
        let children_ptr = (ptr as usize + offset_of!(Task, children)) as *mut ListHead;
        (*children_ptr).init();
        let sibling_ptr = (ptr as usize + offset_of!(Task, sibling)) as *mut ListHead;
        (*sibling_ptr).init();
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
        use core::ptr;
        use core::mem::offset_of;

        const MSG: &[u8] = b"Task::new_task_at: start\n";
        for &b in MSG {
            putchar(b);
        }

        // 根据 Linux 内核的调度优先级计算
        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio;
        let prio = normal_prio;

        // 写入各个字段
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::Running as u32),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pid)) as *mut Pid,
            pid,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, tgid)) as *mut Pid,
            pid,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, policy)) as *mut SchedPolicy,
            policy,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, prio)) as *mut i32,
            prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, static_prio)) as *mut i32,
            static_prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, normal_prio)) as *mut i32,
            normal_prio,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, time_slice)) as *mut u32,
            HZ,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, context)) as *mut CpuContext,
            CpuContext::default(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, kernel_stack)) as *mut Option<*mut u8>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fdtable)) as *mut Option<Box<FdTable>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, signal)) as *mut Option<Box<SignalStruct>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, pending)) as *mut SigPending,
            SigPending::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigmask)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigstack)) as *mut crate::signal::SignalStack,
            crate::signal::SignalStack::new(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe_addr)) as *mut u64,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, sigframe)) as *mut Option<crate::signal::SignalFrame>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent)) as *mut Option<*mut Task>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, exit_code)) as *mut i32,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, parent_children_head)) as *mut *mut ListHead,
            ptr::null_mut(),
        );

        // 初始化 children 和 sibling 链表
        let children_ptr = (ptr as usize + offset_of!(Task, children)) as *mut ListHead;
        (*children_ptr).init();
        let sibling_ptr = (ptr as usize + offset_of!(Task, sibling)) as *mut ListHead;
        (*sibling_ptr).init();

        // 分配内核栈
        let task_ref = &mut *ptr;
        if task_ref.alloc_kernel_stack().is_none() {
            const MSG_ERR: &[u8] = b"Task::new_task_at: failed to allocate kernel stack\n";
            for &b in MSG_ERR {
                putchar(b);
            }
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

    /// 进程睡眠和唤醒机制

    /// 使当前进程进入睡眠状态
    ///
    /// 对应 Linux 内核的 set_current_state() + schedule()
    /// (kernel/sched/core.c)
    ///
    /// 进程调用此函数后会进入睡眠状态，并触发调度
    ///
    /// # 参数
    /// - `state`: 睡眠状态（TaskState::Interruptible 或 Uninterruptible）
    ///
    /// # Safety
    /// 调用此函数后，当前进程会被调度出去，直到被唤醒
    ///
    /// # 示例
    /// ```no_run
    /// # use rux::process::task::TaskState;
    /// // 可中断睡眠（可被信号唤醒）
    /// Task::sleep(TaskState::Interruptible);
    ///
    /// // 不可中断睡眠
    /// Task::sleep(TaskState::Uninterruptible);
    /// ```
    #[inline(never)]
    pub fn sleep(state: TaskState) {
        // 设置当前进程为睡眠状态
        // 对应 Linux 的 set_current_state()
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(state);
            }
        }

        // 触发调度，选择其他进程运行
        // 对应 Linux 的 schedule()
        crate::sched::schedule();
    }

    /// 唤醒进程
    ///
    /// 对应 Linux 内核的 try_to_wake_up() (kernel/sched/core.c)
    ///
    /// 将进程从睡眠状态唤醒，使其可以再次被调度
    ///
    /// # 参数
    /// - `task`: 要唤醒的进程
    ///
    /// # 返回
    /// - true: 成功唤醒
    /// - false: 进程不在睡眠状态
    ///
    /// # 示例
    /// ```no_run
    /// # use rux::sched;
    /// if let Some(child) = sched::find_task_by_pid(2) {
    ///     sched::wake_up_process(child);
    /// }
    /// ```
    #[inline(never)]
    pub fn wake_up(task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }

        unsafe {
            let old_state = (*task).state();

            // 只有在睡眠状态时才需要唤醒
            // 对应 Linux 的 task_is_running() 检查
            match old_state {
                TaskState::Interruptible | TaskState::Uninterruptible => {
                    // 唤醒进程：设置为 Running 状态
                    // 对应 Linux 的 ttwu_do_wakeup()
                    (*task).set_state(TaskState::Running);

                    // 设置 need_resched 标志，触发重新调度
                    // 对应 Linux 的 resched_curr()
                    crate::sched::set_need_resched();

                    true
                }
                _ => false,
            }
        }
    }

    /// 获取 PID
    #[inline]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// 抢占式调度支持

    /// 减少时间片
    ///
    /// 对应 Linux 内核的 scheduler_tick() 中更新时间片的逻辑
    ///
    /// # 返回
    /// - true: 时间片还有剩余
    /// - false: 时间片已用完
    #[inline]
    pub fn tick_time_slice(&mut self) -> bool {
        if self.time_slice > 0 {
            self.time_slice -= 1;
            true
        } else {
            false
        }
    }

    /// 重置时间片
    ///
    /// 当进程被重新调度到 CPU 时调用
    #[inline]
    pub fn reset_time_slice(&mut self) {
        self.time_slice = DEFAULT_TIME_SLICE;
    }

    /// 检查时间片是否用完
    #[inline]
    pub fn time_slice_expired(&self) -> bool {
        self.time_slice == 0
    }

    /// 获取剩余时间片
    #[inline]
    pub fn get_time_slice(&self) -> u32 {
        self.time_slice
    }

    /// 抢占式调度支持结束

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

    /// 分配内核栈
    ///
    /// 对应 Linux 的 `alloc_thread_stack_node()` (kernel/fork.c)
    ///
    /// 为当前任务分配一个内核栈，大小为 KERNEL_STACK_SIZE (16KB)
    ///
    /// # 返回
    /// 成功返回 Some(栈顶地址)，失败返回 None
    pub fn alloc_kernel_stack(&mut self) -> Option<*mut u8> {
        unsafe {
            // 使用全局分配器分配内核栈
            let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
                .ok()?;

            let stack_ptr = alloc(layout);

            if !stack_ptr.is_null() {
                // 清零栈空间
                core::ptr::write_bytes(stack_ptr, 0, KERNEL_STACK_SIZE);

                // 设置栈顶地址（栈向下增长）
                let stack_top = stack_ptr.add(KERNEL_STACK_SIZE);
                self.kernel_stack = Some(stack_top);

                Some(stack_top)
            } else {
                None
            }
        }
    }

    /// 释放内核栈
    ///
    /// 对应 Linux 的 `free_thread_stack_node()` (kernel/fork.c)
    ///
    /// 释放当前任务的内核栈
    pub fn free_kernel_stack(&mut self) {
        if let Some(stack_top) = self.kernel_stack {
            unsafe {
                // 计算栈底地址（栈顶 - 栈大小）
                let stack_bottom = stack_top.sub(KERNEL_STACK_SIZE);

                // 创建 Layout 用于释放内存
                let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
                    .unwrap_or_else(|_| Layout::new::<[u8; KERNEL_STACK_SIZE]>());

                // 释放内存
                dealloc(stack_bottom, layout);
            }

            // 清零引用
            self.kernel_stack = None;
        }
    }

    /// 获取内核栈顶地址
    ///
    /// 用于上下文切换时设置 SP 寄存器
    pub fn get_kernel_stack(&self) -> Option<*mut u8> {
        self.kernel_stack
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

    // ==================== 进程树管理 (Process Tree Management) ====================
    // 以下函数实现 Linux 风格的进程树管理，对应 kernel/sched/core.c

    /// 获取第一个子进程
    ///
    /// 对应 Linux 的 `list_first_entry(&parent->children, struct task_struct, sibling)`
    ///
    /// # 返回
    /// 如果有子进程返回 Some(子进程指针)，否则返回 None
    pub fn first_child(&self) -> Option<*mut Task> {
        unsafe {
            // children 链表可能为空
            if self.children.is_empty() {
                return None;
            }

            // 从 children 链表头获取第一个 sibling 节点
            // 然后使用 list_entry 获取包含该 sibling 的 Task 结构体
            let first_sibling = self.children.next;
            // 计算包含该 sibling 的 Task 结构体指针
            // sibling 字段位于 Task 结构体末尾
            let task_ptr = (first_sibling as usize - offset_of!(Task, sibling)) as *mut Task;
            Some(task_ptr)
        }
    }

    /// 获取下一个兄弟进程
    ///
    /// 对应 Linux 的 `list_next_entry(current, sibling)`
    ///
    /// # Safety
    /// 调用者必须确保 self 不是父进程的 children 链表头
    ///
    /// # 返回
    /// 如果有下一个兄弟进程返回 Some(指针)，否则返回 None
    pub unsafe fn next_sibling(&self) -> Option<*mut Task> {
        // 如果没有保存父进程的 children 链表头，说明不在任何父进程的 children 列表中
        if self.parent_children_head.is_null() {
            return None;
        }

        let next_sibling = self.sibling.next;

        // 如果 next 指向父进程的 children 链表头，说明已经到达链表末尾
        if next_sibling == self.parent_children_head {
            return None;
        }

        // 计算包含该 sibling 的 Task 结构体指针
        let task_ptr = (next_sibling as usize - offset_of!(Task, sibling)) as *mut Task;
        Some(task_ptr)
    }

    /// 检查是否有子进程
    ///
    /// # 返回
    /// 如果有子进程返回 true，否则返回 false
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// 添加子进程到进程树
    ///
    /// 对应 Linux 内核的 fork() 中将子进程添加到父进程的 children 链表
    ///
    /// # Safety
    /// 调用者必须确保：
    /// - self 是有效的父进程引用
    /// - child 是有效的子进程指针
    /// - child 不在任何进程树中
    ///
    /// # 参数
    /// - `child`: 要添加的子进程指针
    ///
    /// # 对应 Linux
    /// `copy_process()` -> `fork()` -> `list_add_tail_rcu(&p->sibling, &parent->children)`
    pub unsafe fn add_child(&self, child: *mut Task) {
        // 设置子进程的父进程
        (*child).parent = Some(self as *const _ as *mut Task);

        // 保存父进程的 children 链表头指针（用于 next_sibling 边界检测）
        (*child).parent_children_head = &self.children as *const _ as *mut ListHead;

        // 将子进程的 sibling 链接到父进程的 children 链表
        // 使用 add_tail 添加到链表尾部
        (*child).sibling.add_tail(&self.children as *const _ as *mut ListHead);
    }

    /// 从进程树中移除子进程
    ///
    /// 对应 Linux 内核的 `release_task()` 或 `__exit_signal()` 中的进程移除
    ///
    /// # Safety
    /// 调用者必须确保：
    /// - child 是有效的子进程指针
    /// - child 在当前进程的 children 链表中
    ///
    /// # 参数
    /// - `child`: 要移除的子进程指针
    ///
    /// # 对应 Linux
    /// `release_task()` -> `list_del_init(&p->sibling)`
    pub unsafe fn remove_child(&self, child: *mut Task) {
        // 从父进程的 children 链表中移除子进程的 sibling
        (*child).sibling.del();

        // 重新初始化 sibling 链表（防止悬空指针）
        (*child).sibling.init();

        // 清除父进程指针
        (*child).parent = None;

        // 清除父进程 children 链表头指针
        (*child).parent_children_head = ptr::null_mut();
    }

    /// 遍历所有子进程
    ///
    /// 对应 Linux 内核的 `for_each_process()`
    ///
    /// # 参数
    /// - `f`: 对每个子进程调用的闭包
    ///
    /// # Safety
    /// 调用者必须确保 self 是有效的，且在遍历期间不修改进程树
    ///
    /// # 对应 Linux
    /// `for_each_process(task)` 或 `list_for_each(pos, &parent->children)`
    pub unsafe fn for_each_child<F>(&self, mut f: F)
    where
        F: FnMut(*mut Task),
    {
        let head = &self.children as *const _ as *mut ListHead;
        let mut iterations = 0usize;
        ListHead::for_each(head, |node| {
            iterations += 1;
            if iterations > 1000 {
                // 防止无限循环
                return;
            }
            let task_ptr = (node as usize - offset_of!(Task, sibling)) as *mut Task;
            f(task_ptr);
        });
    }

    /// 根据 PID 查找子进程
    ///
    /// # 参数
    /// - `pid`: 要查找的进程 ID
    ///
    /// # 返回
    /// 如果找到返回 Some(子进程指针)，否则返回 None
    ///
    /// # Safety
    /// 调用者必须确保 self 是有效的
    pub unsafe fn find_child_by_pid(&self, pid: Pid) -> Option<*mut Task> {
        let head = &self.children as *const _ as *mut ListHead;
        let mut result = None;
        let mut iterations = 0usize;
        ListHead::for_each(head, |node| {
            iterations += 1;
            if iterations > 1000 {
                // 防止无限循环
                return;
            }
            let task_ptr = (node as usize - offset_of!(Task, sibling)) as *mut Task;
            if (*task_ptr).pid == pid {
                result = Some(task_ptr);
            }
        });
        result
    }

    /// 获取子进程数量
    ///
    /// # 返回
    /// 子进程的数量
    ///
    /// # Safety
    /// 调用者必须确保 self 是有效的
    pub unsafe fn count_children(&self) -> usize {
        let head = &self.children as *const _ as *mut ListHead;
        let mut count = 0;
        ListHead::for_each(head, |_| {
            count += 1;
        });
        count
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

/// 默认时间片 (以时钟中断为单位)
///
/// 对应 Linux 内核的 `sched_timeslice_ns` 和 `sysctl_sched_rt_period`
///
/// 100ms / 10ms = 10 个时钟中断
const DEFAULT_TIME_SLICE: u32 = 10;
