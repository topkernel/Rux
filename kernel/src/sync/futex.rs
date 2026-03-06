//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex 实现 - Fast Userspace Mutex
//!
//! 完全参考 Linux kernel/futex/ 实现
//! - kernel/futex/core.c
//! - kernel/futex/waitwake.c
//! - kernel/futex/futex.h

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use crate::process::Task;
use crate::process::task::TaskState;
use crate::syscall::errno::{EINVAL, EFAULT, EAGAIN, ENOSYS};

/// FUTEX 操作码 (来自 Linux include/uapi/linux/futex.h)
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;
pub const FUTEX_LOCK_PI2: i32 = 13;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

// Internal flags (from Linux kernel/futex/futex.h)
pub const FLAGS_SHARED: u32 = 0x0010;
pub const FLAGS_CLOCKRT: u32 = 0x0020;

/// Futex 键 - 唯一标识一个 futex
///
/// 参考 Linux: union futex_key
#[derive(Clone, Copy, Debug)]
pub struct FutexKey {
    /// 用户空间地址
    pub uaddr: usize,
    /// 进程 ID (用于私有 futex)
    pub pid: u32,
    /// 标志
    pub flags: u32,
}

impl FutexKey {
    pub fn new(uaddr: usize, pid: u32, flags: u32) -> Self {
        Self { uaddr, pid, flags }
    }

    /// 检查两个 key 是否匹配
    pub fn matches(&self, other: &FutexKey) -> bool {
        // 对于私有 futex，比较地址和 PID
        if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.pid == other.pid
        } else {
            // 对于共享 futex，只比较地址
            self.uaddr == other.uaddr
        }
    }
}

/// 等待者信息
struct Waiter {
    /// futex 键
    key: FutexKey,
    /// 等待的任务
    task: *mut Task,
    /// bitset
    bitset: u32,
    /// 是否已唤醒
    woken: bool,
    /// 下一个等待者
    next: Option<usize>,
}

// Waiter 可以跨线程发送，因为我们用 Mutex 保护访问
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

/// 等待者池大小
const WAITER_POOL_SIZE: usize = 256;

/// 等待者池
static WAITER_POOL: [spin::Mutex<Option<Waiter>>; WAITER_POOL_SIZE] = {
    const INIT: spin::Mutex<Option<Waiter>> = spin::Mutex::new(None);
    [INIT; WAITER_POOL_SIZE]
};

/// 哈希桶数量
const HASH_SIZE: usize = 64;

/// 每个桶的等待者链表头
static HASH_HEADS: [spin::Mutex<Option<usize>>; HASH_SIZE] = {
    const INIT: spin::Mutex<Option<usize>> = spin::Mutex::new(None);
    [INIT; HASH_SIZE]
};

/// 分配一个等待者槽位
fn alloc_waiter() -> Option<usize> {
    for i in 0..WAITER_POOL_SIZE {
        let mut slot = WAITER_POOL[i].lock();
        if slot.is_none() {
            return Some(i);
        }
    }
    None
}

/// 释放等待者槽位
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock();
    *slot = None;
}

/// 计算 futex 哈希值
fn futex_hash(key: &FutexKey) -> usize {
    let hash = key.uaddr.wrapping_add(key.pid as usize);
    hash % HASH_SIZE
}

/// 唤醒等待者
///
/// 参考 Linux: futex_wake()
pub fn futex_wake(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    // 获取当前进程 PID
    let pid = match crate::sched::current() {
        Some(t) => unsafe { (*t).pid() },
        None => return -EFAULT as i64,
    };

    // 创建 futex key
    let key = FutexKey::new(uaddr, pid, flags);

    // 获取哈希桶索引
    let bucket_idx = futex_hash(&key);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    let mut current_idx = *HASH_HEADS[bucket_idx].lock();

    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        let waiter_slot = WAITER_POOL[idx].lock();
        if let Some(ref waiter) = *waiter_slot {
            if waiter.key.matches(&key) && (waiter.bitset & bitset) != 0 {
                // 标记为已唤醒
                let woken_task = waiter.task;
                let next_idx = waiter.next;

                // 释放锁后再操作
                drop(waiter_slot);

                // 标记唤醒
                {
                    let mut w = WAITER_POOL[idx].lock();
                    if let Some(ref mut w) = *w {
                        w.woken = true;
                    }
                }

                // 设置任务状态为就绪
                if !woken_task.is_null() {
                    unsafe {
                        (*woken_task).set_state(TaskState::new(TaskState::RUNNING));
                    }
                }

                // 从链表中移除
                if prev_idx.is_none() {
                    *HASH_HEADS[bucket_idx].lock() = next_idx;
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock();
                    if let Some(ref mut prev_waiter) = *prev_slot {
                        prev_waiter.next = next_idx;
                    }
                }

                // 释放等待者槽位
                free_waiter(idx);

                ret += 1;
                current_idx = next_idx;
                continue;
            }
            prev_idx = Some(idx);
            current_idx = waiter.next;
        } else {
            break;
        }
    }

    ret
}

/// 等待 futex
///
/// 参考 Linux: futex_wait_setup() + __futex_wait()
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let uaddr_ptr = uaddr as *const AtomicU32;

    if uaddr_ptr.is_null() {
        return -EINVAL as i64;
    }

    // 获取当前进程
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };

    let pid = unsafe { (*current).pid() };

    // 创建 futex key
    let key = FutexKey::new(uaddr, pid, flags);

    // 获取哈希桶索引
    let bucket_idx = futex_hash(&key);

    // 锁定桶头
    let mut head = HASH_HEADS[bucket_idx].lock();

    // 再次检查值 (在持有锁的情况下)
    let uval = unsafe { (*uaddr_ptr).load(Ordering::SeqCst) };

    if uval != val {
        return -EAGAIN as i64;
    }

    // 分配等待者槽位
    let waiter_idx = match alloc_waiter() {
        Some(idx) => idx,
        None => return -ENOMEM as i64,
    };

    // 初始化等待者
    {
        let mut slot = WAITER_POOL[waiter_idx].lock();
        *slot = Some(Waiter {
            key,
            task: current,
            bitset,
            woken: false,
            next: *head,
        });
    }

    // 更新链表头
    *head = Some(waiter_idx);
    drop(head);

    // 设置任务状态为阻塞
    unsafe {
        (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }

    // 释放内核大锁（睡眠前必须释放）
    crate::sync::kernel_lock_release();

    // 调度让出 CPU
    crate::sched::schedule();

    // 唤醒后重新获取内核大锁
    crate::sync::kernel_lock_acquire();

    // 被唤醒后，检查是否需要清理
    {
        let slot = WAITER_POOL[waiter_idx].lock();
        if let Some(ref waiter) = *slot {
            if !waiter.woken {
                // 还没被唤醒，需要从链表中移除
                drop(slot);
                remove_waiter(bucket_idx, waiter_idx);
            }
        }
    }

    0
}

/// 从链表中移除等待者
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut head = HASH_HEADS[bucket_idx].lock();

    if *head == Some(target_idx) {
        // 目标是链表头
        let next = {
            let slot = WAITER_POOL[target_idx].lock();
            slot.as_ref().and_then(|w| w.next)
        };
        *head = next;
        free_waiter(target_idx);
        return;
    }

    // 遍历链表找目标
    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        let next = {
            let slot = WAITER_POOL[idx].lock();
            slot.as_ref().and_then(|w| w.next)
        };

        if next == Some(target_idx) {
            // 找到目标的前一个
            let target_next = {
                let target_slot = WAITER_POOL[target_idx].lock();
                target_slot.as_ref().and_then(|w| w.next)
            };
            {
                let mut slot = WAITER_POOL[idx].lock();
                if let Some(ref mut w) = *slot {
                    w.next = target_next;
                }
            }
            free_waiter(target_idx);
            return;
        }

        current_idx = next;
    }
}

/// ENOMEM
const ENOMEM: i32 = 12;

/// FUTEX_WAIT_BITSET 实现
pub fn futex_wait_bitset(uaddr: usize, flags: u32, val: u32, _timeout: u64, bitset: u32) -> i64 {
    futex_wait(uaddr, flags, val, bitset)
}

/// FUTEX_WAKE_BITSET 实现
pub fn futex_wake_bitset(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    futex_wake(uaddr, flags, nr_wake, bitset)
}

/// 将 FUTEX 操作码转换为内部标志
pub fn futex_to_flags(op: u32) -> u32 {
    let mut flags = 0u32;

    if (op & FUTEX_PRIVATE_FLAG as u32) == 0 {
        flags |= FLAGS_SHARED;
    }

    if (op & FUTEX_CLOCK_REALTIME as u32) != 0 {
        flags |= FLAGS_CLOCKRT;
    }

    flags
}

/// do_futex - 主分发函数
///
/// 参考 Linux: do_futex()
pub fn do_futex(uaddr: usize, op: i32, val: u32, _timeout: u64, uaddr2: usize, val2: u32, val3: u32) -> i64 {
    let flags = futex_to_flags(op as u32);
    let cmd = op & FUTEX_CMD_MASK;

    match cmd {
        FUTEX_WAIT => {
            futex_wait(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            futex_wait_bitset(uaddr, flags, val, _timeout, val3)
        }
        FUTEX_WAKE_BITSET => {
            futex_wake_bitset(uaddr, flags, val as i32, val3)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // 简化实现：只唤醒，不重排队
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAKE_OP => {
            // 简化实现
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        _ => {
            // PI 相关操作暂不支持
            -ENOSYS as i64
        }
    }
}

/// sys_futex 系统调用入口
///
/// 参考 Linux: SYSCALL_DEFINE6(futex, ...)
pub fn sys_futex_handler(args: &[u64; 6]) -> i64 {
    let uaddr = args[0] as usize;
    let op = args[1] as i32;
    let val = args[2] as u32;
    let timeout = args[3];
    let uaddr2 = args[4] as usize;
    let val3 = args[5] as u32;

    do_futex(uaddr, op, val, timeout, uaddr2, 0, val3)
}
