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
    use crate::fs::elf::{ElfLoader, Elf64Ehdr};
    use alloc::vec::Vec;
    use alloc::string::String;

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

    // Build full path
    let full_path = if pathname_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname_str)
    } else {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname_str);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname_str)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname_str)
        }
    };

    // Read ELF file from file system
    let program_data = if crate::fs::ext4::is_mounted() {
        match crate::fs::ext4::read_file_from_mounted(full_path.as_ref()) {
            Some(data) => data,
            None => return -errno::ENOENT as u64,
        }
    } else {
        match crate::fs::read_file_from_rootfs(full_path.as_ref()) {
            Some(data) => data,
            None => return -errno::ENOENT as u64,
        }
    };

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
        // Read interpreter ELF from filesystem
        if crate::fs::ext4::is_mounted() {
            crate::fs::ext4::read_file_from_mounted(interp_str)
        } else {
            crate::fs::read_file_from_rootfs(interp_str)
        }
    } else {
        None
    };

    // Parse argv - need to use copy_from_user for safe user space access
    let argv: Vec<String> = unsafe {
        let mut args = Vec::new();
        if !argv_ptr.is_null() {
            // Enable user memory access
            core::arch::asm!(
                "li t6, 0x40000",
                "csrs sstatus, t6",
                options(nomem, nostack)
            );

            let mut i = 0usize;
            loop {
                let arg_ptr = core::ptr::read_volatile(argv_ptr.add(i));
                if arg_ptr.is_null() {
                    break;
                }
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
                if i > 64 { break; }
            }

            // Disable user memory access
            core::arch::asm!(
                "li t6, 0x40000",
                "csrc sstatus, t6",
                options(nomem, nostack)
            );
        }
        if args.is_empty() {
            args.push(String::from(full_path.as_ref()));
        }
        args
    };

    // Parse envp - same pattern as argv (SUM bit + volatile reads)
    let envp: Vec<String> = unsafe {
        let mut envs = Vec::new();
        if !envp_ptr.is_null() {
            // Enable user memory access
            core::arch::asm!(
                "li t6, 0x40000",
                "csrs sstatus, t6",
                options(nomem, nostack)
            );

            let mut i = 0usize;
            loop {
                let env_str_ptr = core::ptr::read_volatile(envp_ptr.add(i));
                if env_str_ptr.is_null() {
                    break;
                }
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
                if i > 256 { break; }
            }

            // Disable user memory access
            core::arch::asm!(
                "li t6, 0x40000",
                "csrc sstatus, t6",
                options(nomem, nostack)
            );
        }
        envs
    };

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // Execute ELF loading
    match do_execve_elf(current, &program_data, &argv, &envp, entry, phdr_count as usize, &ehdr, full_path.as_ref(), interp_data.as_deref()) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_exit - Exit process

/// sys_exit - Exit process
///
/// # Arguments
/// - args[0]: status - exit status code
///
/// # Returns
/// Does not return
pub fn sys_exit(args: SyscallArgs) -> u64 {
    let exit_code = args[0] as i32;
    crate::sched::do_exit(exit_code);
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
        match crate::sched::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -11 => 0,  // EAGAIN -> return 0 means no child process exited
            Err(e) => e as u32 as u64,
        }
    } else {
        // Blocking wait for child process to exit
        match crate::sched::do_wait(pid, wstatus, options) {
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
                crate::signal::send_signal((*task).pid(), sig);
            }
        });
        return 0;
    }

    if pid < 0 {
        // Send to all processes in process group |pid|
        let pgid = (-pid) as u32;
        crate::sched::for_each_task(|task| unsafe {
            if (*task).pgid() == pgid && sig > 0 {
                crate::signal::send_signal((*task).pid(), sig);
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
            crate::signal::send_signal(pid as u32, sig);
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

/// Execute ELF loading (execve internal function)
///
/// This function will:
/// 1. Create new address space
/// 2. Load ELF segments
/// 3. Set up stack and arguments
/// 4. Update process context
fn do_execve_elf(
    task_ptr: *mut crate::process::task::Task,
    program_data: &[u8],
    argv: &[alloc::string::String],
    envp: &[alloc::string::String],
    entry: u64,
    phdr_count: usize,
    ehdr: &crate::fs::elf::Elf64Ehdr,
    pathname: &str,
    interp_data: Option<&[u8]>,
) -> Result<(), i32> {
    use crate::arch::riscv64::mm::{create_user_address_space, alloc_and_map_to_user_table, PAGE_SIZE, PageTableEntry};
    use core::slice;

    // Close file descriptors with close-on-exec flag
    if let Some(fdtable) = unsafe { (*task_ptr).try_fdtable() } {
        fdtable.close_cloexec_fds();
    }

    // Find virtual address range
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

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

    // Page align
    let virt_start = min_vaddr & !(PAGE_SIZE - 1);
    let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Calculate initial stack size needed
    // Linux: stack_expand = 131072UL (128KB) + actual args size
    let argc = argv.len() as u64;
    let argv_count = argv.len();
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout size (same as below)
    let auxv_slots: usize = 30;  // 15 auxv entries * 2
    let envp_count = envp.len();
    let mut string_space: usize = 0;
    for arg in argv.iter() {
        string_space += ((arg.len() + 1 + 7) / 8) * 8;
    }
    let mut env_string_space: usize = 0;
    for env in envp.iter() {
        env_string_space += ((env.len() + 1 + 7) / 8) * 8;
    }
    let phdr_space: usize = ((phsize + 7) / 8) * 8;
    let total_slots: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots + 2 + (phdr_space + 7) / 8 + (string_space + 7) / 8 + (env_string_space + 7) / 8;
    let args_size = (total_slots * 8) as u64;

    // Initial stack size: args + 128KB (like Linux)
    const STACK_EXPAND: u64 = 128 * 1024;
    let initial_stack_size = (args_size + STACK_EXPAND + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Maximum stack size (8MB, like Linux default)
    const STACK_MAX_SIZE: u64 = 8 * 1024 * 1024;

    // Total size to allocate: ELF segments + initial stack
    let total_size = virt_end - virt_start + initial_stack_size;

    // Create new user address space
    let user_ppn = create_user_address_space().ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Allocate and map user memory (ELF segments + initial stack)
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    let phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, virt_start, total_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Load each segment
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let file_size = phdr.p_filesz;
            let mem_size = phdr.p_memsz;
            let offset = phdr.p_offset as usize;

            let virt_offset = virt_addr - virt_start;
            let phys_addr = (phys_base + virt_offset) as usize;

            // Convert physical address to kernel virtual address for access
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64)).bits() as *mut u8;

            // Copy data
            if file_size > 0 {
                let src = &program_data[offset..offset + file_size as usize];
                unsafe {
                    let dst = slice::from_raw_parts_mut(virt_addr_ptr, file_size as usize);
                    dst.copy_from_slice(src);
                }
            }

            // Zero BSS
            if mem_size > file_size {
                let bss_size = (mem_size - file_size) as usize;
                unsafe {
                    let bss_dst = virt_addr_ptr.add(file_size as usize);
                    core::ptr::write_bytes(bss_dst, 0, bss_size);
                }
            }
        }
    }

    // Load interpreter (dynamic linker) if present
    let (actual_entry, at_base) = if let Some(interp_bytes) = interp_data {
        // Interpreter base address: in the mmap region, below mmap_start
        // Linux loads ld.so in the mmap area; we use a fixed address for simplicity
        let interp_base: u64 = 0x3FBF000000u64;  // mmap_start - 16MB

        // Calculate interpreter size from its PT_LOAD segments
        let interp_ehdr = unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(interp_bytes) }
            .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;

        let mut interp_min_vaddr: u64 = u64::MAX;
        let mut interp_max_vaddr: u64 = 0;
        for i in 0..interp_ehdr.e_phnum as usize {
            if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                if phdr.is_load() {
                    if phdr.p_vaddr < interp_min_vaddr { interp_min_vaddr = phdr.p_vaddr; }
                    let end = phdr.p_vaddr + phdr.p_memsz;
                    if end > interp_max_vaddr { interp_max_vaddr = end; }
                }
            }
        }
        let interp_size = (interp_max_vaddr - interp_min_vaddr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        // Allocate and map memory for interpreter
        let interp_phys = unsafe {
            alloc_and_map_to_user_table(user_ppn, interp_base, interp_size, flags)
        }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

        // Convert to kernel virtual address for writing
        let interp_kva = phys_to_virt(PhysAddr::new(interp_phys as u64)).bits();

        // Load interpreter segments
        let (entry_offset, _) = unsafe {
            crate::fs::elf::ElfLoader::load_dynamic_to(interp_bytes, interp_kva)
        }.map_err(|_| crate::errno::Errno::ExecFormatError.as_neg_i32())?;

        // Interpreter entry point
        let interp_entry = interp_base + entry_offset;

        crate::pr_debug!("execve: loaded interpreter at {:#x}, entry={:#x}, size={:#x}",
            interp_base, interp_entry, interp_size);

        (interp_entry, interp_base)
    } else {
        (entry, 0)
    };

    // Set up stack
    let stack_top = virt_end + initial_stack_size - 256;
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

    // Convert physical address to kernel virtual address for stack access
    let stack_virt_addr = phys_to_virt(PhysAddr::new(phys_stack_top as u64)).bits();

    // auxv constants
    const AT_NULL: u64 = 0;
    const AT_PHDR: u64 = 3;
    const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5;
    const AT_PAGESZ: u64 = 6;
    const AT_BASE: u64 = 7;
    const AT_ENTRY: u64 = 9;
    const AT_UID: u64 = 11;
    const AT_EUID: u64 = 12;
    const AT_GID: u64 = 13;
    const AT_EGID: u64 = 14;
    const AT_HWCAP: u64 = 16;
    const AT_CLKTCK: u64 = 17;
    const AT_SECURE: u64 = 23;
    const AT_RANDOM: u64 = 25;
    const AT_EXECFN: u64 = 31;

    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout
    let auxv_slots: usize = 30;  // 15 auxv entries * 2
    let envp_count = envp.len();
    let mut string_space: usize = 0;
    for arg in argv.iter() {
        string_space += ((arg.len() + 1 + 7) / 8) * 8;
    }
    let mut env_string_space: usize = 0;
    for env in envp.iter() {
        env_string_space += ((env.len() + 1 + 7) / 8) * 8;
    }
    let phdr_space: usize = ((phsize + 7) / 8) * 8;

    let random_offset: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots;
    let phdr_offset: usize = random_offset + 2;
    let env_string_offset: usize = phdr_offset + (phdr_space + 7) / 8;
    let string_offset: usize = env_string_offset + (env_string_space + 7) / 8;
    let total_slots: usize = string_offset + (string_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_slots * 8) as u64);

    let adjusted_virt_offset = adjusted_stack_top - virt_start;
    let adjusted_phys_stack_top = (phys_base + adjusted_virt_offset) as usize;

    // Convert physical address to kernel virtual address for stack access
    let adjusted_stack_virt_addr = phys_to_virt(PhysAddr::new(adjusted_phys_stack_top as u64)).bits();

    unsafe {
        let stack_ptr = adjusted_stack_virt_addr as *mut u64;
        let mut offset: isize = 0;

        let phdr_addr = adjusted_stack_top + (phdr_offset * 8) as u64;
        let random_vaddr = adjusted_stack_top + (random_offset * 8) as u64;

        // Copy program header table
        let src_ptr = program_data.as_ptr().add(ehdr.e_phoff as usize);
        let dst_ptr = (stack_ptr as *mut u8).add(phdr_offset * 8);
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, phsize);

        // Write argv strings
        let mut argv_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(argv_count);
        let mut current_string_offset = string_offset;

        for arg in argv.iter() {
            let string_pos = current_string_offset * 8;
            let arg_bytes = arg.as_bytes();
            for (i, &b) in arg_bytes.iter().enumerate() {
                core::ptr::write_volatile(
                    (stack_ptr as *mut u8).offset((string_pos + i) as isize),
                    b
                );
            }
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((string_pos + arg_bytes.len()) as isize),
                0
            );
            argv_addrs.push(adjusted_stack_top + string_pos as u64);
            current_string_offset += ((arg.len() + 1 + 7) / 8);
        }

        // Write envp strings
        let mut envp_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(envp_count);
        let mut current_env_string_offset = env_string_offset;

        for env in envp.iter() {
            let string_pos = current_env_string_offset * 8;
            let env_bytes = env.as_bytes();
            for (i, &b) in env_bytes.iter().enumerate() {
                core::ptr::write_volatile(
                    (stack_ptr as *mut u8).offset((string_pos + i) as isize),
                    b
                );
            }
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((string_pos + env_bytes.len()) as isize),
                0
            );
            envp_addrs.push(adjusted_stack_top + string_pos as u64);
            current_env_string_offset += ((env.len() + 1 + 7) / 8);
        }

        // argc
        core::ptr::write_volatile(stack_ptr, argc);
        offset += 1;

        // argv
        for &addr in &argv_addrs {
            core::ptr::write_volatile(stack_ptr.offset(offset), addr);
            offset += 1;
        }

        // argv terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // envp pointers
        for &addr in &envp_addrs {
            core::ptr::write_volatile(stack_ptr.offset(offset), addr);
            offset += 1;
        }

        // envp terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // auxv
        let auxv = &[
            (AT_PHDR, phdr_addr),
            (AT_PHENT, phent),
            (AT_PHNUM, phnum),
            (AT_PAGESZ, PAGE_SIZE as u64),
            (AT_BASE, at_base),
            (AT_ENTRY, entry),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_HWCAP, 0),
            (AT_CLKTCK, 100),
            (AT_SECURE, 0),
            (AT_RANDOM, random_vaddr),
            (AT_EXECFN, argv_addrs.first().copied().unwrap_or(0)),
        ];

        for (typ, val) in auxv {
            core::ptr::write_volatile(stack_ptr.offset(offset), *typ);
            core::ptr::write_volatile(stack_ptr.offset(offset + 1), *val);
            offset += 2;
        }

        // AT_NULL
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_NULL);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);

        // Random numbers
        core::ptr::write_volatile(stack_ptr.offset(offset + 2), 0xdeadc0debeefcafeu64);
        core::ptr::write_volatile(stack_ptr.offset(offset + 3), 0x123456789abcdef0u64);
    }

    // Create new address space structure
    let new_addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // Record envp range for /proc/pid/environ
    if !envp.is_empty() {
        let env_start_addr = adjusted_stack_top + (env_string_offset * 8) as u64;
        let mut env_end_offset = env_string_offset;
        for env in envp.iter() {
            env_end_offset += (env.len() + 1 + 7) / 8;
        }
        let env_end_addr = adjusted_stack_top + (env_end_offset * 8) as u64;
        new_addr_space.setup_envp(env_start_addr as usize, env_end_addr as usize);
    }

    // Set up stack VMA with GROWSDOWN flag and stack limit
    // Stack bottom is where stack can grow down to
    let stack_bottom = virt_end;  // Current stack bottom (end of ELF segments)
    let stack_limit = stack_top.saturating_sub(STACK_MAX_SIZE) + PAGE_SIZE;  // +1 page guard
    new_addr_space.set_start_stack(stack_top as usize);
    new_addr_space.set_stack_limit(stack_limit as usize);

    // Add stack VMA with GROWSDOWN flag
    {
        use crate::mm::vma::{Vma, VmaFlags, VmaType};
        let stack_vma = Vma::new(
            crate::mm::page::VirtAddr::new(stack_bottom as usize),
            crate::mm::page::VirtAddr::new(stack_top as usize),
            VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN),
        );
        let mut vma_mgr = new_addr_space.vma_write();
        let _ = vma_mgr.add(stack_vma);
    }

    // Update process
    unsafe {
        // Set new address space (this will drop old Arc if no other references)
        (*task_ptr).set_address_space(Some(alloc::sync::Arc::new(new_addr_space)));

        // Update exe_path
        (*task_ptr).set_exe_path(pathname.as_bytes());

        // Set user stack pointer
        (*task_ptr).set_user_sp(adjusted_stack_top);

        // Switch to new address space
        let satp = (8u64 << 60) | (user_ppn);  // MODE=8 (Sv39), PPN=user_ppn
        core::arch::asm!(
            "csrw satp, {}",
            "sfence.vma",
            in(reg) satp,
            options(nostack)
        );

        // ===== Return to user mode immediately after successful execve =====
        // After execve returns, sret will jump to new program entry

        // Get current trap frame
        use crate::arch::riscv64::trap::current_pt_regs;
        use crate::arch::riscv64::pt_regs::PtRegs;
        let current_regs = current_pt_regs() as *mut PtRegs;
        if current_regs.is_null() {
            // No trap frame, this is the init process case
            // Need to return via ret_from_fork
            return Ok(());
        }

        // Directly modify current trap frame
        // SPP = 0 means return to user mode, SPIE = 1 means enable interrupts
        const SR_SPP: u64 = 1 << 8;
        const SR_SPIE: u64 = 1 << 5;
        const SR_SUM: u64 = 1 << 18;

        unsafe {
            (*current_regs).epc = actual_entry;         // Entry point (interpreter or program)
            (*current_regs).sp = adjusted_stack_top;   // New user stack
            (*current_regs).status = SR_SPIE | SR_SUM; // Clear SPP, set SPIE and SUM
            (*current_regs).tp = 0;                   // Clear TLS pointer - musl libc will reinitialize
            (*current_regs).a0 = 0;                   // argc is on stack
            // Other registers remain 0
        }

        // Note: Do not free PtRegs memory here because trap frame is on stack
    }

    Ok(())
}
