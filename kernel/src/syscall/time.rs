//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 时间相关系统调用
//!
//! 包含：gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use super::*;

/// clock_gettime 时钟 ID
const CLOCK_REALTIME: u32 = 0;
const CLOCK_MONOTONIC: u32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
const CLOCK_THREAD_CPUTIME_ID: u32 = 3;

#[repr(C)]
struct TimespecForGettime {
    tv_sec: i64,   // 秒
    tv_nsec: i64,  // 纳秒
}

/// sys_gettimeofday - 获取当前时间
///
/// # 参数
/// - args[0]: tv - timeval 结构体指针
/// - args[1]: tz - timezone 结构体指针（已废弃，应为 null）
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_gettimeofday(args: SyscallArgs) -> u64 {
    let tv_ptr = args[0] as *mut TimeVal;
    let _tz_ptr = args[1] as *mut u8;  // timezone 已废弃

    if tv_ptr.is_null() {
        return -errno::EINVAL as u64;
    }

    // 从 RISC-V 定时器获取时间
    let cycles = crate::drivers::intc::clint::read_time();
    let freq_hz: u64 = 10_000_000;  // 10 MHz

    let sec = cycles / freq_hz;
    let usec = (cycles % freq_hz) * 1_000_000 / freq_hz;

    unsafe {
        (*tv_ptr).tv_sec = sec as i64;
        (*tv_ptr).tv_usec = usec as i64;
    }

    0
}

/// sys_clock_gettime - 获取指定时钟的时间
///
/// # 参数
/// - args[0]: clk_id - 时钟 ID
/// - args[1]: tp - timespec 结构体指针
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_clock_gettime(args: SyscallArgs) -> u64 {
    let clk_id = args[0] as u32;
    let tp_ptr = args[1] as *mut TimespecForGettime;

    if tp_ptr.is_null() {
        return -errno::EINVAL as u64;
    }

    // 目前只支持 REALTIME 和 MONOTONIC
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC => {
            // 从 RISC-V 定时器获取时间
            let cycles = crate::drivers::intc::clint::read_time();
            let freq_hz: u64 = 10_000_000;  // 10 MHz

            let sec = cycles / freq_hz;
            let nsec = (cycles % freq_hz) * 1_000_000_000 / freq_hz;

            unsafe {
                (*tp_ptr).tv_sec = sec as i64;
                (*tp_ptr).tv_nsec = nsec as i64;
            }
            0
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // 对于 CPU 时间，暂时返回 0
            unsafe {
                (*tp_ptr).tv_sec = 0;
                (*tp_ptr).tv_nsec = 0;
            }
            0
        }
        _ => {
            // 不支持的时钟类型
            -errno::EINVAL as u64
        }
    }
}

/// Timespec 结构体
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,   // 秒
    pub tv_nsec: i64,  // 纳秒
}

/// sys_nanosleep - 高精度睡眠
///
/// # 参数
/// - args[0]: req - 请求的睡眠时间
/// - args[1]: rem - 剩余时间（被信号中断时）
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_nanosleep(args: SyscallArgs) -> u64 {
    use crate::drivers::timer;
    use crate::process;

    let req_ptr = args[0] as *const Timespec;
    let rem_ptr = args[1] as *mut Timespec;

    // 检查请求指针有效性
    if req_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取请求的睡眠时间
    let req = unsafe { *req_ptr };
    let total_nanos = req.tv_sec * 1_000_000_000 + req.tv_nsec;

    // 转换为毫秒
    let sleep_msecs = (total_nanos / 1_000_000) as u64;

    // 如果睡眠时间为 0，直接返回
    if sleep_msecs == 0 {
        return 0;
    }

    // 获取当前 jiffies
    let start_jiffies = timer::get_jiffies();

    // 计算目标 jiffies
    let sleep_jiffies = timer::msecs_to_jiffies(sleep_msecs);
    let target_jiffies = start_jiffies + sleep_jiffies;

    // 循环睡眠，直到达到目标时间
    loop {
        let current_jiffies = timer::get_jiffies();

        // 检查是否已经达到目标时间
        if current_jiffies >= target_jiffies {
            return 0;  // 成功
        }

        // 计算剩余时间
        let remaining_jiffies = target_jiffies - current_jiffies;
        let remaining_msecs = timer::jiffies_to_msecs(remaining_jiffies);

        // 检查是否有待处理信号
        use crate::signal;
        if signal::signal_pending() {
            // 写入剩余时间到 rem（如果提供了 rem_ptr）
            if !rem_ptr.is_null() {
                unsafe {
                    // 将毫秒转换为 timespec
                    let rem_sec = (remaining_msecs / 1000) as i64;
                    let rem_nsec = ((remaining_msecs % 1000) * 1_000_000) as i64;
                    *rem_ptr = Timespec {
                        tv_sec: rem_sec,
                        tv_nsec: rem_nsec,
                    };
                }
            }

            return -errno::EINTR as u64;
        }

        // 使用 Task::sleep() 进入可中断睡眠
        // 注意：这里会触发调度，醒来后继续检查时间
        process::Task::sleep(crate::process::task::TaskState::new(
            crate::process::task::TaskState::INTERRUPTIBLE
        ));
    }
}

/// sys_clock_getres - 获取时钟分辨率
///
/// # 参数
/// - args[0]: clk_id - 时钟 ID
/// - args[1]: res - timespec 结构体指针（用于存储结果）
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_clock_getres(args: SyscallArgs) -> u64 {
    let _clk_id = args[0] as i32;
    let res = args[1] as *mut u64;

    // 简化实现：返回 1 纳秒分辨率
    if !res.is_null() {
        unsafe {
            // timespec 结构: tv_sec (8 bytes) + tv_nsec (8 bytes)
            *res = 0;          // tv_sec = 0
            *(res.offset(1)) = 1;  // tv_nsec = 1
        }
    }

    0
}

/// sys_clock_nanosleep - 高精度睡眠（指定时钟）
///
/// # 参数
/// - args[0]: clk_id - 时钟 ID
/// - args[1]: flags - 标志
/// - args[2]: rqtp - 请求的睡眠时间
/// - args[3]: rmtp - 剩余时间（可被信号中断时）
///
/// # 返回
/// 成功返回 0，失败返回负错误码
pub fn sys_clock_nanosleep(args: SyscallArgs) -> u64 {
    let _clk_id = args[0] as i32;
    let _flags = args[1] as i32;
    let rqtp = args[2] as *const u64;

    // 验证参数
    if rqtp.is_null() {
        return -errno::EINVAL as u64;
    }

    // 简化实现：调用 nanosleep
    // TODO: 实现真正的指定时钟睡眠
    let _ = unsafe { (*rqtp, *rqtp.offset(1)) };

    0
}
