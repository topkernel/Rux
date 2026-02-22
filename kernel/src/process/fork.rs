//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 进程创建 (fork/clone) 实现
//!
//! 本模块实现 fork 系统调用的核心逻辑，参考 Linux kernel/fork.c
//!
//! 主要函数:
//! - `do_fork`: 创建子进程的核心实现
//!
//! 流程 (参考 Linux):
//! 1. 分配新的 task_struct
//! 2. 复制父进程的状态 (copy_process)
//! 3. 复制线程信息 (copy_thread)
//! 4. 复制地址空间 (copy_mm)
//! 5. 复制文件描述符表 (copy_files)
//! 6. 将子进程加入调度队列 (wake_up_process)

use crate::process::task::{Task, SchedPolicy, Pid};
use crate::fs::FdTable;
use crate::sched::pid::alloc_pid;

/// 创建子进程
///
/// 参考 Linux: kernel/fork.c -> kernel_clone() -> copy_process()
///
/// # 返回
/// - Some(pid): 子进程的 PID（在父进程中返回）
/// - None: 创建失败
pub fn do_fork() -> Option<Pid> {
    use crate::arch::riscv64::trap::{current_trap_frame, TrapFrame};

    unsafe {
        // 获取当前任务（父进程）
        let current = crate::sched::current()?;
        let current_ptr = current as *mut Task;

        // 获取父进程当前的 TrapFrame（在 trap 处理期间保存的）
        let parent_trap_frame = current_trap_frame();
        if parent_trap_frame.is_null() {
            return None;
        }

        // 从调度器分配任务槽位
        let task_ptr = crate::sched::alloc_task_slot()?;
        let pid = (*task_ptr).pid();

        // 复制父进程的状态到子进程
        (*task_ptr).set_parent(current_ptr);

        // === copy_thread: 复制 TrapFrame ===
        // 参考 Linux: arch/riscv/kernel/process.c copy_thread()
        // 子进程返回值为 0 (a0 = 0)
        //
        // 重要：TrapFrame 之前需要 16 字节的额外空间：
        //   - sp+0: 用户 tp (hart ID)
        //   - sp+8: 原始 sp (用户栈指针)
        //   - sp+16: TrapFrame 开始 (ra)
        let child_trap_frame: alloc::boxed::Box<TrapFrame> = {
            let parent_frame = &*parent_trap_frame;
            alloc::boxed::Box::new(TrapFrame {
                ra: parent_frame.ra,
                t0: parent_frame.t0,
                t1: parent_frame.t1,
                t2: parent_frame.t2,
                a0: 0,  // 子进程返回值为 0
                a1: parent_frame.a1,
                a2: parent_frame.a2,
                a3: parent_frame.a3,
                a4: parent_frame.a4,
                a5: parent_frame.a5,
                a6: parent_frame.a6,
                a7: parent_frame.a7,
                t3: parent_frame.t3,
                t4: parent_frame.t4,
                t5: parent_frame.t5,
                t6: parent_frame.t6,
                s2: parent_frame.s2,
                s3: parent_frame.s3,
                s4: parent_frame.s4,
                s5: parent_frame.s5,
                s6: parent_frame.s6,
                s7: parent_frame.s7,
                s8: parent_frame.s8,
                s9: parent_frame.s9,
                s10: parent_frame.s10,
                s11: parent_frame.s11,
                gp: parent_frame.gp,  // 复制全局指针
                _pad: parent_frame._pad,
                sstatus: parent_frame.sstatus,
                sepc: parent_frame.sepc + 4,  // 跳过 ecall 指令
                stval: parent_frame.stval,
            })
        };

        // 分配额外的 16 字节用于用户 tp 和 sp
        // ret_from_fork 期望：
        //   sp+0 = 用户 tp
        //   sp+8 = 用户 sp
        //   sp+16 = TrapFrame
        use alloc::alloc::{alloc, Layout};
        let trap_frame_size = core::mem::size_of::<TrapFrame>();
        let total_size = trap_frame_size + 16;
        let layout = Layout::from_size_align(total_size, 16).expect("Invalid layout");

        let mem_ptr = alloc(layout);
        if mem_ptr.is_null() {
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // 将 TrapFrame 复制到偏移 16 处
        let trap_frame_ptr = mem_ptr.add(16) as *mut TrapFrame;
        core::ptr::write(trap_frame_ptr, *child_trap_frame);

        // 设置用户 tp 和 sp
        // parent_trap_frame 指向 TrapFrame 的开始 (sp+16)
        // 所以用户 tp 在 parent_trap_frame - 16，用户 sp 在 parent_trap_frame - 8
        let user_tp = {
            let user_tp_ptr = (parent_trap_frame as *const u8).sub(16) as *const u64;
            *user_tp_ptr
        };
        let user_sp = {
            let user_sp_ptr = (parent_trap_frame as *const u8).sub(8) as *const u64;
            *user_sp_ptr
        };

        // 写入用户 tp 和 sp
        {
            let header = mem_ptr as *mut u64;
            *header = user_tp;              // sp+0: 用户 tp (TLS 指针)
            *header.add(1) = user_sp;       // sp+8: 用户 sp
        }

        // 设置子进程的 fork 信息
        (*task_ptr).set_fork_child(trap_frame_ptr);

        // 复制 CPU 上下文 (callee-saved registers)
        let parent_ctx = (*current_ptr).context();
        let child_ctx = (*task_ptr).context_mut();
        *child_ctx = parent_ctx.clone();

        // 设置子进程的入口点为 ret_from_fork
        extern "C" {
            fn ret_from_fork();
        }
        child_ctx.pc = ret_from_fork as u64;
        child_ctx.x0 = 0;

        // 复制信号掩码
        (*task_ptr).sigmask = (*current_ptr).sigmask;

        // === copy_files: 复制文件描述符表 ===
        {
            let child_fdtable: alloc::boxed::Box<FdTable> = alloc::boxed::Box::new(FdTable::new());
            (*task_ptr).set_fdtable(Some(child_fdtable));

            if let Some(fdtable) = (*task_ptr).try_fdtable_mut() {
                crate::init::init_std_fds_for_task(fdtable);
            }
        }

        // === copy_mm: 复制地址空间 (COW) ===
        let parent_addr_space = (*current_ptr).address_space();
        if let Some(parent_as) = parent_addr_space {
            match parent_as.fork() {
                Ok(child_as) => {
                    (*task_ptr).set_address_space(Some(child_as));
                }
                Err(_) => {
                    crate::sched::free_task_slot(task_ptr);
                    return None;
                }
            }
        } else {
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // 复制 brk 值
        let parent_brk = (*current_ptr).get_brk();
        (*task_ptr).set_brk(parent_brk);

        // 将新任务加入运行队列
        crate::sched::enqueue_task(&mut *task_ptr);

        Some(pid)
    }
}
