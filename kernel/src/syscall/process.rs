//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process-related system calls
//!
//! Includes: clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname, etc.

use super::*;
use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};
use crate::arch::riscv64::uaccess::strncpy_from_user;
use crate::process::exec::do_execve_elf;

/// sys_clone - Create child process/thread
///
/// # Arguments
/// - args[0]: flags - clone flags
/// - args[1]: stack - new stack pointer
/// - args[2]: parent_tid - parent TID pointer
/// - args[3]: tls - TLS pointer
/// - args[4]: child_tid - child TID pointer
///
/// # Returns
/// Returns child process PID in parent, 0 in child, negative error code on failure
pub fn sys_clone(args: SyscallArgs) -> u64 {
    use crate::process::fork::{do_clone, CloneArgs};

    let flags = args[0];
    let stack = args[1];
    let parent_tid = args[2] as *mut i32;
    let child_tid = args[4] as *mut i32;
    let tls = args[3];

    let clone_args = CloneArgs {
        flags,
        stack,
        parent_tid,
        child_tid,
        tls,
    };

    match do_clone(clone_args) {
        Some(pid) => pid as u64,
        None => -errno::ENOMEM as u64,
    }
}

/// Maximum shebang line length (same as Linux BINPRM_BUF_SIZE)
const BINPRM_BUF_SIZE: usize = 256;

/// Maximum recursion depth for shebang interpretation (Linux uses 4)
const MAX_BINPRM_RECURSION: u32 = 4;

/// Parse shebang (#!) line from file data.
/// Returns (interpreter_path, optional_argument) if present.
fn parse_shebang(data: &[u8]) -> Option<(&str, Option<&str>)> {
    if data.len() < 2 || data[0] != b'#' || data[1] != b'!' {
        return None;
    }

    let line = if let Some(pos) = data[2..BINPRM_BUF_SIZE.min(data.len())].iter().position(|&c| c == b'\n') {
        core::str::from_utf8(&data[2..2 + pos]).ok()?
    } else if data.len() >= BINPRM_BUF_SIZE {
        return None; // line too long
    } else {
        core::str::from_utf8(&data[2..]).ok()?
    };

    // Skip leading spaces after #!
    let line = line.trim_start();

    // Split into interpreter and optional single argument (Linux behavior)
    if let Some(space_pos) = line.find(|c: char| c == ' ' || c == '\t') {
        let interp = &line[..space_pos];
        let rest = line[space_pos..].trim_start();
        let arg = if rest.is_empty() { None } else {
            Some(if let Some(end) = rest.find(|c: char| c == ' ' || c == '\t') {
                &rest[..end]
            } else {
                rest
            })
        };
        if interp.is_empty() { return None; }
        Some((interp, arg))
    } else if line.is_empty() {
        None
    } else {
        Some((line, None))
    }
}

/// Read file content from filesystem (ext4 or rootfs)
fn read_exec_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    if crate::fs::ext4::is_mounted() {
        crate::fs::ext4::read_file_from_mounted(path)
    } else {
        crate::fs::read_file_from_rootfs(path)
    }
}

/// Read argv array from user space
fn copy_argv_from_user(argv_ptr: *const *const u8) -> alloc::vec::Vec<alloc::string::String> {
    use alloc::string::String;
    let mut args = alloc::vec::Vec::new();
    if argv_ptr.is_null() {
        return args;
    }
    unsafe {
        core::arch::asm!(
            "li t6, 0x40000",
            "csrs sstatus, t6",
            options(nomem, nostack)
        );
        let mut i = 0usize;
        loop {
            if i > 64 { break; }
            let arg_ptr = core::ptr::read_volatile(argv_ptr.add(i));
            if arg_ptr.is_null() { break; }
            let mut len = 0usize;
            let mut p = arg_ptr;
            while core::ptr::read_volatile(p) != 0 && len < 1024 {
                len += 1;
                p = p.add(1);
            }
            let arg_slice = core::slice::from_raw_parts(arg_ptr, len);
            if let Ok(s) = core::str::from_utf8(arg_slice) {
                args.push(String::from(s));
            }
            i += 1;
        }
        core::arch::asm!(
            "li t6, 0x40000",
            "csrc sstatus, t6",
            options(nomem, nostack)
        );
    }
    args
}

/// Read envp array from user space
fn copy_envp_from_user(envp_ptr: *const *const u8) -> alloc::vec::Vec<alloc::string::String> {
    use alloc::string::String;
    let mut envs = alloc::vec::Vec::new();
    if envp_ptr.is_null() {
        return envs;
    }
    unsafe {
        core::arch::asm!(
            "li t6, 0x40000",
            "csrs sstatus, t6",
            options(nomem, nostack)
        );
        let mut i = 0usize;
        loop {
            if i > 256 { break; }
            let env_str_ptr = core::ptr::read_volatile(envp_ptr.add(i));
            if env_str_ptr.is_null() { break; }
            let mut len = 0usize;
            let mut p = env_str_ptr;
            while core::ptr::read_volatile(p) != 0 && len < 4096 {
                len += 1;
                p = p.add(1);
            }
            let env_slice = core::slice::from_raw_parts(env_str_ptr, len);
            if let Ok(s) = core::str::from_utf8(env_slice) {
                envs.push(String::from(s));
            }
            i += 1;
        }
        core::arch::asm!(
            "li t6, 0x40000",
            "csrc sstatus, t6",
            options(nomem, nostack)
        );
    }
    envs
}

/// Core execve implementation: load and execute an ELF binary.
/// Handles shebang (#!) scripts by recursing with the interpreter.
fn do_execve(pathname: &str, argv: &[alloc::string::String], envp: &[alloc::string::String], recursion_depth: u32) -> u64 {
    use crate::fs::elf::{ElfLoader, Elf64Ehdr};
    use alloc::string::String;
    use alloc::vec::Vec;

    // Build full path
    let full_path = if pathname.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname)
    } else {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = String::with_capacity(cwd_str.len() + pathname.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname)
        }
    };

    // Read file from file system
    let program_data = match read_exec_file(full_path.as_ref()) {
        Some(data) => data,
        None => return -errno::ENOENT as u64,
    };

    // Check for shebang (#!) script — binfmt_script behavior
    if let Some((interp, opt_arg)) = parse_shebang(&program_data) {
        if recursion_depth >= MAX_BINPRM_RECURSION {
            return -errno::ELOOP as u64;
        }
        // Build new argv: [interpreter, opt_arg?, script_path, original_argv[1:]...]
        let mut new_argv = alloc::vec::Vec::with_capacity(argv.len() + 2);
        new_argv.push(String::from(interp));
        if let Some(arg) = opt_arg {
            new_argv.push(String::from(arg));
        }
        new_argv.push(String::from(full_path.as_ref()));
        new_argv.extend_from_slice(&argv[1..]);

        return do_execve(interp, &new_argv, envp, recursion_depth + 1);
    }

    // Validate ELF format
    if ElfLoader::validate(&program_data).is_err() {
        return -errno::ENOEXEC as u64;
    }

    // Get entry point
    let entry = match ElfLoader::get_entry(&program_data) {
        Ok(e) => e,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    // Get program header count
    let phdr_count = match ElfLoader::get_program_headers(&program_data) {
        Ok(n) => n,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    let ehdr = match unsafe { Elf64Ehdr::from_bytes(&program_data) } {
        Some(e) => e,
        None => return -errno::ENOEXEC as u64,
    };

    // Check for PT_INTERP (dynamic executable)
    let interp_data: Option<alloc::vec::Vec<u8>> = if let Some(interp_path) = ElfLoader::get_interpreter(&program_data) {
        let interp_str = match core::str::from_utf8(interp_path) {
            Ok(s) => s,
            Err(_) => return -errno::ENOEXEC as u64,
        };
        read_exec_file(interp_str)
    } else {
        None
    };

    // Build final argv (use provided argv, fallback to full_path if empty)
    let final_argv: Vec<String> = if argv.is_empty() {
        alloc::vec![String::from(full_path.as_ref())]
    } else {
        argv.to_vec()
    };

    let final_envp: Vec<String> = envp.to_vec();

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // Execute ELF loading
    match do_execve_elf(current, &program_data, &final_argv, &final_envp, entry, phdr_count as usize, &ehdr, full_path.as_ref(), interp_data.as_deref()) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_execve - Execute program
///
/// # Arguments
/// - args[0]: pathname - program path
/// - args[1]: argv - argument array
/// - args[2]: envp - environment variable array
///
/// # Returns
/// Does not return on success, negative error code on failure
pub fn sys_execve(args: SyscallArgs) -> u64 {
    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;
    let envp_ptr = args[2] as *const *const u8;

    // Check path pointer
    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Copy argv and envp from user space
    let argv = copy_argv_from_user(argv_ptr);
    let envp = copy_envp_from_user(envp_ptr);

    do_execve(pathname_str, &argv, &envp, 0)
}

/// sys_exit - Exit process
///
/// # Arguments
/// - args[0]: status - exit status code
///
/// # Returns
/// Does not return
pub fn sys_exit(args: SyscallArgs) -> u64 {
    let status = args[0] as i32;
    crate::process::exit::do_exit(status);
    0 // unreachable
}

/// sys_wait4 - Wait for child process
///
/// # Arguments
/// - args[0]: pid - process ID to wait for
/// - args[1]: status - pointer to store exit status
/// - args[2]: options - wait options
/// - args[3]: rusage - resource usage statistics pointer
///
/// # Returns
/// Returns child process PID on success, negative error code on failure
pub fn sys_wait4(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // Validate wstatus pointer
    if !wstatus.is_null() && !crate::arch::riscv64::uaccess::access_ok(wstatus as usize, 4) {
        return -errno::EFAULT as u64;
    }

    // WNOHANG: If no child process has exited, return 0 immediately
    const WNOHANG: i32 = 0x00000001;

    if options & WNOHANG != 0 {
        // WNOHANG mode: non-blocking check
        match crate::process::exit::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -11 => 0,  // EAGAIN -> return 0 means no child process exited
            Err(e) => e as u32 as u64,
        }
    } else {
        // Blocking wait for child process to exit
        match crate::process::exit::do_wait(pid, wstatus, options) {
            Ok(child_pid) => child_pid as u64,
            Err(e) => e as u32 as u64,
        }
    }
}

/// sys_getpid - Get process ID
pub fn sys_getpid(_args: SyscallArgs) -> u64 {
    if let Some(current) = crate::sched::current() {
        unsafe { (*current).pid() as u64 }
    } else {
        0
    }
}

/// sys_gettid - Get thread ID
///
/// In single-threaded processes, tid == pid.
/// RISC-V syscall number: 178
pub fn sys_gettid(_args: SyscallArgs) -> u64 {
    if let Some(current) = crate::sched::current() {
        unsafe { (*current).pid() as u64 }
    } else {
        0
    }
}

/// sys_getppid - Get parent process ID
pub fn sys_getppid(_args: SyscallArgs) -> u64 {
    crate::process::current_ppid() as u64
}

/// sys_kill - Send signal
pub fn sys_kill(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    if sig < 0 || sig > 64 {
        return -errno::EINVAL as u64;
    }

    if pid == 0 {
        // Send to all processes in the caller's process group
        let pgid = match crate::sched::current() {
            Some(t) => unsafe { (*t).pgid() },
            None => return -errno::ESRCH as u64,
        };
        crate::sched::for_each_task(|task| unsafe {
            if (*task).pgid() == pgid && sig > 0 {
                let _ = crate::signal::send_signal((*task).pid(), sig);
            }
        });
        return 0;
    }

    if pid < 0 {
        // Send to all processes in process group |pid|
        let pgid = (-pid) as u32;
        crate::sched::for_each_task(|task| unsafe {
            if (*task).pgid() == pgid && sig > 0 {
                let _ = crate::signal::send_signal((*task).pid(), sig);
            }
        });
        return 0;
    }

    // pid > 0: send to specific process
    unsafe {
        let target = crate::sched::find_task_by_pid(pid as u32);
        if target.is_null() {
            return -errno::ESRCH as u64;
        }

        if sig > 0 {
            let _ = crate::signal::send_signal(pid as u32, sig);
        }
    }

    0
}

/// sys_set_tid_address - Set TID address
pub fn sys_set_tid_address(args: SyscallArgs, tp: u64) -> u64 {
    let tidptr = args[0] as *mut i32;

    // Validate tidptr pointer
    if !tidptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(tidptr as usize, 4) {
        return -errno::EFAULT as u64;
    }

    if let Some(current) = crate::sched::current() {
        unsafe {
            (*current).set_clear_child_tid(tidptr);
            return (*current).pid() as u64;
        }
    }

    0
}

/// sys_set_robust_list - Set robust list
pub fn sys_set_robust_list(_args: SyscallArgs) -> u64 {
    // Simplified implementation
    0
}

/// sys_uname - Get system information
pub fn sys_uname(args: SyscallArgs) -> u64 {
    #[repr(C)]
    struct Utsname {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    let buf = args[0] as *mut Utsname;

    if buf.is_null() {
        return -errno::EFAULT as u64;
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, core::mem::size_of::<Utsname>()) {
        return -errno::EFAULT as u64;
    }

    unsafe {
        let uname = &mut *buf;

        // Fill system information
        let sysname = b"Rux\0";
        let nodename = b"rux\0";
        let release = b"0.1.0\0";
        let version = b"Rux OS v0.1.0\0";
        let machine = b"riscv64\0";
        let domainname = b"\0";

        uname.sysname[..sysname.len()].copy_from_slice(sysname);
        uname.nodename[..nodename.len()].copy_from_slice(nodename);
        uname.release[..release.len()].copy_from_slice(release);
        uname.version[..version.len()].copy_from_slice(version);
        uname.machine[..machine.len()].copy_from_slice(machine);
        uname.domainname[..domainname.len()].copy_from_slice(domainname);
    }

    0
}

/// sys_getuid - Get user ID
pub fn sys_getuid(_args: SyscallArgs) -> u64 {
    if let Some(task) = crate::sched::current() {
        task.cred().uid as u64
    } else {
        0
    }
}

/// sys_getgid - Get group ID
pub fn sys_getgid(_args: SyscallArgs) -> u64 {
    if let Some(task) = crate::sched::current() {
        task.cred().gid as u64
    } else {
        0
    }
}

/// sys_geteuid - Get effective user ID
pub fn sys_geteuid(_args: SyscallArgs) -> u64 {
    if let Some(task) = crate::sched::current() {
        task.cred().euid as u64
    } else {
        0
    }
}

/// sys_getegid - Get effective group ID
pub fn sys_getegid(_args: SyscallArgs) -> u64 {
    if let Some(task) = crate::sched::current() {
        task.cred().egid as u64
    } else {
        0
    }
}

/// sys_setuid - Set user ID
///
/// # Arguments
/// - args[0]: uid - user ID to set
pub fn sys_setuid(args: SyscallArgs) -> u64 {
    let uid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();
            if cred.euid == 0 {
                // Root: set all uid fields
                cred.uid = uid;
                cred.euid = uid;
                cred.suid = uid;
                cred.fsuid = uid;
            } else if cred.uid == uid || cred.suid == uid {
                // Unprivileged: can set euid to real or saved uid
                cred.euid = uid;
                cred.fsuid = uid;
            } else {
                return -errno::EPERM as u64;
            }
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setgid - Set group ID
///
/// # Arguments
/// - args[0]: gid - group ID to set
pub fn sys_setgid(args: SyscallArgs) -> u64 {
    let gid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();
            if cred.euid == 0 {
                // Root: set all gid fields
                cred.gid = gid;
                cred.egid = gid;
                cred.sgid = gid;
                cred.fsgid = gid;
            } else if cred.gid == gid || cred.sgid == gid {
                // Unprivileged: can set egid to real or saved gid
                cred.egid = gid;
                cred.fsgid = gid;
            } else {
                return -errno::EPERM as u64;
            }
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setreuid - Set real and effective user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
pub fn sys_setreuid(args: SyscallArgs) -> u64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();
            let old_ruid = cred.uid;
            let old_euid = cred.euid;
            let old_suid = cred.suid;

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                old_ruid
            } else if cred.euid == 0 || ruid as u32 == old_ruid || ruid as u32 == old_euid || ruid as u32 == old_suid {
                ruid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                old_euid
            } else if cred.euid == 0 || euid as u32 == old_ruid || euid as u32 == old_euid || euid as u32 == old_suid {
                euid as u32
            } else {
                return -errno::EPERM as u64;
            };

            cred.uid = new_ruid;
            cred.euid = new_euid;
            cred.fsuid = new_euid;
            if ruid != -1 {
                cred.suid = new_euid;
            }
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setregid - Set real and effective group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
pub fn sys_setregid(args: SyscallArgs) -> u64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();
            let old_rgid = cred.gid;
            let old_egid = cred.egid;
            let old_sgid = cred.sgid;

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                old_rgid
            } else if cred.euid == 0 || rgid as u32 == old_rgid || rgid as u32 == old_egid || rgid as u32 == old_sgid {
                rgid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                old_egid
            } else if cred.euid == 0 || egid as u32 == old_rgid || egid as u32 == old_egid || egid as u32 == old_sgid {
                egid as u32
            } else {
                return -errno::EPERM as u64;
            };

            cred.gid = new_rgid;
            cred.egid = new_egid;
            cred.fsgid = new_egid;
            if rgid != -1 {
                cred.sgid = new_egid;
            }
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_getgroups - Get supplementary group IDs
///
/// # Arguments
/// - args[0]: size - size of groups array
/// - args[1]: list - pointer to group ID array
///
/// # Returns
/// Number of groups on success, negative error on failure
pub fn sys_getgroups(args: SyscallArgs) -> u64 {
    let size = args[0] as i32;
    let list_ptr = args[1] as *mut u32;

    // Currently no supplementary groups, return 0
    if size == 0 {
        return 0;
    }
    if size < 0 {
        return -errno::EINVAL as u64;
    }

    // No supplementary groups to return
    0
}

/// sys_setgroups - Set supplementary group IDs
///
/// # Arguments
/// - args[0]: size - number of groups
/// - args[1]: list - pointer to group ID array
pub fn sys_setgroups(args: SyscallArgs) -> u64 {
    // Only root can set supplementary groups
    if let Some(task) = crate::sched::current() {
        unsafe {
            if (*task).cred().euid != 0 {
                return -errno::EPERM as u64;
            }
        }
        // TODO: implement supplementary group storage
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setpgid - Set process group ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: pgid - process group ID (0 = pid)
pub fn sys_setpgid(args: SyscallArgs) -> u64 {
    let target_pid = args[0] as i32;
    let pgid = args[1] as i32;

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    let current_pid = unsafe { (*current).pid() };

    // Resolve target pid
    let target_pid = if target_pid == 0 {
        current_pid as i32
    } else {
        target_pid
    };

    // Resolve pgid
    let pgid = if pgid == 0 {
        target_pid
    } else {
        pgid
    };

    // Cannot set pgid for processes in different sessions
    if target_pid as u32 == current_pid {
        // Setting own pgid
        unsafe {
            if (*current).sid() != pgid as u32 {
                // pgid must be in same session (simplified: just check it's valid)
            }
            (*current).set_pgid(pgid as u32);
        }
    } else {
        // Setting child's pgid
        let target = unsafe { crate::sched::find_task_by_pid(target_pid as u32) };
        if target.is_null() {
            return -errno::ESRCH as u64;
        }
        unsafe {
            // Target must be a child of current process
            if (*target).ppid() != current_pid {
                return -errno::ESRCH as u64;
            }
            // Target must be in same session
            if (*target).sid() != (*current).sid() {
                return -errno::EPERM as u64;
            }
            (*target).set_pgid(pgid as u32);
        }
    }

    0
}

/// sys_getpgid - Get process group ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
pub fn sys_getpgid(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            return unsafe { (*task).pgid() as u64 };
        }
        return -errno::ESRCH as u64;
    }

    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -errno::ESRCH as u64;
    }
    unsafe { (*target).pgid() as u64 }
}

/// sys_setsid - Create a new session
///
/// # Returns
/// New session ID on success, negative error on failure
pub fn sys_setsid(_args: SyscallArgs) -> u64 {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    unsafe {
        let pid = (*current).pid();

        // Process must not be a process group leader
        if (*current).pgid() == pid {
            return -errno::EPERM as u64;
        }

        // Create new session and process group
        (*current).set_sid(pid);
        (*current).set_pgid(pid);

        pid as u64
    }
}

/// sys_getsid - Get session ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
pub fn sys_getsid(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            return unsafe { (*task).sid() as u64 };
        }
        return -errno::ESRCH as u64;
    }

    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -errno::ESRCH as u64;
    }
    unsafe { (*target).sid() as u64 }
}

/// sys_prlimit64 - Get/set resource limits
pub fn sys_prlimit64(args: SyscallArgs) -> u64 {
    let _pid = args[0] as i32;
    let resource = args[1] as i32;
    let new_rlim = args[2] as *const u8;
    let old_rlim = args[3] as *mut u8;

    // Validate pointers
    if !new_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(new_rlim as usize, 16) {
        return -errno::EFAULT as u64;
    }
    if !old_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(old_rlim as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // Only support querying
    if !new_rlim.is_null() {
        return -errno::EPERM as u64;
    }

    if old_rlim.is_null() {
        return -errno::EFAULT as u64;
    }

    // RLIMIT_NOFILE = 7
    if resource == 7 {
        // Return default file descriptor limit using copy_to_user
        let rlimit: [u64; 2] = [1024, 1024 * 1024];  // rlim_cur, rlim_max
        let uncopied = unsafe {
            crate::arch::riscv64::uaccess::copy_to_user(
                old_rlim as *mut u8,
                rlimit.as_ptr() as *const u8,
                core::mem::size_of::<[u64; 2]>()
            )
        };
        if uncopied != 0 {
            return -errno::EFAULT as u64;
        }
        return 0;
    }

    -errno::EINVAL as u64
}
