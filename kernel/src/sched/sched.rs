//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 调度器实现
//!
//!
//! - 调度类 (sched_class): fair, rt, idle, deadline
//! - 运行队列 (rq): 每个 CPU 一个 rq
//! - 调度实体 (sched_entity): fair 调度单位
//! - 调度入口: schedule() -> __schedule() -> context_switch()
//!
//! 当前实现: 简单的 FIFO 调度器（可扩展为 CFS）
//!
//! 注意：使用原始指针以避免借用检查器限制，这在 OS 内核开发中是常见做法

use crate::errno;
use crate::process::task::{Task, TaskState, SchedPolicy, Pid};
use crate::arch;
use crate::println;
use crate::fs::{FdTable, File, FileFlags, FileOps, CharDev};
use crate::config::{MAX_CPUS, DEFAULT_TIME_SLICE_MS, TIME_SLICE_TICKS};
use alloc::sync::Arc;
use alloc::boxed::Box;
use crate::process::pid::alloc_pid;
use core::arch::asm;
use spin::Mutex;

const MAX_TASKS: usize = 256;

pub struct RunQueue {
    /// CFS 运行队列
    ///
    /// 使用 vruntime 排序的红黑树（BTreeMap 实现）
    pub cfs_rq: crate::sched::cfs::CfsRunQueue,

    /// 运行队列 - 使用原始指针（保留用于非 CFS 调度）
    tasks: [*mut Task; MAX_TASKS],

    /// 当前运行的任务
    pub current: *mut Task,

    /// 任务数量
    nr_running: usize,

    /// 空闲任务
    idle: *mut Task,

    /// 是否使用 CFS 调度器
    ///
    /// true: 使用 CFS 调度
    /// false: 使用简单的 Round Robin 调度
    use_cfs: bool,
}

unsafe impl Send for RunQueue {}

static mut PER_CPU_RQ: [Option<Mutex<RunQueue>>; MAX_CPUS] = [None, None, None, None];

static RQ_INIT_LOCK: Mutex<[bool; MAX_CPUS]> = Mutex::new([false; MAX_CPUS]);


static mut NEED_RESCHED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

#[inline]
pub fn need_resched() -> bool {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return false;
        }
        NEED_RESCHED[cpu_id].load(core::sync::atomic::Ordering::Acquire)
    }
}

#[inline]
pub fn set_need_resched() {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            NEED_RESCHED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
        }
    }
}

#[inline]
fn clear_need_resched() {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            NEED_RESCHED[cpu_id].store(false, core::sync::atomic::Ordering::Release);
        }
    }
}

pub fn scheduler_tick() {
    // 获取当前 CPU 的运行队列
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => return,
    };

    let mut rq_inner = rq.lock();
    let current = rq_inner.current;

    if current.is_null() {
        return;
    }

    // 如果使用 CFS 调度器
    if rq_inner.use_cfs {
        // 获取当前时间
        let now = crate::sched::cfs::sched_clock();

        // 更新当前任务的执行时间
        rq_inner.cfs_rq.update_curr(now);

        unsafe {
            // 第一步：获取当前任务的调度信息（不可变借用）
            let (curr_vruntime, curr_weight) = {
                let task = &*current;
                let se = task.sched_entity();
                (se.get_vruntime(), se.load.weight)
            };

            // 计算时间片
            let slice_ns = rq_inner.cfs_rq.sched_slice(&crate::sched::cfs::SchedEntity {
                load: crate::sched::cfs::LoadWeight::new(curr_weight),
                vruntime: core::sync::atomic::AtomicU64::new(curr_vruntime),
                sum_exec_runtime: core::sync::atomic::AtomicU64::new(0),
                exec_start: core::sync::atomic::AtomicU64::new(0),
                prev_sum_exec_runtime: core::sync::atomic::AtomicU64::new(0),
                on_rq: core::sync::atomic::AtomicBool::new(false),
                slice: core::sync::atomic::AtomicU64::new(0),
            });
            let slice_ticks = (slice_ns / 10_000_000) as u32;

            // 第二步：更新时间片和减少时间片（可变借用）
            let still_has_slice = {
                let task = &mut *current;
                task.set_time_slice(slice_ticks.max(1));
                task.tick_time_slice()
            };

            if !still_has_slice {
                // 时间片用完，设置需要重新调度标志
                drop(rq_inner);
                set_need_resched();
            } else {
                // 检查是否需要抢占
                // 如果队列中有 vruntime 更小的任务，应该抢占
                if let Some(next) = rq_inner.cfs_rq.peek_next() {
                    if !next.is_null() && next != current {
                        // 获取下一个任务的 vruntime
                        let next_vruntime = {
                            let next_task = &*next;
                            let next_se = next_task.sched_entity();
                            next_se.get_vruntime()
                        };

                        // 检查是否需要抢占
                        let wakeup_granularity = crate::sched::cfs::SCHED_MIN_GRANULARITY_NS;
                        if curr_vruntime > next_vruntime {
                            let delta = curr_vruntime - next_vruntime;
                            if delta > wakeup_granularity {
                                drop(rq_inner);
                                set_need_resched();
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    // Round Robin 调度器
    // 更新时间片（使用 Task 的公共方法）
    let task = unsafe { &mut *current };
    let still_has_slice = task.tick_time_slice();

    // 检查时间片是否用完
    if !still_has_slice {
        // 时间片用完，重新分配时间片
        task.reset_time_slice();

        // 设置 need_resched 标志，触发重新调度
        drop(rq_inner);  // 释放锁后再设置标志
        set_need_resched();
    }
}

pub fn resched_curr() {
    set_need_resched();
}

/// 远程触发指定 CPU 重新调度
///
/// 当某个 CPU 上的任务需要被调度时，
/// 发送 IPI 通知目标 CPU
///
///
/// # 参数
/// * `cpu` - 目标 CPU ID
pub fn resched_cpu(cpu: usize) {
    // 发送 Reschedule IPI 到目标 CPU
    #[cfg(feature = "riscv64")]
    crate::arch::ipi::send_reschedule_ipi(cpu);
}


pub fn wake_up_process(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up(task)
}

pub fn this_cpu_rq() -> Option<&'static Mutex<RunQueue>> {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn cpu_rq(cpu_id: usize) -> Option<&'static Mutex<RunQueue>> {
    unsafe {
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn init_per_cpu_rq(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    let mut init_flags = RQ_INIT_LOCK.lock();
    if init_flags[cpu_id] {
        return;  // 已经初始化
    }

    unsafe {
        PER_CPU_RQ[cpu_id] = Some(Mutex::new(RunQueue {
            cfs_rq: crate::sched::cfs::CfsRunQueue::new(),
            tasks: [core::ptr::null_mut(); MAX_TASKS],
            current: core::ptr::null_mut(),
            nr_running: 0,
            idle: core::ptr::null_mut(),
            use_cfs: true,  // 默认使用 CFS 调度器
        }));

        init_flags[cpu_id] = true;
    }
}

// 每个 CPU 需要自己的 idle 任务存储
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; MAX_CPUS] = [
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
];

const TASK_POOL_SIZE: usize = 16;

// 计算 Task 结构体的实际大小，确保每个槽位足够大
// Task 包含：CpuContext、AddressSpace、Option<Box<FdTable>>、
//            Option<Box<SignalStruct>>、ListHead 等
const TASK_SIZE: usize = core::mem::size_of::<Task>();

// Task 结构体的对齐要求
const TASK_ALIGN: usize = core::mem::align_of::<Task>();

// 计算对齐后的槽位大小（向上舍入到对齐边界）
const TASK_SLOT_SIZE: usize = (TASK_SIZE + TASK_ALIGN - 1) / TASK_ALIGN * TASK_ALIGN;

// 任务池锁 - 保护 TASK_POOL 和 TASK_POOL_NEXT
static TASK_POOL_LOCK: Mutex<()> = Mutex::new(());

// 使用对齐的静态数组作为任务池
// 使用 repr(align) 确保数组有正确的对齐
#[repr(C, align(16))]
struct AlignedTaskPool {
    data: [u8; TASK_POOL_SIZE * TASK_SLOT_SIZE],
}

static mut TASK_POOL: AlignedTaskPool = AlignedTaskPool {
    data: [0; TASK_POOL_SIZE * TASK_SLOT_SIZE],
};
static mut TASK_POOL_NEXT: usize = 0;

/// 从任务池分配一个槽位
///
/// 返回已初始化的 Task 指针，调用者负责设置 Task 的其他字段
pub fn alloc_task_slot() -> Option<*mut Task> {
    let _lock = TASK_POOL_LOCK.lock();

    unsafe {
        if TASK_POOL_NEXT >= TASK_POOL_SIZE {
            return None;
        }

        let pool_idx = TASK_POOL_NEXT;
        TASK_POOL_NEXT += 1;

        let pool_slot_addr = TASK_POOL.data.as_ptr().add(pool_idx * TASK_SLOT_SIZE);
        let task_ptr: *mut Task = pool_slot_addr as *mut Task;

        // 分配 PID
        let pid = match alloc_pid() {
            Some(p) => p,
            None => {
                TASK_POOL_NEXT -= 1;
                return None;
            }
        };

        // 初始化 Task
        Task::new_task_at(task_ptr, pid, SchedPolicy::Normal);

        Some(task_ptr)
    }
}

/// 释放任务池槽位（回滚分配）
pub fn free_task_slot(_task_ptr: *mut Task) {
    let _lock = TASK_POOL_LOCK.lock();
    unsafe {
        if TASK_POOL_NEXT > 0 {
            TASK_POOL_NEXT -= 1;
        }
    }
    // 注意：这里没有真正释放内存，因为任务池是静态分配的
}

pub fn init() {
    // 初始化当前 CPU 的运行队列
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    // 检查 CPU ID 是否有效
    if cpu_id >= MAX_CPUS {
        println!("sched: init: invalid cpu_id {}", cpu_id);
        return;
    }

    init_per_cpu_rq(cpu_id);

    unsafe {
        // 使用当前 CPU 专用的 idle 任务存储
        let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
        Task::new_idle_at(idle_ptr);

        // 为 idle 任务分配内核栈
        if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
            // 更新 context.sp 指向栈顶
            (*idle_ptr).context_mut().sp = stack_top as u64;
        } else {
            println!("sched: failed to allocate kernel stack for idle task");
        }

        // 设置 idle task 的 ti_cpu 字段
        // 这样 cpu_id() 可以从 tp 指向的 task_struct 中获取 hart_id
        (*idle_ptr).set_ti_cpu(cpu_id as i32);

        // ===== 切换到 Linux 风格的 tp/sscratch 协议 =====
        //
        // 在此之前：
        //   - tp = hart_id (OpenSBI 传递)
        //   - sscratch = 未定义
        //
        // 在此之后：
        //   - tp = idle task 指针 (current task_struct)
        //   - sscratch = 0 (表示内核态)
        //
        // 这允许 trap.S 使用 sscratch 交换来检测 user/kernel

        // 1. 设置 sscratch = 0 (表示当前在内核态)
        core::arch::asm!("csrw sscratch, zero");

        // 2. 切换 tp 指向 idle task
        //    现在 tp 指向当前 CPU 的 current task_struct
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);

        // 设置当前 CPU 的运行队列
        if let Some(rq) = this_cpu_rq() {
            let mut rq_inner = rq.lock();
            rq_inner.idle = idle_ptr;
            rq_inner.current = idle_ptr;
        }
    }
}

#[inline(never)]
pub fn schedule() {
    unsafe {
        __schedule();
    }
}

unsafe fn __schedule() {
    // 清除 need_resched 标志
    clear_need_resched();

    // 获取当前 CPU 的运行队列
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => return,
    };

    let mut rq_inner = rq.lock();

    // 获取当前任务
    let prev = rq_inner.current;

    if prev.is_null() {
        return;
    }

    // 更新当前任务的执行时间（CFS）
    if rq_inner.use_cfs {
        let now = crate::sched::cfs::sched_clock();
        rq_inner.cfs_rq.update_curr(now);
    }

    // 如果只有 idle 任务（nr_running == 0），尝试负载均衡
    if rq_inner.nr_running == 0 {
        drop(rq_inner);
        load_balance();

        let rq = match this_cpu_rq() {
            Some(r) => r,
            None => return,
        };
        rq_inner = rq.lock();

        // 即使 nr_running == 0，也继续执行以切换到 idle 任务
        // 不要提前返回，否则会导致页错误处理后的 sret 返回到错误的上下文
    }

    // 如果当前任务仍在运行状态，将其重新加入 CFS 队列
    // （如果使用 CFS 且当前任务之前在队列中）
    // 注意：idle 任务 (pid=0) 不应该被加入队列
    if rq_inner.use_cfs && !prev.is_null() {
        let prev_task = &*prev;
        let prev_pid = prev_task.pid();
        let is_running = prev_task.state() == TaskState::new(TaskState::RUNNING);
        if is_running && prev_pid != 0 {
            // 重新加入 CFS 队列
            rq_inner.cfs_rq.enqueue(prev);
        }
    }

    // 选择下一个任务
    let next = pick_next_task(&mut *rq_inner);

    if next == prev {
        return;
    }

    // 上下文切换（需要在锁外执行）
    drop(rq_inner);
    context_switch(&mut *prev, &mut *next);
}

unsafe fn pick_next_task(rq: &mut RunQueue) -> *mut Task {
    // 如果使用 CFS 调度器
    if rq.use_cfs {
        return pick_next_task_cfs(rq);
    }

    // 回退到 Round Robin 调度器
    pick_next_task_rr(rq)
}

/// CFS 调度器：选择下一个任务
///
/// 选择 vruntime 最小的任务
unsafe fn pick_next_task_cfs(rq: &mut RunQueue) -> *mut Task {
    // 更新当前任务的运行时间
    let now = crate::sched::cfs::sched_clock();
    rq.cfs_rq.update_curr(now);

    // 从 CFS 队列选择下一个任务
    if let Some(next) = rq.cfs_rq.pick_next() {
        // 设置为当前任务
        rq.cfs_rq.set_curr(next);

        // 计算并设置时间片
        let task = &mut *next;
        let se = task.sched_entity();
        let slice_ns = rq.cfs_rq.sched_slice(se);
        let slice_ms = crate::sched::cfs::sched_slice_to_ms(slice_ns);
        task.set_time_slice(slice_ms.max(1) as u32);  // 至少 1ms

        return next;
    }

    // CFS 队列为空，检查当前任务
    let current = rq.current;
    if !current.is_null() && (*current).state() == TaskState::new(TaskState::RUNNING) {
        return current;
    }

    // 没有可运行任务，返回 idle 任务
    rq.idle
}

/// Round Robin 调度器：选择下一个任务（保留作为备用）
unsafe fn pick_next_task_rr(rq: &mut RunQueue) -> *mut Task {
    let current = rq.current;

    // 简单的线性查找
    for i in 0..MAX_TASKS {
        let task_ptr = rq.tasks[i];

        if !task_ptr.is_null() && task_ptr != current {
            if (*task_ptr).state() == TaskState::new(TaskState::RUNNING) {
                return task_ptr;
            }
        }
    }

    // 没找到其他可运行任务，检查当前任务是否可运行
    if !current.is_null() && (*current).state() == TaskState::new(TaskState::RUNNING) {
        return current;
    }

    // 没有可运行任务，返回 idle 任务
    rq.idle
}

unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 获取当前 CPU ID
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    // 更新当前任务
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        rq_inner.current = next;
    }

    // 设置 next 的 ti_cpu 字段
    (*next).set_ti_cpu(cpu_id as i32);

    // 清除 fork 子进程标志（只执行一次）
    // fork 子进程的 context.ra 已经设置为 ret_from_fork
    // 标准的 cpu_switch_to 会恢复 ra，然后 ret 指令跳转到 ret_from_fork
    if (*next).is_fork_child() {
        (*next).clear_fork_child();
    }

    // 切换到 next 的用户页表
    if let Some(addr_space) = (*next).address_space() {
        let user_ppn = addr_space.root_ppn();
        let satp_value = (8u64 << 60) | user_ppn;  // Mode=8 (Sv39), ASID=0, PPN=user_ppn

        // 设置用户页表
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp_value,
            options(nostack, preserves_flags)
        );
    }

    // 检查是否是首次启动的用户进程（execve 创建的新进程）
    // 如果 context.sp 为 0，说明还没有设置内核栈，需要首次启动
    let ctx = (*next).context();
    let is_first_run = ctx.sp == 0;
    let user_ctx_ptr = ctx.a[1] as *const crate::arch::riscv64::context::UserContext;

    if is_first_run && !user_ctx_ptr.is_null() {
        // 首次启动的用户进程：切换到用户模式执行
        // 这是由 execve 创建的新进程，通过 switch_to_user 启动
        drop(&mut *prev);
        crate::arch::riscv64::context::switch_to_user(user_ctx_ptr);
    } else {
        // 非首次启动：只做内核态上下文切换
        // 参考 Linux: __switch_to 只保存/恢复 callee-saved 寄存器
        // 进程通过 trap 返回机制回到用户态
        //
        // fork 子进程也走这条路径：
        // - context.ra = ret_from_fork
        // - context.sp = pt_regs_ptr
        // - cpu_switch_to 恢复 ra 和 sp
        // - ret 指令跳转到 ret_from_fork
        // - ret_from_fork 从 pt_regs 恢复所有寄存器并返回用户态
        drop(&mut *next);
        crate::arch::context::context_switch(prev, next);
    }
}

pub fn enqueue_task(task: &'static mut Task) {
    let pid = task.pid();
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        if rq_inner.nr_running < MAX_TASKS {
            let task_ptr = task as *mut Task;

            // 设置任务状态为 RUNNING
            task.set_state(TaskState::new(TaskState::RUNNING));

            // 如果使用 CFS，同时加入 CFS 队列
            if rq_inner.use_cfs {
                rq_inner.cfs_rq.enqueue(task_ptr);
            }

            // 同时加入传统队列（兼容性）
            for i in 0..MAX_TASKS {
                if rq_inner.tasks[i].is_null() {
                    rq_inner.tasks[i] = task_ptr;
                    rq_inner.nr_running += 1;
                    return;
                }
            }
        }
    }
}

pub fn dequeue_task(task: &Task) {
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        let task_ptr = task as *const Task as *mut Task;

        // 如果使用 CFS，从 CFS 队列移除
        if rq_inner.use_cfs {
            rq_inner.cfs_rq.dequeue(task_ptr);
        }

        // 从传统队列移除
        for i in 0..MAX_TASKS {
            if rq_inner.tasks[i] == task_ptr {
                rq_inner.tasks[i] = core::ptr::null_mut();
                rq_inner.nr_running -= 1;
                return;
            }
        }
    }
}

pub fn yield_cpu() {
    schedule();
}

pub fn current() -> Option<&'static mut Task> {
    if let Some(rq) = this_cpu_rq() {
        let rq_inner = rq.lock();
        let current = rq_inner.current;
        if current.is_null() {
            None
        } else {
            unsafe { Some(&mut *current) }
        }
    } else {
        None
    }
}

pub fn get_current_pid() -> u32 {
    if let Some(rq) = this_cpu_rq() {
        let rq_inner = rq.lock();
        let current = rq_inner.current;
        if current.is_null() {
            0
        } else {
            unsafe { (*current).pid() }
        }
    } else {
        0
    }
}

pub fn get_current_ppid() -> u32 {
    if let Some(rq) = this_cpu_rq() {
        let rq_inner = rq.lock();
        let current = rq_inner.current;
        if current.is_null() {
            0
        } else {
            unsafe { (*current).ppid() }
        }
    } else {
        0
    }
}

pub unsafe fn find_task_by_pid(pid: Pid) -> *mut Task {
    // 遍历所有 CPU 的运行队列
    for cpu_id in 0..MAX_CPUS {
        if let Some(rq) = cpu_rq(cpu_id) {
            let rq_inner = rq.lock();
            for i in 0..rq_inner.nr_running {
                let task = rq_inner.tasks[i];
                if !task.is_null() && (*task).pid() == pid {
                    return task;
                }
            }
        }
    }
    core::ptr::null_mut()
}

pub fn get_current_fdtable() -> Option<&'static FdTable> {
    let rq_opt = this_cpu_rq();

    if rq_opt.is_none() {
        return None;
    }

    let rq = rq_opt.unwrap();
    let rq_inner = rq.lock();
    let current = rq_inner.current;

    if current.is_null() {
        return None;
    }

    unsafe { (*current).try_fdtable() }
}

pub fn init_std_fds() {
    use crate::fs::char_dev::{CharDev, CharDevType};

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                return;
            }

            // Idle 任务没有 fdtable
            let fdtable = match (*current).try_fdtable_mut() {
                Some(ft) => ft,
                None => return,
            };

            // 创建 UART 字符设备
            let uart_dev = CharDev::new(CharDevType::UartConsole, 0);

            // 文件操作函数表
            static UART_OPS: FileOps = FileOps {
                read: Some(uart_file_read),
                write: Some(uart_file_write),
                lseek: None,
                close: None,
            };

            // 创建 stdin (fd=0)
            let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
            stdin.set_ops(&UART_OPS);
            stdin.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 创建 stdout (fd=1)
            let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
            stdout.set_ops(&UART_OPS);
            stdout.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 创建 stderr (fd=2)
            let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
            stderr.set_ops(&UART_OPS);
            stderr.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 安装标准文件描述符
            let _ = fdtable.install_fd(0, stdin);
            let _ = fdtable.install_fd(1, stdout);
            let _ = fdtable.install_fd(2, stderr);
        }
    }
}

fn uart_file_read(file: &File, buf: &mut [u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { return char_dev.read(buf.as_mut_ptr(), buf.len()) };
    }
    -9  // EBADF
}

fn uart_file_write(file: &File, buf: &[u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { return char_dev.write(buf.as_ptr(), buf.len()) };
    }
    -9  // EBADF
}

// ============================================================================
// 信号处理
// ============================================================================

pub fn send_signal(pid: Pid, sig: i32) -> Result<(), i32> {
    use crate::signal::Signal;

    // 检查信号编号是否有效
    if sig < 1 || sig > 64 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    unsafe {
        // 遍历所有 CPU 的运行队列查找目标进程
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = cpu_rq(cpu_id) {
                let rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // 检查 PID 是否匹配
                    if task.pid() != pid {
                        continue;
                    }

                    // SIGKILL 和 SIGSTOP 不能被忽略
                    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
                        // 直接加入待处理信号
                        task.pending.add(sig);
                        // 唤醒睡眠的进程
                        drop(rq_inner);  // 释放锁
                        use crate::signal;
                        signal::signal_wake_up(task_ptr);
                        return Ok(());
                    }

                    // Idle 任务没有信号处理
                    let signal_ref: &crate::signal::SignalStruct = match task.signal.as_ref() {
                        Some(s) => s,
                        None => {
                            // 没有 signal 结构，直接加入待处理队列
                            task.pending.add(sig);
                            // 唤醒睡眠的进程
                            drop(rq_inner);  // 释放锁
                            use crate::signal;
                            signal::signal_wake_up(task_ptr);
                            return Ok(());
                        }
                    };

                    // 检查信号是否被屏蔽
                    if signal_ref.is_masked(sig) {
                        return Err(errno::Errno::TryAgain.as_neg_i32());
                    }

                    // 检查信号处理动作
                    if let Some(action) = signal_ref.get_action(sig) {
                        match action.action() {
                            crate::signal::SigActionKind::Ignore => {
                                return Ok(());  // 忽略信号
                            }
                            crate::signal::SigActionKind::Default => {
                                // 默认处理：加入待处理队列
                                task.pending.add(sig);
                                // 唤醒睡眠的进程
                                drop(rq_inner);  // 释放锁
                                use crate::signal;
                                signal::signal_wake_up(task_ptr);
                                return Ok(());
                            }
                            crate::signal::SigActionKind::Handler => {
                                // 用户自定义处理：加入待处理队列
                                task.pending.add(sig);
                                // 唤醒睡眠的进程
                                drop(rq_inner);  // 释放锁
                                use crate::signal;
                                signal::signal_wake_up(task_ptr);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        // 未找到进程
        Err(errno::Errno::NoSuchProcess.as_neg_i32())
    }
}

pub fn send_signal_self(sig: i32) -> Result<(), i32> {
    let current_pid = get_current_pid();
    send_signal(current_pid, sig)
}

pub fn handle_pending_signals() {

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                return;
            }

            // 获取第一个待处理信号
            while let Some(sig) = (*current).pending.first() {
                // 获取信号处理动作
                let signal_ref: &crate::signal::SignalStruct = match (*current).signal.as_ref() {
                    Some(s) => s,
                    None => {
                        // 没有 signal 结构，使用默认处理
                        // 移除信号并继续
                        (*current).pending.remove(sig);
                        continue;
                    }
                };

                let action = signal_ref.get_action(sig).unwrap();

                match action.action() {
                    crate::signal::SigActionKind::Ignore => {
                        // 忽略信号，直接移除
                        (*current).pending.remove(sig);
                    }
                    crate::signal::SigActionKind::Default => {
                        // 默认处理
                        match sig {
                            15 | 9 => {  // SIGTERM=15, SIGKILL=9
                                // 终止进程
                                (*current).pending.remove(sig);
                                // TODO: 实现进程终止
                            }
                            19 => {  // SIGSTOP
                                // 停止进程
                                (*current).set_state(TaskState::new(TaskState::STOPPED));
                                (*current).pending.remove(sig);
                            }
                            18 => {  // SIGCONT
                                // 继续进程
                                (*current).set_state(TaskState::new(TaskState::RUNNING));
                                (*current).pending.remove(sig);
                            }
                            _ => {
                                // 其他信号，移除
                                (*current).pending.remove(sig);
                            }
                        }
                    }
                    crate::signal::SigActionKind::Handler => {
                        // 调用用户处理函数
                        // TODO: 实现用户态信号处理函数调用
                        (*current).pending.remove(sig);
                    }
                }

                // 如果处理了信号，可能需要重新调度
                if (*current).state() == TaskState::new(TaskState::STOPPED) {
                    drop(rq_inner);
                    schedule();
                    break;
                }
            }
        }
    }
}

pub fn check_and_handle_signals() {
    handle_pending_signals();
}

// ============================================================================
// 进程退出和等待
// ============================================================================

pub fn do_exit(exit_code: i32) -> ! {
    use crate::signal::Signal;

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                // 没有当前进程，直接停机
                loop {
                    asm!("wfi", options(nomem, nostack));
                }
            }

            let current_pid = (*current).pid();
            let parent_pid = (*current).ppid();

            // 设置退出码
            (*current).set_exit_code(exit_code);

            // 设置进程状态为 Zombie
            (*current).set_state(TaskState::new(TaskState::ZOMBIE));

            // 从运行队列移除
            drop(rq_inner);  // 释放锁后再调用 dequeue_task
            dequeue_task(&*current);

            // 向父进程发送 SIGCHLD 信号并唤醒父进程
            if parent_pid != 0 {
                let _ = send_signal(parent_pid, Signal::SIGCHLD as i32);

                // 唤醒父进程（如果父进程在 wait4 中阻塞等待）
                let parent = find_task_by_pid(parent_pid);
                if !parent.is_null() {
                    wake_up_process(parent);
                }
            }

            // 调度器选择下一个进程运行
            schedule();

            // 永远不会到达这里
            loop {
                asm!("wfi", options(nomem, nostack));
            }
        }
    } else {
        // 没有运行队列，直接停机
        loop {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }
    }
}

pub fn do_wait(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    unsafe {
        let current = if let Some(rq) = this_cpu_rq() {
            rq.lock().current
        } else {
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // 如果当前是 idle task (PID 0)，说明没有真正的进程在运行
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        // 循环等待子进程退出
        loop {
            let mut found_child = false;

            // 遍历所有 CPU 的运行队列查找僵尸子进程
            for cpu_id in 0..MAX_CPUS {
                if let Some(rq) = cpu_rq(cpu_id) {
                    let mut rq_inner = rq.lock();

                    for i in 0..MAX_TASKS {
                        let task_ptr = rq_inner.tasks[i];
                        if task_ptr.is_null() {
                            continue;
                        }

                        let task = &*task_ptr;
                        let task_ppid = task.ppid();

                        // 检查是否是子进程
                        if task_ppid != current_pid {
                            continue;
                        }

                        found_child = true;

                        // 检查是否是指定的 PID (如果指定了)
                        if pid > 0 && task.pid() != pid as u32 {
                            continue;
                        }

                        // 检查是否是 Zombie 状态
                        if task.state() == TaskState::new(TaskState::ZOMBIE) {
                            let child_pid = task.pid();
                            let exit_code = task.exit_code();

                            // 写入退出状态
                            if !status_ptr.is_null() {
                                *status_ptr = exit_code;
                            }

                            // 从运行队列移除
                            rq_inner.tasks[i] = core::ptr::null_mut();
                            rq_inner.nr_running -= 1;

                            // 回收 PID
                            // TODO: 实现 pid_free()

                            return Ok(child_pid);
                        }
                    }
                }
            }

            // 有子进程但还没有退出的
            if found_child {
                // 使用 Task::sleep() 进入可中断睡眠状态
                crate::process::Task::sleep(crate::process::task::TaskState::new(TaskState::INTERRUPTIBLE));

                // 被唤醒后，检查是否有信号到达
                use crate::signal;
                if signal::signal_pending() {
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());  // EINTR
                }
            } else {
                // 没有子进程
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}

pub fn do_wait_nonblock(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    unsafe {
        let current = if let Some(rq) = this_cpu_rq() {
            rq.lock().current
        } else {
            // 没有 runqueue，说明未初始化，直接返回 ECHILD
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            // current 为 null（可能从非进程上下文调用），返回 ECHILD
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // 如果当前是 idle task (PID 0)，说明没有真正的进程在运行
        // 返回 ECHILD，因为 idle task 没有子进程
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let mut found_child = false;

        // 遍历所有 CPU 的运行队列查找僵尸子进程
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = cpu_rq(cpu_id) {
                let mut rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // 检查是否是子进程
                    if task.ppid() != current_pid {
                        continue;
                    }

                    found_child = true;

                    // 检查是否是指定的 PID (如果指定了)
                    if pid > 0 && task.pid() != pid as u32 {
                        continue;
                    }

                    // 检查是否是 Zombie 状态
                    if task.state() == TaskState::new(TaskState::ZOMBIE) {
                        let child_pid = task.pid();
                        let exit_code = task.exit_code();

                        // 写入退出状态
                        if !status_ptr.is_null() {
                            *status_ptr = exit_code;
                        }

                        // 从运行队列移除
                        rq_inner.tasks[i] = core::ptr::null_mut();
                        rq_inner.nr_running -= 1;

                        // 回收 PID
                        // TODO: 实现 pid_free()

                        return Ok(child_pid);
                    }
                }
            }
        }

        // 有子进程但还没有退出的
        if found_child {
            // 返回 EAGAIN (-11)，sys_wait4 会将其转换为 0
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            // 没有子进程
            // 返回 ECHILD (-10)
            Err(errno::Errno::NoChild.as_neg_i32())
        }
    }
}

// ============================================================================
// 负载均衡机制 (Load Balancing)
// ============================================================================

fn rq_load(rq: &RunQueue) -> usize {
    rq.nr_running
}

fn find_busiest_cpu(this_cpu: usize) -> Option<usize> {
    let this_rq = cpu_rq(this_cpu)?;
    let this_load = rq_load(&*this_rq.lock());

    let mut busiest_cpu = None;
    let mut max_load = this_load;

    // 负载不平衡阈值（至少差 2 个任务才进行迁移）
    const LOAD_IMBALANCE_THRESH: usize = 2;

    for cpu in 0..MAX_CPUS {
        if cpu == this_cpu {
            continue;  // 跳过当前 CPU
        }

        if let Some(rq) = cpu_rq(cpu) {
            let load = rq_load(&*rq.lock());

            // 只有当其他 CPU 负载明显更高时才进行迁移
            if load > max_load + LOAD_IMBALANCE_THRESH {
                max_load = load;
                busiest_cpu = Some(cpu);
            }
        }
    }

    busiest_cpu
}

fn steal_task(src_rq: &mut RunQueue) -> Option<*mut Task> {
    // 从队尾开始查找（最久未运行的任务）
    for i in (0..src_rq.nr_running).rev() {
        let task = src_rq.tasks[i];

        if task.is_null() {
            continue;
        }

        let task_ref = unsafe { &*task };

        // 不要窃取 idle 任务 (PID 0)
        if task_ref.pid() == 0 {
            continue;
        }

        // 不要窃取当前正在运行的任务
        if task == src_rq.current {
            continue;
        }

        // 找到可迁移的任务
        // 从源队列移除
        src_rq.tasks[i] = core::ptr::null_mut();
        src_rq.nr_running -= 1;

        // 移动剩余任务填补空位
        for j in i..src_rq.nr_running {
            src_rq.tasks[j] = src_rq.tasks[j + 1];
        }
        src_rq.tasks[src_rq.nr_running] = core::ptr::null_mut();

        return Some(task);
    }

    None
}

pub fn load_balance() {
    unsafe {
        let this_cpu = crate::arch::cpu_id() as u64 as usize;

        // 获取当前 CPU 的运行队列
        let this_rq = match this_cpu_rq() {
            Some(r) => r,
            None => return,
        };

        let this_rq_inner = this_rq.lock();
        let this_load = rq_load(&*this_rq_inner);

        // 只有当前 CPU 空闲或很空闲时才进行负载均衡
        // 阈值：当前负载 <= 1（只有 idle 任务或只有一个用户任务）
        if this_load > 1 {
            return;  // 当前 CPU 有足够任务，不需要负载均衡
        }

        drop(this_rq_inner);  // 释放锁，避免死锁

        // 查找最繁忙的 CPU
        if let Some(busiest_cpu) = find_busiest_cpu(this_cpu) {
            if let Some(busiest_rq) = cpu_rq(busiest_cpu) {
                let mut busiest_rq_inner = busiest_rq.lock();

                // 从繁忙 CPU 窃取任务
                if let Some(task) = steal_task(&mut *busiest_rq_inner) {
                    // 获取任务信息
                    let _task_pid = (*task).pid();

                    // 释放繁忙 CPU 的锁
                    drop(busiest_rq_inner);

                    // 重新获取当前 CPU 的锁
                    let mut this_rq_inner = this_rq.lock();

                    // 添加任务到当前 CPU 的运行队列
                    enqueue_task_locked(&mut *this_rq_inner, task);

                    // 更新任务的 CPU 亲和性（可选）
                    // (*task).set_cpu(this_cpu);
                }
            }
        }
    }
}

fn enqueue_task_locked(rq: &mut RunQueue, task: *mut Task) {
    if rq.nr_running >= MAX_TASKS {
        return;
    }

    // 添加到队尾
    rq.tasks[rq.nr_running] = task;
    rq.nr_running += 1;
}

// ============================================================================
// CPU 空闲循环 (CPU Idle Loop)
// ============================================================================

/// CPU 空闲循环
///
/// 当 CPU 没有任务可运行时调用此函数
/// 会尝试负载均衡，如果没有任务则进入 WFI 休眠
pub fn cpu_idle_loop() -> ! {
    use crate::arch;

    loop {
        // 1. 尝试调度任务
        unsafe {
            schedule();
        }

        // 2. 检查是否只有 idle 任务
        if let Some(rq) = this_cpu_rq() {
            let rq_inner = rq.lock();
            let current = rq_inner.current;
            let nr_running = rq_inner.nr_running;
            drop(rq_inner);

            // 如果只有 idle 任务（nr_running == 1 且 current 是 idle）
            // 或者完全没有任务（nr_running == 0，不应该发生）
            if nr_running == 1 && !current.is_null() {
                unsafe {
                    let pid = (*current).pid();
                    if pid == 0 {
                        // 只有 idle 任务，尝试负载均衡
                        drop(rq);
                        load_balance();

                        // 负载均衡后重新调度
                        schedule();
                    }
                }
            }
        }

        // 3. 进入 WFI 休眠，等待中断唤醒
        // 中断会设置 need_resched 标志，从而跳出 WFI
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}
