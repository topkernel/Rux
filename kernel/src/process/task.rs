//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 任务控制块 (Task Control Block)
//!
//!
//! 关键设计要点：

use core::sync::atomic::{AtomicU32, Ordering};
use core::ptr;
use crate::mm::pagemap::AddressSpace;
use crate::fs::FdTable;
use crate::signal::{SignalStruct, SigPending};
use crate::config::TIME_SLICE_TICKS as DEFAULT_TIME_SLICE;
use alloc::boxed::Box;
use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::mem::offset_of;
use crate::list::ListHead;

/// 内核栈大小 (32KB = 8 个页面)
///
/// RISC-V 通常使用 16KB 内核栈，但我们增加到 32KB
/// 因为某些操作（如 FdTable 创建）需要较大的栈空间
const KERNEL_STACK_SIZE: usize = 32768;  // 32KB

/// 进程状态标志（位图形式，参考 Linux）
///
/// Linux 使用位图来表示进程状态，允许组合状态
/// 例如：TASK_UNINTERRUPTIBLE | __TASK_STOPPED
///
/// 参考：include/linux/sched.h
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState(u32);

impl TaskState {
    /// 可运行状态 (TASK_RUNNING)
    /// 进程在 CPU 上运行或在运行队列中等待
    pub const RUNNING: u32 = 0x00000000;

    /// 可中断睡眠 (TASK_INTERRUPTIBLE)
    /// 进程在等待某个事件，可被信号唤醒
    pub const INTERRUPTIBLE: u32 = 0x00000001;

    /// 不可中断睡眠 (TASK_UNINTERRUPTIBLE)
    /// 进程在等待某个事件，不能被信号唤醒
    pub const UNINTERRUPTIBLE: u32 = 0x00000002;

    /// 停止状态 (__TASK_STOPPED)
    /// 进程被信号停止 (SIGSTOP, SIGTSTP, etc.)
    pub const STOPPED: u32 = 0x00000004;

    /// 跟踪状态 (__TASK_TRACED)
    /// 进程被 ptrace 跟踪
    pub const TRACED: u32 = 0x00000008;

    /// 退出僵死 (EXIT_ZOMBIE)
    /// 进程已退出，但父进程尚未等待 (wait)
    pub const ZOMBIE: u32 = 0x00000010;

    /// 退出死亡 (EXIT_DEAD)
    /// 进程最终状态，将被回收
    pub const DEAD: u32 = 0x00000020;

    /// 创建新状态
    #[inline]
    pub const fn new(bits: u32) -> Self {
        TaskState(bits)
    }

    /// 获取位值
    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// 检查是否包含指定标志
    #[inline]
    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) != 0
    }

    /// 检查是否正在运行
    #[inline]
    pub fn is_running(&self) -> bool {
        self.0 == Self::RUNNING
    }

    /// 检查是否在睡眠（可中断或不可中断）
    #[inline]
    pub fn is_sleeping(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE) || self.contains(Self::UNINTERRUPTIBLE)
    }

    /// 检查是否已退出（僵死或死亡）
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.contains(Self::ZOMBIE) || self.contains(Self::DEAD)
    }

    /// 检查是否可被信号唤醒
    #[inline]
    pub fn is_interruptible(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE)
    }
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState::new(TaskState::RUNNING)
    }
}

///
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
pub mod task_flags {
    use bitflags::bitflags;

    bitflags! {
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
/// 以及进程切换时的 cpu_context (arch/arm64/kernel/process.c)
/// CPU 上下文结构体
///
/// 布局必须与 `cpu_switch_to` 汇编代码匹配：
/// - offset 0:  ra (返回地址)
/// - offset 8:  sp (栈指针)
/// - offset 16: s0 (帧指针)
/// - offset 24-104: s1-s11 (被调用者保存寄存器)
///
/// 后续字段用于信号处理等，不影响上下文切换
#[repr(C)]
#[derive(Debug, Clone)]
pub struct CpuContext {
    /// 返回地址 (x1) - 汇编 offset 0
    pub ra: u64,

    /// 栈指针 (x2) - 汇编 offset 8
    pub sp: u64,

    /// 被调用者保存寄存器 s0-s11 (x8, x9, x18-x27) - 汇编 offset 16-104
    /// s0 也是帧指针 (fp)
    pub s: [u64; 12],  // s[0]=s0/fp, s[1]=s1, s[2]=s2, ..., s[11]=s11

    // === 以下字段用于信号处理，不影响上下文切换 ===

    /// 程序计数器 (用于信号处理)
    pub pc: u64,

    /// 参数寄存器 a0-a7 (用于信号处理函数参数)
    pub a: [u64; 8],

    /// 用户栈指针
    pub user_sp: u64,

    /// 用户程序状态寄存器
    pub user_spsr: u64,
}

impl Default for CpuContext {
    fn default() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
            pc: 0,
            a: [0; 8],
            user_sp: 0,
            user_spsr: 0,
        }
    }
}

impl CpuContext {
    /// 创建新的上下文，用于新任务
    pub fn new_for_task(pc: u64, sp: u64) -> Self {
        Self {
            ra: pc,  // 返回地址设为入口点
            sp,
            s: [0; 12],
            pc,
            a: [0; 8],
            user_sp: 0,
            user_spsr: 0,
        }
    }

    /// 帧指针别名 (s[0] = s0 = fp)
    #[inline]
    pub fn fp(&self) -> u64 {
        self.s[0]
    }

    /// 帧指针别名 (可变)
    #[inline]
    pub fn fp_mut(&mut self) -> &mut u64 {
        &mut self.s[0]
    }

    /// 参数寄存器别名 (a0-a7)
    #[inline]
    pub fn x(&self, i: usize) -> u64 {
        self.a.get(i).copied().unwrap_or(0)
    }

    /// 参数寄存器别名 (可变)
    #[inline]
    pub fn x_mut(&mut self, i: usize) -> &mut u64 {
        static mut DUMMY: u64 = 0;
        // SAFETY: 单线程访问，仅用于避免编译错误
        self.a.get_mut(i).unwrap_or(unsafe { &mut DUMMY })
    }
}

/// 进程标识符 (PID 类型)
///
pub type Pid = u32;

// ==================== thread_info 风格标志 ====================
// 参考 Linux: include/linux/sched.h

/// TIF_SIGPENDING - 有待处理信号
pub const TIF_SIGPENDING: u32 = 0;
/// TIF_NEED_RESCHED - 需要重新调度
pub const TIF_NEED_RESCHED: u32 = 1;
/// TIF_NOTIFY_RESUME - 返回用户态前通知
pub const TIF_NOTIFY_RESUME: u32 = 2;
/// TIF_UPROBE - uprobe 待处理
pub const TIF_UPROBE: u32 = 3;
/// TIF_MEMDIE - 正在退出（内存不足）
pub const TIF_MEMDIE: u32 = 4;

/// 任务控制块 (Task Control Block)
///
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
///
/// Linux 兼容性设计：
/// - thread_info 风格字段在结构体开头
/// - tp 寄存器指向 Task 结构体
/// - 内核栈通过 kernel_sp 字段管理
#[repr(C)]
pub struct Task {
    // ==================== thread_info 风格字段 (offset 0) ====================
    // 参考 Linux: arch/riscv/include/asm/thread_info.h
    // 这些字段必须在结构体开头，以便通过 tp 快速访问

    /// 进程标志 (thread_info.flags)
    /// 位定义: TIF_SIGPENDING, TIF_NEED_RESCHED 等
    ti_flags: AtomicU32,

    /// 抢占计数 (thread_info.preempt_count)
    /// > 0 表示禁止抢占
    ti_preempt_count: core::sync::atomic::AtomicI32,

    /// 内核栈指针 (thread_info.kernel_sp)
    /// 指向内核栈顶
    ti_kernel_sp: core::sync::atomic::AtomicU64,

    /// 用户栈指针 (thread_info.user_sp)
    /// 保存用户态 sp，用于 trap 返回
    ti_user_sp: core::sync::atomic::AtomicU64,

    /// 运行在哪个 CPU (thread_info.cpu)
    ti_cpu: core::sync::atomic::AtomicI32,

    // ==================== task_struct 字段 ====================

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

    /// CFS 调度实体
    ///
    /// 包含 vruntime、权重等 CFS 调度信息
    /// 参考 Linux: task_struct::se
    sched_entity: crate::sched::cfs::SchedEntity,

    /// CPU 上下文
    context: CpuContext,

    /// 内核栈
    /// TODO: 实现内核栈分配
    kernel_stack: Option<*mut u8>,

    /// fork 子进程标志
    /// 如果为 true，表示这是 fork 创建的子进程，需要从 ret_from_fork 恢复
    is_fork_child: core::sync::atomic::AtomicBool,

    /// fork 子进程的 PtRegs 指针
    /// 当 is_fork_child 为 true 时，这个指针指向子进程的 PtRegs
    /// 调度器会使用这个 PtRegs 来恢复子进程的状态
    fork_pt_regs: core::sync::atomic::AtomicU64,

    /// 地址空间 (mm_struct)
    /// 内核线程为 None，用户进程为 Some
    /// 使用 Box 以减少 Task 的大小
    address_space: Option<Box<AddressSpace>>,

    /// 活动地址空间 (active_mm)
    ///
    /// 对于用户进程：active_mm == mm
    /// 对于内核线程：active_mm 是借用的地址空间（用于访问用户内存）
    ///
    /// 参考 Linux: task_struct::active_mm
    active_mm: Option<*const AddressSpace>,

    /// 架构相关线程状态
    ///
    /// 存储 FPU 状态、TLS 指针等
    /// 参考 Linux: task_struct::thread
    thread: crate::arch::riscv64::thread::ThreadStruct,

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
    /// 这是一个链表头，所有子进程通过各自的 sibling 字段链接到此
    pub children: ListHead,

    /// 兄弟进程链表节点
    ///
    /// 用于将此进程链接到父进程的 children 链表中
    pub sibling: ListHead,

    /// 父进程的 children 链表头指针（用于 next_sibling 边界检测）
    ///
    /// 当进程添加到父进程时，保存父进程 children 的地址
    /// 用于 next_sibling() 判断是否到达链表末尾
    parent_children_head: *mut ListHead,

    /// 清除子进程 TID 的用户空间地址 (set_tid_address)
    ///
    /// 当进程退出时，内核会将此地址指向的值清零
    /// 用于 pthread 线程同步
    clear_child_tid: *mut i32,

    /// Robust futex 列表头 (set_robust_list)
    ///
    /// 用于 robust mutex 实现
    robust_list_head: *const u8,
    robust_list_len: usize,

    /// 进程堆边界 (brk)
    ///
    /// 指向进程堆的末尾地址，由 sys_brk 管理
    /// 初始值为 0，在第一次 brk 调用时设置为默认值
    brk: core::sync::atomic::AtomicU64,

    /// 当前工作目录
    ///
    /// 存储进程的当前工作目录路径
    /// 初始值为 "/"
    cwd: alloc::boxed::Box<[u8]>,

    /// 可执行文件路径
    ///
    /// 存储进程的可执行文件路径
    /// 用于 /proc/self/exe 等
    exe_path: alloc::boxed::Box<[u8]>,
}

impl Task {
    /// 创建新任务
    ///
    pub fn new(pid: Pid, policy: SchedPolicy) -> Self {
        // PRIO_TO_PRIO: static_prio 120 -> prio 120
        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio; // SCHED_NORMAL 时 normal_prio == static_prio
        let prio = normal_prio;

        // Idle 任务不需要文件描述符表和信号处理
        // 暂时禁用 FdTable 和 Signal 创建，避免堆分配问题
        let (fdtable, signal) = (None, None);

        let state = AtomicU32::new(TaskState::RUNNING);
        let context = CpuContext::default();
        let pending = SigPending::new();
        let sigstack = crate::signal::SignalStack::new();

        let mut task = Self {
            // thread_info 风格字段
            ti_flags: AtomicU32::new(0),
            ti_preempt_count: core::sync::atomic::AtomicI32::new(0),
            ti_kernel_sp: core::sync::atomic::AtomicU64::new(0),
            ti_user_sp: core::sync::atomic::AtomicU64::new(0),
            ti_cpu: core::sync::atomic::AtomicI32::new(-1),

            // task_struct 字段
            state,
            pid,
            tgid: pid, // 单线程进程 tgid == pid
            policy,
            prio,
            static_prio,
            normal_prio,
            time_slice: DEFAULT_TIME_SLICE, // 默认时间片 (10 个时钟中断 = 100ms)
            sched_entity: crate::sched::cfs::SchedEntity::new(),
            context,
            kernel_stack: None,
            is_fork_child: core::sync::atomic::AtomicBool::new(false),
            fork_pt_regs: core::sync::atomic::AtomicU64::new(0),
            address_space: None,
            active_mm: None,
            thread: crate::arch::riscv64::thread::ThreadStruct::new(),
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
            clear_child_tid: ptr::null_mut(),
            robust_list_head: ptr::null(),
            robust_list_len: 0,
            brk: core::sync::atomic::AtomicU64::new(0),
            cwd: Box::from(&b"/"[..]),
            exe_path: Box::from(&b""[..]),
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

        // 初始化 thread_info 风格字段（必须在开头）
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_flags)) as *mut AtomicU32,
            AtomicU32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_preempt_count)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_kernel_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_user_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_cpu)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(-1),
        );

        // 使用 ptr::write 和 offset_of 来安全地初始化每个字段
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::RUNNING),
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
            (ptr as usize + offset_of!(Task, sched_entity)) as *mut crate::sched::cfs::SchedEntity,
            crate::sched::cfs::SchedEntity::new(),
        );

        // 初始化 idle 任务的上下文
        // 设置 pc 指向 cpu_idle_loop 函数，这样 context_switch 时可以正确跳转
        //
        // 注意：idle 任务实际上不需要通过 context_switch 来执行，
        // 因为 cpu_idle_loop 是直接从内核主函数调用的。
        // 但为了防止意外切换到 idle 任务，我们设置一个有效的 pc。
        fn idle_loop_wrapper() -> ! {
            loop {
                unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
            }
        }
        let idle_ctx = CpuContext {
            ra: idle_loop_wrapper as u64,
            sp: 0,
            s: [0; 12],
            pc: 0,
            a: [0; 8],
            user_sp: 0,
            user_spsr: 0,
        };
        ptr::write(
            (ptr as usize + offset_of!(Task, context)) as *mut CpuContext,
            idle_ctx,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, kernel_stack)) as *mut Option<*mut u8>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, is_fork_child)) as *mut core::sync::atomic::AtomicBool,
            core::sync::atomic::AtomicBool::new(false),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fork_pt_regs)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<Box<AddressSpace>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, active_mm)) as *mut Option<*const AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, thread)) as *mut crate::arch::riscv64::thread::ThreadStruct,
            crate::arch::riscv64::thread::ThreadStruct::new(),
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
        ptr::write(
            (ptr as usize + offset_of!(Task, clear_child_tid)) as *mut *mut i32,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_head)) as *mut *const u8,
            ptr::null(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_len)) as *mut usize,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, cwd)) as *mut Box<[u8]>,
            Box::from(&b"/"[..]),
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

        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio;
        let prio = normal_prio;

        // 初始化 thread_info 风格字段（必须在开头）
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_flags)) as *mut AtomicU32,
            AtomicU32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_preempt_count)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_kernel_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_user_sp)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, ti_cpu)) as *mut core::sync::atomic::AtomicI32,
            core::sync::atomic::AtomicI32::new(-1),
        );

        // 写入各个字段
        ptr::write(
            (ptr as usize + offset_of!(Task, state)) as *mut AtomicU32,
            AtomicU32::new(TaskState::RUNNING),
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
            (ptr as usize + offset_of!(Task, sched_entity)) as *mut crate::sched::cfs::SchedEntity,
            crate::sched::cfs::SchedEntity::new(),
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
            (ptr as usize + offset_of!(Task, is_fork_child)) as *mut core::sync::atomic::AtomicBool,
            core::sync::atomic::AtomicBool::new(false),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, fork_pt_regs)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, address_space)) as *mut Option<Box<AddressSpace>>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, active_mm)) as *mut Option<*const AddressSpace>,
            None,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, thread)) as *mut crate::arch::riscv64::thread::ThreadStruct,
            crate::arch::riscv64::thread::ThreadStruct::new(),
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
        ptr::write(
            (ptr as usize + offset_of!(Task, clear_child_tid)) as *mut *mut i32,
            ptr::null_mut(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_head)) as *mut *const u8,
            ptr::null(),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, robust_list_len)) as *mut usize,
            0,
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, brk)) as *mut core::sync::atomic::AtomicU64,
            core::sync::atomic::AtomicU64::new(0),
        );
        ptr::write(
            (ptr as usize + offset_of!(Task, cwd)) as *mut Box<[u8]>,
            Box::from(&b"/"[..]),
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
        TaskState::new(self.state.load(Ordering::Relaxed))
    }

    /// 设置进程状态
    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state.bits(), Ordering::Release);
    }

    /// 检查进程是否在指定状态
    #[inline]
    pub fn is_state(&self, flag: u32) -> bool {
        self.state.load(Ordering::Relaxed) & flag != 0
    }

    /// 进程睡眠和唤醒机制

    /// 使当前进程进入睡眠状态
    ///
    /// (kernel/sched/core.c)
    ///
    /// 进程调用此函数后会进入睡眠状态，并触发调度
    ///
    /// # 参数
    /// - `state`: 睡眠状态（TaskState::INTERRUPTIBLE 或 TaskState::UNINTERRUPTIBLE）
    ///
    /// # Safety
    /// 调用此函数后，当前进程会被调度出去，直到被唤醒
    ///
    /// # 示例
    /// ```no_run
    /// # use rux::process::task::TaskState;
    /// // 可中断睡眠（可被信号唤醒）
    /// Task::sleep(TaskState::new(TaskState::INTERRUPTIBLE));
    ///
    /// // 不可中断睡眠
    /// Task::sleep(TaskState::new(TaskState::UNINTERRUPTIBLE));
    /// ```
    #[inline(never)]
    pub fn sleep(state: TaskState) {
        // 设置当前进程为睡眠状态
        if let Some(current) = crate::sched::current() {
            unsafe {
                (*current).set_state(state);
            }
        }

        // 释放内核大锁（睡眠前必须释放，否则其他进程无法获取锁）
        crate::sync::kernel_lock_release();

        // 触发调度，选择其他进程运行
        crate::sched::schedule();

        // 唤醒后重新获取内核大锁（继续执行 syscall）
        crate::sync::kernel_lock_acquire();
    }

    /// 唤醒进程
    ///
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
            if old_state.is_sleeping() {
                // 唤醒进程：设置为 RUNNING 状态
                (*task).set_state(TaskState::new(TaskState::RUNNING));

                // 将进程加入运行队列（关键！）
                crate::sched::enqueue_task(&mut *task);

                // 设置 need_resched 标志，触发重新调度
                crate::sched::set_need_resched();

                true
            } else {
                false
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

    /// 设置时间片
    #[inline]
    pub fn set_time_slice(&mut self, slice: u32) {
        self.time_slice = slice;
    }

    /// 抢占式调度支持结束

    // ==================== CFS 调度支持 ====================

    /// 获取 CFS 调度实体
    #[inline]
    pub fn sched_entity(&self) -> &crate::sched::cfs::SchedEntity {
        &self.sched_entity
    }

    /// 获取 CFS 调度实体（可变引用）
    #[inline]
    pub fn sched_entity_mut(&mut self) -> &mut crate::sched::cfs::SchedEntity {
        &mut self.sched_entity
    }

    /// 获取 nice 值
    ///
    /// nice 值范围: -20 到 +19
    /// 从 static_prio 计算: nice = static_prio - 120
    #[inline]
    pub fn nice(&self) -> i32 {
        self.static_prio - 120
    }

    /// 设置 nice 值
    ///
    /// 同时更新 static_prio 和调度实体权重
    pub fn set_nice(&mut self, nice: i32) {
        // nice 值范围: -20 到 +19
        let nice = nice.clamp(-20, 19);

        // 更新 static_prio
        self.static_prio = nice + 120;
        self.normal_prio = self.static_prio;
        self.prio = self.normal_prio;

        // 更新调度实体权重
        self.sched_entity.set_nice(nice);
    }

    // ==================== 进程树管理 ====================

    /// 获取父进程 PID (PPID)
    #[inline]
    pub fn ppid(&self) -> Pid {
        match self.parent {
            Some(parent_ptr) => unsafe { (*parent_ptr).pid },
            None => 0, // 没有父进程，返回 0
        }
    }

    /// 检查是否是 fork 子进程
    #[inline]
    pub fn is_fork_child(&self) -> bool {
        self.is_fork_child.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 设置为 fork 子进程
    #[inline]
    pub fn set_fork_child(&self, pt_regs_ptr: *const crate::arch::riscv64::pt_regs::PtRegs) {
        self.is_fork_child.store(true, core::sync::atomic::Ordering::Relaxed);
        self.fork_pt_regs.store(pt_regs_ptr as u64, core::sync::atomic::Ordering::Relaxed);
    }

    /// 获取 fork 子进程的 PtRegs 指针
    #[inline]
    pub fn fork_pt_regs(&self) -> *const crate::arch::riscv64::pt_regs::PtRegs {
        self.fork_pt_regs.load(core::sync::atomic::Ordering::Relaxed) as *const crate::arch::riscv64::pt_regs::PtRegs
    }

    /// 清除 fork 子进程标志
    /// 在子进程首次被调度并开始执行后调用
    #[inline]
    pub fn clear_fork_child(&self) {
        self.is_fork_child.store(false, core::sync::atomic::Ordering::Relaxed);
        self.fork_pt_regs.store(0, core::sync::atomic::Ordering::Relaxed);
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
        self.address_space.as_mut().map(|b| b.as_mut())
    }

    /// 获取地址空间的引用
    pub fn address_space(&self) -> Option<&AddressSpace> {
        self.address_space.as_ref().map(|b| b.as_ref())
    }

    /// 设置地址空间
    pub fn set_address_space(&mut self, addr_space: Option<alloc::boxed::Box<AddressSpace>>) {
        // 更新 active_mm 指针
        if let Some(ref aspace) = addr_space {
            self.active_mm = Some(aspace.as_ref() as *const AddressSpace);
        } else {
            self.active_mm = None;
        }
        self.address_space = addr_space;
    }

    /// 获取活动地址空间（对于内核线程是借用的地址空间）
    pub fn active_mm(&self) -> Option<&AddressSpace> {
        if let Some(ref aspace) = self.address_space {
            Some(aspace.as_ref())
        } else if let Some(mm_ptr) = self.active_mm {
            unsafe { Some(&*mm_ptr) }
        } else {
            None
        }
    }

    /// 获取架构相关线程状态
    pub fn thread(&self) -> &crate::arch::riscv64::thread::ThreadStruct {
        &self.thread
    }

    /// 获取架构相关线程状态的可变引用
    pub fn thread_mut(&mut self) -> &mut crate::arch::riscv64::thread::ThreadStruct {
        &mut self.thread
    }

    // ==================== thread_info 风格访问器 ====================

    /// 获取 thread_info 标志
    #[inline]
    pub fn ti_flags(&self) -> u32 {
        self.ti_flags.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 设置 thread_info 标志
    #[inline]
    pub fn set_ti_flags(&self, flags: u32) {
        self.ti_flags.store(flags, core::sync::atomic::Ordering::Release);
    }

    /// 测试 thread_info 标志位
    #[inline]
    pub fn test_ti_flag(&self, flag: u32) -> bool {
        (self.ti_flags.load(core::sync::atomic::Ordering::Relaxed) & flag) != 0
    }

    /// 设置 thread_info 标志位
    #[inline]
    pub fn set_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_or(flag, core::sync::atomic::Ordering::Release);
    }

    /// 清除 thread_info 标志位
    #[inline]
    pub fn clear_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_and(!flag, core::sync::atomic::Ordering::Release);
    }

    /// 检查是否需要重新调度
    #[inline]
    pub fn need_resched(&self) -> bool {
        self.test_ti_flag(TIF_NEED_RESCHED)
    }

    /// 设置需要重新调度标志
    #[inline]
    pub fn set_need_resched_flag(&self) {
        self.set_ti_flag(TIF_NEED_RESCHED);
    }

    /// 清除需要重新调度标志
    #[inline]
    pub fn clear_need_resched_flag(&self) {
        self.clear_ti_flag(TIF_NEED_RESCHED);
    }

    /// 检查是否有待处理信号
    #[inline]
    pub fn has_pending_signal(&self) -> bool {
        self.test_ti_flag(TIF_SIGPENDING)
    }

    /// 设置待处理信号标志
    #[inline]
    pub fn set_pending_signal_flag(&self) {
        self.set_ti_flag(TIF_SIGPENDING);
    }

    /// 获取抢占计数
    #[inline]
    pub fn preempt_count(&self) -> i32 {
        self.ti_preempt_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 增加抢占计数
    #[inline]
    pub fn inc_preempt_count(&self) {
        self.ti_preempt_count.fetch_add(1, core::sync::atomic::Ordering::Release);
    }

    /// 减少抢占计数
    #[inline]
    pub fn dec_preempt_count(&self) {
        self.ti_preempt_count.fetch_sub(1, core::sync::atomic::Ordering::Release);
    }

    /// 检查是否可抢占
    #[inline]
    pub fn preemptible(&self) -> bool {
        self.preempt_count() == 0
    }

    /// 获取内核栈指针 (thread_info.kernel_sp)
    #[inline]
    pub fn ti_kernel_sp(&self) -> u64 {
        self.ti_kernel_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 设置内核栈指针 (thread_info.kernel_sp)
    #[inline]
    pub fn set_ti_kernel_sp(&self, sp: u64) {
        self.ti_kernel_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// 获取用户栈指针 (thread_info.user_sp)
    #[inline]
    pub fn ti_user_sp(&self) -> u64 {
        self.ti_user_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 设置用户栈指针 (thread_info.user_sp)
    #[inline]
    pub fn set_ti_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// 获取运行 CPU (thread_info.cpu)
    #[inline]
    pub fn ti_cpu(&self) -> i32 {
        self.ti_cpu.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 设置运行 CPU (thread_info.cpu)
    #[inline]
    pub fn set_ti_cpu(&self, cpu: i32) {
        self.ti_cpu.store(cpu, core::sync::atomic::Ordering::Release);
    }

    // ==================== 内核栈管理 ====================

    /// 分配内核栈
    ///
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

                // 同时设置 ti_kernel_sp
                self.set_ti_kernel_sp(stack_top as u64);

                Some(stack_top)
            } else {
                None
            }
        }
    }

    /// 释放内核栈
    ///
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
            // 清零 ti_kernel_sp
            self.set_ti_kernel_sp(0);
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

    /// 设置文件描述符表
    #[inline]
    pub fn set_fdtable(&mut self, fdtable: Option<alloc::boxed::Box<FdTable>>) {
        self.fdtable = fdtable;
    }

    /// 设置父进程
    pub fn set_parent(&mut self, parent: *const Task) {
        if parent.is_null() {
            self.parent = None;
        } else {
            self.parent = Some(parent);
        }
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

    /// 获取第一个子进程
    ///
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
    ///
    /// # Safety
    /// 调用者必须确保：
    /// - child 是有效的子进程指针
    /// - child 在当前进程的 children 链表中
    ///
    /// # 参数
    /// - `child`: 要移除的子进程指针
    ///
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
    ///
    /// # 参数
    /// - `f`: 对每个子进程调用的闭包
    ///
    /// # Safety
    /// 调用者必须确保 self 是有效的，且在遍历期间不修改进程树
    ///
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

    // ==================== musl libc 支持 (set_tid_address, set_robust_list) ====================

    /// 设置 clear_child_tid 地址
    ///
    /// 当进程退出时，内核会将此地址指向的值清零
    #[inline]
    pub fn set_clear_child_tid(&mut self, tidptr: *mut i32) {
        self.clear_child_tid = tidptr;
    }

    /// 获取 clear_child_tid 地址
    #[inline]
    pub fn clear_child_tid(&self) -> *mut i32 {
        self.clear_child_tid
    }

    /// 设置 robust list
    ///
    /// 用于 robust mutex 实现
    #[inline]
    pub fn set_robust_list(&mut self, head: *const u8, len: usize) {
        self.robust_list_head = head;
        self.robust_list_len = len;
    }

    /// 获取 robust list 头指针
    #[inline]
    pub fn robust_list_head(&self) -> *const u8 {
        self.robust_list_head
    }

    /// 获取 robust list 长度
    #[inline]
    pub fn robust_list_len(&self) -> usize {
        self.robust_list_len
    }

    /// 获取当前 brk 值
    #[inline]
    pub fn get_brk(&self) -> u64 {
        self.brk.load(core::sync::atomic::Ordering::Acquire)
    }

    /// 设置 brk 值
    #[inline]
    pub fn set_brk(&self, value: u64) {
        self.brk.store(value, core::sync::atomic::Ordering::Release);
    }

    /// 获取当前工作目录
    pub fn get_cwd(&self) -> &[u8] {
        &self.cwd
    }

    /// 设置当前工作目录
    pub fn set_cwd(&mut self, path: &[u8]) {
        self.cwd = Box::from(path);
    }

    /// 获取可执行文件路径
    pub fn get_exe_path(&self) -> &[u8] {
        &self.exe_path
    }

    /// 设置用户栈指针
    pub fn set_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// 设置可执行文件路径
    pub fn set_exe_path(&mut self, path: &[u8]) {
        self.exe_path = Box::from(path);
    }
}

///
/// 可选: 100, 250, 300, 1000
const HZ: u32 = 100;

// ==================== 偏移量常量 (供汇编使用) ====================
// 参考 Linux: asm-offsets.c

/// Task 结构体中 thread_info 字段的偏移量
#[allow(dead_code)]
pub mod task_offsets {
    use super::*;

    pub const TI_FLAGS: usize = core::mem::offset_of!(Task, ti_flags);
    pub const TI_PREEMPT_COUNT: usize = core::mem::offset_of!(Task, ti_preempt_count);
    pub const TI_KERNEL_SP: usize = core::mem::offset_of!(Task, ti_kernel_sp);
    pub const TI_USER_SP: usize = core::mem::offset_of!(Task, ti_user_sp);
    pub const TI_CPU: usize = core::mem::offset_of!(Task, ti_cpu);

    // 其他常用字段偏移
    pub const TASK_STATE: usize = core::mem::offset_of!(Task, state);
    pub const TASK_PID: usize = core::mem::offset_of!(Task, pid);
    pub const TASK_CONTEXT: usize = core::mem::offset_of!(Task, context);
    pub const TASK_KERNEL_STACK: usize = core::mem::offset_of!(Task, kernel_stack);
    pub const TASK_THREAD: usize = core::mem::offset_of!(Task, thread);
}

/// 导出偏移量常量
pub use task_offsets::*;
