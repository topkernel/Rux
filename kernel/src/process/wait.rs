//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 等待队列 (Wait Queue) 机制
//!
//! 完全...
//! - `kernel/sched/wait.c` - 等待队列操作
//!
//! 核心概念：
//! - 等待队列用于实现进程阻塞和唤醒
//! - 当进程需要等待某个条件时，加入等待队列并调用 schedule()
//! - 当条件满足时，通过 wake_up() 唤醒等待的进程

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::Task;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum WakeUpHint {
    /// 正常唤醒
    Normal = 0,
    /// 异步唤醒（不实际唤醒进程，仅标记）
    Async = 1,
}

#[repr(C)]
pub struct WaitQueueEntry {
    /// 关联的任务
    task: *mut Task,
    /// 独占标志 (WQ_FLAG_EXCLUSIVE)
    exclusive: bool,
    /// 是否已唤醒
    woken: AtomicBool,
}

impl WaitQueueEntry {
    /// 创建新的等待队列项
    ///
    /// # 参数
    /// * `task` - 关联的任务
    /// * `exclusive` - 是否为独占模式（互斥）
    pub fn new(task: *mut Task, exclusive: bool) -> Self {
        Self {
            task,
            exclusive,
            woken: AtomicBool::new(false),
        }
    }

    /// 检查是否已被唤醒
    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    /// 标记为已唤醒
    pub fn set_woken(&self) {
        self.woken.store(true, Ordering::Release);
    }

    /// 获取关联的任务
    pub fn task(&self) -> *mut Task {
        self.task
    }

    /// 检查是否为独占模式
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }
}

#[repr(C)]
pub struct WaitQueueHead {
    /// 等待队列列表
    /// 使用 Vec 存储等待的进程
    list: Mutex<Vec<WaitQueueEntry>>,
}

impl WaitQueueHead {
    /// 创建新的等待队列头
    ///
    /// ...
    pub const fn new() -> Self {
        Self {
            list: Mutex::new(Vec::new()),
        }
    }

    /// 初始化等待队列头（运行时初始化）
    ///
    /// ...
    pub fn init(&self) {
        // Vec 已经自动初始化
    }

    /// 添加到等待队列
    ///
    /// # 参数
    /// * `entry` - 等待队列项
    ///
    /// ...
    pub fn add(&self, entry: WaitQueueEntry) {
        let mut list = self.list.lock();
        // 非独占项添加到头部，独占项添加到尾部
        if entry.is_exclusive() {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
    }

    /// 从等待队列移除
    ///
    /// # 参数
    /// * `task` - 要移除的任务
    ///
    /// ...
    pub fn remove(&self, task: *mut Task) {
        let mut list = self.list.lock();
        list.retain(|entry| entry.task() != task);
    }

    /// 唤醒等待队列中的进程
    ///
    /// # 参数
    /// * `mode` - 唤醒模式
    /// * `nr` - 要唤醒的进程数量 (0 表示唤醒所有)
    ///
    /// # 返回
    /// 实际唤醒的进程数量
    ///
    /// ...
    pub fn wake_up(&self, _mode: WakeUpHint, nr: usize) -> usize {
        let list = self.list.lock();
        let mut awakened = 0;

        // 确定最大唤醒数量
        let max_wake = if nr == 0 { usize::MAX } else { nr };

        // 从列表头部开始唤醒
        for entry in list.iter() {
            if awakened >= max_wake {
                break;
            }

            if !entry.is_woken() {
                entry.set_woken();
                // TODO: 实际唤醒进程
                // 当前简化实现：标记为已唤醒
                // 完整实现需要将进程添加到运行队列
                awakened += 1;

                // 独占模式：只唤醒一个
                if entry.is_exclusive() {
                    break;
                }
            }
        }

        awakened
    }

    /// 唤醒所有等待的进程（非独占）
    ///
    /// ...
    pub fn wake_up_all(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 0)
    }

    /// 唤醒一个进程（独占）
    ///
    /// ...
    pub fn wake_up_one(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 1)
    }
}

#[macro_export]
macro_rules! wait_event {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // 检查条件
            if $condition {
                break;
            }

            // 条件不满足，添加到等待队列
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break,
            };

            let entry = $crate::process::wait::WaitQueueEntry::new(current, false);

            // 添加到等待队列
            wq_head.add(entry);

            // 让出 CPU
            #[cfg(feature = "riscv64")]
            crate::sched::schedule();

            // 被唤醒后，从等待队列移除
            wq_head.remove(current);

            // 重新检查条件
        }
    }};
}

#[macro_export]
macro_rules! wait_event_interruptible {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // 检查条件
            if $condition {
                break true;
            }

            // 检查是否有待处理信号
            // TODO: 实现信号检查
            // if has_pending_signal() {
            //     break false;
            // }

            // 条件不满足，添加到等待队列
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break true,
            };

            let entry = $crate::process::wait::WaitQueueEntry::new(current, false);

            // 添加到等待队列
            wq_head.add(entry);

            // 让出 CPU
            #[cfg(feature = "riscv64")]
            crate::sched::schedule();

            // 被唤醒后，从等待队列移除
            wq_head.remove(current);

            // 重新检查条件
        }
    }};
}
