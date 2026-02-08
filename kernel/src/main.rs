#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]

extern crate log;
extern crate alloc;

use core::panic::PanicInfo;

mod arch;
mod sbi;
mod mm;
mod console;
mod print;
mod drivers;
mod config;
mod process;
mod sched;
mod fs;
mod signal;
mod collection;
mod sync;
mod errno;

// 嵌入的用户程序
#[cfg(feature = "riscv64")]
mod embedded_user_programs;

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// 包含平台特定的汇编代码
#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

// RISC-V kernel main function
#[cfg(feature = "riscv64")]
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // 初始化控制台（所有 CPU 都需要）
    console::init();

    // 初始化堆分配器（所有 CPU 都需要）
    mm::init_heap();

    // 初始化 trap 处理（所有 CPU 都需要）
    arch::trap::init();

    // 初始化 MMU（所有 CPU 都需要，但只有启动核执行完整初始化）
    #[cfg(feature = "riscv64")]
    arch::mm::init();

    println!("main: MMU init completed");

    // 初始化 SMP（多核支持）
    // 只有启动核返回，次核会进入空闲循环
    #[cfg(feature = "riscv64")]
    let is_boot_hart = arch::smp::init();

    println!("main: SMP init completed, is_boot_hart={}", is_boot_hart);

    // 只有启动核才会执行到这里
    if is_boot_hart {
        println!("Rux OS v{} - RISC-V 64-bit", env!("CARGO_PKG_VERSION"));

        // 初始化用户物理页分配器
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing user physical allocator...");
            arch::mm::init_user_phys_allocator(0x80000000, 0x8000000); // 128MB 内存
            println!("main: User physical allocator initialized");
        }

        // 初始化 PLIC（中断控制器）
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing PLIC...");
            drivers::intc::init();
            println!("main: PLIC initialized");
        }

        // 初始化 IPI（核间中断）
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing IPI...");
            arch::ipi::init();
            println!("main: IPI initialized");
        }

        // 初始化文件系统
        {
            println!("main: Initializing file system...");
            fs::rootfs::init_rootfs().expect("Failed to initialize RootFS");
            println!("main: RootFS initialized");
        }

        // 初始化进程调度器
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing process scheduler...");
            sched::init();
            println!("main: Process scheduler initialized");
        }

        // 使能外部中断
        #[cfg(feature = "riscv64")]
        arch::trap::enable_external_interrupt();

        // 使能 timer interrupt
        println!("main: Enabling timer interrupt...");
        arch::trap::enable_timer_interrupt();

        // 设置第一次定时器中断
        drivers::timer::set_next_trigger();

        println!("[OK] Timer interrupt enabled, system ready.");

        // 测试 file_open() 功能
        #[cfg(feature = "unit-test")]
        test_file_open();

        // 测试进程树管理功能
        #[cfg(feature = "unit-test")]
        test_process_tree();

        println!("test: Entering main loop...");
    }

    // 主循环：等待中断
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"PANIC!\n";
        for &b in MSG {
            putchar(b);
        }
    }
    loop {}
}

// 测试：执行 shell 用户程序
#[cfg(feature = "riscv64")]
fn test_shell_execution() {
    use crate::arch::riscv64::mm::{self, PageTableEntry};
    use crate::fs::elf::ElfLoader;
    use core::slice;

    println!("test: Starting shell user program execution...");

    unsafe {
        // 获取 shell ELF 数据
        let shell_data = crate::embedded_user_programs::SHELL_ELF;

        // 验证 ELF 格式
        if let Err(e) = ElfLoader::validate(shell_data) {
            println!("test: Invalid ELF format: {:?}", e);
            return;
        }

        // 创建用户地址空间
        let user_root_ppn = match mm::create_user_address_space() {
            Some(ppn) => ppn,
            None => {
                println!("test: Failed to create user address space");
                return;
            }
        };

        println!("test: User address space created, root PPN = {:#x}", user_root_ppn);

        // 解析 ELF
        let entry = match ElfLoader::get_entry(shell_data) {
            Ok(addr) => addr,
            Err(e) => {
                println!("test: Failed to get entry point: {:?}", e);
                return;
            }
        };

        let phdr_count = match ElfLoader::get_program_headers(shell_data) {
            Ok(count) => count,
            Err(e) => {
                println!("test: Failed to get program headers: {:?}", e);
                return;
            }
        };

        // 获取 ELF 头用于读取程序头
        let ehdr = match unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(shell_data) } {
            Some(e) => e,
            None => {
                println!("test: Failed to get ELF header");
                return;
            }
        };

        // 第一遍：找到虚拟地址范围
        let mut min_vaddr: u64 = u64::MAX;
        let mut max_vaddr: u64 = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(shell_data, i) } {
                Some(p) => p,
                None => {
                    println!("test: Failed to get phdr {}", i);
                    return;
                }
            };

            if phdr.is_load() {
                let virt_addr = phdr.p_vaddr;
                let mem_size = phdr.p_memsz;

                if virt_addr < min_vaddr {
                    min_vaddr = virt_addr;
                }
                if virt_addr + mem_size > max_vaddr {
                    max_vaddr = virt_addr + mem_size;
                }
            }
        }

        // 页对齐
        let virt_start = min_vaddr & !(mm::PAGE_SIZE - 1);
        let virt_end = (max_vaddr + mm::PAGE_SIZE - 1) & !(mm::PAGE_SIZE - 1);
        let total_size = virt_end - virt_start;

        println!("test: Virtual range: {:#x} - {:#x} ({} bytes)", virt_start, virt_end, total_size);

        // 一次性分配并映射整个用户内存范围
        let flags = PageTableEntry::V | PageTableEntry::U |
                   PageTableEntry::R | PageTableEntry::W |
                   PageTableEntry::X | PageTableEntry::A |
                   PageTableEntry::D;

        let phys_base = match mm::alloc_and_map_user_memory(
            user_root_ppn,
            virt_start,
            total_size,
            flags,
        ) {
            Some(addr) => addr,
            None => {
                println!("test: Failed to allocate user memory");
                return;
            }
        };

        println!("test: User memory allocated at phys={:#x}", phys_base);

        // 第二遍：加载每个段的数据
        let mut loaded = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(shell_data, i) } {
                Some(p) => p,
                None => {
                    println!("test: Failed to get phdr {}", i);
                    return;
                }
            };

            if phdr.is_load() {
                let virt_addr = phdr.p_vaddr;
                let file_size = phdr.p_filesz;
                let mem_size = phdr.p_memsz;
                let offset = phdr.p_offset as usize;

                // 计算物理地址
                let virt_offset = virt_addr - virt_start;
                let phys_addr = (phys_base + virt_offset) as usize;

                // 复制 ELF 数据到物理内存
                if file_size > 0 {
                    let src = &shell_data[offset..offset + file_size as usize];
                    let dst = slice::from_raw_parts_mut(phys_addr as *mut u8, file_size as usize);
                    dst.copy_from_slice(src);
                }

                // 清零 BSS
                if mem_size > file_size {
                    let bss_start = phys_addr + file_size as usize;
                    let bss_size = (mem_size - file_size) as usize;
                    let bss_dst = slice::from_raw_parts_mut(bss_start as *mut u8, bss_size);
                    bss_dst.fill(0);
                }

                loaded += 1;
            }
        }

        println!("test: Loaded {} segments", loaded);

        // 分配用户栈 (64KB)
        const USER_STACK_TOP: u64 = 0x000000003FFF8000;
        const USER_STACK_SIZE: u64 = 0x10000;

        let stack_flags = PageTableEntry::V | PageTableEntry::U |
                         PageTableEntry::R | PageTableEntry::W |
                         PageTableEntry::A | PageTableEntry::D;

        let _user_stack_phys = match mm::alloc_and_map_user_memory(
            user_root_ppn,
            USER_STACK_TOP - USER_STACK_SIZE,
            USER_STACK_SIZE,
            stack_flags,
        ) {
            Some(addr) => addr,
            None => {
                println!("test: Failed to allocate user stack");
                return;
            }
        };

        println!("test: User stack ready, entry={:#x}", entry);

        // 切换到用户模式执行
        mm::switch_to_user(user_root_ppn, entry, USER_STACK_TOP);
    }
}

// 测试：file_open() 功能
#[cfg(feature = "unit-test")]
fn test_file_open() {
    use alloc::vec::Vec;
    use crate::fs::vfs;
    use crate::fs::file::{FileFlags, close_file_fd};
    use crate::fs::rootfs;
    use crate::sched;

    println!("test: Testing file_open() functionality...");

    // 先获取 RootFS 超级块
    let sb_ptr = rootfs::get_rootfs();
    if sb_ptr.is_null() {
        println!("test: RootFS not initialized!");
        return;
    }

    // 初始化当前任务的 fdtable（用于测试）
    println!("test: Initializing fdtable for testing...");
    unsafe {
        if sched::get_current_fdtable().is_none() {
            println!("test:    No fdtable - skipping fd-dependent tests");
            println!("test:    Testing file lookup and creation logic only...");

            let sb = &*sb_ptr;

            // 测试 1: 文件查找
            println!("test: 1. RootFS lookup /test_existing.txt...");
            let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
            match sb.lookup("/test_existing.txt") {
                Some(_) => println!("test:    SUCCESS - file found"),
                None => println!("test:    FAILED - file not found"),
            }

            // 测试 2: 文件不存在
            println!("test: 2. RootFS lookup /nonexistent...");
            match sb.lookup("/nonexistent") {
                Some(_) => println!("test:    UNEXPECTED SUCCESS"),
                None => println!("test:    EXPECTED FAILURE - not found"),
            }

            // 测试 3: O_CREAT 创建文件
            println!("test: 3. RootFS create_file /test_new_file...");
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => println!("test:    SUCCESS - file created"),
                Err(e) => println!("test:    FAILED - error={}", e),
            }

            // 测试 4: 验证文件已创建
            println!("test: 4. RootFS lookup /test_new_file after creation...");
            match sb.lookup("/test_new_file") {
                Some(_) => println!("test:    SUCCESS - file found"),
                None => println!("test:    FAILED - file not found"),
            }

            // 测试 5: 创建已存在的文件（应该失败）
            println!("test: 5. RootFS create_file /test_new_file (exists)...");
            match sb.create_file("/test_new_file", Vec::new()) {
                Ok(_) => println!("test:    UNEXPECTED SUCCESS"),
                Err(e) => println!("test:    EXPECTED FAILURE - error={}", e),
            }

            println!("test: file_open() logic testing completed (no fdtable).");
            return;
        }
    }

    // 如果有 fdtable，执行完整测试
    unsafe {
        let sb = &*sb_ptr;
        // 创建 /test_existing.txt
        let _ = sb.create_file("/test_existing.txt", b"Hello, Rux!\n".to_vec());
    }

    // 测试 1: 打开已存在的文件（应该成功）
    println!("test: 1. Opening existing file /test_existing.txt...");
    match vfs::file_open("/test_existing.txt", FileFlags::O_RDONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    // 测试 2: 打开不存在的文件（应该失败）
    println!("test: 2. Opening non-existent file /nonexistent...");
    match vfs::file_open("/nonexistent", FileFlags::O_RDONLY, 0) {
        Ok(_) => {
            println!("test:    UNEXPECTED SUCCESS");
        }
        Err(e) => {
            println!("test:    EXPECTED FAILURE - error={}", e);
        }
    }

    // 测试 3: O_CREAT - 创建新文件
    println!("test: 3. Creating new file /test_new_file...");
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    // 测试 4: O_EXCL - 独占创建已存在的文件（应该失败）
    println!("test: 4. O_EXCL with existing file /test_new_file...");
    match vfs::file_open("/test_new_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(_) => {
            println!("test:    UNEXPECTED SUCCESS (should fail with EEXIST)");
        }
        Err(e) => {
            println!("test:    EXPECTED FAILURE - error={}", e);
        }
    }

    // 测试 5: O_EXCL - 独占创建新文件（应该成功）
    println!("test: 5. O_EXCL with new file /test_excl_file...");
    match vfs::file_open("/test_excl_file", FileFlags::O_CREAT | FileFlags::O_EXCL | FileFlags::O_WRONLY, 0) {
        Ok(fd) => {
            println!("test:    SUCCESS - fd={}", fd);
            unsafe { let _ = close_file_fd(fd); }
        }
        Err(e) => {
            println!("test:    FAILED - error={}", e);
        }
    }

    println!("test: file_open() testing completed.");
}

// 测试：进程树管理功能
#[cfg(feature = "unit-test")]
fn test_process_tree() {
    use crate::process::Task;
    use crate::process::task::SchedPolicy;
    use alloc::boxed::Box;

    println!("test: Testing process tree management...");

    // 创建父进程（使用堆分配避免栈溢出）
    println!("test: 1. Creating parent task (PID 1)...");
    let mut parent_task_box = Box::new(Task::new(1, SchedPolicy::Normal));
    // 重新初始化 children 和 sibling（因为 Box::new 后地址改变了）
    parent_task_box.children.init();
    parent_task_box.sibling.init();
    let parent_task = Box::leak(parent_task_box) as *mut Task;

    // 创建子进程 1（使用堆分配）
    println!("test: 2. Creating child task 1 (PID 2)...");
    let mut child1_box = Box::new(Task::new(2, SchedPolicy::Normal));
    child1_box.children.init();
    child1_box.sibling.init();
    let child1 = Box::leak(child1_box) as *mut Task;

    // 创建子进程 2（使用堆分配）
    println!("test: 3. Creating child task 2 (PID 3)...");
    let mut child2_box = Box::new(Task::new(3, SchedPolicy::Normal));
    child2_box.children.init();
    child2_box.sibling.init();
    let child2 = Box::leak(child2_box) as *mut Task;

    unsafe {
        // 测试添加子进程
        println!("test: 4. Adding child1 (PID 2) to parent...");
        (*parent_task).add_child(child1);
        println!("test:    Child1 added");

        println!("test: 5. Adding child2 (PID 3) to parent...");
        (*parent_task).add_child(child2);
        println!("test:    Child2 added");

        // 测试 has_children
        println!("test: 6. Checking if parent has children...");
        if (*parent_task).has_children() {
            println!("test:    YES - parent has children");
        } else {
            println!("test:    FAILED - parent should have children");
        }

        // 测试 first_child
        println!("test: 7. Getting first child...");
        match (*parent_task).first_child() {
            Some(_) => {
                println!("test:    SUCCESS - first child found");
            }
            None => {
                println!("test:    FAILED - no first child");
            }
        }

        // 测试 next_sibling
        println!("test: 8. Getting next sibling of first child...");
        if let Some(child1_ptr) = (*parent_task).first_child() {
            match (*child1_ptr).next_sibling() {
                Some(_) => {
                    println!("test:    SUCCESS - next sibling found");
                }
                None => {
                    println!("test:    No next sibling (unexpected)");
                }
            }
        }

        // 测试 count_children
        println!("test: 9. Counting children...");
        let count = (*parent_task).count_children();
        println!("test:    Parent has {} children", count);
        if count == 2 {
            println!("test:    SUCCESS - count is correct");
        } else {
            println!("test:    FAILED - expected 2 children, got {}", count);
        }

        // 测试 find_child_by_pid
        println!("test: 10. Finding child by PID 2...");
        match (*parent_task).find_child_by_pid(2) {
            Some(_) => {
                println!("test:    SUCCESS - found child with PID 2");
            }
            None => {
                println!("test:    FAILED - child not found");
            }
        }

        // 测试 for_each_child
        println!("test: 11. Iterating over all children...");
        let mut iteration_count = 0;
        (*parent_task).for_each_child(|_child| {
            iteration_count += 1;
            println!("test:    Child #{}", iteration_count);
        });
        if iteration_count == 2 {
            println!("test:    SUCCESS - iterated over all children");
        } else {
            println!("test:    FAILED - expected 2 iterations, got {}", iteration_count);
        }

        // 测试 remove_child
        println!("test: 12. Removing first child...");
        if let Some(child1_ptr) = (*parent_task).first_child() {
            (*parent_task).remove_child(child1_ptr);
            println!("test:    Child removed");

            // 验证删除后的计数
            let new_count = (*parent_task).count_children();
            println!("test:    Parent now has {} children", new_count);
            if new_count == 1 {
                println!("test:    SUCCESS - count is correct after removal");
            } else {
                println!("test:    FAILED - expected 1 child, got {}", new_count);
            }
        }

        // 测试 next_sibling after removal
        println!("test: 13. Testing sibling after removal...");
        if let Some(first_child) = (*parent_task).first_child() {
            match (*first_child).next_sibling() {
                Some(_) => {
                    println!("test:    UNEXPECTED - should have no more siblings");
                }
                None => {
                    println!("test:    SUCCESS - no more siblings (correct)");
                }
            }
        }

        // 测试链表完整性
        println!("test: 14. Testing list integrity...");
        let final_count = (*parent_task).count_children();
        println!("test:    Final child count: {}", final_count);
        if final_count == 1 {
            println!("test:    SUCCESS - list integrity maintained");
        } else {
            println!("test:    FAILED - expected 1 child, got {}", final_count);
        }
    }

    println!("test: process tree testing completed.");
}
