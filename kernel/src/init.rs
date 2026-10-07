//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Init process management module
//!
//!
//! The init process is the first userspace process after kernel boot, responsible for:
//! - Mounting the root filesystem
//! - Starting system services
//! - Running shell

use crate::fs::elf::{ElfLoader, ElfError, Elf64Ehdr};
use crate::fs::char_dev::CharDev;
use crate::fs::FdTable;
use crate::sched;
use crate::process::task::{Task, SchedPolicy};
use crate::println;
use crate::cmdline;
use alloc::vec::Vec;
use alloc::sync::Arc;

// Static storage: init process and user context
// Use MaybeUninit to avoid auto-initialization issues
static mut INIT_TASK_STORAGE: core::mem::MaybeUninit<Task> = core::mem::MaybeUninit::uninit();

/// E8-REPAR: address of the boot init task's static storage — the ONE
/// Task that lives outside the kernel linear map (Task::is_plausible_
/// task_ptr accepts exactly this address in addition to the linear-map
/// window, so the pointer screen needs no unmapped image-range holes).
#[inline]
pub fn init_task_storage_addr() -> usize {
    // SAFETY: only the address of the static is read; the storage may be
    // uninitialized at call time.
    unsafe { core::ptr::addr_of!(INIT_TASK_STORAGE) as usize }
}

/// Initialize init process (PID 1)
///
///
/// # Features
/// 1. Create init process (PID 1)
/// 2. Load init program
/// 3. Set up standard file descriptors
/// 4. Add init process to scheduler
///
/// # Init selection (OH Phase 1 — mirrors Linux kernel_init())
///
/// Candidates are tried in order; the first one that loads and starts
/// becomes PID 1:
/// 1. `rdinit=` (initramfs init override, absolutized)
/// 2. `/init` — only when an initrd was unpacked into the rootfs
///    (Linux's `ramdisk_execute_command`)
/// 3. `init=` (absolutized; OH passes `init=init` → `/init`)
/// 4. `/sbin/init`, `/etc/init`, `/bin/init`, `/bin/sh`
///
/// # Note
/// - Init process is the ancestor of all userspace processes
/// - If init exits, kernel will panic
pub fn init() {
    let rootfs_first = crate::initrd::loaded();
    for init_path in init_candidates() {
        println!("init: trying {}", init_path);
        let program_data = load_init_program(&init_path, rootfs_first);
        if let Some(data) = program_data {
            if create_and_start_init_process(&data, &init_path, rootfs_first).is_some() {
                return;
            }
            println!("init: Failed to create init process for {}", init_path);
        } else {
            println!("init: {} not found on any filesystem", init_path);
        }
    }
    println!("init: no init program could be loaded — halting");
    halt();
}

/// Build the init candidate list (see `init()` for the ordering contract).
fn init_candidates() -> Vec<alloc::string::String> {
    use alloc::string::String;
    let mut candidates: Vec<String> = Vec::new();

    let mut push = |p: String, v: &mut Vec<String>| {
        if !v.contains(&p) {
            v.push(p);
        }
    };

    // 1. rdinit= (initramfs override).
    if let Some(rd) = cmdline::get_rdinit_program() {
        push(rd, &mut candidates);
    }
    // 2. /init — the initramfs init, only when an initrd was unpacked
    //    (Linux keeps ramdisk_execute_command NULL without an initrd).
    if crate::initrd::loaded() {
        push(String::from("/init"), &mut candidates);
    }
    // 3. init= (bare names resolve against /, e.g. OH's init=init).
    if let Some(init) = cmdline::get_param("init") {
        push(cmdline::absolutize_init_path(&init), &mut candidates);
    }
    // 4. Linux's default fallback chain.
    for def in ["/sbin/init", "/etc/init", "/bin/init", "/bin/sh"] {
        push(String::from(def), &mut candidates);
    }
    candidates
}

/// Load init program data
///
/// # Arguments
/// - `path`: init program path
/// - `rootfs_first`: try the ramfs rootfs before the ext4 disk (initrd
///   boot: the unpacked rootfs IS the root; a stray ext4 disk must not
///   shadow its init)
///
/// # Returns
/// - `Some(data)`: Program data
/// - `None`: Load failed
///
/// # Loading order
/// rootfs_first: rootfs → PCI ext4 → MMIO ext4
/// otherwise:    PCI ext4 → MMIO ext4 → rootfs (pre-OH behavior)
fn load_init_program(path: &str, rootfs_first: bool) -> Option<Vec<u8>> {
    let read_ext4 = |path: &str| -> Option<Vec<u8>> {
        // 1. PCI VirtIO block device's ext4 filesystem
        if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
            if let Some(data) = crate::fs::ext4::read_file(disk as *const _, path) {
                return Some(data);
            }
        }
        // 2. MMIO VirtIO block device's ext4 filesystem
        if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
            let disk_ptr = &virtio_dev.disk as *const crate::drivers::blkdev::GenDisk;
            if let Some(data) = crate::fs::ext4::read_file(disk_ptr, path) {
                return Some(data);
            }
        }
        None
    };

    if rootfs_first {
        if let Some(data) = crate::fs::read_file_from_rootfs(path) {
            return Some(data);
        }
        read_ext4(path)
    } else {
        read_ext4(path).or_else(|| crate::fs::read_file_from_rootfs(path))
    }
}

/// Create and start init process
///
/// This function will:
/// 1. Create init process structure
/// 2. Load ELF program into memory
/// 3. Mark init process as user process
/// 4. Add to scheduler run queue
fn create_and_start_init_process(
    program_data: &[u8],
    init_path: &str,
    rootfs_first: bool,
) -> Option<*mut Task> {
    unsafe {
        let task_ptr = INIT_TASK_STORAGE.as_mut_ptr();

        // Create init task, PID is fixed to 1
        // Note: new_task_at already allocates kernel stack internally
        let _ = Task::new_task_at(task_ptr, 1, SchedPolicy::Normal); // boot init: 32MB heap, cannot fail meaningfully

        (*task_ptr).set_parent(core::ptr::null_mut());

        // cgroup v2 (U1b): track PID 1 in the root cgroup so its (and its
        // children's) membership and memory usage are visible from
        // /sys/fs/cgroup from the first moment; systemd later moves itself
        // into /init.scope by writing cgroup.procs.
        crate::sched::cgroup::attach_init_to_root(task_ptr);

        // Create and initialize file descriptor table
        let fdtable = alloc::sync::Arc::new(FdTable::new());
        (*task_ptr).set_fdtable(Some(fdtable));

        // Create and initialize signal handling structure
        let signal_struct = alloc::sync::Arc::new(crate::signal::SignalStruct::new());
        (*task_ptr).signal = Some(signal_struct);

        // Create and initialize filesystem info (cwd, root, umask)
        let fs_struct = alloc::sync::Arc::new(crate::fs::FsStruct::new());
        (*task_ptr).set_fs(Some(fs_struct));

        // Initialize standard file descriptors
        // Note: FdTable has interior mutability, so &FdTable is sufficient
        if let Some(fdtable) = (*task_ptr).try_fdtable() {
            init_std_fds_for_task(fdtable);
        } else {
            return None;
        }

        // Load ELF program into memory and set up user context
        if let Err(e) = load_and_setup_elf(task_ptr, program_data, init_path, rootfs_first) {
            println!("init: ELF load failed err={:?} path={} len={} first8={:02x}{:02x}{:02x}{:02x}",
                e, init_path, program_data.len(),
                program_data.get(0).copied().unwrap_or(0),
                program_data.get(1).copied().unwrap_or(0),
                program_data.get(2).copied().unwrap_or(0),
                program_data.get(3).copied().unwrap_or(0));
            return None;
        }

        // R52: init stays TASK_NEW (from new_task_at) through hash-insert
        // until the enqueue's class insert flips it to RUNNING — same
        // discipline as fork/kthread. (The old explicit RUNNING write
        // here re-opened the wakeable-half-built-task window.)

        // Register init process in PID hash table (required for find_task_by_pid)
        crate::process::pid_hash::pid_hash_insert(task_ptr);

        // Add init process to run queue
        // R52: check the insert. INIT_TASK_STORAGE is a static (not heap)
        // and the pid is fixed — on refusal just fail boot; nothing to free.
        if !sched::sched::enqueue_task(&mut *task_ptr) {
            println!("init: enqueue refused for pid 1 (R52 tripwire)");
            return None;
        }

        Some(task_ptr)
    }
}

/// Load the init ELF image and set up the user context for PID 1.
///
/// This routes the kernel-started init through the SAME image loader as
/// execve (`do_execve_elf`). The old hand-rolled loader here ignored
/// PT_INTERP entirely: a dynamically linked init (e.g. `init=/bin/sh`
/// where sh is dash on an Ubuntu rootfs) was mapped like a static image —
/// no ld.so, no relocations — so _start's first PLT call jumped through
/// the file-initial GOT sentinel (-1; jalr clears bit 0 -> epc -2) and
/// PID 1 died with SIGSEGV on every boot. It also predated the exec-path
/// hardening: AT_PHDR pointed at the stack copy instead of the mapped
/// image (wrong load bias for PIE ld.so), the brk was never reset to the
/// new image, and the stack was a 1 MB island next to the image instead
/// of the Linux-style high stack.
///
/// `do_execve_elf` already handles all of that (interpreter load at
/// INTERP_BASE, AT_BASE/AT_PHDR/AT_ENTRY auxv, per-segment W^X, brk reset,
/// growable high stack, interpreter VMAs). What it cannot do for a task
/// that has never been scheduled is set up the first context-switch
/// entry: after it returns, the caller still has to point thread.ra at
/// ret_from_exception with thread.sp at the task's pt_regs so the first
/// __switch_to lands in user mode.
fn load_and_setup_elf(
    task_ptr: *mut Task,
    program_data: &[u8],
    init_path: &str,
    rootfs_first: bool,
) -> Result<(), ElfError> {
    use alloc::string::String;

    // Validate ELF format and pull the pieces do_execve_elf wants pre-parsed.
    ElfLoader::validate(program_data)?;
    let entry = ElfLoader::get_entry(program_data)?;
    let phdr_count = ElfLoader::get_program_headers(program_data)?;
    // SAFETY: program_data was validated by ElfLoader::validate above, so
    // from_bytes re-checks and returns a header view over the same buffer.
    let ehdr = unsafe { Elf64Ehdr::from_bytes(program_data) }
        .ok_or(ElfError::InvalidHeader)?;

    // A dynamic init MUST have its interpreter — never fall back to
    // running the image statically (that is the epc=-2 PID-1 crash).
    let interp_data: Option<Vec<u8>> = if let Some(interp_path) = ElfLoader::get_interpreter(program_data) {
        let interp_str = match core::str::from_utf8(interp_path) {
            Ok(s) => s,
            Err(_) => return Err(ElfError::InvalidHeader),
        };
        // The interpreter lives on the same filesystem as the init image.
        let mut data = load_init_program(interp_str, rootfs_first);
        let mut attempt = 1;
        while data.is_none() && attempt < 3 {
            data = load_init_program(interp_str, rootfs_first);
            attempt += 1;
        }
        match data {
            Some(d) => Some(d),
            None => {
                println!("init: interpreter read failed: {}", interp_str);
                return Err(ElfError::InvalidFormat);
            }
        }
    } else {
        None
    };

    // Same argv/envp the old inline stack builder provided.
    let argv: Vec<String> = alloc::vec![String::from(init_path)];
    let envp: Vec<String> = alloc::vec![
        String::from("PATH=/bin:/usr/bin:/sbin:/usr/sbin"),
        String::from("HOME=/root"),
        String::from("TERM=linux"),
        String::from("PS1=\x1b[1;32mroot\x1b[0m:\x1b[1;34m${PWD}\x1b[0m# "),
        String::from("ENV=/etc/mrshrc"),
    ];

    // do_execve_elf switches this hart's satp to the new (init) address
    // space; capture the kernel root so boot continues on the kernel page
    // table afterwards, exactly as before this call existed.
    // SAFETY: reads the boot-time root PPN, no invariants to uphold.
    let kernel_root_ppn = unsafe { crate::arch::mm::mmu_init::root_page_table_ppn() };

    // SAFETY: task_ptr points to the freshly built PID 1 task (static
    // INIT_TASK_STORAGE); do_execve_elf only needs it to carry a valid
    // kernel stack + pt_regs, both allocated by Task::new_task_at.
    let result = unsafe {
        crate::process::exec::do_execve_elf(
            task_ptr,
            program_data,
            &argv,
            &envp,
            entry,
            phdr_count,
            &ehdr,
            init_path,
            interp_data.as_deref(),
            false,
        )
    };
    match result {
        Ok(()) => {}
        Err(e) => {
            println!("init: do_execve_elf failed: errno {} for {}", e, init_path);
            return Err(ElfError::InvalidFormat);
        }
    }

    // Restore the kernel page table on this hart (do_execve_elf left the
    // init mm active). ASID_KERNEL = 0.
    // SAFETY: kernel_root_ppn is the boot page-table root; the switch
    // only activates it on this CPU.
    unsafe {
        crate::mm::switch_address_space(kernel_root_ppn, 0);
    }

    // First entry into user mode happens through ret_from_exception with
    // the frame do_execve_elf just wrote at the task's pt_regs.
    // SAFETY: task_ptr is the init task; pt_regs() is the outermost frame
    // on its kernel stack (allocated by Task::new_task_at).
    unsafe {
        let child_regs = (*task_ptr).pt_regs();
        if child_regs.is_null() {
            return Err(ElfError::OutOfMemory);
        }
        extern "C" {
            /// riscv64 user-return trampoline; the x86_64 trap contract
            /// exposes the same behavior as ret_from_fork.
            #[cfg(feature = "riscv64")]
            fn ret_from_exception();
            #[cfg(feature = "x86_64")]
            fn ret_from_fork();
        }
        // Kernel is linked at KERNEL_LINK_ADDR, so function pointers are
        // already virtual addresses.
        #[cfg(feature = "riscv64")]
        let entry = ret_from_exception as u64;
        #[cfg(feature = "x86_64")]
        let entry = ret_from_fork as u64;
        let thread = (*task_ptr).thread_mut();
        crate::process::thread_set_entry(thread, entry, child_regs as u64);
    }

    Ok(())
}

/// Initialize standard file descriptors for task (stdin/stdout/stderr)
///
/// This function is public and can be reused by fork and other operations
pub fn init_std_fds_for_task(fdtable: &crate::fs::FdTable) {
    use crate::fs::char_dev::{CharDev, CharDevType, UART_OPS};
    use crate::fs::{File, FileFlags};
    use alloc::sync::Arc;

    // Create UART character device (use static to avoid dangling pointer)
    static UART_DEV: CharDev = CharDev::new(CharDevType::UartConsole, 0);

    // Create stdin (fd=0)
    let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    stdin.set_ops(&UART_OPS);
    stdin.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Create stdout (fd=1)
    let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stdout.set_ops(&UART_OPS);
    stdout.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Create stderr (fd=2)
    let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stderr.set_ops(&UART_OPS);
    stderr.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Install standard file descriptors
    let _ = fdtable.install_fd(0, stdin);
    let _ = fdtable.install_fd(1, stdout);
    let _ = fdtable.install_fd(2, stderr);
}

/// Halt the system
fn halt() -> ! {
    loop {
        crate::arch::cpu::wfi();
    }
}
