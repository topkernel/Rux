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

/// Maximum shebang line length
const BINPRM_BUF_SIZE: usize = 256;

/// Maximum recursion depth for shebang interpretation
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

    // Split into interpreter and optional single argument
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
        Ok(()) => {
            crate::pr_info!("exec: pid={} path={}", crate::process::current_pid(), full_path.as_ref());
            0
        }
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

/// sys_waitid - wait for child process state change (Linux ABI)
///
/// Arguments: (idtype, id, infop, options, rusage)
/// - idtype: P_ALL(0), P_PID(1), P_PGID(2)
/// - id: PID or PGID
/// - infop: user pointer to siginfo_t
/// - options: WNOHANG | WEXITED | WSTOPPED | WCONTINUED | WNOWAIT
/// - rusage: ignored
///
/// Returns: 0 on success, negative errno on error
pub fn sys_waitid(args: SyscallArgs) -> u64 {
    let idtype = args[0] as i32;
    let id = args[1] as i32;
    let infop = args[2] as *mut u8;
    let options = args[3] as i32;
    let _rusage = args[4] as *mut u8;

    // Validate idtype
    if idtype < 0 || idtype > 2 {
        return -errno::EINVAL as u64;
    }

    // Validate infop pointer
    if infop.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(infop as usize, 128) {
        return -errno::EFAULT as u64;
    }

    match crate::process::exit::do_waitid(idtype, id, infop, options) {
        Ok(()) => 0,
        Err(e) => e as u32 as u64,
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

/// sys_setresuid - Set real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
/// - args[2]: suid - saved user ID (-1 to leave unchanged)
pub fn sys_setresuid(args: SyscallArgs) -> u64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    let suid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                cred.uid
            } else if cred.euid == 0
                || ruid as u32 == cred.uid
                || ruid as u32 == cred.euid
                || ruid as u32 == cred.suid
            {
                ruid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                cred.euid
            } else if cred.euid == 0
                || euid as u32 == cred.uid
                || euid as u32 == cred.euid
                || euid as u32 == cred.suid
            {
                euid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new suid
            let new_suid = if suid == -1 {
                cred.suid
            } else if cred.euid == 0
                || suid as u32 == cred.uid
                || suid as u32 == cred.euid
                || suid as u32 == cred.suid
            {
                suid as u32
            } else {
                return -errno::EPERM as u64;
            };

            cred.uid = new_ruid;
            cred.euid = new_euid;
            cred.suid = new_suid;
            cred.fsuid = new_euid;
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_getresuid - Get real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - pointer to store real user ID
/// - args[1]: euid - pointer to store effective user ID
/// - args[2]: suid - pointer to store saved user ID
pub fn sys_getresuid(args: SyscallArgs) -> u64 {
    let ruid_ptr = args[0] as *mut u32;
    let euid_ptr = args[1] as *mut u32;
    let suid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        let cred = unsafe { (*task).cred() };
        unsafe {
            if !ruid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(ruid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(ruid_ptr, cred.uid);
            }
            if !euid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(euid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(euid_ptr, cred.euid);
            }
            if !suid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(suid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(suid_ptr, cred.suid);
            }
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setresgid - Set real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
/// - args[2]: sgid - saved group ID (-1 to leave unchanged)
pub fn sys_setresgid(args: SyscallArgs) -> u64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    let sgid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let cred = (*task).cred_mut();

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                cred.gid
            } else if cred.euid == 0
                || rgid as u32 == cred.gid
                || rgid as u32 == cred.egid
                || rgid as u32 == cred.sgid
            {
                rgid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                cred.egid
            } else if cred.euid == 0
                || egid as u32 == cred.gid
                || egid as u32 == cred.egid
                || egid as u32 == cred.sgid
            {
                egid as u32
            } else {
                return -errno::EPERM as u64;
            };

            // Determine new sgid
            let new_sgid = if sgid == -1 {
                cred.sgid
            } else if cred.euid == 0
                || sgid as u32 == cred.gid
                || sgid as u32 == cred.egid
                || sgid as u32 == cred.sgid
            {
                sgid as u32
            } else {
                return -errno::EPERM as u64;
            };

            cred.gid = new_rgid;
            cred.egid = new_egid;
            cred.sgid = new_sgid;
            cred.fsgid = new_egid;
        }
        0
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_getresgid - Get real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - pointer to store real group ID
/// - args[1]: egid - pointer to store effective group ID
/// - args[2]: sgid - pointer to store saved group ID
pub fn sys_getresgid(args: SyscallArgs) -> u64 {
    let rgid_ptr = args[0] as *mut u32;
    let egid_ptr = args[1] as *mut u32;
    let sgid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        let cred = unsafe { (*task).cred() };
        unsafe {
            if !rgid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(rgid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(rgid_ptr, cred.gid);
            }
            if !egid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(egid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(egid_ptr, cred.egid);
            }
            if !sgid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(sgid_ptr as usize, 4) {
                    return -errno::EFAULT as u64;
                }
                core::ptr::write_volatile(sgid_ptr, cred.sgid);
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

/// sys_prctl - manipulate process attributes
///
/// Arguments: (option, arg2, arg3, arg4, arg5)
pub fn sys_prctl(args: SyscallArgs) -> u64 {
    use crate::arch::riscv64::uaccess::{copy_to_user, strncpy_from_user};

    let option = args[0] as i32;
    let arg2 = args[1];
    let arg3 = args[2];
    let arg4 = args[3];
    let _arg5 = args[4];

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };

    match option {
        1 => {
            // PR_SET_PDEATHSIG
            let sig = arg2 as i32;
            if sig < 0 || sig > 64 {
                return -errno::EINVAL as u64;
            }
            unsafe { (*current).pdeath_signal = sig as u32; }
            0
        }
        2 => {
            // PR_GET_PDEATHSIG
            let ptr = arg2 as *mut u32;
            if ptr.is_null() {
                return -errno::EFAULT as u64;
            }
            if !crate::arch::riscv64::uaccess::access_ok(ptr as usize, 4) {
                return -errno::EFAULT as u64;
            }
            unsafe {
                core::ptr::write_volatile(ptr, (*current).pdeath_signal);
            }
            0
        }
        3 => {
            // PR_GET_DUMPABLE
            unsafe { (*current).dumpable as u64 }
        }
        4 => {
            // PR_SET_DUMPABLE
            let val = arg2 as u32;
            if val > 1 {
                return -errno::EINVAL as u64;
            }
            unsafe { (*current).dumpable = val; }
            0
        }
        15 => {
            // PR_SET_NAME
            let ptr = arg2 as *const u8;
            if ptr.is_null() {
                return -errno::EFAULT as u64;
            }
            let mut buf = [0u8; 16];
            match strncpy_from_user(ptr, 16, &mut buf) {
                Ok(_) => unsafe { (*current).set_comm(&buf); },
                Err(_) => return -errno::EFAULT as u64,
            }
            0
        }
        16 => {
            // PR_GET_NAME
            let ptr = arg2 as *mut u8;
            if ptr.is_null() {
                return -errno::EFAULT as u64;
            }
            if !crate::arch::riscv64::uaccess::access_ok(ptr as usize, 16) {
                return -errno::EFAULT as u64;
            }
            unsafe {
                let comm = (*current).comm();
                let _ = copy_to_user(ptr, comm.as_ptr(), 16);
            }
            0
        }
        36 => {
            // PR_SET_CHILD_SUBREAPER
            let val = arg2 as u32;
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.store(val != 0, core::sync::atomic::Ordering::Relaxed);
                }
            }
            0
        }
        37 => {
            // PR_GET_CHILD_SUBREAPER
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.load(core::sync::atomic::Ordering::Relaxed) as u64
                } else {
                    0
                }
            }
        }
        _ => -errno::EINVAL as u64,
    }
}

/// sys_tgkill - send signal to a thread group
///
/// # Arguments
/// - args[0]: tgid - thread group ID
/// - args[1]: tid - thread ID
/// - args[2]: sig - signal number
pub fn sys_tgkill(args: SyscallArgs) -> u64 {
    let _tgid = args[0] as i32;
    let tid = args[1] as u32;
    let sig = args[2] as i32;
    crate::signal::send_signal(tid, sig)
        .map(|_| 0)
        .unwrap_or(-errno::EINVAL as u64)
}

/// sys_rt_sigqueueinfo - send signal with data
///
/// # Arguments
/// - args[0]: tgid - thread group ID
/// - args[1]: tid - thread ID
/// - args[2]: sig - signal number
/// - args[3]: uinfo - siginfo_t pointer (user)
pub fn sys_rt_sigqueueinfo(_args: SyscallArgs) -> u64 {
    // TODO: implement siginfo_t handling
    -errno::ENOSYS as u64
}

/// sys_rt_sigtimedwait - synchronously wait for signals
///
/// # Arguments
/// - args[0]: uinfo - siginfo_t pointer (user)
/// - args[1]: timeout - timespec pointer (user)
/// - args[2]: sigsetsize - size of signal mask
pub fn sys_rt_sigtimedwait(_args: SyscallArgs) -> u64 {
    // TODO: implement sigtimedwait
    -errno::ENOSYS as u64
}

/// sys_getcpu - get CPU number and node
///
/// # Arguments
/// - args[0]: cpuset_ptr - CPU set pointer
/// - args[1]: node_ptr - NUMA node pointer
/// - args[2]: cache_ptr - cache ID pointer
pub fn sys_getcpu(args: SyscallArgs) -> u64 {
    use crate::arch::riscv64::smp::cpu_id;

    let cpuset_ptr = args[0] as *mut u32;
    let node_ptr = args[1] as *mut u32;
    let _cache_ptr = args[2] as *mut u32;

    if !cpuset_ptr.is_null() {
        unsafe {
            if !crate::arch::riscv64::uaccess::access_ok(cpuset_ptr as usize, 4) {
                return -errno::EFAULT as u64;
            }
            core::ptr::write_volatile(cpuset_ptr, 1u32 << cpu_id());
            core::ptr::write_volatile(cpuset_ptr.add(1), 0u32);
            core::ptr::write_volatile(cpuset_ptr.add(2), 0u32);
            core::ptr::write_volatile(cpuset_ptr.add(3), 0u32);
        }
    }
    if !node_ptr.is_null() {
        unsafe {
            if !crate::arch::riscv64::uaccess::access_ok(node_ptr as usize, 4) {
                return -errno::EFAULT as u64;
            }
            core::ptr::write_volatile(node_ptr, 0u32);
            core::ptr::write_volatile(node_ptr.add(1), 0u32);
            core::ptr::write_volatile(node_ptr.add(2), 0u32);
            core::ptr::write_volatile(node_ptr.add(3), 0u32);
        }
    }
    0
}

/// sys_execveat - execute program relative to directory fd
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - program path pointer
/// - args[2]: argv - argument vector pointer
/// - args[3]: envp - environment pointer
/// - args[4]: flags - AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW, etc.
pub fn sys_execveat(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let argv_ptr = args[2] as *const *const u8;
    let envp_ptr = args[3] as *const *const u8;
    let _flags = args[4] as i32;

    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    let path = match crate::syscall::file::resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let argv = copy_argv_from_user(argv_ptr);
    let envp = copy_envp_from_user(envp_ptr);

    do_execve(&path, &argv, &envp, 0)
}

/// sys_setfsuid - Set filesystem user ID
///
/// # Arguments
/// - args[0]: fsuid - filesystem user ID
pub fn sys_setfsuid(args: SyscallArgs) -> u64 {
    let fsuid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let old_fsuid = (*task).cred().fsuid;
            let cred = (*task).cred_mut();
            if cred.euid == 0 {
                cred.fsuid = fsuid;
            } else if fsuid == cred.uid || fsuid == cred.euid || fsuid == cred.suid {
                cred.fsuid = fsuid;
            }
            old_fsuid as u64
        }
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_setfsgid - Set filesystem group ID
///
/// # Arguments
/// - args[0]: fsgid - filesystem group ID
pub fn sys_setfsgid(args: SyscallArgs) -> u64 {
    let fsgid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        unsafe {
            let old_fsgid = (*task).cred().fsgid;
            let cred = (*task).cred_mut();
            if cred.euid == 0 {
                cred.fsgid = fsgid;
            } else if fsgid == cred.gid || fsgid == cred.egid || fsgid == cred.sgid {
                cred.fsgid = fsgid;
            }
            old_fsgid as u64
        }
    } else {
        -errno::ESRCH as u64
    }
}

/// sys_times - Get process times
///
/// # Arguments
/// - args[0]: buf - pointer to struct tms
///
/// # Returns
/// Clock ticks since system boot on success
pub fn sys_times(args: SyscallArgs) -> u64 {
    let buf_ptr = args[0] as *mut u64;
    if !buf_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, 32) {
            return -errno::EFAULT as u64;
        }
        // struct tms: tms_utime, tms_stime, tms_cutime, tms_cstime (all clock_t = i64)
        unsafe { core::ptr::write_bytes(buf_ptr, 0, 32); }
    }
    // Return clock ticks since boot (simplified: use jiffies)
    crate::drivers::timer::get_jiffies() as u64
}

/// sys_sysinfo - Get system information
///
/// # Arguments
/// - args[0]: info - pointer to struct sysinfo
pub fn sys_sysinfo(args: SyscallArgs) -> u64 {
    let info_ptr = args[0] as *mut u8;
    if info_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(info_ptr as usize, 112) {
        return -errno::EFAULT as u64;
    }
    // struct sysinfo: 112 bytes, fill with available info
    unsafe {
        // uptime (seconds) - from jiffies
        let uptime = crate::drivers::timer::get_jiffies() as u64 / crate::drivers::timer::HZ as u64;
        core::ptr::write_volatile(info_ptr as *mut u64, uptime);
        // loads[1],2,3] - zero
        core::ptr::write_volatile(info_ptr.add(8) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(16) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(24) as *mut u64, 0);
        // totalram (bytes)
        core::ptr::write_volatile(info_ptr.add(32) as *mut u64, crate::config::PHYS_MEMORY_SIZE as u64);
        // freeram
        core::ptr::write_volatile(info_ptr.add(40) as *mut u64, crate::config::PHYS_MEMORY_SIZE as u64 / 2);
        // sharedram, bufferram
        core::ptr::write_volatile(info_ptr.add(48) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(56) as *mut u64, 0);
        // totalswap, freeswap
        core::ptr::write_volatile(info_ptr.add(64) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(72) as *mut u64, 0);
        // procs (current process count)
        use core::sync::atomic::{AtomicU16, Ordering};
        static PROC_COUNT: AtomicU16 = AtomicU16::new(0);
        PROC_COUNT.store(0, Ordering::Relaxed);
        crate::sched::for_each_task(|_| { PROC_COUNT.fetch_add(1, Ordering::Relaxed); });
        core::ptr::write_volatile(info_ptr.add(80) as *mut u16, PROC_COUNT.load(Ordering::Relaxed));
        // totalhigh, freehigh, mem_unit
        core::ptr::write_volatile(info_ptr.add(88) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(96) as *mut u64, 0);
        core::ptr::write_volatile(info_ptr.add(104) as *mut u32, 1); // mem_unit = 1 (bytes)
    }
    0
}

/// sys_membarrier - Issue memory barriers
///
/// # Arguments
/// - args[0]: cmd - MEMBARRIER_CMD_QUERY, MEMBARRIER_CMD_GLOBAL, etc.
/// - args[1]: flags - 0 or MEMBARRIER_FLAG_SYNC_CORE
pub fn sys_membarrier(args: SyscallArgs) -> u64 {
    let cmd = args[0] as i32;
    let _flags = args[1] as u32;

    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = (1 << 0);
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = (1 << 1);

    match cmd {
        MEMBARRIER_CMD_QUERY => {
            // Report which commands are supported
            MEMBARRIER_CMD_GLOBAL as u64 | MEMBARRIER_CMD_GLOBAL_EXPEDITED as u64
        }
        MEMBARRIER_CMD_GLOBAL | MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            // Full memory barrier
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        }
        _ => -errno::EINVAL as u64,
    }
}

/// sys_userfaultfd - Create userfaultfd file descriptor
pub fn sys_userfaultfd(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_kcmp - Compare two processes
pub fn sys_kcmp(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_finit_module - Load kernel module from file descriptor
pub fn sys_finit_module(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_init_module - Load kernel module
pub fn sys_init_module(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_delete_module - Unload kernel module
pub fn sys_delete_module(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_kexec_load - Load new kernel for reboot
pub fn sys_kexec_load(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_process_vm_readv - Read from another process memory
pub fn sys_process_vm_readv(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_process_vm_writev - Write to another process memory
pub fn sys_process_vm_writev(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_perf_event_open - Open performance event
pub fn sys_perf_event_open(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_seccomp - Operate on seccomp state
pub fn sys_seccomp(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_bpf - BPF system call
pub fn sys_bpf(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_capget - Get capabilities
pub fn sys_capget(args: SyscallArgs) -> u64 {
    let hdr_ptr = args[0] as *const u32;
    let data_ptr = args[1] as *mut u32;

    if !hdr_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(hdr_ptr as usize, 8) {
        return -errno::EFAULT as u64;
    }
    if !data_ptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(data_ptr as usize, 24) {
        return -errno::EFAULT as u64;
    }
    if hdr_ptr.is_null() || data_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // All capabilities = 0 (no capabilities)
    unsafe {
        core::ptr::write_bytes(data_ptr, 0, 24);
    }
    0
}

/// sys_capset - Set capabilities
pub fn sys_capset(_args: SyscallArgs) -> u64 {
    // Root only, simplified: always deny
    -errno::EPERM as u64
}

/// sys_personality - Set process execution domain
pub fn sys_personality(args: SyscallArgs) -> u64 {
    let _persona = args[0] as u64;
    // Return current personality (0 = PER_LINUX)
    0
}

/// sys_msgget - Get message queue identifier
pub fn sys_msgget(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_msgctl - Message queue control
pub fn sys_msgctl(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_msgsnd - Send message
pub fn sys_msgsnd(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_msgrcv - Receive message
pub fn sys_msgrcv(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_semget - Get semaphore set identifier
pub fn sys_semget(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_semctl - Semaphore set control
pub fn sys_semctl(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_semop - Semaphore operations
pub fn sys_semop(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_semtimedop - Semaphore operations (timed)
pub fn sys_semtimedop(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_open - Open message queue
pub fn sys_mq_open(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_unlink - Unlink message queue
pub fn sys_mq_unlink(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_timedsend - Send to message queue
pub fn sys_mq_timedsend(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_timedreceive - Receive from message queue
pub fn sys_mq_timedreceive(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_notify - Register notification for message queue
pub fn sys_mq_notify(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_mq_getsetattr - Get/set message queue attributes
pub fn sys_mq_getsetattr(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_pivot_root - Change root filesystem (NR 41)
pub fn sys_pivot_root(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_setns - reassociate thread with a namespace
///
/// # Arguments
/// - args[0]: fd - namespace file descriptor
/// - args[1]: nstype - namespace type
pub fn sys_setns(_args: SyscallArgs) -> u64 {
    // TODO: implement namespace support
    -errno::ENOSYS as u64
}

/// sys_getrlimit - Get resource limits (deprecated, use prlimit64)
///
/// # Arguments
/// - args[0]: resource - resource type (RLIMIT_*)
/// - args[1]: rlim - pointer to struct rlimit
pub fn sys_getrlimit(args: SyscallArgs) -> u64 {
    let resource = args[0] as u32;
    let rlim_ptr = args[1] as *mut u64;

    if rlim_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(rlim_ptr as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // struct rlimit { rlim_cur: u64, rlim_max: u64 }
    const RLIMIT_NOFILE: u32 = 7;
    let (cur, max) = match resource {
        RLIMIT_NOFILE => (1024u64, 1024 * 1024),
        _ => return -errno::EINVAL as u64,
    };

    unsafe {
        core::ptr::write_volatile(rlim_ptr, cur);
        core::ptr::write_volatile(rlim_ptr.add(1), max);
    }
    0
}

/// sys_setrlimit - Set resource limits (deprecated, use prlimit64)
///
/// # Arguments
/// - args[0]: resource - resource type (RLIMIT_*)
/// - args[1]: rlim - pointer to struct rlimit
pub fn sys_setrlimit(args: SyscallArgs) -> u64 {
    let _resource = args[0] as u32;
    let _rlim_ptr = args[1] as *const u64;
    // TODO: implement setrlimit
    -errno::ENOSYS as u64
}

/// sys_getrusage - Get resource usage
///
/// # Arguments
/// - args[0]: who - RUSAGE_SELF (0), RUSAGE_CHILDREN (-1)
/// - args[1]: rusage - pointer to struct rusage
pub fn sys_getrusage(args: SyscallArgs) -> u64 {
    let _who = args[0] as i32;
    let rusage_ptr = args[1] as *mut u8;

    if rusage_ptr.is_null() {
        return -errno::EFAULT as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(rusage_ptr as usize, 136) {
        return -errno::EFAULT as u64;
    }

    // Fill rusage with zeros (no resource tracking yet)
    unsafe {
        core::ptr::write_bytes(rusage_ptr, 0, 136);
    }
    0
}

/// sys_sethostname - Set hostname
///
/// # Arguments
/// - args[0]: name - pointer to hostname string
/// - args[1]: len - hostname length
pub fn sys_sethostname(args: SyscallArgs) -> u64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    // Only root can set hostname
    if let Some(task) = crate::sched::current() {
        if task.cred().euid != 0 {
            return -errno::EPERM as u64;
        }
    } else {
        return -errno::EPERM as u64;
    }

    if name_ptr.is_null() || len == 0 || len > 65 {
        return -errno::EINVAL as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(name_ptr as usize, len) {
        return -errno::EFAULT as u64;
    }

    // TODO: implement hostname storage
    0
}

/// sys_setdomainname - Set NIS domain name
///
/// # Arguments
/// - args[0]: name - pointer to domain name string
/// - args[1]: len - domain name length
pub fn sys_setdomainname(args: SyscallArgs) -> u64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    if let Some(task) = crate::sched::current() {
        if task.cred().euid != 0 {
            return -errno::EPERM as u64;
        }
    } else {
        return -errno::EPERM as u64;
    }

    if name_ptr.is_null() || len == 0 || len > 65 {
        return -errno::EINVAL as u64;
    }
    if !crate::arch::riscv64::uaccess::access_ok(name_ptr as usize, len) {
        return -errno::EFAULT as u64;
    }

    // TODO: implement domain name storage
    0
}

/// sys_reboot - Reboot or halt the system
///
/// # Arguments
/// - args[0]: magic1 - magic number (LINUX_REBOOT_MAGIC1 = 0xfee1dead)
/// - args[1]: magic2 - magic number (LINUX_REBOOT_MAGIC2 or MAGIC2C)
/// - args[2]: cmd - reboot command
pub fn sys_reboot(args: SyscallArgs) -> u64 {
    let magic1 = args[0] as u32;
    let magic2 = args[1] as u32;
    let cmd = args[2] as u32;

    const LINUX_REBOOT_MAGIC1: u32 = 0xfee1dead;
    const LINUX_REBOOT_MAGIC2: u32 = 672274793;
    const LINUX_REBOOT_MAGIC2C: u32 = 85072278;

    const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;
    const LINUX_REBOOT_CMD_HALT: u32 = 0xCDEF0123;
    const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;

    // Only root can reboot
    if let Some(task) = crate::sched::current() {
        if task.cred().euid != 0 {
            return -errno::EPERM as u64;
        }
    } else {
        return -errno::EPERM as u64;
    }

    if magic1 != LINUX_REBOOT_MAGIC1 || (magic2 != LINUX_REBOOT_MAGIC2 && magic2 != LINUX_REBOOT_MAGIC2C) {
        return -errno::EINVAL as u64;
    }

    match cmd {
        LINUX_REBOOT_CMD_RESTART => {
            crate::println!("reboot: restarting system");
            // SBI legacy shutdown ecall (0x8)
            unsafe {
                core::arch::asm!(
                    "ecall",
                    in("a7") 0x8u64,
                    out("a0") _,
                    out("a1") _,
                    options(nomem)
                );
            }
            // If SBI returns, halt in a loop
            loop {}
        }
        LINUX_REBOOT_CMD_HALT | LINUX_REBOOT_CMD_POWER_OFF => {
            crate::println!("reboot: system halt/poweroff");
            unsafe {
                core::arch::asm!(
                    "ecall",
                    in("a7") 0x8u64,
                    out("a0") _,
                    out("a1") _,
                    options(nomem)
                );
            }
            loop {}
        }
        _ => return -errno::EINVAL as u64,
    }

    0 // unreachable
}

/// sys_shmget - Create or find shared memory segment
///
/// # Arguments
/// - args[0]: key - shared memory key
/// - args[1]: size - segment size
/// - args[2]: shmflg - flags (IPC_CREAT, IPC_EXCL, permissions)
pub fn sys_shmget(_args: SyscallArgs) -> u64 {
    // TODO: implement System V shared memory
    -errno::ENOSYS as u64
}

/// sys_shmctl - Shared memory control
///
/// # Arguments
/// - args[0]: shmid - shared memory ID
/// - args[1]: cmd - IPC_STAT, IPC_SET, IPC_RMID
/// - args[2]: buf - pointer to shmid_ds
pub fn sys_shmctl(_args: SyscallArgs) -> u64 {
    // TODO: implement System V shared memory
    -errno::ENOSYS as u64
}

/// sys_shmat - Attach shared memory segment
///
/// # Arguments
/// - args[0]: shmid - shared memory ID
/// - args[1]: shmaddr - desired attach address
/// - args[2]: shmflg - SHM_RDONLY, SHM_REMAP, etc.
pub fn sys_shmat(_args: SyscallArgs) -> u64 {
    // TODO: implement System V shared memory
    -errno::ENOSYS as u64
}

/// sys_shmdt - Detach shared memory segment
///
/// # Arguments
/// - args[0]: shmaddr - address of attached segment
pub fn sys_shmdt(_args: SyscallArgs) -> u64 {
    // TODO: implement System V shared memory
    -errno::ENOSYS as u64
}

/// sys_unshare - Create new namespace
///
/// # Arguments
/// - args[0]: flags - CLONE_NEWNS, CLONE_NEWUTS, CLONE_NEWIPC, etc.
pub fn sys_unshare(_args: SyscallArgs) -> u64 {
    // TODO: implement namespace support
    -errno::ENOSYS as u64
}

/// sys_syncfs - Sync filesystem of a file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
pub fn sys_syncfs(_args: SyscallArgs) -> u64 {
    // Flush all buffer cache (simplified: sync everything)
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// sys_memfd_create - Create anonymous memory file
///
/// # Arguments
/// - args[0]: name - file name (can be NULL)
/// - args[1]: flags - MFD_CLOEXEC, MFD_ALLOW_SEALING
pub fn sys_memfd_create(args: SyscallArgs) -> u64 {
    let _name_ptr = args[0] as *const u8;
    let _flags = args[1] as u32;
    // TODO: implement memfd_create
    -errno::ENOSYS as u64
}

/// sys_ioprio_set - Set I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
/// - args[2]: ioprio - I/O priority class + value
pub fn sys_ioprio_set(_args: SyscallArgs) -> u64 {
    // TODO: implement I/O priority
    -errno::ENOSYS as u64
}

/// sys_ioprio_get - Get I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
pub fn sys_ioprio_get(args: SyscallArgs) -> u64 {
    let _which = args[0] as i32;
    let _who = args[1] as i32;
    // Default I/O priority: IOPRIO_PRIO_VALUE(IO_PRIO_CLASS_BE, 0) = 0
    0
}

/// sys_quotactl - Disk quota operations
///
/// # Arguments
/// - args[0]: cmd - Q_QUOTAON, Q_QUOTAOFF, Q_GETQUOTA, etc.
/// - args[1]: special - path to filesystem
/// - args[2]: id - user/group ID
/// - args[3]: addr - pointer to dqblk structure
pub fn sys_quotactl(_args: SyscallArgs) -> u64 {
    // TODO: implement quota support
    -errno::ENOSYS as u64
}

/// sys_ptrace - Process tracing
///
/// # Arguments
/// - args[0]: request - PTRACE_TRACEME, PTRACE_PEEKTEXT, etc.
/// - args[1]: pid - tracee PID
/// - args[2]: addr - address
/// - args[3]: data - data
pub fn sys_ptrace(_args: SyscallArgs) -> u64 {
    // TODO: implement ptrace (complex - debugger support)
    -errno::ENOSYS as u64
}

/// sys_riscv_hwprobe - Probe RISC-V hardware features
///
/// # Arguments
/// - args[0]: pairs - pointer to key-value pairs
/// - args[1]: count - number of pairs
/// - args[2]: cpu_count - pointer to CPU count (or NULL)
/// - args[3]: cpus - pointer to CPU set (or NULL)
pub fn sys_riscv_hwprobe(args: SyscallArgs) -> u64 {
    let pairs_ptr = args[0] as *mut u64;
    let count = args[1] as usize;
    let _cpu_count_ptr = args[2] as *mut u32;
    let _cpus_ptr = args[3] as *const usize;

    if pairs_ptr.is_null() || count == 0 {
        return 0;
    }
    if !crate::arch::riscv64::uaccess::access_ok(pairs_ptr as usize, count * 16) {
        return -errno::EFAULT as u64;
    }

    // struct riscv_hwprobe_pair { key, value }
    const KEY_MVENDORID: u64 = 0;
    const KEY_MARCHID: u64 = 1;
    const KEY_IMPID: u64 = 2;
    const KEY_MMU: u64 = 6;

    unsafe {
        for i in 0..count {
            let key = core::ptr::read_volatile(pairs_ptr.add(i * 2));
            let value = match key {
                KEY_MVENDORID => {
                    let mut val: u64;
                    core::arch::asm!("csrr {}, mvendorid", out(reg) val);
                    val
                }
                KEY_MARCHID => {
                    let mut val: u64;
                    core::arch::asm!("csrr {}, marchid", out(reg) val);
                    val
                }
                KEY_IMPID => {
                    let mut val: u64;
                    core::arch::asm!("csrr {}, mimpid", out(reg) val);
                    val
                }
                KEY_MMU => 1, // sv39
                _ => u64::MAX,
            };
            core::ptr::write_volatile(pairs_ptr.add(i * 2 + 1), value);
        }
    }

    count as u64
}

/// sys_riscv_flush_icache - Flush instruction cache
///
/// # Arguments
/// - args[0]: start - start address
/// - args[1]: size - size in bytes
/// - args[2]: flags - SYS_RISCV_FLUSH_ICACHE_ALL
pub fn sys_riscv_flush_icache(args: SyscallArgs) -> u64 {
    let _start = args[0] as usize;
    let _size = args[1] as usize;
    let flags = args[2] as u32;

    const SYS_RISCV_FLUSH_ICACHE_ALL: u32 = 1;

    if flags & SYS_RISCV_FLUSH_ICACHE_ALL != 0 {
        // Flush entire I-cache: use fence.i
        unsafe { core::arch::asm!("fence.i"); }
    } else {
        // Flush specific range: fence.i is sufficient for RISC-V
        unsafe { core::arch::asm!("fence.i"); }
    }

    0
}

// ============================================================================
// NR 294: kexec_file_load
// ============================================================================

/// sys_kexec_file_load - Load new kernel from file descriptor (NR 294)
pub fn sys_kexec_file_load(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

// ============================================================================
// NR 424-440: pidfd, io_uring, clone3, close_range, etc.
// ============================================================================

/// sys_pidfd_send_signal - Send signal to process via pidfd (NR 424)
pub fn sys_pidfd_send_signal(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_io_uring_setup - Setup io_uring instance (NR 425)
pub fn sys_io_uring_setup(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_io_uring_enter - Enter io_uring (NR 426)
pub fn sys_io_uring_enter(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_io_uring_register - Register io_uring buffers/files (NR 427)
pub fn sys_io_uring_register(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_clone3 - Create child process (extended) (NR 435)
pub fn sys_clone3(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_close_range - Close file descriptors in range (NR 436)
pub fn sys_close_range(args: SyscallArgs) -> u64 {
    let fd = args[0] as u32;
    let max_fd = args[1] as u32;
    let _flags = args[2] as u32;

    if fd > max_fd {
        return -errno::EINVAL as u64;
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -errno::EBADF as u64,
    };

    let mut closed = 0u32;
    for target_fd in fd..=max_fd {
        if fdtable.close_fd(target_fd as usize).is_err() {
            // fd not open, skip
        } else {
            closed += 1;
        }
    }
    closed as u64
}

/// sys_pidfd_open - Get pidfd for process (NR 434)
pub fn sys_pidfd_open(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_pidfd_getfd - Get file descriptor from process via pidfd (NR 438)
pub fn sys_pidfd_getfd(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_faccessat2 - Check file access permissions (extended) (NR 439)
pub fn sys_faccessat2(args: SyscallArgs) -> u64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as i32;
    let _flags = args[3] as i32;
    // Delegate to faccessat (NR 48), ignoring extra flags
    let faccessat_args = [dirfd as u64, pathname_ptr as u64, mode as u64, 0, 0, 0];
    crate::syscall::file::sys_faccessat(faccessat_args)
}

/// sys_process_madvise - Advise kernel about process memory (NR 440)
pub fn sys_process_madvise(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_memfd_secret - Create anonymous memory file (secret) (NR 447)
pub fn sys_memfd_secret(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}

/// sys_process_mrelease - Release process memory (NR 448)
pub fn sys_process_mrelease(_args: SyscallArgs) -> u64 {
    -errno::ENOSYS as u64
}
