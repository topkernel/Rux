//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/[pid] - Process information directory
//!
//! Contains process-specific files like:
//! - /proc/[pid]/status - Process status
//! - /proc/[pid]/cmdline - Command line arguments
//! - /proc/[pid]/stat - Process statistics
//! - /proc/[pid]/fd/ - File descriptors

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use alloc::sync::Arc;

/// RAII guard for the SUM (Supervisor User Memory Access) bit in sstatus.
///
/// Enables user-memory reads on construction; restores the previous
/// state on drop, making it panic-safe.
struct SumGuard(());

impl SumGuard {
    fn new() -> Self {
        // SAFETY: single core kernel, no SMP concerns.
        unsafe {
            core::arch::asm!(
                "li t6, 0x40000",
                "csrs sstatus, t6",
                options(nomem, nostack)
            );
        }
        SumGuard(())
    }
}

impl Drop for SumGuard {
    fn drop(&mut self) {
        // SAFETY: single core kernel, restores the SUM bit cleared.
        unsafe {
            core::arch::asm!(
                "li t6, 0x40000",
                "csrc sstatus, t6",
                options(nomem, nostack)
            );
        }
    }
}

/// Check if a directory name is a valid PID directory
///
/// PID directories are numeric strings like "1", "123", etc.
pub fn is_pid_dir(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    name.iter().all(|&c| c >= b'0' && c <= b'9')
}

/// Check if a PID value corresponds to a valid (existing) process
pub fn is_valid_pid(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    use crate::process::{current_pid, find_task_by_pid};
    current_pid() as u64 == pid || find_task_by_pid(pid as u32).is_some()
}

/// Parse PID from directory name
pub fn parse_pid(name: &[u8]) -> Option<u64> {
    if !is_pid_dir(name) {
        return None;
    }

    let mut pid: u64 = 0;
    for &c in name {
        pid = pid * 10 + (c - b'0') as u64;
    }
    Some(pid)
}

/// Generate /proc/[pid]/status content
pub fn generate_status(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let mut content = String::new();

    // Try to get task info
    let (name, ppid, tgid) = if current_pid() as u64 == pid {
        if let Some(task) = current_task() {
            (task.get_exe_path(), task.ppid(), task.tgid())
        } else {
            (b"unknown".as_slice(), 0, 0)
        }
    } else if let Some(task) = find_task_by_pid(pid as u32) {
        (task.get_exe_path(), task.ppid(), task.tgid())
    } else {
        content.push_str(&format!("Pid:\t{}\n", pid));
        content.push_str("State:\tX (dead)\n");
        return content.into_bytes();
    };

    // Convert name to string
    let name_str = core::str::from_utf8(name).unwrap_or("unknown");

    content.push_str(&format!("Name:\t{}\n", name_str));
    content.push_str(&format!("Pid:\t{}\n", pid));
    content.push_str(&format!("PPid:\t{}\n", ppid));
    content.push_str(&format!("Tgid:\t{}\n", tgid));
    content.push_str("State:\tR (running)\n");
    content.push_str("Uid:\t0\t0\t0\t0\n");
    content.push_str("Gid:\t0\t0\t0\t0\n");
    content.push_str("Groups:\t\n");
    content.push_str("VmSize:\t0 kB\n");
    content.push_str("VmRSS:\t0 kB\n");
    content.push_str("VmData:\t0 kB\n");
    content.push_str("VmStk:\t0 kB\n");
    content.push_str("VmExe:\t0 kB\n");
    content.push_str("VmLib:\t0 kB\n");
    content.push_str("Threads:\t1\n");
    content.push_str("SigQ:\t0/0\n");
    content.push_str("SigPnd:\t0000000000000000\n");
    content.push_str("ShdPnd:\t0000000000000000\n");
    content.push_str("SigBlk:\t0000000000000000\n");
    content.push_str("SigIgn:\t0000000000000000\n");
    content.push_str("SigCgt:\t0000000000000000\n");
    content.push_str("CapInh:\t0000000000000000\n");
    content.push_str("CapPrm:\t0000000000000000\n");
    content.push_str("CapEff:\t0000000000000000\n");
    content.push_str("CapBnd:\t0000000000000000\n");
    content.push_str("Seccomp:\t0\n");

    content.into_bytes()
}

/// Generate /proc/[pid]/cmdline content
///
/// Format: arguments separated by null bytes
pub fn generate_cmdline(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let name = if current_pid() as u64 == pid {
        if let Some(task) = current_task() {
            task.get_exe_path().to_vec()
        } else {
            Vec::new()
        }
    } else if let Some(task) = find_task_by_pid(pid as u32) {
        task.get_exe_path().to_vec()
    } else {
        Vec::new()
    };

    let mut result = name;
    result.push(0);  // Null terminator
    result
}

/// Generate /proc/[pid]/stat content
///
/// Format: (pid) (comm) (state) (ppid) (pgrp) (session) (tty_nr) (tpgid) ...
pub fn generate_stat(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let (name, ppid) = if current_pid() as u64 == pid {
        if let Some(task) = current_task() {
            (task.get_exe_path(), task.ppid())
        } else {
            (b"unknown".as_slice(), 0)
        }
    } else if let Some(task) = find_task_by_pid(pid as u32) {
        (task.get_exe_path(), task.ppid())
    } else {
        return format!("{} (unknown) X 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n", pid).into_bytes();
    };

    let name_str = core::str::from_utf8(name).unwrap_or("unknown");

    // Format: pid (comm) state ppid ...
    let content = format!(
        "{} ({}) R {} {} {} 0 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        pid,
        name_str,
        ppid,
        pid,  // pgrp = pid
        pid,  // session = pid
        pid,  // tty_pgrp = pid
    );
    content.into_bytes()
}

/// Generate /proc/[pid]/exe symlink target
pub fn generate_exe_link(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let name = if current_pid() as u64 == pid {
        if let Some(task) = current_task() {
            task.get_exe_path()
        } else {
            b""
        }
    } else if let Some(task) = find_task_by_pid(pid as u32) {
        task.get_exe_path()
    } else {
        return b"/".to_vec();
    };

    let name_str = core::str::from_utf8(name).unwrap_or("");
    format!("/bin/{}", name_str).into_bytes()
}

/// Generate /proc/[pid]/cwd symlink target
pub fn generate_cwd_link(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    if current_pid() as u64 == pid {
        if let Some(task) = current_task() {
            task.get_cwd().to_vec()
        } else {
            b"/".to_vec()
        }
    } else if let Some(task) = find_task_by_pid(pid as u32) {
        task.get_cwd().to_vec()
    } else {
        b"/".to_vec()
    }
}

/// Generate /proc/[pid]/environ content
///
/// Format: VAR=value\0VAR=value\0...
pub fn generate_environ(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let addr_space = match task.address_space() {
        Some(a) => a,
        None => return Vec::new(),
    };

    let env_start = addr_space.env_start();
    let env_end = addr_space.env_end();
    if env_start == 0 || env_end == 0 || env_end <= env_start {
        return Vec::new();
    }

    let env_len = env_end - env_start;
    let mut result = alloc::vec::Vec::with_capacity(env_len);

    let _guard = SumGuard::new();
    unsafe {
        let mut p = env_start as *const u8;
        let end = env_end as *const u8;
        while p < end {
            let b = core::ptr::read_volatile(p);
            result.push(b);
            p = p.add(1);
        }
    }

    result
}

/// Generate /proc/[pid]/maps content
///
/// Format: start-end perms offset dev inode pathname
/// e.g.: 00010000-00020000 r-xp 00000000 00:00 0 [exe]
pub fn generate_maps(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};
    use crate::mm::vma::{VmaFlags, VmaType};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let addr_space = match task.address_space() {
        Some(a) => a,
        None => return Vec::new(),
    };

    let heap_start = addr_space.start_brk();

    let mut content = String::new();
    let vma_mgr = addr_space.vma_read();

    for vma in vma_mgr.iter() {
        let start = vma.start().as_usize();
        let end = vma.end().as_usize();
        let flags = vma.flags();

        let r = if flags.is_readable() { 'r' } else { '-' };
        let w = if flags.is_writable() { 'w' } else { '-' };
        let x = if flags.is_executable() { 'x' } else { '-' };
        let s = if flags.is_shared() { 's' } else { 'p' };

        let offset = vma.offset();

        // Determine pathname and inode
        let (pathname, inode): (String, u64) = if flags.contains(VmaFlags::GROWSDOWN) {
            ("[stack]".into(), 0)
        } else if flags.contains(VmaFlags::EXECUTABLE) {
            ("[exe]".into(), 0)
        } else if start == heap_start {
            ("[heap]".into(), 0)
        } else if vma.vma_type() == VmaType::FileBacked {
            let fd = vma.file_fd();
            if fd >= 0 {
                // Look up file from fdtable to get path and inode
                match unsafe { task.fdtable().get_file(fd as usize) } {
                    Some(file) => {
                        let dentry_opt = unsafe { &*file.dentry.get() };
                        match dentry_opt {
                            Some(dentry) => {
                                let path = dentry.build_path();
                                let ino = dentry.get_inode()
                                    .map(|inode| inode.ino)
                                    .unwrap_or(0);
                                (path, ino)
                            }
                            None => (String::new(), 0),
                        }
                    }
                    None => (String::new(), 0),
                }
            } else {
                (String::new(), 0)
            }
        } else {
            (String::new(), 0)
        };

        if pathname.is_empty() {
            content.push_str(&format!(
                "{:012x}-{:012x} {}{}{}{} {:08x} 00:00 {} \n",
                start, end, r, w, x, s, offset, inode
            ));
        } else {
            content.push_str(&format!(
                "{:012x}-{:012x} {}{}{}{} {:08x} 00:00 {} {}\n",
                start, end, r, w, x, s, offset, inode, pathname
            ));
        }
    }

    content.into_bytes()
}

/// List file descriptors for /proc/[pid]/fd/
///
/// Returns a list of (fd_number, path_string) tuples.
/// For each open fd, resolves the file path from dentry.
pub fn list_fds(pid: u64) -> Vec<(u32, alloc::string::String)> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let fdtable = match task.try_fdtable() {
        Some(ft) => ft,
        None => return Vec::new(),
    };

    let mut fds = Vec::new();
    for fd in 0..1024 {
        if let Some(file) = fdtable.get_file(fd) {
            let path = get_fd_path(&file);
            fds.push((fd as u32, path));
        }
    }

    fds
}

/// Get the path string for a file descriptor's File object
fn get_fd_path(file: &alloc::sync::Arc<crate::fs::File>) -> alloc::string::String {
    let dentry_opt = unsafe { &*file.dentry.get() };
    match dentry_opt {
        Some(dentry) => dentry.build_path(),
        None => {
            // No dentry (pipe, socket, epoll, etc.)
            // Try to get inode number for display
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt {
                Some(inode) => {
                    // Check if it looks like a pipe
                    let rdev = inode.rdev;
                    if rdev != 0 {
                        alloc::format!("anon_inode:[{}]", rdev)
                    } else {
                        alloc::format!("anon_inode:[{}]", inode.ino)
                    }
                }
                None => alloc::string::String::from("anon_inode"),
            }
        }
    }
}

/// Generate symlink target for /proc/[pid]/fd/N
///
/// Returns the path that the fd symlink points to.
pub fn generate_fd_link(pid: u64, fd: u32) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let fdtable = match task.try_fdtable() {
        Some(ft) => ft,
        None => return Vec::new(),
    };

    match fdtable.get_file(fd as usize) {
        Some(file) => {
            let path = get_fd_path(&file);
            path.into_bytes()
        }
        None => Vec::new(),
    }
}

/// Generate /proc/[pid]/oom_score content
///
/// Returns the OOM badness score for the process (read-only).
/// Score is computed dynamically: total_vm + oom_score_adj * totalpages / 1000
pub fn generate_oom_score(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};
    use crate::mm::pglist::first_online_node_mut;
    use crate::mm::zone::ZoneType;

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return b"0\n".to_vec(),
    };

    // Get totalpages from zone
    // SAFETY: read-only zone statistics — no concurrent mutation concern.
    let totalpages = unsafe { first_online_node_mut() }
        .and_then(|node| {
            for zt in [ZoneType::ZoneNormal, ZoneType::ZoneDma32, ZoneType::ZoneDma] {
                if let Some(zone) = node.zone(zt) {
                    if zone.is_initialized() {
                        return Some(zone.managed_pages() as u64);
                    }
                }
            }
            None
        })
        .unwrap_or(0);

    let score = unsafe { crate::mm::oom_kill::oom_badness(&*task, totalpages) };
    format!("{}\n", score).into_bytes()
}

/// Generate /proc/[pid]/oom_score_adj content
///
/// Returns the OOM score adjustment for the process (read-only).
/// Range: -1000 (immune) to 1000 (always kill).
pub fn generate_oom_score_adj(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => return b"0\n".to_vec(),
    };

    let adj = unsafe { (*task).oom_score_adj() };
    format!("{}\n", adj).into_bytes()
}
