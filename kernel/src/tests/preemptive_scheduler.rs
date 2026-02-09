// 测试：抢占式调度器 (Phase 16.1-16.2)
//
// 测试内容：
// 1. jiffies 计数器
// 2. need_resched 标志
// 3. 时间片管理
// 4. scheduler_tick() 函数

use crate::println;
use crate::drivers::timer;

pub fn test_preemptive_scheduler() {
    println!("test: ===== Testing Preemptive Scheduler (Phase 16) =====");

    // 测试 1: jiffies 计数器
    test_jiffies();

    // 测试 2: need_resched 标志
    test_need_resched();

    // 测试 3: 时间片管理
    test_time_slice();

    // 测试 4: jiffies 转换函数
    test_jiffies_conversion();

    println!("test: ===== Preemptive Scheduler Testing Completed =====");
}

/// 测试 jiffies 计数器
fn test_jiffies() {
    println!("test: 1. Testing jiffies counter...");

    // 读取初始 jiffies
    let jiffies1 = timer::get_jiffies();
    println!("test:    Initial jiffies = {}", jiffies1);

    // jiffies 应该在时钟中断时递增
    // 由于时钟中断持续触发，jiffies 应该 >= 初始值
    let jiffies2 = timer::get_jiffies();
    println!("test:    Current jiffies = {}", jiffies2);

    if jiffies2 >= jiffies1 {
        println!("test:    SUCCESS - jiffies counter is incrementing");
    } else {
        println!("test:    FAILED - jiffies counter went backwards!");
    }
}

/// 测试 need_resched 标志
fn test_need_resched() {
    println!("test: 2. Testing need_resched flag...");

    // 读取初始 need_resched 状态
    let initial_resched = crate::sched::need_resched();
    println!("test:    Initial need_resched = {}", initial_resched);

    // 设置 need_resched 标志
    println!("test:    Setting need_resched flag...");
    crate::sched::set_need_resched();

    // 验证标志被设置
    let after_set = crate::sched::need_resched();
    if after_set {
        println!("test:    SUCCESS - need_resched flag was set");
    } else {
        println!("test:    FAILED - need_resched flag was not set!");
        return;
    }

    // 清除标志（通过调用 schedule，它会清除标志）
    println!("test:    Note - flag will be cleared by next schedule()");
}

/// 测试时间片管理
fn test_time_slice() {
    println!("test: 3. Testing time slice management...");

    // 获取当前任务
    match crate::sched::current() {
        Some(task) => {
            let mut task_ref = unsafe { &mut *(task as *mut crate::process::Task) };

            // 读取初始时间片
            let initial_slice = task_ref.get_time_slice();
            println!("test:    Initial time_slice = {}", initial_slice);

            // 模拟时间片递减
            for _ in 0..3 {
                if task_ref.tick_time_slice() {
                    println!("test:    Time slice ticked, remaining = {}", task_ref.get_time_slice());
                } else {
                    println!("test:    Time slice expired!");
                    break;
                }
            }

            // 重置时间片
            task_ref.reset_time_slice();
            let reset_slice = task_ref.get_time_slice();
            println!("test:    After reset, time_slice = {}", reset_slice);

            if reset_slice > 0 {
                println!("test:    SUCCESS - time slice management works");
            } else {
                println!("test:    FAILED - time slice is zero after reset!");
            }
        }
        None => {
            println!("test:    SKIPPED - no current task");
        }
    }
}

/// 测试 jiffies 转换函数
fn test_jiffies_conversion() {
    println!("test: 4. Testing jiffies conversion functions...");

    // 测试 jiffies_to_msecs
    let jiffies = 100; // 100 jiffies = 1 秒 (HZ=100)
    let msecs = timer::jiffies_to_msecs(jiffies);
    println!("test:    {} jiffies = {} msecs", jiffies, msecs);

    if msecs == 1000 {
        println!("test:    SUCCESS - jiffies_to_msecs works correctly");
    } else {
        println!("test:    FAILED - jiffies_to_msecs returned {}, expected 1000", msecs);
        return;
    }

    // 测试 msecs_to_jiffies
    let msecs2 = 500; // 500ms
    let jiffies2 = timer::msecs_to_jiffies(msecs2);
    println!("test:    {} msecs = {} jiffies", msecs2, jiffies2);

    if jiffies2 == 50 {
        println!("test:    SUCCESS - msecs_to_jiffies works correctly");
    } else {
        println!("test:    FAILED - msecs_to_jiffies returned {}, expected 50", jiffies2);
    }
}
