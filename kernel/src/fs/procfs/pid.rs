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

/// Parse PID from directory name.
/// Rejects leading zeros (except "0" itself) and checks for overflow.
pub fn parse_pid(name: &[u8]) -> Option<u64> {
    if !is_pid_dir(name) {
        return None;
    }
    // Reject leading zeros (e.g., "01", "00")
    if name.len() > 1 && name[0] == b'0' {
        return None;
    }

    let mut pid: u64 = 0;
    for &c in name {
        let digit = (c - b'0') as u64;
        // Check overflow: pid * 10 + digit must not overflow
        pid = pid.checked_mul(10)?.checked_add(digit)?;
    }
    Some(pid)
}

/// Return the single-character task state code used in /proc/[pid]/stat.
///
/// Mirrors the kernel's `task_state_array[]` ordering:
/// R=running, S=sleeping, D=disk sleep, T=stopped, Z=zombie, X=dead.
fn task_state_char(task: &crate::process::Task) -> u8 {
    use crate::process::task::TaskState;
    let st = task.state();
    if st.is_running() { b'R' }
    else if st.contains(TaskState::INTERRUPTIBLE) { b'S' }
    else if st.contains(TaskState::UNINTERRUPTIBLE) { b'D' }
    else if st.contains(TaskState::STOPPED) { b'T' }
    else if st.contains(TaskState::ZOMBIE) { b'Z' }
    else if st.contains(TaskState::DEAD) { b'X' }
    else { b'S' }
}

/// Return the human-readable task state string used in /proc/[pid]/status.
fn task_state_str(task: &crate::process::Task) -> &'static str {
    use crate::process::task::TaskState;
    let st = task.state();
    if st.is_running() { "running" }
    else if st.contains(TaskState::INTERRUPTIBLE) { "sleeping" }
    else if st.contains(TaskState::UNINTERRUPTIBLE) { "disk sleep" }
    else if st.contains(TaskState::STOPPED) { "stopped" }
    else if st.contains(TaskState::ZOMBIE) { "zombie" }
    else if st.contains(TaskState::DEAD) { "dead" }
    else { "sleeping" }
}

/// The task's comm: basename of the exe path, truncated to 15 characters
/// (Linux TASK_COMM_LEN - 1). Used for /proc/[pid]/comm, the "Name:" line
/// of /proc/[pid]/status and field 2 of /proc/[pid]/stat — systemd's PID 1
/// check compares /proc/1/comm against "systemd".
fn task_comm(task: &crate::process::Task) -> alloc::borrow::Cow<'_, str> {
    let path = task.get_exe_path();
    // Basename: everything after the last '/'.
    let base = match path.iter().rposition(|&b| b == b'/') {
        Some(i) => &path[i + 1..],
        None => path,
    };
    let s = core::str::from_utf8(base).unwrap_or("unknown");
    let mut len = s.len().min(15);
    // Do not split a multi-byte UTF-8 character.
    while len > 0 && !s.is_char_boundary(len) {
        len -= 1;
    }
    alloc::borrow::Cow::Borrowed(&s[..len])
}

/// Generate /proc/[pid]/comm content (U2): "<comm>\n".
pub fn generate_comm(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    match task {
        Some(t) => {
            let mut out: Vec<u8> = task_comm(&t).as_bytes().to_vec();
            out.push(b'\n');
            out
        }
        None => b"\n".to_vec(),
    }
}

/// Generate /proc/[pid]/cgroup content (U2).
///
/// Rux exposes a single cgroups v2 unified hierarchy with every task in
/// the root group — exactly what Linux reports for a v2-only system:
/// `0::/\n` (one line per hierarchy; here just the unified one).
pub fn generate_cgroup(_pid: u64) -> Vec<u8> {
    b"0::/\n".to_vec()
}

/// Generate /proc/[pid]/status content
pub fn generate_status(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let mut content = String::new();

    // Try to get task info
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = match task {
        Some(t) => t,
        None => {
            content.push_str(&format!("Pid:\t{}\n", pid));
            content.push_str("State:\tX (dead)\n");
            return content.into_bytes();
        }
    };

    let name_str = task_comm(&task);
    let state_str = task_state_str(task);
    let state_char = task_state_char(task) as char;
    let ppid = task.ppid();
    let tgid = task.tgid();
    let cred = task.cred();

    content.push_str(&format!("Name:\t{}\n", name_str));
    content.push_str("Umask:\t0022\n");
    content.push_str(&format!("State:\t{} ({})\n", state_char, state_str));
    content.push_str(&format!("Tgid:\t{}\n", tgid));
    content.push_str(&format!("Ngid:\t0\n"));
    content.push_str(&format!("Pid:\t{}\n", pid));
    content.push_str(&format!("PPid:\t{}\n", ppid));
    content.push_str(&format!("TracerPid:\t0\n"));
    content.push_str(&format!("Uid:\t{}\t{}\t{}\t{}\n", cred.uid, cred.euid, cred.suid, cred.fsuid));
    content.push_str(&format!("Gid:\t{}\t{}\t{}\t{}\n", cred.gid, cred.egid, cred.sgid, cred.fsgid));
    content.push_str(&format!("FDSize:\t64\n"));
    content.push_str("Groups:\t\n");
    content.push_str("VmSize:\t0 kB\n");
    content.push_str("VmRSS:\t0 kB\n");
    content.push_str("VmData:\t0 kB\n");
    content.push_str("VmStk:\t0 kB\n");
    content.push_str("VmExe:\t0 kB\n");
    content.push_str("VmLib:\t0 kB\n");

    // Threads: live thread count from the thread-group leader (P2: real
    // count — single-threaded processes report 1, CLONE_THREAD groups
    // report the leader's nr_threads).
    let threads = {
        let leader = unsafe { &*task.group_leader_ptr() };
        leader.nr_threads().max(1)
    };
    content.push_str(&format!("Threads:\t{}\n", threads));

    // Signal masks (P2: real values). SigPnd = this task's private
    // pending set; ShdPnd shares it (no separate shared queue yet).
    // SigBlk = the task's blocked mask. SigIgn/SigCgt are computed from
    // the signal-handling table (SIG_IGN dispositions / user handlers).
    let sig_pnd = task.pending.get_all();
    let sig_blk = task.sigmask;
    let (sig_ign, sig_cgt) = task
        .signal
        .as_ref()
        .map(|s| s.ign_cgt_masks())
        .unwrap_or((0, 0));
    content.push_str("SigQ:\t0/0\n");
    content.push_str(&format!("SigPnd:\t{:016x}\n", sig_pnd));
    content.push_str(&format!("ShdPnd:\t{:016x}\n", sig_pnd));
    content.push_str(&format!("SigBlk:\t{:016x}\n", sig_blk));
    content.push_str(&format!("SigIgn:\t{:016x}\n", sig_ign));
    content.push_str(&format!("SigCgt:\t{:016x}\n", sig_cgt));
    content.push_str("CapInh:\t0000000000000000\n");
    content.push_str("CapPrm:\t0000000000000000\n");
    content.push_str("CapEff:\t0000000000000000\n");
    content.push_str("CapBnd:\t0000000000000000\n");
    content.push_str("Seccomp:\t0\n");

    content.into_bytes()
}

/// Read a range of USER memory from an arbitrary task's address space.
///
/// Review 5.7 (high): cmdline/environ used copy_from_user, which walks the
/// CURRENT task's page tables — reading another process's arg/env area
/// returned whatever lived at the same VA in the reader (garbage in `ps`).
/// This helper walks the TARGET's page tables directly (SV39, 3 levels,
/// 4 KiB and 2 MiB leaves) and copies through the kernel linear mapping.
///
/// Returns the bytes actually readable (shorter than requested when the
/// range runs into an unmapped page).
fn read_target_user_mm(
    addr_space: &crate::mm::mm_struct::AddressSpace,
    start: usize,
    len: usize,
) -> Vec<u8> {
    use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

    let root_ppn = addr_space.root_ppn();
    if root_ppn == 0 || len == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(len);
    let mut remaining = len;
    let mut va = start as u64;

    while remaining > 0 {
        // SV39 walk: level-2 → level-1 → level-0
        let a2 = (root_ppn << 12) + ((va >> 30) & 0x1FF) * 8;
        // SAFETY: a2 is the physical address of a valid PTE in the target's
        // level-2 table (page-aligned base + in-table index).
        let e2 = unsafe { core::ptr::read_volatile(phys_to_virt(PhysAddr::new(a2 as u64)).0 as *const u64) };
        if e2 & 1 == 0 {
            break; // unmapped
        }
        let mut pte = e2;
        let mut level = 2u32;
        loop {
            let is_leaf = pte & 0xE != 0; // R|W|X
            if is_leaf || level == 0 {
                break;
            }
            let shift = 12 + 9 * (level - 1);
            let next_table = ((pte >> 10) & 0xFFF_FFFF_FFFF) << 12;
            let idx = (va >> shift) & 0x1FF;
            // SAFETY: next_table is a valid page-table page in the target mm.
            pte = unsafe {
                core::ptr::read_volatile(
                    phys_to_virt(PhysAddr::new(next_table + idx * 8)).0 as *const u64,
                )
            };
            if pte & 1 == 0 {
                return out; // unmapped
            }
            level -= 1;
        }

        // Compute the physical page base for this virtual page.
        let (page_phys, page_len) = if level == 0 {
            (((pte >> 10) & 0xFFF_FFFF_FFFF) << 12, 4096u64)
        } else {
            // 2 MiB (or 1 GiB) leaf: lower PPN bits are zero for the base.
            let shift = 12 + 9 * level;
            let mask: u64 = !((1u64 << shift) - 1);
            ((((pte >> 10) & 0xFFF_FFFF_FFFF) << 12) & mask, 1u64 << shift)
        };

        let in_page_off = (va & (page_len - 1)) as usize;
        let copy = core::cmp::min(remaining, page_len as usize - in_page_off);
        // SAFETY: page_phys+off is a mapped user page of the target; the
        // kernel linear mapping makes it readable here.
        let src = phys_to_virt(PhysAddr::new(page_phys + in_page_off as u64)).0 as *const u8;
        let chunk = unsafe { core::slice::from_raw_parts(src, copy) };
        out.extend_from_slice(chunk);

        remaining -= copy;
        va += copy as u64;
    }

    out
}

/// ptrace_may_access (minimal form) for /proc/[pid]/environ: the caller
/// must be the target itself or hold CAP_SYS_PTRACE (review 5.7).
pub fn environ_access_allowed(pid: u64) -> bool {
    use crate::process::current_pid;
    if current_pid() as u64 == pid {
        return true;
    }
    crate::security::capable(crate::security::CAP_SYS_PTRACE)
}

/// Generate /proc/[pid]/cmdline content
///
/// Format: arguments separated by null bytes, read from the TARGET process's
/// user memory via its own page tables (review 5.7: was read through the
/// caller's address space — garbage for other processes).
pub fn generate_cmdline(pid: u64) -> Vec<u8> {
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

    let arg_start = addr_space.arg_start();
    let arg_end = addr_space.arg_end();
    if arg_start == 0 || arg_end == 0 || arg_end <= arg_start {
        return Vec::new();
    }

    let arg_len = core::cmp::min(arg_end - arg_start, 64 * 1024);
    read_target_user_mm(&addr_space, arg_start, arg_len)
}

/// Generate /proc/[pid]/stat content
///
/// Format: (pid) (comm) (state) (ppid) (pgrp) (session) (tty_nr) (tpgid) ...
/// 52 fields total, matching the format of /proc/[pid]/stat.
pub fn generate_stat(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let (name_str, ppid, state_ch, vsize_pages, sig_mask, exit_code) = match task {
        Some(t) => {
            let ch = task_state_char(&t) as char;
            let vsize = t.address_space().map(|mm| mm.total_vm()).unwrap_or(0);
            let sigmask = t.sigmask;
            let ec = t.exit_code();
            // comm (basename, like Linux), not the full exe path. Owned:
            // the Cow would borrow from the match-local binding.
            (task_comm(&t).into_owned(), t.ppid(), ch, vsize, sigmask, ec)
        }
        None => (alloc::string::String::from("unknown"), 0, 'X', 0, 0, 0),
    };

    // Key fields filled: pid(1), comm(2), state(3), ppid(4), pgrp(5), session(6),
    // vsize(23) in bytes, signal block mask(30), exit_code(52).
    // Fields 7-22 and 24-29,31-51 remain 0 — filled as accounting infrastructure grows.
    let vsize_bytes = vsize_pages * 4096;
    let content = format!(
        "{} ({}) {} {} {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {}\n",
        pid,
        name_str,
        state_ch,
        ppid,
        pid,  // pgrp = pid
        pid,  // session = pid
        vsize_bytes,
        sig_mask,
        exit_code,
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
    if name_str.starts_with('/') {
        name.to_vec()
    } else {
        format!("/{}", name_str).into_bytes()
    }
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
///
/// Read through the TARGET's page tables (review 5.7) and gated behind a
/// ptrace_may_access-style permission check (self or CAP_SYS_PTRACE) —
/// unauthorized readers get an empty buffer; sys_openat's /proc shortcut
/// additionally fails the open with EACCES.
pub fn generate_environ(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    if !environ_access_allowed(pid) {
        return Vec::new();
    }

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

    let env_len = core::cmp::min(env_end - env_start, 64 * 1024);
    read_target_user_mm(&addr_space, env_start, env_len)
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
    for fd in 0..crate::fs::file::MAX_FDS {
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
