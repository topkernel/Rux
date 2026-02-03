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

use crate::process::task::{Task, TaskState, SchedPolicy, CpuContext};
use crate::process::Pid;
use crate::arch;
use crate::println;
use crate::debug_println;
use crate::fs::{FdTable, File, FileFlags, FileOps, CharDev};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::arch::asm;

/// 运行队列最大任务数
const MAX_TASKS: usize = 256;

/// 全局运行队列
///
/// 对应 Linux 内核的 runqueue (rq)
/// 使用原始指针以避免借用检查器问题
pub struct RunQueue {
    /// 运行队列 - 使用原始指针
    tasks: [*mut Task; MAX_TASKS],

    /// 当前运行的任务
    pub current: *mut Task,

    /// 任务数量
    nr_running: usize,

    /// 空闲任务
    idle: *mut Task,
}

unsafe impl Send for RunQueue {}

/// 全局运行队列
///
/// TODO: 多核支持，每个 CPU 一个 rq
pub static mut RQ: RunQueue = RunQueue {
    tasks: [core::ptr::null_mut(); MAX_TASKS],
    current: core::ptr::null_mut(),
    nr_running: 0,
    idle: core::ptr::null_mut(),
};

/// Idle 任务的静态存储
/// 使用静态存储以避免启动时堆未初始化的问题
static mut IDLE_TASK_STORAGE: Option<Task> = None;

/// 调度器初始化
///
/// 对应 Linux 内核的 sched_init() (kernel/sched/core.c)
pub fn init() {
    use crate::console::putchar;
    const MSG: &[u8] = b"Scheduler: initializing...\n";
    for &b in MSG {
        unsafe { putchar(b); }
    }

    unsafe {
        const MSG2: &[u8] = b"Scheduler: using static storage for idle task\n";
        for &b in MSG2 {
            putchar(b);
        }

        // 在静态存储上直接构造 Task
        // 使用 ptr::write 避免先创建再移动
        let idle_ptr = &mut IDLE_TASK_STORAGE as *mut _ as *mut Task;

        const MSG3: &[u8] = b"Scheduler: initializing idle task at static location\n";
        for &b in MSG3 {
            putchar(b);
        }

        Task::new_idle_at(idle_ptr);

        const MSG4: &[u8] = b"Scheduler: Task initialized\n";
        for &b in MSG4 {
            putchar(b);
        }

        RQ.idle = idle_ptr;
        RQ.current = idle_ptr;

        const MSG5: &[u8] = b"Scheduler: idle task (PID 0) setup complete\n";
        for &b in MSG5 {
            putchar(b);
        }
    }

    const MSG6: &[u8] = b"Scheduler: initialization complete\n";
    for &b in MSG6 {
        unsafe { putchar(b); }
    }

    // 直接使用 UART 测试输出
    unsafe {
        putchar(b'!');
        putchar(b'\n');
        putchar(b'O');
        putchar(b'K');
        putchar(b'\n');
    }
}

/// 调度入口函数
///
/// 对应 Linux 内核的 schedule() (kernel/sched/core.c)
///
/// schedule() 是调度器的主入口，当进程需要让出 CPU 时调用：
/// - 当前进程自愿放弃 CPU (yield)
/// - 时间片用完
/// - 等待资源（睡眠）
#[inline(never)]
pub fn schedule() {
    unsafe {
        __schedule();
    }
}

/// 内部调度函数
///
/// 对应 Linux 内核的 __schedule() (kernel/sched/core.c)
///
/// 核心调度流程：
/// 1. 保存当前进程状态
/// 2. 选择下一个进程 (pick_next_task)
/// 3. 上下文切换 (context_switch)
unsafe fn __schedule() {
    // 获取当前任务
    let prev = RQ.current;

    if prev.is_null() {
        return;
    }

    let prev_pid = (*prev).pid();

    // 如果只有 idle 任务，直接返回
    if RQ.nr_running == 0 || (RQ.nr_running == 1 && prev_pid == 0) {
        return;
    }

    // 选择下一个任务
    let next = pick_next_task();

    if next == prev {
        // 还是当前任务，不需要切换
        return;
    }

    // 上下文切换
    context_switch(&mut *prev, &mut *next);
}

/// 选择下一个要运行的任务
///
/// 对应 Linux 内核的 pick_next_task() (kernel/sched/core.c)
///
/// Linux 使用调度类 (sched_class) 来选择任务：
/// - deadline 调度类
/// - 实时调度类
/// - fair 调度类 (CFS)
/// - idle 调度类
///
/// 当前实现: 简单的 FIFO 选择
unsafe fn pick_next_task() -> *mut Task {
    let current = RQ.current;

    // 找到第一个非当前任务
    for i in 0..MAX_TASKS {
        let task_ptr = RQ.tasks[i];
        if !task_ptr.is_null() && task_ptr != current {
            return task_ptr;
        }
    }

    // 没找到，返回 idle
    RQ.idle
}

/// 上下文切换
///
/// 对应 Linux 内核的 context_switch() (kernel/sched/core.c)
///
/// context_switch() 执行进程切换：
/// 1. 切换地址空间 (switch_mm_irqs_off) - TODO
/// 2. 切换寄存器上下文 (switch_to)
unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 更新当前任务
    RQ.current = next;

    // 执行实际的上下文切换
    arch::context_switch(prev, next);
}

/// 将任务加入运行队列
///
/// 对应 Linux 内核的 enqueue_task() (kernel/sched/core.c)
pub fn enqueue_task(task: &'static mut Task) {
    unsafe {
        if RQ.nr_running < MAX_TASKS {
            for i in 0..MAX_TASKS {
                if RQ.tasks[i].is_null() {
                    RQ.tasks[i] = task;
                    RQ.nr_running += 1;
                    task.set_state(TaskState::Running);
                    return;
                }
            }
        }
    }
}

/// 将任务从运行队列移除
///
/// 对应 Linux 内核的 dequeue_task() (kernel/sched/core.c)
pub fn dequeue_task(task: &Task) {
    unsafe {
        let task_ptr = task as *const Task as *mut Task;
        for i in 0..MAX_TASKS {
            if RQ.tasks[i] == task_ptr {
                RQ.tasks[i] = core::ptr::null_mut();
                RQ.nr_running -= 1;
                return;
            }
        }
    }
}

/// 主动让出 CPU
///
/// 对应 Linux 内核的 schedule() + PREEMPT_ACTIVE
pub fn yield_cpu() {
    schedule();
}

/// 获取当前运行的任务
pub fn current() -> Option<&'static mut Task> {
    unsafe {
        if RQ.current.is_null() {
            None
        } else {
            Some(&mut *RQ.current)
        }
    }
}

/// fork 系统调用 - 创建新进程
///
/// 对应 Linux 内核的 do_fork() (kernel/fork.c)
pub fn do_fork() -> Option<Pid> {
    use crate::process::pid::alloc_pid;

    unsafe {
        // 获取当前任务（父进程）
        let current = RQ.current;
        if current.is_null() {
            println!("do_fork: no current task");
            return None;
        }

        // 分配新的PID
        let pid = alloc_pid()?;
        let mut new_task = Task::new(pid, SchedPolicy::Normal);

        // 复制地址空间（如果父进程有地址空间）
        if let Some(parent_addr_space) = (*current).address_space() {
            match parent_addr_space.fork() {
                Ok(child_addr_space) => {
                    new_task.set_address_space(Some(child_addr_space));
                    println!("do_fork: forked address space for PID {}", pid);
                }
                Err(e) => {
                    println!("do_fork: failed to fork address space: {:?}", e);
                    return None;
                }
            }
        }

        // 设置父进程指针
        new_task.set_parent(current);

        // TODO: 复制父进程上下文（寄存器状态）
        // TODO: 分配内核栈

        // 将新任务加入运行队列
        let task_box = Box::leak(Box::new(new_task));
        enqueue_task(task_box);

        println!("do_fork: created process PID {}", pid);

        Some(pid)
    }
}

/// 获取当前进程的PID
pub fn get_current_pid() -> u32 {
    unsafe {
        if RQ.current.is_null() {
            0
        } else {
            (*RQ.current).pid()
        }
    }
}

/// 获取当前进程的父进程PID
pub fn get_current_ppid() -> u32 {
    unsafe {
        if RQ.current.is_null() {
            0
        } else {
            (*RQ.current).ppid()
        }
    }
}

/// 获取当前进程的文件描述符表
pub fn get_current_fdtable() -> Option<&'static FdTable> {
    unsafe {
        if RQ.current.is_null() {
            None
        } else {
            (*RQ.current).try_fdtable()
        }
    }
}

/// 初始化标准文件描述符 (stdin, stdout, stderr)
///
/// 为当前任务设置标准的输入、输出、错误文件描述符
pub fn init_std_fds() {
    use crate::fs::char_dev::{CharDev, CharDevType};

    unsafe {
        if RQ.current.is_null() {
            return;
        }

        // Idle 任务没有 fdtable
        let fdtable = match (*RQ.current).try_fdtable_mut() {
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

        println!("Scheduler: initialized stdin/stdout/stderr");
    }
}

/// UART 文件读取操作
unsafe fn uart_file_read(file: *mut File, buf: *mut u8, count: usize) -> isize {
    if let Some(priv_data) = *(*file).private_data.get() {
        let char_dev = &*(priv_data as *const CharDev);
        return char_dev.read(buf, count);
    }
    -9  // EBADF
}

/// UART 文件写入操作
unsafe fn uart_file_write(file: *mut File, buf: *const u8, count: usize) -> isize {
    if let Some(priv_data) = *(*file).private_data.get() {
        let char_dev = &*(priv_data as *const CharDev);
        return char_dev.write(buf, count);
    }
    -9  // EBADF
}

// ============================================================================
// 信号处理
// ============================================================================

/// 发送信号到指定进程
///
/// 对应 Linux 内核的 kill_something_info (kernel/signal.c)
pub fn send_signal(pid: Pid, sig: i32) -> Result<(), i32> {
    use crate::signal::{Signal, SigAction};

    // 检查信号编号是否有效
    if sig < 1 || sig > 64 {
        return Err(-22_i32);  // EINVAL - 无效参数
    }

    unsafe {
        // 遍历运行队列查找目标进程
        for i in 0..MAX_TASKS {
            let task_ptr = RQ.tasks[i];
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
                return Ok(());
            }

            // Idle 任务没有信号处理
            let signal_ref: &crate::signal::SignalStruct = match task.signal.as_ref() {
                Some(s) => s,
                None => {
                    // 没有 signal 结构，直接加入待处理队列
                    task.pending.add(sig);
                    return Ok(());
                }
            };

            // 检查信号是否被屏蔽
            if signal_ref.is_masked(sig) {
                println!("Signal: signal {} is masked for PID {}", sig, pid);
                return Err(-11_i32);  // EAGAIN - 信号被屏蔽
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
                        return Ok(());
                    }
                    crate::signal::SigActionKind::Handler => {
                        // 用户自定义处理：加入待处理队列
                        task.pending.add(sig);
                        println!("Signal: sent signal {} to PID {} (handler)", sig, pid);
                        return Ok(());
                    }
                }
            }
        }

        // 未找到进程
        Err(-3_i32)  // ESRCH - 没有该进程
    }
}

/// 发送信号到当前进程
pub fn send_signal_self(sig: i32) -> Result<(), i32> {
    let current_pid = get_current_pid();
    send_signal(current_pid, sig)
}

/// 处理待处理的信号
///
/// 对应 Linux 内核的 do_signal (arch/arm64/kernel/signal.c)
pub fn handle_pending_signals() {
    use crate::signal::{Signal, SigActionKind};

    unsafe {
        let current = RQ.current;
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
                schedule();
                break;
            }
        }
    }
}

/// 检查并处理信号
///
/// 在返回用户空间前调用
pub fn check_and_handle_signals() {
    handle_pending_signals();
}

// ============================================================================
// 进程退出和等待
// ============================================================================

/// 进程退出
///
/// 对应 Linux 内核的 do_exit() (kernel/exit.c)
///
/// do_exit() 终止当前进程：
/// 1. 设置进程状态为 Zombie
/// 2. 保存退出码
/// 3. 向父进程发送 SIGCHLD
/// 4. 调度器选择新进程运行
pub fn do_exit(exit_code: i32) -> ! {
    use crate::signal::Signal;

    unsafe {
        let current = RQ.current;
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
        dequeue_task(&*current);

        // 向父进程发送 SIGCHLD 信号
        if parent_pid != 0 {
            println!("do_exit: sending SIGCHLD to parent PID {}", parent_pid);
            let _ = send_signal(parent_pid, Signal::SIGCHLD as i32);
        }

        // 调度器选择下一个进程运行
        println!("do_exit: scheduling next process");
        schedule();

        // 永远不会到达这里
        loop {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

/// 等待子进程
///
/// 对应 Linux 内核的 do_wait() (kernel/exit.c)
///
/// do_wait() 等待子进程状态改变：
/// - 如果子进程已退出 (Zombie)，回收资源并返回 PID
/// - 如果没有子进程，返回 ECHILD
/// - 如果子进程还未退出，阻塞等待（TODO）
pub fn do_wait(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    use crate::process::pid::alloc_pid;

    unsafe {
        let current = RQ.current;
        if current.is_null() {
            return Err(-1_i32);
        }

        let current_pid = (*current).pid();
        let mut found_child = false;

        println!("do_wait: PID {} waiting for child (pid={})", current_pid, pid);

        // 遍历运行队列查找僵尸子进程
        for i in 0..MAX_TASKS {
            let task_ptr = RQ.tasks[i];
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
                RQ.tasks[i] = core::ptr::null_mut();
                RQ.nr_running -= 1;

                // 回收 PID
                // TODO: 实现 pid_free()

                println!("do_wait: reaped child PID {}", child_pid);
                return Ok(child_pid);
            }
        }

        // 有子进程但还没有退出的
        if found_child {
            // TODO: 实现阻塞等待
            println!("do_wait: children exist but none exited yet");
            Err(-10_i32)  // EAGAIN - 资源暂时不可用
        } else {
            // 没有子进程
            println!("do_wait: no children");
            Err(-10_i32)  // ECHILD - 没有子进程
        }
    }
}
