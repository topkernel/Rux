//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 调度器实现
//!
//! 完全遵循 Linux 内核的调度器设计 (kernel/sched/core.c)
//!
//! Linux 调度器架构：
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
use crate::debug_println;
use crate::fs::{FdTable, File, FileFlags, FileOps, CharDev};
use crate::collection::SimpleArc;
use crate::sched::pid::alloc_pid;
use core::arch::asm;
use spin::Mutex;

const MAX_TASKS: usize = 256;

pub const MAX_CPUS: usize = 4;

pub struct RunQueue {
    /// 运行队列 - 使用原始指针
    tasks: [*mut Task; MAX_TASKS],

    /// 当前运行的任务
    pub current: *mut Task,

    /// 任务数量
    nr_running: usize,

    /// 空闲任务
    idle: *mut Task,

    /// Round Robin 调度索引
    /// 记录上一次调度到的位置，实现循环遍历
    sched_index: usize,
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

const DEFAULT_TIME_SLICE_MS: u32 = 100;

const TIME_SLICE_TICKS: u32 = DEFAULT_TIME_SLICE_MS as u32;  // 10

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
            tasks: [core::ptr::null_mut(); MAX_TASKS],
            current: core::ptr::null_mut(),
            nr_running: 0,
            idle: core::ptr::null_mut(),
            sched_index: 0,
        }));

        init_flags[cpu_id] = true;
    }
}

static mut IDLE_TASK_STORAGE: core::mem::MaybeUninit<Task> = core::mem::MaybeUninit::uninit();

const TASK_POOL_SIZE: usize = 16;

// 计算 Task 结构体的实际大小，确保每个槽位足够大
// Task 包含：CpuContext、AddressSpace、Option<Box<FdTable>>、
//            Option<Box<SignalStruct>>、ListHead 等
const TASK_SIZE: usize = core::mem::size_of::<Task>();

// 使用原始字节数组作为任务池，每个槽位大小为 TASK_SIZE
// 这样可以避免 Copy trait 要求，同时确保有足够的空间
static mut TASK_POOL: [u8; TASK_POOL_SIZE * TASK_SIZE] = [0; TASK_POOL_SIZE * TASK_SIZE];
static mut TASK_POOL_NEXT: usize = 0;

pub fn init() {
    // 初始化当前 CPU 的运行队列
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    init_per_cpu_rq(cpu_id);

    unsafe {
        // 在静态存储上直接构造 Task
        // 使用 MaybeUninit 避免布局问题
        let idle_ptr = IDLE_TASK_STORAGE.as_mut_ptr();
        Task::new_idle_at(idle_ptr);

        // 设置当前 CPU 的运行队列
        if let Some(rq) = this_cpu_rq() {
            let mut rq_inner = rq.lock();
            rq_inner.idle = idle_ptr;
            rq_inner.current = idle_ptr;
        }
    }

    println!("sched: Process scheduler initialized");
}

#[inline(never)]
pub fn schedule() {
    unsafe {
        __schedule();
    }
}

unsafe fn __schedule() {
    // 清除 need_resched 标志（对应 Linux 的 clear_tsk_need_resched）
    clear_need_resched();

    // 获取当前 CPU 的运行队列
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => {
            // 运行队列未初始化
            return;
        }
    };

    let mut rq_inner = rq.lock();

    // 获取当前任务
    let prev = rq_inner.current;

    if prev.is_null() {
        return;
    }

    let prev_pid = (*prev).pid();

    // 如果只有 idle 任务，尝试负载均衡
    if rq_inner.nr_running == 0 || (rq_inner.nr_running == 1 && prev_pid == 0) {
        drop(rq_inner);
        load_balance();  // 尝试从其他 CPU 窃取任务

        // 重新获取运行队列
        let rq = match this_cpu_rq() {
            Some(r) => r,
            None => return,
        };
        rq_inner = rq.lock();

        // 如果还是没有任务，直接返回
        if rq_inner.nr_running == 0 || (rq_inner.nr_running == 1 && prev_pid == 0) {
            return;
        }
    }

    // 选择下一个任务
    let next = pick_next_task(&mut *rq_inner);

    if next == prev {
        // 还是当前任务，不需要切换
        return;
    }

    // 上下文切换（需要在锁外执行）
    drop(rq_inner);
    context_switch(&mut *prev, &mut *next);
}

unsafe fn pick_next_task(rq: &mut RunQueue) -> *mut Task {
    let current = rq.current;
    let start_index = rq.sched_index;

    // 从 sched_index + 1 开始查找，实现循环遍历
    for offset in 1..=MAX_TASKS {
        let idx = (start_index + offset) % MAX_TASKS;
        let task_ptr = rq.tasks[idx];

        // 找到一个非空且不是当前任务的任务
        if !task_ptr.is_null() && task_ptr != current {
            // 检查任务状态，只选择 Running 状态的任务
            // 对应 Linux 的 task_is_running() (include/linux/sched.h)
            if (*task_ptr).state() == TaskState::Running {
                // 更新 sched_index 到这个任务的位置
                rq.sched_index = idx;
                return task_ptr;
            }
        }
    }

    // 没找到其他可运行任务，检查当前任务是否可运行
    if !current.is_null() && (*current).state() == TaskState::Running {
        return current;
    }

    // 没有可运行任务，返回 idle 任务
    rq.idle
}

unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 更新当前任务
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        rq_inner.current = next;
    }

    // 执行实际的上下文切换
    arch::context_switch(prev, next);
}

pub fn enqueue_task(task: &'static mut Task) {
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();

        if rq_inner.nr_running < MAX_TASKS {
            for i in 0..MAX_TASKS {
                if rq_inner.tasks[i].is_null() {
                    rq_inner.tasks[i] = task;
                    rq_inner.nr_running += 1;
                    task.set_state(TaskState::Running);
                    return;
                }
            }
        }
    } else {
        println!("sched: enqueue_task - no runqueue");
    }
}

pub fn dequeue_task(task: &Task) {
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        let task_ptr = task as *const Task as *mut Task;
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

#[inline(never)]
fn debug_schedule(msg: &str) {
    unsafe {
        use crate::console::putchar;
        const PREFIX: &[u8] = b"[sched:";
        for &b in PREFIX {
            putchar(b);
        }
        for &b in msg.as_bytes() {
            putchar(b);
        }
        const SUFFIX: &[u8] = b"]\n";
        for &b in SUFFIX {
            putchar(b);
        }
    }
}

#[inline(never)]
fn debug_schedule_num(msg: &str, num: u32) {
    unsafe {
        use crate::console::putchar;
        const PREFIX: &[u8] = b"[sched:";
        for &b in PREFIX {
            putchar(b);
        }
        for &b in msg.as_bytes() {
            putchar(b);
        }
        const SEP: &[u8] = b"=";
        for &b in SEP {
            putchar(b);
        }
        // 打印数字
        let mut n = num;
        let mut digits = [0u8; 10];
        let mut len = 0;
        if n == 0 {
            digits[0] = b'0';
            len = 1;
        } else {
            while n > 0 {
                digits[len] = b'0' + (n % 10) as u8;
                n /= 10;
                len += 1;
            }
        }
        for i in (0..len).rev() {
            putchar(digits[i]);
        }
        const SUFFIX: &[u8] = b"]\n";
        for &b in SUFFIX {
            putchar(b);
        }
    }
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

pub fn do_fork() -> Option<Pid> {
    unsafe {
        // 获取当前任务（父进程）
        let rq = match this_cpu_rq() {
            Some(r) => r,
            None => {
                println!("do_fork: no runqueue");
                return None;
            }
        };

        let current = rq.lock().current;
        if current.is_null() {
            println!("do_fork: no current task");
            return None;
        }

        // 从任务池分配一个槽位
        if TASK_POOL_NEXT >= TASK_POOL_SIZE {
            println!("do_fork: task pool exhausted");
            return None;
        }

        let pool_idx = TASK_POOL_NEXT;
        TASK_POOL_NEXT += 1;

        // 分配新的PID
        let pid = match alloc_pid() {
            Some(p) => p,
            None => {
                println!("do_fork: failed to allocate PID");
                return None;
            }
        };

        // 在任务池槽位上直接构造 Task
        let pool_slot_addr = TASK_POOL.as_ptr().add(pool_idx * TASK_SIZE);
        let task_ptr: *mut Task = pool_slot_addr as *mut Task;

        Task::new_task_at(task_ptr, pid, SchedPolicy::Normal);

        // 复制父进程的状态到子进程
        (*task_ptr).set_parent(current);

        // 复制父进程的 CPU 上下文
        // 子进程将从系统调用返回后的位置继续执行
        let parent_ctx = (*current).context();
        let child_ctx = (*task_ptr).context_mut();
        *child_ctx = parent_ctx.clone();

        // 设置子进程的返回值为 0（x0/a0 寄存器）
        // 这是 fork 的关键特性：子进程返回 0，父进程返回子进程 PID
        child_ctx.x0 = 0;

        // 复制信号状态
        (*task_ptr).sigmask = (*current).sigmask;
        // TODO: 复制 SignalStruct（当前为 None）

        // TODO: 复制文件描述符表
        // 当前实现：两个进程共享相同的 FdTable（不安全，需要实现 copy-on-write）

        // TODO: 复制内存映射
        // 当前实现：暂时不复制地址空间，两个进程共享同一地址空间
        // 这需要实现写时复制（Copy-on-Write）机制

        // 将新任务加入运行队列
        enqueue_task(&mut *task_ptr);

        // TODO: 将子进程添加到父进程的 children 链表
        // 当前需要实现 Task::add_child()

        // 返回子进程 PID（父进程的返回值）
        Some(pid)
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
    if let Some(rq) = this_cpu_rq() {
        let rq_inner = rq.lock();
        let current = rq_inner.current;
        if current.is_null() {
            None
        } else {
            unsafe { (*current).try_fdtable() }
        }
    } else {
        None
    }
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
            let stdin = SimpleArc::new(File::new(FileFlags::new(FileFlags::O_RDONLY))).expect("Failed to create stdin");
            stdin.set_ops(&UART_OPS);
            stdin.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 创建 stdout (fd=1)
            let stdout = SimpleArc::new(File::new(FileFlags::new(FileFlags::O_WRONLY))).expect("Failed to create stdout");
            stdout.set_ops(&UART_OPS);
            stdout.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 创建 stderr (fd=2)
            let stderr = SimpleArc::new(File::new(FileFlags::new(FileFlags::O_WRONLY))).expect("Failed to create stderr");
            stderr.set_ops(&UART_OPS);
            stderr.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // 安装标准文件描述符
            let _ = fdtable.install_fd(0, stdin);
            let _ = fdtable.install_fd(1, stdout);
            let _ = fdtable.install_fd(2, stderr);

            println!("Scheduler: initialized stdin/stdout/stderr");
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
                        println!("Signal: sent signal {} to PID {}", sig, pid);
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
                        println!("Signal: signal {} is masked for PID {}", sig, pid);
                        return Err(errno::Errno::TryAgain.as_neg_i32());
                    }

                    // 检查信号处理动作
                    if let Some(action) = signal_ref.get_action(sig) {
                        match action.action() {
                            crate::signal::SigActionKind::Ignore => {
                                println!("Signal: ignoring signal {} for PID {}", sig, pid);
                                return Ok(());  // 忽略信号
                            }
                            crate::signal::SigActionKind::Default => {
                                // 默认处理：加入待处理队列
                                task.pending.add(sig);
                                println!("Signal: sent signal {} to PID {} (default action)", sig, pid);
                                // 唤醒睡眠的进程
                                drop(rq_inner);  // 释放锁
                                use crate::signal;
                                signal::signal_wake_up(task_ptr);
                                return Ok(());
                            }
                            crate::signal::SigActionKind::Handler => {
                                // 用户自定义处理：加入待处理队列
                                task.pending.add(sig);
                                println!("Signal: sent signal {} to PID {} (handler)", sig, pid);
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
                                println!("Signal: terminating PID {} due to signal {}", (*current).pid(), sig);
                                (*current).pending.remove(sig);
                                // TODO: 实现进程终止
                            }
                            19 => {  // SIGSTOP
                                // 停止进程
                                println!("Signal: stopping PID {} due to signal {}", (*current).pid(), sig);
                                (*current).set_state(TaskState::Stopped);
                                (*current).pending.remove(sig);
                            }
                            18 => {  // SIGCONT
                                // 继续进程
                                println!("Signal: continuing PID {} due to signal {}", (*current).pid(), sig);
                                (*current).set_state(TaskState::Running);
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
                        println!("Signal: calling handler for signal {} on PID {}", sig, (*current).pid());
                        // TODO: 实现用户态信号处理函数调用
                        (*current).pending.remove(sig);
                    }
                }

                // 如果处理了信号，可能需要重新调度
                if (*current).state() == TaskState::Stopped {
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

            println!("do_exit: PID {} exiting with code {}", current_pid, exit_code);

            // 设置退出码
            (*current).set_exit_code(exit_code);

            // 设置进程状态为 Zombie
            (*current).set_state(TaskState::Zombie);

            // 从运行队列移除
            drop(rq_inner);  // 释放锁后再调用 dequeue_task
            dequeue_task(&*current);

            // 向父进程发送 SIGCHLD 信号并唤醒父进程
            if parent_pid != 0 {
                println!("do_exit: sending SIGCHLD to parent PID {}", parent_pid);
                let _ = send_signal(parent_pid, Signal::SIGCHLD as i32);

                // 唤醒父进程（如果父进程在 wait4 中阻塞等待）
                // 对应 Linux 内核的 wake_up_process(current->parent) (kernel/exit.c)
                let parent = find_task_by_pid(parent_pid);
                if !parent.is_null() {
                    println!("do_exit: waking up parent PID {}", parent_pid);
                    wake_up_process(parent);
                }
            }

            // 调度器选择下一个进程运行
            println!("do_exit: scheduling next process");
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

        // 循环等待子进程退出
        // 对应 Linux 内核的 do_wait() 循环 (kernel/exit.c)
        loop {
            let mut found_child = false;

            println!("do_wait: PID {} waiting for child (pid={})", current_pid, pid);

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
                        if task.state() == TaskState::Zombie {
                            let child_pid = task.pid();
                            let exit_code = task.exit_code();

                            println!("do_wait: found zombie child PID {}, exit code {}", child_pid, exit_code);

                            // 写入退出状态
                            if !status_ptr.is_null() {
                                *status_ptr = exit_code;
                            }

                            // 从运行队列移除
                            rq_inner.tasks[i] = core::ptr::null_mut();
                            rq_inner.nr_running -= 1;

                            // 回收 PID
                            // TODO: 实现 pid_free()

                            println!("do_wait: reaped child PID {}", child_pid);
                            return Ok(child_pid);
                        }
                    }
                }
            }

            // 有子进程但还没有退出的
            if found_child {
                // 真正的阻塞等待
                // 对应 Linux 内核的 set_current_state(TASK_INTERRUPTIBLE) + schedule()
                println!("do_wait: children exist but none exited yet, sleeping...");

                // 使用 Task::sleep() 进入可中断睡眠状态
                // 这会设置当前进程状态为 Interruptible 并触发调度
                crate::process::Task::sleep(crate::process::task::TaskState::Interruptible);

                // 被唤醒后，检查是否有信号到达
                // 对应 Linux 内核的 signal_pending() (include/linux/sched/signal.h)
                use crate::signal;
                if signal::signal_pending() {
                    println!("do_wait: interrupted by signal");
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());  // EINTR
                }

                // 继续循环检查是否有子进程退出
                println!("do_wait: woke up, checking again...");
            } else {
                // 没有子进程
                println!("do_wait: no children");
                // 返回 ECHILD (-10)
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

        println!("do_wait_nonblock: PID {} checking child (pid={})", current_pid, pid);

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
                    if task.state() == TaskState::Zombie {
                        let child_pid = task.pid();
                        let exit_code = task.exit_code();

                        println!("do_wait_nonblock: found zombie child PID {}, exit code {}", child_pid, exit_code);

                        // 写入退出状态
                        if !status_ptr.is_null() {
                            *status_ptr = exit_code;
                        }

                        // 从运行队列移除
                        rq_inner.tasks[i] = core::ptr::null_mut();
                        rq_inner.nr_running -= 1;

                        // 回收 PID
                        // TODO: 实现 pid_free()

                        println!("do_wait_nonblock: reaped child PID {}", child_pid);
                        return Ok(child_pid);
                    }
                }
            }
        }

        // 有子进程但还没有退出的
        if found_child {
            println!("do_wait_nonblock: children exist but none exited yet");
            // 返回 EAGAIN (-11)，sys_wait4 会将其转换为 0
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            // 没有子进程
            println!("do_wait_nonblock: no children");
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

                    println!("load_balance: migrated task from CPU {} to CPU {}", busiest_cpu, this_cpu);

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
