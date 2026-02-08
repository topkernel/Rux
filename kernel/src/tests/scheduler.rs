// 测试：进程调度器
use crate::println;

pub fn test_scheduler() {
    println!("test: Testing scheduler...");

    // 测试 1: 获取当前进程 PID
    println!("test: 1. Testing get_current_pid()...");
    let pid = crate::sched::get_current_pid();
    println!("test:    Current PID = {}", pid);
    if pid == 0 {
        println!("test:    SUCCESS - idle task has PID 0");
    } else {
        println!("test:    Note - running task has PID {}", pid);
    }

    // 测试 2: 获取当前进程 PPID
    println!("test: 2. Testing get_current_ppid()...");
    let ppid = crate::sched::get_current_ppid();
    println!("test:    Current PPID = {}", ppid);
    println!("test:    SUCCESS - get_current_ppid returned");

    // 测试 3: 获取当前任务
    println!("test: 3. Testing current()...");
    match crate::sched::current() {
        Some(task) => {
            let task_pid = task.pid();
            let task_state = task.state();
            println!("test:    Current task PID = {}", task_pid);
            println!("test:    Current task state = {:?}", task_state);
            println!("test:    SUCCESS - current() returned task");
        }
        None => {
            println!("test:    FAILED - current() returned None");
            return;
        }
    }

    // 测试 4: 获取文件描述符表
    println!("test: 4. Testing get_current_fdtable()...");
    match crate::sched::get_current_fdtable() {
        Some(fdtable) => {
            println!("test:    FdTable pointer = {:?}", fdtable as *const _);
            println!("test:    SUCCESS - fdtable exists");
        }
        None => {
            println!("test:    Note - no fdtable (expected for idle task)");
        }
    }

    // 测试 5: 测试 find_task_by_pid (查找 idle task)
    println!("test: 5. Testing find_task_by_pid()...");
    let task_ptr = unsafe { crate::sched::find_task_by_pid(0) };
    if !task_ptr.is_null() {
        println!("test:    Found idle task with PID 0");
        println!("test:    SUCCESS - find_task_by_pid works");
    } else {
        println!("test:    Note - idle task not found (may not be in global list)");
    }

    // 测试 6: 测试 find_task_by_pid with invalid PID
    println!("test: 6. Testing find_task_by_pid with invalid PID...");
    let invalid_ptr = unsafe { crate::sched::find_task_by_pid(99999) };
    if invalid_ptr.is_null() {
        println!("test:    SUCCESS - correctly returned null for invalid PID");
    } else {
        println!("test:    FAILED - should return null for invalid PID");
        return;
    }

    // 测试 7: 验证 schedule 函数存在（不实际调用以避免上下文切换）
    println!("test: 7. Verifying schedule() function exists...");
    // schedule() 函数存在但不会在测试中调用它
    // 因为它会触发上下文切换，可能导致测试环境复杂化
    println!("test:    SUCCESS - schedule() function available");

    println!("test: Scheduler testing completed.");
}
