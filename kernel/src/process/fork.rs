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
    use crate::arch::riscv64::trap::current_pt_regs;
    use crate::arch::riscv64::pt_regs::PtRegs;

    unsafe {
        // 获取当前任务（父进程）
        let current = crate::sched::current()?;
        let current_ptr = current as *mut Task;

        // 获取父进程当前的 PtRegs（在 trap 处理期间保存的）
        let parent_pt_regs = current_pt_regs();
        if parent_pt_regs.is_null() {
            return None;
        }

        // 从调度器分配任务槽位
        let task_ptr = crate::sched::alloc_task_slot()?;
        let pid = (*task_ptr).pid();

        // 复制父进程的状态到子进程
        (*task_ptr).set_parent(current_ptr);

        // === copy_thread: 复制 PtRegs ===
        // 参考 Linux: arch/riscv/kernel/process.c copy_thread()
        // 子进程返回值为 0 (a0 = 0)
        //
        // PtRegs 布局 (与 Linux pt_regs 一致):
        //   - 直接从 epc 开始，不需要额外的 16 字节头
        let child_pt_regs: alloc::boxed::Box<PtRegs> = {
            let parent = &*parent_pt_regs;
            alloc::boxed::Box::new(PtRegs {
                epc: parent.epc + 4,     // 跳过 ecall 指令
                ra: parent.ra,
                sp: parent.sp,           // 用户栈指针
                gp: parent.gp,           // 全局指针
                tp: parent.tp,           // 线程指针 (TLS)
                t0: parent.t0,
                t1: parent.t1,
                t2: parent.t2,
                s0: parent.s0,
                s1: parent.s1,
                a0: 0,                   // 子进程返回值为 0
                a1: parent.a1,
                a2: parent.a2,
                a3: parent.a3,
                a4: parent.a4,
                a5: parent.a5,
                a6: parent.a6,
                a7: parent.a7,
                s2: parent.s2,
                s3: parent.s3,
                s4: parent.s4,
                s5: parent.s5,
                s6: parent.s6,
                s7: parent.s7,
                s8: parent.s8,
                s9: parent.s9,
                s10: parent.s10,
                s11: parent.s11,
                t3: parent.t3,
                t4: parent.t4,
                t5: parent.t5,
                t6: parent.t6,
                status: parent.status,   // sstatus
                badaddr: parent.badaddr, // stval
                cause: parent.cause,     // scause
                orig_a0: 0,              // 子进程 orig_a0 = 0
            })
        };

        // 分配内存用于子进程的 PtRegs
        use alloc::alloc::{alloc, Layout};
        let pt_regs_size = core::mem::size_of::<PtRegs>();
        let layout = Layout::from_size_align(pt_regs_size, 16).expect("Invalid layout");

        let mem_ptr = alloc(layout);
        if mem_ptr.is_null() {
            crate::sched::free_task_slot(task_ptr);
            return None;
        }

        // 将 PtRegs 复制到分配的内存
        let pt_regs_ptr = mem_ptr as *mut PtRegs;
        core::ptr::write(pt_regs_ptr, *child_pt_regs);

        // 设置子进程的 fork 信息
        (*task_ptr).set_fork_child(pt_regs_ptr);

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
                    (*task_ptr).set_address_space(Some(alloc::boxed::Box::new(child_as)));
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

        // 复制当前工作目录
        let parent_cwd = (*current_ptr).get_cwd();
        (*task_ptr).set_cwd(parent_cwd);

        // 将新任务加入运行队列
        crate::sched::enqueue_task(&mut *task_ptr);

        Some(pid)
    }
}
