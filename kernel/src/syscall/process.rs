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
pub fn sys_clone(args: SyscallArgs) -> i64 {
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
        Some(pid) => pid as i64,
        None => -(errno::ENOMEM as i64),
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

/// Read argv array from user space using fault-safe uaccess helpers.
fn copy_argv_from_user(argv_ptr: *const *const u8) -> alloc::vec::Vec<alloc::string::String> {
    use alloc::string::String;
    let mut args = alloc::vec::Vec::new();
    if argv_ptr.is_null() {
        return args;
    }

    let mut buf = [0u8; 1024];
    for i in 0..65 {
        // Read one pointer from the user argv array via get_user (handles SUM + exception table).
        let arg_ptr = match unsafe { crate::arch::riscv64::uaccess::get_user(argv_ptr.add(i)) } {
            Some(p) => p,
            None => break, // fault or end of array
        };
        if arg_ptr.is_null() {
            break;
        }
        // Read the null-terminated string via strncpy_from_user (byte-by-byte get_user).
        match crate::arch::riscv64::uaccess::strncpy_from_user(arg_ptr, 1024, &mut buf) {
            Ok(slice) => {
                if let Ok(s) = core::str::from_utf8(slice) {
                    args.push(String::from(s));
                }
            }
            Err(_) => break, // page fault reading user string
        }
    }
    args
}

/// Read envp array from user space using fault-safe uaccess helpers.
fn copy_envp_from_user(envp_ptr: *const *const u8) -> alloc::vec::Vec<alloc::string::String> {
    use alloc::string::String;
    let mut envs = alloc::vec::Vec::new();
    if envp_ptr.is_null() {
        return envs;
    }

    let mut buf = [0u8; 4096];
    for i in 0..257 {
        // Read one pointer from the user envp array via get_user (handles SUM + exception table).
        let env_str_ptr = match unsafe { crate::arch::riscv64::uaccess::get_user(envp_ptr.add(i)) } {
            Some(p) => p,
            None => break,
        };
        if env_str_ptr.is_null() {
            break;
        }
        match crate::arch::riscv64::uaccess::strncpy_from_user(env_str_ptr, 4096, &mut buf) {
            Ok(slice) => {
                if let Ok(s) = core::str::from_utf8(slice) {
                    envs.push(String::from(s));
                }
            }
            Err(_) => break,
        }
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
            // SAFETY: current is guaranteed non-null from sched::current() above.
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

    // SAFETY: program_data is a valid slice read from a verified ELF file;
    // from_bytes validates the ELF header structure before returning.
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

    // ---- setuid / setgid exec handling ----
    // Check file mode bits for S_ISUID / S_ISGID and update credentials
    // accordingly.  This must happen after file is verified as valid ELF
    // but before the actual program image replaces the address space.
    {
        use crate::fs::stat_file_by_path;
        use crate::security::capability::Cap;

        const S_ISUID: u32 = 0o4000;
        const S_ISGID: u32 = 0o2000;

        let mut file_stat = crate::fs::Stat::new();
        if stat_file_by_path(full_path.as_ref(), &mut file_stat).is_ok() {
            let file_mode = file_stat.st_mode;
            let file_uid = file_stat.st_uid;
            let file_gid = file_stat.st_gid;
            let is_setuid = (file_mode & S_ISUID) != 0;
            let is_setgid = (file_mode & S_ISGID) != 0;

            // SAFETY: current is a valid, non-null task pointer from sched::current().
            // We hold a reference to the current task, so cred_mut() is safe to call.
            unsafe {
                let cred = (*current).cred_mut();
                let old_euid = cred.euid;
                let old_egid = cred.egid;

                if is_setuid && file_uid != old_euid {
                    if file_uid == 0 {
                        // setuid root: elevate to root with full caps
                        cred.euid = 0;
                        cred.fsuid = 0;
                        cred.cap_effective = Cap::FULL;
                        cred.cap_permitted = Cap::FULL;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    } else {
                        // setuid non-root: drop all caps, change euid
                        cred.euid = file_uid;
                        cred.fsuid = file_uid;
                        cred.cap_effective = Cap::EMPTY;
                        cred.cap_permitted = Cap::EMPTY;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    }
                    cred.suid = cred.euid;
                }

                if is_setgid && file_gid != old_egid {
                    cred.egid = file_gid;
                    cred.fsgid = file_gid;
                    cred.sgid = cred.egid;
                    // setgid non-root also drops caps (unless setuid root above)
                    if !(is_setuid && file_uid == 0) {
                        cred.cap_effective = Cap::EMPTY;
                        cred.cap_permitted = Cap::EMPTY;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    }
                }

                if !is_setuid && !is_setgid {
                    // Normal exec: compute effective caps from ambient
                    let ambient = cred.cap_inheritable
                        .intersect(cred.cap_bounding)
                        .intersect(cred.cap_permitted);
                    cred.cap_effective = cred.cap_permitted.intersect(
                        cred.cap_inheritable.union(ambient)
                    );
                    // Clear ambient for non-setuid exec
                    cred.cap_ambient = Cap::EMPTY;
                }
            }
        }
    }

    // Execute ELF loading
    let phdr_count_usize = if phdr_count > 1024 {
        return -(crate::errno::constants::EINVAL as i64) as u64;
    } else {
        phdr_count as usize
    };
    match do_execve_elf(current, &program_data, &final_argv, &final_envp, entry, phdr_count_usize, &ehdr, full_path.as_ref(), interp_data.as_deref()) {
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
pub fn sys_execve(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;
    let envp_ptr = args[2] as *const *const u8;

    // Check path pointer
    if pathname_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -(errno::EINVAL as i64),
    };

    // Copy argv and envp from user space
    let argv = copy_argv_from_user(argv_ptr);
    let envp = copy_envp_from_user(envp_ptr);

    do_execve(pathname_str, &argv, &envp, 0) as i64
}

/// sys_exit - Exit process
///
/// # Arguments
/// - args[0]: status - exit status code
///
/// # Returns
/// Does not return
pub fn sys_exit(args: SyscallArgs) -> i64 {
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
pub fn sys_wait4(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // Validate wstatus pointer
    if !wstatus.is_null() && !crate::arch::riscv64::uaccess::access_ok(wstatus as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    // WNOHANG: If no child process has exited, return 0 immediately
    const WNOHANG: i32 = 0x00000001;

    if options & WNOHANG != 0 {
        // WNOHANG mode: non-blocking check
        match crate::process::exit::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as i64,
            Err(e) if e == -11 => 0,  // EAGAIN -> return 0 means no child process exited
            Err(e) => e as i32 as i64,
        }
    } else {
        // Blocking wait for child process to exit
        let result = match crate::process::exit::do_wait(pid, wstatus, options) {
            Ok(child_pid) => {
                child_pid as i64
            }
            Err(e) => {
                e as i32 as i64
            }
        };
        result
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
pub fn sys_waitid(args: SyscallArgs) -> i64 {
    let idtype = args[0] as i32;
    let id = args[1] as i32;
    let infop = args[2] as *mut u8;
    let options = args[3] as i32;
    let _rusage = args[4] as *mut u8;

    // Validate idtype
    if idtype < 0 || idtype > 2 {
        return -(errno::EINVAL as i64);
    }

    // Validate infop pointer
    if infop.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(infop as usize, 128) {
        return -(errno::EFAULT as i64);
    }

    match crate::process::exit::do_waitid(idtype, id, infop, options) {
        Ok(()) => 0,
        Err(e) => e as i32 as i64,
    }
}

/// sys_getpid - Get process ID
pub fn sys_getpid(_args: SyscallArgs) -> i64 {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is guaranteed valid and non-null by sched::current().
        unsafe { (*current).pid() as i64 }
    } else {
        0
    }
}

/// sys_gettid - Get thread ID
///
/// In single-threaded processes, tid == pid.
/// RISC-V syscall number: 178
pub fn sys_gettid(_args: SyscallArgs) -> i64 {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is guaranteed valid and non-null by sched::current().
        unsafe { (*current).pid() as i64 }
    } else {
        0
    }
}

/// sys_getppid - Get parent process ID
pub fn sys_getppid(_args: SyscallArgs) -> i64 {
    crate::process::current_ppid() as i64
}

/// sys_kill - Send signal
pub fn sys_kill(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }

    if pid == 0 {
        // Send to all processes in the caller's process group
        let pgid = match crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid when Some.
            Some(t) => unsafe { (*t).pgid() },
            None => return -(errno::ESRCH as i64),
        };
        // SAFETY: for_each_task provides valid task pointers; pgid/sig checks guard usage.
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
        // SAFETY: for_each_task provides valid task pointers; pgid/sig checks guard usage.
        crate::sched::for_each_task(|task| unsafe {
            if (*task).pgid() == pgid && sig > 0 {
                let _ = crate::signal::send_signal((*task).pid(), sig);
            }
        });
        return 0;
    }

    // pid > 0: send to specific process
    // SAFETY: find_task_by_pid returns a valid pointer when non-null; we check
    // null before dereferencing and verify permissions before sending signal.
    unsafe {
        let target = crate::sched::find_task_by_pid(pid as u32);
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }

        if sig > 0 {
            let target_task = &*target;
            if !crate::security::can_send_signal(target_task.cred()) {
                return -(errno::EPERM as i64);
            }
            let _ = crate::signal::send_signal(pid as u32, sig);
        }
    }

    0
}

/// sys_set_tid_address - Set TID address
pub fn sys_set_tid_address(args: SyscallArgs) -> i64 {
    let tidptr = args[0] as *mut i32;

    // Validate tidptr pointer
    if !tidptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(tidptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    if let Some(current) = crate::sched::current() {
        // SAFETY: current is a valid task pointer from sched::current().
        unsafe {
            (*current).set_clear_child_tid(tidptr);
            return (*current).pid() as i64;
        }
    }

    0
}

/// sys_set_robust_list - Set robust list
pub fn sys_set_robust_list(_args: SyscallArgs) -> i64 {
    // Simplified implementation
    0
}

/// sys_uname - Get system information
pub fn sys_uname(args: SyscallArgs) -> i64 {
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
        return -(errno::EFAULT as i64);
    }

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, core::mem::size_of::<Utsname>()) {
        return -(errno::EFAULT as i64);
    }

    // Build Utsname on stack, then copy to user with copy_to_user
    let uname = Utsname {
        sysname: {
            let mut a = [0u8; 65];
            let s = b"Rux\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        nodename: {
            let mut a = [0u8; 65];
            let s = b"rux\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        release: {
            let mut a = [0u8; 65];
            let s = b"0.1.0\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        version: {
            let mut a = [0u8; 65];
            let s = b"Rux OS v0.1.0\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        machine: {
            let mut a = [0u8; 65];
            let s = b"riscv64\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        domainname: {
            let mut a = [0u8; 65];
            a[0] = 0;
            a
        },
    };

    // SAFETY: buf validated with access_ok above; copy_to_user handles SUM bit.
    unsafe {
        let remaining = crate::arch::riscv64::uaccess::copy_to_user(
            buf as *mut u8,
            &uname as *const Utsname as *const u8,
            core::mem::size_of::<Utsname>(),
        );
        if remaining != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_getuid - Get user ID
pub fn sys_getuid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().uid as i64
    } else {
        0
    }
}

/// sys_getgid - Get group ID
pub fn sys_getgid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().gid as i64
    } else {
        0
    }
}

/// sys_geteuid - Get effective user ID
pub fn sys_geteuid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().euid as i64
    } else {
        0
    }
}

/// sys_getegid - Get effective group ID
pub fn sys_getegid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().egid as i64
    } else {
        0
    }
}

/// sys_setuid - Set user ID
///
/// # Arguments
/// - args[0]: uid - user ID to set
pub fn sys_setuid(args: SyscallArgs) -> i64 {
    let uid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETUID) {
                // CAP_SETUID: set all uid fields
                cred.uid = uid;
                cred.euid = uid;
                cred.suid = uid;
                cred.fsuid = uid;
            } else if cred.uid == uid || cred.suid == uid {
                // Unprivileged: can set euid to real or saved uid
                cred.euid = uid;
                cred.fsuid = uid;
            } else {
                return -(errno::EPERM as i64);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setgid - Set group ID
///
/// # Arguments
/// - args[0]: gid - group ID to set
pub fn sys_setgid(args: SyscallArgs) -> i64 {
    let gid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETGID) {
                // CAP_SETGID: set all gid fields
                cred.gid = gid;
                cred.egid = gid;
                cred.sgid = gid;
                cred.fsgid = gid;
            } else if cred.gid == gid || cred.sgid == gid {
                // Unprivileged: can set egid to real or saved gid
                cred.egid = gid;
                cred.fsgid = gid;
            } else {
                return -(errno::EPERM as i64);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setreuid - Set real and effective user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
pub fn sys_setreuid(args: SyscallArgs) -> i64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            let old_ruid = cred.uid;
            let old_euid = cred.euid;
            let old_suid = cred.suid;

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                old_ruid
            } else if crate::security::capable(crate::security::CAP_SETUID) || ruid as u32 == old_ruid || ruid as u32 == old_euid || ruid as u32 == old_suid {
                ruid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                old_euid
            } else if crate::security::capable(crate::security::CAP_SETUID) || euid as u32 == old_ruid || euid as u32 == old_euid || euid as u32 == old_suid {
                euid as u32
            } else {
                return -(errno::EPERM as i64);
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
        -(errno::ESRCH as i64)
    }
}

/// sys_setregid - Set real and effective group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
pub fn sys_setregid(args: SyscallArgs) -> i64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            let old_rgid = cred.gid;
            let old_egid = cred.egid;
            let old_sgid = cred.sgid;

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                old_rgid
            } else if crate::security::capable(crate::security::CAP_SETGID) || rgid as u32 == old_rgid || rgid as u32 == old_egid || rgid as u32 == old_sgid {
                rgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                old_egid
            } else if crate::security::capable(crate::security::CAP_SETGID) || egid as u32 == old_rgid || egid as u32 == old_egid || egid as u32 == old_sgid {
                egid as u32
            } else {
                return -(errno::EPERM as i64);
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
        -(errno::ESRCH as i64)
    }
}

/// sys_setresuid - Set real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
/// - args[2]: suid - saved user ID (-1 to leave unchanged)
pub fn sys_setresuid(args: SyscallArgs) -> i64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    let suid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                cred.uid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || ruid as u32 == cred.uid
                || ruid as u32 == cred.euid
                || ruid as u32 == cred.suid
            {
                ruid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                cred.euid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || euid as u32 == cred.uid
                || euid as u32 == cred.euid
                || euid as u32 == cred.suid
            {
                euid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new suid
            let new_suid = if suid == -1 {
                cred.suid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || suid as u32 == cred.uid
                || suid as u32 == cred.euid
                || suid as u32 == cred.suid
            {
                suid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.uid = new_ruid;
            cred.euid = new_euid;
            cred.suid = new_suid;
            cred.fsuid = new_euid;
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_getresuid - Get real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - pointer to store real user ID
/// - args[1]: euid - pointer to store effective user ID
/// - args[2]: suid - pointer to store saved user ID
pub fn sys_getresuid(args: SyscallArgs) -> i64 {
    let ruid_ptr = args[0] as *mut u32;
    let euid_ptr = args[1] as *mut u32;
    let suid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred() returns a reference.
        let cred = unsafe { (*task).cred() };
        // SAFETY: task is valid; user pointers are validated with access_ok before each write.
        unsafe {
            if !ruid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(ruid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(ruid_ptr, cred.uid);
            }
            if !euid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(euid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(euid_ptr, cred.euid);
            }
            if !suid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(suid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(suid_ptr, cred.suid);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setresgid - Set real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
/// - args[2]: sgid - saved group ID (-1 to leave unchanged)
pub fn sys_setresgid(args: SyscallArgs) -> i64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    let sgid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                cred.gid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || rgid as u32 == cred.gid
                || rgid as u32 == cred.egid
                || rgid as u32 == cred.sgid
            {
                rgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                cred.egid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || egid as u32 == cred.gid
                || egid as u32 == cred.egid
                || egid as u32 == cred.sgid
            {
                egid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new sgid
            let new_sgid = if sgid == -1 {
                cred.sgid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || sgid as u32 == cred.gid
                || sgid as u32 == cred.egid
                || sgid as u32 == cred.sgid
            {
                sgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.gid = new_rgid;
            cred.egid = new_egid;
            cred.sgid = new_sgid;
            cred.fsgid = new_egid;
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_getresgid - Get real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - pointer to store real group ID
/// - args[1]: egid - pointer to store effective group ID
/// - args[2]: sgid - pointer to store saved group ID
pub fn sys_getresgid(args: SyscallArgs) -> i64 {
    let rgid_ptr = args[0] as *mut u32;
    let egid_ptr = args[1] as *mut u32;
    let sgid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred() returns a reference.
        let cred = unsafe { (*task).cred() };
        // SAFETY: task is valid; user pointers validated with access_ok before each write.
        unsafe {
            if !rgid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(rgid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(rgid_ptr, cred.gid);
            }
            if !egid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(egid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(egid_ptr, cred.egid);
            }
            if !sgid_ptr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(sgid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                core::ptr::write_volatile(sgid_ptr, cred.sgid);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
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
pub fn sys_getgroups(args: SyscallArgs) -> i64 {
    let size = args[0] as i32;
    let list_ptr = args[1] as *mut u32;

    // Currently no supplementary groups, return 0
    if size == 0 {
        return 0;
    }
    if size < 0 {
        return -(errno::EINVAL as i64);
    }

    // No supplementary groups to return
    0
}

/// sys_setgroups - Set supplementary group IDs
///
/// # Arguments
/// - args[0]: size - number of groups
/// - args[1]: list - pointer to group ID array
pub fn sys_setgroups(args: SyscallArgs) -> i64 {
    // Only CAP_SETGID can set supplementary groups
    if !crate::security::capable(crate::security::CAP_SETGID) {
        return -(errno::EPERM as i64);
    }
    // TODO: implement supplementary group storage
    0
}

/// sys_setpgid - Set process group ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: pgid - process group ID (0 = pid)
pub fn sys_setpgid(args: SyscallArgs) -> i64 {
    let target_pid = args[0] as i32;
    let pgid = args[1] as i32;

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // SAFETY: current is a valid task pointer from sched::current().
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
        // SAFETY: current is a valid task pointer from sched::current().
        unsafe {
            if (*current).sid() != pgid as u32 {
                // pgid must be in same session (simplified: just check it's valid)
            }
            (*current).set_pgid(pgid as u32);
        }
    } else {
        // Setting child's pgid
        // SAFETY: find_task_by_pid returns valid pointer when non-null.
        let target = unsafe { crate::sched::find_task_by_pid(target_pid as u32) };
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }
        // SAFETY: target is validated non-null; current is valid from sched::current().
        unsafe {
            // Target must be a child of current process
            if (*target).ppid() != current_pid {
                return -(errno::ESRCH as i64);
            }
            // Target must be in same session
            if (*target).sid() != (*current).sid() {
                return -(errno::EPERM as i64);
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
pub fn sys_getpgid(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid.
            return unsafe { (*task).pgid() as i64 };
        }
        return -(errno::ESRCH as i64);
    }

    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    unsafe { (*target).pgid() as i64 }
}

/// sys_setsid - Create a new session
///
/// # Returns
/// New session ID on success, negative error on failure
pub fn sys_setsid(_args: SyscallArgs) -> i64 {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // SAFETY: current is a valid, non-null task pointer from sched::current().
    unsafe {
        let pid = (*current).pid();

        // Process must not be a process group leader
        if (*current).pgid() == pid {
            return -(errno::EPERM as i64);
        }

        // Create new session and process group
        (*current).set_sid(pid);
        (*current).set_pgid(pid);

        pid as i64
    }
}

/// sys_getsid - Get session ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
pub fn sys_getsid(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid.
            return unsafe { (*task).sid() as i64 };
        }
        return -(errno::ESRCH as i64);
    }

    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    unsafe { (*target).sid() as i64 }
}

/// sys_prlimit64 - Get/set resource limits
pub fn sys_prlimit64(args: SyscallArgs) -> i64 {
    let _pid = args[0] as i32;
    let resource = args[1] as i32;
    let new_rlim = args[2] as *const u8;
    let old_rlim = args[3] as *mut u8;

    // Validate pointers
    if !new_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(new_rlim as usize, 16) {
        return -(errno::EFAULT as i64);
    }
    if !old_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(old_rlim as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Only support querying
    if !new_rlim.is_null() {
        return -(errno::EPERM as i64);
    }

    if old_rlim.is_null() {
        return -(errno::EFAULT as i64);
    }

    // RLIMIT_NOFILE = 7
    if resource == 7 {
        // Return default file descriptor limit using copy_to_user
        let rlimit: [u64; 2] = [1024, 1024 * 1024];  // rlim_cur, rlim_max
        // SAFETY: old_rlim validated non-null and access_ok above; copy_to_user handles
        // user pointer writes safely.
        let uncopied = unsafe {
            crate::arch::riscv64::uaccess::copy_to_user(
                old_rlim as *mut u8,
                rlimit.as_ptr() as *const u8,
                core::mem::size_of::<[u64; 2]>()
            )
        };
        if uncopied != 0 {
            return -(errno::EFAULT as i64);
        }
        return 0;
    }

    -(errno::EINVAL as i64)
}

/// sys_prctl - manipulate process attributes
///
/// Arguments: (option, arg2, arg3, arg4, arg5)
pub fn sys_prctl(args: SyscallArgs) -> i64 {
    use crate::arch::riscv64::uaccess::{copy_to_user, strncpy_from_user};

    let option = args[0] as i32;
    let arg2 = args[1];
    let arg3 = args[2];
    let arg4 = args[3];
    let _arg5 = args[4];

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    match option {
        1 => {
            // PR_SET_PDEATHSIG
            let sig = arg2 as i32;
            if sig < 0 || sig > 64 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer from sched::current().
            unsafe { (*current).pdeath_signal = sig as u32; }
            0
        }
        2 => {
            // PR_GET_PDEATHSIG
            let ptr = arg2 as *mut u32;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            if !crate::arch::riscv64::uaccess::access_ok(ptr as usize, 4) {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: ptr validated with access_ok; current is valid.
            unsafe {
                core::ptr::write_volatile(ptr, (*current).pdeath_signal);
            }
            0
        }
        3 => {
            // PR_GET_DUMPABLE
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).dumpable as i64 }
        }
        4 => {
            // PR_SET_DUMPABLE
            let val = arg2 as u32;
            if val > 1 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).dumpable = val; }
            0
        }
        15 => {
            // PR_SET_NAME
            let ptr = arg2 as *const u8;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            let mut buf = [0u8; 16];
            match strncpy_from_user(ptr, 16, &mut buf) {
                // SAFETY: current is valid; buf contains safely copied user data.
                Ok(_) => unsafe { (*current).set_comm(&buf); },
                Err(_) => return -(errno::EFAULT as i64),
            }
            0
        }
        16 => {
            // PR_GET_NAME
            let ptr = arg2 as *mut u8;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            if !crate::arch::riscv64::uaccess::access_ok(ptr as usize, 16) {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: ptr validated with access_ok; current is valid; comm() returns a
            // reference to a fixed-size internal buffer of 16 bytes.
            unsafe {
                let comm = (*current).comm();
                let _ = copy_to_user(ptr, comm.as_ptr(), 16);
            }
            0
        }
        36 => {
            // PR_SET_CHILD_SUBREAPER
            let val = arg2 as u32;
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.store(val != 0, core::sync::atomic::Ordering::Relaxed);
                }
            }
            0
        }
        37 => {
            // PR_GET_CHILD_SUBREAPER
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.load(core::sync::atomic::Ordering::Relaxed) as i64
                } else {
                    0
                }
            }
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_tgkill - send signal to a thread group
///
/// # Arguments
/// - args[0]: tgid - thread group ID
/// - args[1]: tid - thread ID
/// - args[2]: sig - signal number
pub fn sys_tgkill(args: SyscallArgs) -> i64 {
    let _tgid = args[0] as i32;
    let tid = args[1] as u32;
    let sig = args[2] as i32;

    if sig < 0 {
        return -(errno::EINVAL as i64);
    }

    // Validate target exists and caller has permission, even for sig==0.
    // SAFETY: find_task_by_pid returns valid pointer when non-null; we check null.
    let target = unsafe { crate::sched::find_task_by_pid(tid) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    let target_task = unsafe { &*target };
    if !crate::security::can_send_signal(target_task.cred()) {
        return -(errno::EPERM as i64);
    }

    // sig==0: just a permission check, don't actually send a signal.
    if sig == 0 {
        return 0;
    }

    crate::signal::send_signal(tid, sig)
        .map(|_| 0)
        .unwrap_or(-(errno::EINVAL as i64))
}

/// sys_rt_sigqueueinfo - send signal with data
///
/// # Arguments
/// - args[0]: tgid - thread group ID
/// - args[1]: tid - thread ID
/// - args[2]: sig - signal number
/// - args[3]: uinfo - siginfo_t pointer (user)
pub fn sys_rt_sigqueueinfo(args: SyscallArgs) -> i64 {
    let _tgid = args[0] as i32;
    let tid = args[1] as u32;
    let sig = args[2] as i32;
    let uinfo = args[3] as *const u8;

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }
    if uinfo.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(uinfo as usize, 128) {
        return -(errno::EFAULT as i64);
    }

    if sig > 0 {
        // SAFETY: find_task_by_pid returns valid pointer when non-null; we check null.
        let target = unsafe { crate::sched::find_task_by_pid(tid) };
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }
        // SAFETY: target validated non-null above.
        let target_task = unsafe { &*target };
        if !crate::security::can_send_signal(target_task.cred()) {
            return -(errno::EPERM as i64);
        }
    }

    // Send the signal (without siginfo data — simplified)
    crate::signal::send_signal(tid, sig)
        .map(|_| 0)
        .unwrap_or(-(errno::EINVAL as i64))
}

/// sys_rt_sigtimedwait - synchronously wait for signals
///
/// # Arguments
/// - args[0]: uthese - pointer to signal mask (sigset_t)
/// - args[1]: uinfo - siginfo_t pointer (user output)
/// - args[2]: uts - timeout timespec pointer (user, NULL = block forever)
/// - args[3]: sigsetsize - size of signal mask
pub fn sys_rt_sigtimedwait(args: SyscallArgs) -> i64 {
    let uthese = args[0] as *const u64;
    let uinfo = args[1] as *mut u8;
    let uts = args[2] as *const u8;
    let sigsetsize = args[3] as usize;

    if uthese.is_null() || uinfo.is_null() {
        return -(errno::EFAULT as i64);
    }
    if sigsetsize < 8 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(uthese as usize, sigsetsize) {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(uinfo as usize, 128) {
        return -(errno::EFAULT as i64);
    }
    if !uts.is_null() && !crate::arch::riscv64::uaccess::access_ok(uts as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Check for already pending signals
    // Read the signal set (first 8 bytes = 64 signals)
    // SAFETY: uthese validated non-null and access_ok above.
    let sigset = unsafe { core::ptr::read_volatile(uthese) };
    let pending = crate::signal::signal_pending();
    if !pending {
        // No signal pending — if timeout is zero, return EAGAIN
        if !uts.is_null() {
            // SAFETY: uts validated with access_ok above; reading i64 fields at known offsets.
            let ts_sec = unsafe { *((uts) as *const i64) };
            let ts_nsec = unsafe { *((uts.add(8)) as *const i64) };
            if ts_sec == 0 && ts_nsec == 0 {
                return -(errno::EAGAIN as i64);
            }
        }
        return -(errno::EINTR as i64);
    }

    // Find first pending signal that's in the set
    for i in 0..64u64 {
        if (sigset & (1u64 << i)) != 0 {
            // Fill siginfo_t with the signal number
            // SAFETY: uinfo validated non-null and access_ok above; writing 128 bytes is safe.
            unsafe {
                core::ptr::write_bytes(uinfo, 0, 128);
                // si_signo at offset 0
                core::ptr::write_volatile(uinfo as *mut i32, (i + 1) as i32);
            }
            return (i + 1) as i64;
        }
    }

    -(errno::EINTR as i64)
}

/// sys_getcpu - get CPU number and node
///
/// # Arguments
/// - args[0]: cpuset_ptr - CPU set pointer
/// - args[1]: node_ptr - NUMA node pointer
/// - args[2]: cache_ptr - cache ID pointer
pub fn sys_getcpu(args: SyscallArgs) -> i64 {
    use crate::arch::riscv64::smp::cpu_id;

    let cpuset_ptr = args[0] as *mut u32;
    let node_ptr = args[1] as *mut u32;
    let _cache_ptr = args[2] as *mut u32;

    if !cpuset_ptr.is_null() {
        // SAFETY: cpuset_ptr validated with access_ok; writing 4 x u32 = 16 bytes.
        unsafe {
            if !crate::arch::riscv64::uaccess::access_ok(cpuset_ptr as usize, 16) {
                return -(errno::EFAULT as i64);
            }
            core::ptr::write_volatile(cpuset_ptr, 1u32 << cpu_id());
            core::ptr::write_volatile(cpuset_ptr.add(1), 0u32);
            core::ptr::write_volatile(cpuset_ptr.add(2), 0u32);
            core::ptr::write_volatile(cpuset_ptr.add(3), 0u32);
        }
    }
    if !node_ptr.is_null() {
        // SAFETY: node_ptr validated with access_ok; writing 4 x u32 = 16 bytes.
        unsafe {
            if !crate::arch::riscv64::uaccess::access_ok(node_ptr as usize, 16) {
                return -(errno::EFAULT as i64);
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
pub fn sys_execveat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let argv_ptr = args[2] as *const *const u8;
    let envp_ptr = args[3] as *const *const u8;
    let _flags = args[4] as i32;

    if pathname_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    let path = match crate::syscall::file::resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    let argv = copy_argv_from_user(argv_ptr);
    let envp = copy_envp_from_user(envp_ptr);

    do_execve(&path, &argv, &envp, 0) as i64
}

/// sys_setfsuid - Set filesystem user ID
///
/// # Arguments
/// - args[0]: fsuid - filesystem user ID
pub fn sys_setfsuid(args: SyscallArgs) -> i64 {
    let fsuid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred()/cred_mut()
        // return references to the task's credential structure.
        unsafe {
            let old_fsuid = (*task).cred().fsuid;
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETUID) {
                cred.fsuid = fsuid;
            } else if fsuid == cred.uid || fsuid == cred.euid || fsuid == cred.suid {
                cred.fsuid = fsuid;
            }
            old_fsuid as i64
        }
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setfsgid - Set filesystem group ID
///
/// # Arguments
/// - args[0]: fsgid - filesystem group ID
pub fn sys_setfsgid(args: SyscallArgs) -> i64 {
    let fsgid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred()/cred_mut()
        // return references to the task's credential structure.
        unsafe {
            let old_fsgid = (*task).cred().fsgid;
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETGID) {
                cred.fsgid = fsgid;
            } else if fsgid == cred.gid || fsgid == cred.egid || fsgid == cred.sgid {
                cred.fsgid = fsgid;
            }
            old_fsgid as i64
        }
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_times - Get process times
///
/// # Arguments
/// - args[0]: buf - pointer to struct tms
///
/// # Returns
/// Clock ticks since system boot on success
pub fn sys_times(args: SyscallArgs) -> i64 {
    let buf_ptr = args[0] as *mut u64;
    if !buf_ptr.is_null() {
        if !crate::arch::riscv64::uaccess::access_ok(buf_ptr as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        // struct tms: tms_utime, tms_stime, tms_cutime, tms_cstime (all clock_t = i64)
        // SAFETY: buf_ptr validated with access_ok; writing 32 bytes to user space.
        unsafe { core::ptr::write_bytes(buf_ptr, 0, 32); }
    }
    // Return clock ticks since boot (simplified: use jiffies)
    crate::drivers::timer::get_jiffies() as i64
}

/// sys_sysinfo - Get system information
///
/// # Arguments
/// - args[0]: info - pointer to struct sysinfo
pub fn sys_sysinfo(args: SyscallArgs) -> i64 {
    let info_ptr = args[0] as *mut u8;
    if info_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(info_ptr as usize, 112) {
        return -(errno::EFAULT as i64);
    }
    // struct sysinfo: 112 bytes, fill with available info
    // SAFETY: info_ptr validated with access_ok above; all writes are within the
    // 112-byte struct sysinfo layout.
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
pub fn sys_membarrier(args: SyscallArgs) -> i64 {
    let cmd = args[0] as i32;
    let _flags = args[1] as u32;

    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = (1 << 0);
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = (1 << 1);

    match cmd {
        MEMBARRIER_CMD_QUERY => {
            // Report which commands are supported
            MEMBARRIER_CMD_GLOBAL as i64 | MEMBARRIER_CMD_GLOBAL_EXPEDITED as i64
        }
        MEMBARRIER_CMD_GLOBAL | MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            // Full memory barrier
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_userfaultfd - Create userfaultfd file descriptor
pub fn sys_userfaultfd(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_kcmp - Compare two processes
pub fn sys_kcmp(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_finit_module - Load kernel module from file descriptor
pub fn sys_finit_module(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_init_module - Load kernel module
pub fn sys_init_module(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_delete_module - Unload kernel module
pub fn sys_delete_module(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_kexec_load - Load new kernel for reboot
pub fn sys_kexec_load(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_process_vm_readv - Read from another process memory
pub fn sys_process_vm_readv(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_process_vm_writev - Write to another process memory
pub fn sys_process_vm_writev(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_perf_event_open - Open performance event
pub fn sys_perf_event_open(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_seccomp - Operate on seccomp state
pub fn sys_seccomp(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_bpf - BPF system call
pub fn sys_bpf(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_capget - Get capabilities for a process
///
/// # Arguments
/// - args[0]: hdr_ptr - pointer to __user_cap_header_struct { version: u32, pid: i32 }
/// - args[1]: data_ptr - pointer to __user_cap_data_struct array(s)
pub fn sys_capget(args: SyscallArgs) -> i64 {
    use crate::arch::riscv64::uaccess::{copy_from_user, copy_to_user};

    const _LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
    const _LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
    const _LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let hdr_ptr = args[0] as usize;
    let data_ptr = args[1] as usize;

    // Both pointers are required
    if hdr_ptr == 0 || data_ptr == 0 {
        return -(errno::EFAULT as i64);
    }
    // Header is 8 bytes: version (u32) + pid (i32)
    if !crate::arch::riscv64::uaccess::access_ok(hdr_ptr, 8) {
        return -(errno::EFAULT as i64);
    }

    // Read header
    let mut hdr = [0u32; 2];
    // SAFETY: hdr_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(hdr.as_mut_ptr() as *mut u8, hdr_ptr as *const u8, 8) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    let version = hdr[0];
    let pid = hdr[1] as i32;

    // Determine the target task
    let target = if pid == 0 {
        match crate::sched::current() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        }
    } else {
        // SAFETY: find_task_by_pid returns valid pointer when non-null.
        let ptr = unsafe { crate::sched::find_task_by_pid(pid as u32) };
        if ptr.is_null() {
            return -(errno::ESRCH as i64);
        }
        // SAFETY: ptr validated non-null above.
        unsafe { &*ptr }
    };

    let cred = target.cred();

    // Validate version and determine data size
    let data_count: usize;
    match version {
        _LINUX_CAPABILITY_VERSION_1 => data_count = 1,  // 1 x 3 u32s = 12 bytes
        _LINUX_CAPABILITY_VERSION_2 => data_count = 2,  // 2 x 3 u32s = 24 bytes
        _LINUX_CAPABILITY_VERSION_3 => data_count = 2,  // 2 x 3 u32s = 24 bytes
        _ => {
            // Unknown version: write back the highest supported version and return -EINVAL
            let supported = _LINUX_CAPABILITY_VERSION_3;
            // SAFETY: hdr_ptr validated with access_ok; copy_to_user handles user writes.
            unsafe {
                copy_to_user(hdr_ptr as *mut u8, &supported as *const u32 as *const u8, 4);
            }
            return -(errno::EINVAL as i64);
        }
    }

    let data_size = data_count * 3 * 4; // each entry is 3 u32s
    if !crate::arch::riscv64::uaccess::access_ok(data_ptr, data_size) {
        return -(errno::EFAULT as i64);
    }

    // Build the data array: [effective_lo, permitted_lo, inheritable_lo, effective_hi, permitted_hi, inheritable_hi]
    let mut data: [u32; 6] = [
        cred.cap_effective.lo(),
        cred.cap_permitted.lo(),
        cred.cap_inheritable.lo(),
        cred.cap_effective.hi(),
        cred.cap_permitted.hi(),
        cred.cap_inheritable.hi(),
    ];

    // SAFETY: data_ptr validated with access_ok; copy_to_user handles user writes.
    unsafe {
        if copy_to_user(data_ptr as *mut u8, data.as_mut_ptr() as *const u8, data_size) != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_capset - Set capabilities for the calling process
///
/// # Arguments
/// - args[0]: hdr_ptr - pointer to __user_cap_header_struct { version: u32, pid: i32 }
/// - args[1]: data_ptr - pointer to __user_cap_data_struct array(s)
pub fn sys_capset(args: SyscallArgs) -> i64 {
    use crate::arch::riscv64::uaccess::copy_from_user;
    use crate::security::capability::Cap;

    const _LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
    const _LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
    const _LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let hdr_ptr = args[0] as usize;
    let data_ptr = args[1] as usize;

    if hdr_ptr == 0 || data_ptr == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(hdr_ptr, 8) {
        return -(errno::EFAULT as i64);
    }

    // Read header
    let mut hdr = [0u32; 2];
    // SAFETY: hdr_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(hdr.as_mut_ptr() as *mut u8, hdr_ptr as *const u8, 8) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    let version = hdr[0];
    let _pid = hdr[1] as i32;

    // Determine data size from version
    let data_count: usize;
    match version {
        _LINUX_CAPABILITY_VERSION_1 => data_count = 1,
        _LINUX_CAPABILITY_VERSION_2 => data_count = 2,
        _LINUX_CAPABILITY_VERSION_3 => data_count = 2,
        _ => return -(errno::EINVAL as i64),
    }
    let data_size = data_count * 3 * 4;
    if !crate::arch::riscv64::uaccess::access_ok(data_ptr, data_size) {
        return -(errno::EFAULT as i64);
    }

    // Read data from userspace
    let mut data = [0u32; 6];
    // SAFETY: data_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(data.as_mut_ptr() as *mut u8, data_ptr as *const u8, data_size) != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    // Reconstruct capabilities from the data array
    let new_effective = Cap::from_halves(data[0], data[3]);
    let new_permitted = Cap::from_halves(data[1], data[4]);
    let new_inheritable = Cap::from_halves(data[2], data[5]);

    // capset can only operate on the current process
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Permission checks:
    // 1. new permitted must be a subset of old permitted
    // 2. new inheritable must be a subset of old permitted
    // 3. new effective must be a subset of new permitted
    let cred = current.cred();
    if !new_permitted.is_subset_of(cred.cap_permitted) {
        return -(errno::EPERM as i64);
    }
    if !new_inheritable.is_subset_of(cred.cap_permitted) {
        return -(errno::EPERM as i64);
    }
    if !new_effective.is_subset_of(new_permitted) {
        return -(errno::EPERM as i64);
    }

    // Apply changes (bounding set is not modified by capset)
    // SAFETY: current is a valid task pointer; cred_mut() returns mutable reference.
    unsafe {
        let cred_mut = (*current).cred_mut();
        cred_mut.cap_effective = new_effective;
        cred_mut.cap_permitted = new_permitted;
        cred_mut.cap_inheritable = new_inheritable;
    }

    0
}

/// sys_personality - Set process execution domain
pub fn sys_personality(args: SyscallArgs) -> i64 {
    let _persona = args[0] as u64;
    // Return current personality (0 = PER_LINUX)
    0
}

/// sys_pivot_root - Change root filesystem (NR 41)
pub fn sys_pivot_root(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_setns - reassociate thread with a namespace
///
/// # Arguments
/// - args[0]: fd - namespace file descriptor
/// - args[1]: nstype - namespace type
pub fn sys_setns(_args: SyscallArgs) -> i64 {
    // TODO: implement namespace support
    -(errno::ENOSYS as i64)
}

/// sys_getrlimit - Get resource limits (deprecated, use prlimit64)
///
/// # Arguments
/// - args[0]: resource - resource type (RLIMIT_*)
/// - args[1]: rlim - pointer to struct rlimit
pub fn sys_getrlimit(args: SyscallArgs) -> i64 {
    let resource = args[0] as u32;
    let rlim_ptr = args[1] as *mut u64;

    if rlim_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(rlim_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // struct rlimit { rlim_cur: u64, rlim_max: u64 }
    const RLIMIT_NOFILE: u32 = 7;
    let (cur, max) = match resource {
        RLIMIT_NOFILE => (1024u64, 1024 * 1024),
        _ => return -(errno::EINVAL as i64),
    };

    // SAFETY: rlim_ptr validated with access_ok above; writes two u64 values.
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
pub fn sys_setrlimit(args: SyscallArgs) -> i64 {
    let resource = args[0] as u32;
    let rlim_ptr = args[1] as *const u64;

    if rlim_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(rlim_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: rlim_ptr validated with access_ok; reads two u64 values.
    let rlim_cur = unsafe { core::ptr::read_volatile(rlim_ptr) };
    let rlim_max = unsafe { core::ptr::read_volatile(rlim_ptr.add(1)) };

    const RLIMIT_NOFILE: u32 = 7;
    const RLIMIT_DATA: u32 = 2;
    const RLIMIT_STACK: u32 = 3;
    const RLIMIT_CORE: u32 = 4;
    const RLIMIT_RSS: u32 = 5;
    const RLIMIT_NPROC: u32 = 6;
    const RLIMIT_MEMLOCK: u32 = 8;
    const RLIMIT_AS: u32 = 9;
    const RLIMIT_LOCKS: u32 = 10;
    const RLIMIT_SIGPENDING: u32 = 11;
    const RLIMIT_MSGQUEUE: u32 = 12;
    const RLIMIT_NICE: u32 = 13;
    const RLIMIT_RTPRIO: u32 = 14;
    const RLIMIT_RTTIME: u32 = 15;

    if rlim_cur > rlim_max {
        return -(errno::EINVAL as i64);
    }

    // Only root can raise hard limits
    let is_root = if let Some(task) = crate::sched::current() {
        task.cred().euid == 0
    } else {
        false
    };

    match resource {
        RLIMIT_NOFILE | RLIMIT_DATA | RLIMIT_STACK | RLIMIT_CORE
        | RLIMIT_RSS | RLIMIT_NPROC | RLIMIT_MEMLOCK | RLIMIT_AS
        | RLIMIT_LOCKS | RLIMIT_SIGPENDING | RLIMIT_MSGQUEUE
        | RLIMIT_NICE | RLIMIT_RTPRIO | RLIMIT_RTTIME => {
            // CAP_SYS_RESOURCE required to raise hard limits
            if !is_root && !crate::security::capable(crate::security::CAP_SYS_RESOURCE) {
                // Would need to check if new hard limit > old hard limit
                // but no per-task storage yet; just allow
            }
            // Silently accept — no per-task storage yet
            0
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_getrusage - Get resource usage
///
/// # Arguments
/// - args[0]: who - RUSAGE_SELF (0), RUSAGE_CHILDREN (-1)
/// - args[1]: rusage - pointer to struct rusage
pub fn sys_getrusage(args: SyscallArgs) -> i64 {
    let _who = args[0] as i32;
    let rusage_ptr = args[1] as *mut u8;

    if rusage_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(rusage_ptr as usize, 136) {
        return -(errno::EFAULT as i64);
    }

    // Fill rusage with zeros (no resource tracking yet)
    // SAFETY: rusage_ptr validated with access_ok; writing 136 bytes of zeros.
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
pub fn sys_sethostname(args: SyscallArgs) -> i64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    // CAP_SYS_ADMIN required to set hostname
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }

    if name_ptr.is_null() || len == 0 || len > 65 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(name_ptr as usize, len) {
        return -(errno::EFAULT as i64);
    }

    // TODO: implement hostname storage
    0
}

/// sys_setdomainname - Set NIS domain name
///
/// # Arguments
/// - args[0]: name - pointer to domain name string
/// - args[1]: len - domain name length
pub fn sys_setdomainname(args: SyscallArgs) -> i64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    // CAP_SYS_ADMIN required to set domain name
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }

    if name_ptr.is_null() || len == 0 || len > 65 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(name_ptr as usize, len) {
        return -(errno::EFAULT as i64);
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
pub fn sys_reboot(args: SyscallArgs) -> i64 {
    let magic1 = args[0] as u32;
    let magic2 = args[1] as u32;
    let cmd = args[2] as u32;

    const LINUX_REBOOT_MAGIC1: u32 = 0xfee1dead;
    const LINUX_REBOOT_MAGIC2: u32 = 672274793;
    const LINUX_REBOOT_MAGIC2C: u32 = 85072278;

    const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;
    const LINUX_REBOOT_CMD_HALT: u32 = 0xCDEF0123;
    const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;

    // CAP_SYS_BOOT required to reboot
    if !crate::security::capable(crate::security::CAP_SYS_BOOT) {
        return -(errno::EPERM as i64);
    }

    if magic1 != LINUX_REBOOT_MAGIC1 || (magic2 != LINUX_REBOOT_MAGIC2 && magic2 != LINUX_REBOOT_MAGIC2C) {
        return -(errno::EINVAL as i64);
    }

    match cmd {
        LINUX_REBOOT_CMD_RESTART => {
            crate::println!("reboot: restarting system");
            // SBI legacy shutdown ecall (0x8)
            // SAFETY: This is a privileged SBI ecall; only reached after CAP_SYS_BOOT check.
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
            // SAFETY: This is a privileged SBI ecall; only reached after CAP_SYS_BOOT check.
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
        _ => return -(errno::EINVAL as i64),
    }

    0 // unreachable
}

/// sys_unshare - Create new namespace
///
/// # Arguments
/// - args[0]: flags - CLONE_NEWNS, CLONE_NEWUTS, CLONE_NEWIPC, etc.
pub fn sys_unshare(_args: SyscallArgs) -> i64 {
    // TODO: implement namespace support
    -(errno::ENOSYS as i64)
}

/// sys_syncfs - Sync filesystem of a file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
pub fn sys_syncfs(_args: SyscallArgs) -> i64 {
    // Flush all buffer cache (simplified: sync everything)
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// sys_memfd_create - Create anonymous memory file
///
/// # Arguments
/// - args[0]: name - file name (can be NULL)
/// - args[1]: flags - MFD_CLOEXEC, MFD_ALLOW_SEALING
pub fn sys_memfd_create(args: SyscallArgs) -> i64 {
    let _name_ptr = args[0] as *const u8;
    let _flags = args[1] as u32;
    // TODO: implement memfd_create
    -(errno::ENOSYS as i64)
}

/// sys_ioprio_set - Set I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
/// - args[2]: ioprio - I/O priority class + value
pub fn sys_ioprio_set(args: SyscallArgs) -> i64 {
    let _which = args[0] as i32;
    let _who = args[1] as i32;
    let ioprio = args[2] as i32;

    // IOPRIO_CLASS_SHIFT = 13
    // Class bits: who << 13 | data
    let class = (ioprio >> 13) & 0x7;

    // IOPRIO_CLASS_NONE = 0, IOPRIO_CLASS_RT = 1, IOPRIO_CLASS_BE = 2, IOPRIO_CLASS_IDLE = 3
    if class > 3 {
        return -(errno::EINVAL as i64);
    }

    // CAP_SYS_NICE required to set RT or idle class
    if class == 1 || class == 3 {
        if !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
    }

    // Accept and ignore — no I/O priority storage yet
    0
}

/// sys_ioprio_get - Get I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
pub fn sys_ioprio_get(args: SyscallArgs) -> i64 {
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
pub fn sys_quotactl(args: SyscallArgs) -> i64 {
    let cmd = args[0] as u32;
    let _special = args[1] as *const u8;
    let _id = args[2] as u32;
    let addr = args[3] as *mut u8;

    // Q_QUOTAOFF = 0x800001, Q_GETINFO = 0x800007, Q_GETFMT = 0x800800
    let subcmd = cmd >> 8;

    match subcmd {
        // Q_GETFMT: return quota format (4 bytes at addr)
        0x800 => {
            // Return -ENOTSUP to indicate no quota format
            if !addr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(addr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                // SAFETY: addr validated with access_ok; writing 4 bytes.
                unsafe { core::ptr::write_volatile(addr as *mut i32, -1); }
            }
            0
        }
        // Q_GETINFO: return struct if_dqinfo (16 bytes)
        0x8000 => {
            if !addr.is_null() {
                if !crate::arch::riscv64::uaccess::access_ok(addr as usize, 16) {
                    return -(errno::EFAULT as i64);
                }
                // SAFETY: addr validated with access_ok; writing 16 bytes of zeros.
                unsafe { core::ptr::write_bytes(addr, 0, 16); }
            }
            0
        }
        _ => -(errno::ENOSYS as i64),
    }
}

/// sys_ptrace - Process tracing
///
/// # Arguments
/// - args[0]: request - PTRACE_TRACEME, PTRACE_PEEKTEXT, etc.
/// - args[1]: pid - tracee PID
/// - args[2]: addr - address
/// - args[3]: data - data
pub fn sys_ptrace(_args: SyscallArgs) -> i64 {
    // TODO: implement ptrace (complex - debugger support)
    -(errno::ENOSYS as i64)
}

/// sys_riscv_hwprobe - Probe RISC-V hardware features
///
/// # Arguments
/// - args[0]: pairs - pointer to key-value pairs
/// - args[1]: count - number of pairs
/// - args[2]: cpu_count - pointer to CPU count (or NULL)
/// - args[3]: cpus - pointer to CPU set (or NULL)
pub fn sys_riscv_hwprobe(args: SyscallArgs) -> i64 {
    let pairs_ptr = args[0] as *mut u64;
    let count = args[1] as usize;
    let _cpu_count_ptr = args[2] as *mut u32;
    let _cpus_ptr = args[3] as *const usize;

    if pairs_ptr.is_null() || count == 0 {
        return 0;
    }
    if !crate::arch::riscv64::uaccess::access_ok(pairs_ptr as usize, count.saturating_mul(16)) {
        return -(errno::EFAULT as i64);
    }

    // struct riscv_hwprobe_pair { key, value }
    const KEY_MVENDORID: u64 = 0;
    const KEY_MARCHID: u64 = 1;
    const KEY_IMPID: u64 = 2;
    const KEY_MMU: u64 = 6;

    // SAFETY: pairs_ptr validated with access_ok; reads keys and writes values
    // within the validated count*16 byte range.
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

    count as i64
}

/// sys_riscv_flush_icache - Flush instruction cache
///
/// # Arguments
/// - args[0]: start - start address
/// - args[1]: size - size in bytes
/// - args[2]: flags - SYS_RISCV_FLUSH_ICACHE_ALL
pub fn sys_riscv_flush_icache(args: SyscallArgs) -> i64 {
    let _start = args[0] as usize;
    let _size = args[1] as usize;
    let flags = args[2] as u32;

    const SYS_RISCV_FLUSH_ICACHE_ALL: u32 = 1;

    if flags & SYS_RISCV_FLUSH_ICACHE_ALL != 0 {
        // Flush entire I-cache: use fence.i
        // SAFETY: fence.i is a valid RISC-V instruction, always safe to execute.
        unsafe { core::arch::asm!("fence.i"); }
    } else {
        // Flush specific range: fence.i is sufficient for RISC-V
        // SAFETY: fence.i is a valid RISC-V instruction, always safe to execute.
        unsafe { core::arch::asm!("fence.i"); }
    }

    0
}

// ============================================================================
// NR 294: kexec_file_load
// ============================================================================

/// sys_kexec_file_load - Load new kernel from file descriptor (NR 294)
pub fn sys_kexec_file_load(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 424-440: pidfd, io_uring, clone3, close_range, etc.
// ============================================================================

/// sys_pidfd_send_signal - Send signal to process via pidfd (NR 424)
pub fn sys_pidfd_send_signal(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_io_uring_setup - Setup io_uring instance (NR 425)
pub fn sys_io_uring_setup(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_setup(args) as i64
}

/// sys_io_uring_enter - Enter io_uring (NR 426)
pub fn sys_io_uring_enter(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_enter(args) as i64
}

/// sys_io_uring_register - Register io_uring buffers/files (NR 427)
pub fn sys_io_uring_register(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_register(args) as i64
}

/// sys_clone3 - Create child process (extended) (NR 435)
pub fn sys_clone3(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_close_range - Close file descriptors in range (NR 436)
pub fn sys_close_range(args: SyscallArgs) -> i64 {
    let fd = args[0] as u32;
    let max_fd = args[1] as u32;
    let _flags = args[2] as u32;

    if fd > max_fd {
        return -(errno::EINVAL as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let mut closed = 0u32;
    for target_fd in fd..=max_fd {
        if fdtable.close_fd(target_fd as usize).is_err() {
            // fd not open, skip
        } else {
            closed += 1;
        }
    }
    closed as i64
}

/// sys_pidfd_open - Get pidfd for process (NR 434)
pub fn sys_pidfd_open(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_pidfd_getfd - Get file descriptor from process via pidfd (NR 438)
pub fn sys_pidfd_getfd(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_faccessat2 - Check file access permissions (extended) (NR 439)
pub fn sys_faccessat2(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as i32;
    let _flags = args[3] as i32;
    // Delegate to faccessat (NR 48), ignoring extra flags
    let faccessat_args = [dirfd as u64, pathname_ptr as u64, mode as u64, 0, 0, 0];
    crate::syscall::file::sys_faccessat(faccessat_args)
}

/// sys_process_madvise - Advise kernel about process memory (NR 440)
pub fn sys_process_madvise(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_memfd_secret - Create anonymous memory file (secret) (NR 447)
pub fn sys_memfd_secret(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_process_mrelease - Release process memory (NR 448)
pub fn sys_process_mrelease(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}
