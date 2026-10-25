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

/// The task's comm: Linux reads task->comm — set at exec (binary
/// basename), copied at fork, overwritten by prctl(PR_SET_NAME). Thread
/// names (OH's OS_IPC_* workers) live in the same field; deriving comm
/// from the exe path hid every thread's name. Falls back to the exe
/// basename / kthread name when Task.comm is empty (early tasks).
/// Truncated to 15 characters (Linux TASK_COMM_LEN - 1). Used for
/// /proc/[pid]/comm, the "Name:" line of /proc/[pid]/status and field 2
/// of /proc/[pid]/stat — systemd's PID 1 check compares /proc/1/comm
/// against "systemd". Kernel threads (no exe) report their kthread name
/// like Linux.
fn task_comm(task: &crate::process::Task) -> alloc::borrow::Cow<'_, str> {
    let comm = task.comm();
    let nul = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
    if nul > 0 {
        // Truncate to 15 bytes (TASK_COMM_LEN - 1) without splitting a
        // multi-byte UTF-8 sequence: shrink until the prefix is valid.
        let mut len = nul.min(15);
        while len > 0 {
            if let Ok(s) = core::str::from_utf8(&comm[..len]) {
                return alloc::borrow::Cow::Borrowed(s);
            }
            len -= 1;
        }
    }
    let path = task.get_exe_path();
    if path.is_empty() {
        if let Some(name) = crate::process::kthread::kthread_name(task.pid()) {
            let mut len = name.len().min(15);
            while len > 0 && !name.is_char_boundary(len) {
                len -= 1;
            }
            return alloc::borrow::Cow::Borrowed(&name[..len]);
        }
    }
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


/// Pinned foreign-task guard for procfs generators: the /proc walkers hold
/// raw &Task/&MmStruct borrows across multi-line (sometimes multi-second)
/// reads; a concurrent exit frees that memory under the walker (the
/// exitwalk #GP family). The guard pins the task slot (reaper cannot free
/// while pinned) and releases on scope end — including every early return.
struct PinnedTask {
    ptr: *mut crate::process::task::Task,
}
impl PinnedTask {
    fn lookup(pid: u32) -> Option<PinnedTask> {
        let p = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
        if p.is_null() { None } else { Some(PinnedTask { ptr: p }) }
    }
    fn get(&mut self) -> &'static mut crate::process::task::Task {
        // SAFETY: pinned at construction; released only in Drop. The
        // 'static lifetime mirrors find_task_by_pid's contract — the pin
        // extends the slot's validity for the whole borrow.
        unsafe { &mut *self.ptr }
    }
}
impl core::ops::Drop for PinnedTask {
    fn drop(&mut self) {
        crate::process::task::Task::task_put(self.ptr);
    }
}

/// Generate /proc/[pid]/comm content (U2): "<comm>\n".
pub fn generate_comm(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
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
    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
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
    // Real VM counters (pages → kB). VmLck sums whole VMAs carrying
    // VM_LOCKED — our mlock pins at VMA granularity, so the locked size
    // is the locked VMA span (LTP mmap14 mlocks a 1MB mapping and reads
    // VmLck back). The old hardcoded zeros made every Vm* report 0 kB.
    let (vm_size_kb, vm_rss_kb, vm_lck_kb) = {
        let page_kb = (crate::mm::page::PAGE_SIZE / 1024) as u64;
        let aspace = task.address_space_arc();
        let (total, rss) = match &aspace {
            Some(a) => (a.total_vm(), a.rss()),
            None => (0, 0),
        };
        let lck_pages: u64 = match &aspace {
            Some(a) => a
                .vma_read()
                .iter()
                .filter(|v| v.flags().contains(crate::mm::vma::VmaFlags::LOCKED))
                .map(|v| (v.end().as_usize() - v.start().as_usize()) as u64)
                .sum::<u64>()
                / crate::mm::page::PAGE_SIZE as u64,
            None => 0,
        };
        (total * page_kb, rss * page_kb, lck_pages * page_kb)
    };
    content.push_str(&format!("VmPeak:\t{} kB\n", vm_size_kb));
    content.push_str(&format!("VmSize:\t{} kB\n", vm_size_kb));
    content.push_str(&format!("VmLck:\t{} kB\n", vm_lck_kb));
    content.push_str(&format!("VmRSS:\t{} kB\n", vm_rss_kb));
    content.push_str("VmData:\t0 kB\n");
    content.push_str("VmStk:\t0 kB\n");
    content.push_str("VmExe:\t0 kB\n");
    content.push_str("VmLib:\t0 kB\n");
    // No swap on Rux: report 0 like non-swapping Linux boxes. LTP
    // getrusage03 scans this line after each mlockall+touch round.
    content.push_str("VmSwap:\t0 kB\n");

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
    // Capability sets (real values — libcap-ng's capng_get_caps_process()
    // parses CapBnd from here; all-zero values made capng_change_id() fail
    // and killed dbus-daemon startup).
    content.push_str(&format!("CapInh:\t{:016x}\n", cred.cap_inheritable.bits()));
    content.push_str(&format!("CapPrm:\t{:016x}\n", cred.cap_permitted.bits()));
    content.push_str(&format!("CapEff:\t{:016x}\n", cred.cap_effective.bits()));
    content.push_str(&format!("CapBnd:\t{:016x}\n", cred.cap_bounding.bits()));
    content.push_str("Seccomp:\t0\n");

    // Context-switch counters (Linux /proc/[pid]/status tail): a
    // voluntary switch is one where the task went to sleep, an
    // involuntary one where it was preempted while still runnable.
    content.push_str(&format!(
        "voluntary_ctxt_switches:\t{}\n",
        task.nvcsw.load(core::sync::atomic::Ordering::Relaxed)
    ));
    content.push_str(&format!(
        "nonvoluntary_ctxt_switches:\t{}\n",
        task.nivcsw.load(core::sync::atomic::Ordering::Relaxed)
    ));

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
    use crate::arch::mm::{phys_to_virt, PhysAddr};

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

    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let addr_space = match task.address_space_arc() {
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
/// /proc/[pid]/schedstat — the per-task scheduler statistics triple
/// (Documentation/filesystems/proc.rst): "time spent on the cpu (in
/// nanoseconds), time spent waiting on a runqueue (in ns), # of
/// timeslices run on this cpu" — one line, three space-separated
/// numbers. The counters are maintained by the scheduler's schedstat
/// edges (sched/stats.rs gates them; /proc/sys/kernel/sched_schedstats
/// is the runtime knob, default off like the reference's). With the
/// gate off the file still renders — zeros, the honest value of never
/// having been counted.
pub fn generate_schedstat(pid: u64) -> Vec<u8> {
    use crate::process::{current_pid, current_task};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        match PinnedTask::lookup(pid as u32) {
            Some(mut g) => Some(g.get()),
            None => return Vec::new(),
        }
    };
    let Some(task) = task else { return Vec::new() };
    use core::sync::atomic::Ordering::Relaxed;
    alloc::format!(
        "{} {} {}\n",
        task.schedstat_runtime_ns.load(Relaxed),
        task.schedstat_wait_ns.load(Relaxed),
        task.schedstat_timeslices.load(Relaxed)
    )
    .into_bytes()
}

pub fn generate_stat(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};
    use alloc::string::ToString;

    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
    };

    let (name_str, ppid, state_ch, vsize_pages, rss_pages, sig_mask, exit_code,
         utime, stime, cutime, cstime, priority, nice, threads, processor, policy,
         min_flt, maj_flt, start_jiffies) = match task {
        Some(t) => {
            let ch = task_state_char(&t) as char;
            let vsize = t.address_space_arc().map(|mm| mm.total_vm()).unwrap_or(0);
            // Linux reports rss in PAGES (field 24); vsize stays in bytes.
            let rss = t.address_space_arc().map(|mm| mm.rss()).unwrap_or(0);
            let sigmask = t.sigmask;
            let ec = t.exit_code();
            use core::sync::atomic::Ordering::Relaxed;
            let ticks = (
                t.utime_ticks.load(Relaxed),
                t.stime_ticks.load(Relaxed),
                t.cutime_ticks.load(Relaxed),
                t.cstime_ticks.load(Relaxed),
            );
            // Linux's prio field is the static priority of a normal task
            // (120 - nice); a real-time task carries its RT priority in
            // field 40 instead.
            let nice = t.nice();
            let prio = 120 - nice;
            let threads = {
                let leader = unsafe { &*t.group_leader_ptr() };
                leader.nr_threads().max(1)
            };
            // SchedPolicy already uses Linux's SCHED_* numbering
            // (SCHED_OTHER=0, FIFO=1, RR=2, BATCH=3, IDLE=5).
            let policy = t.policy() as i32;
            let processor = t.ti_cpu();
            // comm (basename, like Linux), not the full exe path. Owned:
            // the Cow would borrow from the match-local binding.
            let faults = (
                t.min_flt.load(Relaxed),
                t.maj_flt.load(Relaxed),
                t.start_jiffies.load(Relaxed),
            );
            (task_comm(&t).into_owned(), t.ppid(), ch, vsize, rss, sigmask, ec,
             ticks.0, ticks.1, ticks.2, ticks.3, prio, nice, threads, processor, policy,
             faults.0, faults.1, faults.2)
        }
        None => (alloc::string::String::from("unknown"), 0, 'X', 0, 0, 0, 0,
                 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
    };

    // Linux's /proc/[pid]/stat has exactly 52 fields (verified against a
    // Linux guest: `awk '{print NF}' /proc/self/stat` → 52). The list is
    // built field by field here so the numbering stays visible and
    // checkable — a mistyped format string silently shifts every field
    // after it.
    //
    // Filled from the kernel: pid(1), comm(2), state(3), ppid(4),
    // pgrp(5), session(6), utime(14)/stime(15) (own user/system CPU in
    // ticks, sampled in the timer IRQ), cutime(16)/cstime(17) (reaped
    // children's CPU folded in at their exit), priority(18)/nice(19),
    // num_threads(20), vsize(23) in bytes, rss(24) in pages, signal
    // block mask(31), processor(39), policy(41), exit_code(52).
    // The rest report 0 until the matching accounting exists.
    let vsize_bytes = vsize_pages * 4096;
    let mut fields: Vec<String> = Vec::with_capacity(52);
    fields.push(pid.to_string()); // 1  pid
    fields.push(format!("({})", name_str)); // 2  comm
    fields.push(state_ch.to_string()); // 3  state
    fields.push(ppid.to_string()); // 4  ppid
    fields.push(pid.to_string()); // 5  pgrp
    fields.push(pid.to_string()); // 6  session
    fields.push("0".into()); // 7  tty_nr
    fields.push("0".into()); // 8  tpgid
    fields.push("0".into()); // 9  flags
    fields.push(min_flt.to_string()); // 10 minflt
    fields.push("0".into()); // 11 cminflt
    fields.push(maj_flt.to_string()); // 12 majflt
    fields.push("0".into()); // 13 cmajflt
    fields.push(utime.to_string()); // 14 utime
    fields.push(stime.to_string()); // 15 stime
    fields.push(cutime.to_string()); // 16 cutime
    fields.push(cstime.to_string()); // 17 cstime
    fields.push(priority.to_string()); // 18 priority
    fields.push(nice.to_string()); // 19 nice
    fields.push(threads.to_string()); // 20 num_threads
    fields.push("0".into()); // 21 itrealvalue
    fields.push(start_jiffies.to_string()); // 22 starttime
    fields.push(vsize_bytes.to_string()); // 23 vsize
    fields.push(rss_pages.to_string()); // 24 rss
    fields.push("0".into()); // 25 rsslim
    fields.push("0".into()); // 26 startcode
    fields.push("0".into()); // 27 endcode
    fields.push("0".into()); // 28 startstack
    fields.push("0".into()); // 29 kstkesp
    fields.push("0".into()); // 30 kstkeip
    fields.push(sig_mask.to_string()); // 31 signal
    fields.push("0".into()); // 32 blocked
    fields.push("0".into()); // 33 sigignore
    fields.push("0".into()); // 34 sigcatch
    fields.push("0".into()); // 35 wchan
    fields.push("0".into()); // 36 nswap
    fields.push("0".into()); // 37 cnswap
    fields.push("0".into()); // 38 exit_signal
    fields.push(processor.to_string()); // 39 processor
    fields.push("0".into()); // 40 rt_priority
    fields.push(policy.to_string()); // 41 policy
    fields.push("0".into()); // 42 delayacct_blkio_ticks
    fields.push("0".into()); // 43 guest_time
    fields.push("0".into()); // 44 cguest_time
    fields.push("0".into()); // 45 start_data
    fields.push("0".into()); // 46 end_data
    fields.push("0".into()); // 47 start_brk
    fields.push("0".into()); // 48 arg_start
    fields.push("0".into()); // 49 arg_end
    fields.push("0".into()); // 50 env_start
    fields.push("0".into()); // 51 env_end
    fields.push(exit_code.to_string()); // 52 exit_code

    let mut content = fields.join(" ");
    content.push('\n');
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

    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let addr_space = match task.address_space_arc() {
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

/// Generate /proc/[pid]/smaps content
///
/// Per-VMA block: the maps-style header line followed by the size
/// counters. LTP mlock05 locates its mapping by the header's starting
/// address and reads the Rss/Locked fields — Rss counts RESIDENT pages
/// (PTE walk), Locked reports the VM_LOCKED span of the VMA (mlock now
/// splits VMAs range-precisely, so this equals the locked bytes).
pub fn generate_smaps(pid: u64) -> Vec<u8> {
    let maps = generate_maps(pid);
    let task = if crate::process::current_pid() as u64 == pid {
        crate::process::current_task()
    } else {
        crate::process::find_task_by_pid(pid as u32)
    };
    let task = match task {
        Some(t) => t,
        None => return maps,
    };
    let addr_space = match task.address_space_arc() {
        Some(a) => a,
        None => return maps,
    };
    let page_kb = (crate::mm::page::PAGE_SIZE / 1024) as u64;

    // maps lines are VMA-ordered; append the counter block after each.
    let mut out = String::new();
    let header_re = re_header();
    for line in maps.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let line_str = String::from_utf8_lossy(line);
        out.push_str(&line_str);
        out.push('\n');
        if let Some((start, end)) = header_re(&line_str) {
            let size_kb = ((end - start) / 1024) as u64;
            // Resident pages: walk the PTEs of the VMA span.
            let mut rss_pages: u64 = 0;
            let root = addr_space.root_ppn();
            let mut p = start & !(crate::mm::page::PAGE_SIZE - 1);
            while p < end {
                // SAFETY: read-only walk of this task's own page tables.
                if unsafe {
                    crate::arch::mm::mm_ops::PageTableWalker::walk(root, p as u64)
                }
                .is_some()
                {
                    rss_pages += 1;
                }
                p += crate::mm::page::PAGE_SIZE;
            }
            let locked_kb = {
                let mgr = addr_space.vma_read();
                let mut lck = 0u64;
                for v in mgr.iter() {
                    if v.start().as_usize() == start
                        && v.flags().contains(crate::mm::vma::VmaFlags::LOCKED)
                    {
                        lck = ((v.end().as_usize() - v.start().as_usize()) / 1024) as u64;
                        break;
                    }
                }
                lck
            };
            out.push_str(&format!("Size:       {} kB\n", size_kb));
            out.push_str(&format!("Rss:        {} kB\n", rss_pages * page_kb));
            out.push_str(&format!("Pss:        {} kB\n", rss_pages * page_kb));
            out.push_str("Shared_Clean:       0 kB\n");
            out.push_str("Shared_Dirty:       0 kB\n");
            out.push_str("Private_Clean:      0 kB\n");
            out.push_str("Private_Dirty:      0 kB\n");
            out.push_str("Referenced:         0 kB\n");
            out.push_str("Anonymous:          0 kB\n");
            out.push_str("LazyFree:           0 kB\n");
            out.push_str(&format!("AnonHugePages:      0 kB\n"));
            out.push_str("ShmemPmdMapped:     0 kB\n");
            out.push_str("FilePmdMapped:      0 kB\n");
            out.push_str("Shared_Hugetlb:     0 kB\n");
            out.push_str("Private_Hugetlb:    0 kB\n");
            out.push_str("Swap:               0 kB\n");
            out.push_str("SwapPss:            0 kB\n");
            out.push_str("KernelPageSize:     4 kB\n");
            out.push_str("MMUPageSize:        4 kB\n");
            out.push_str(&format!("Locked:             {} kB\n", locked_kb));
            out.push_str("THPeligible:        0\n");
            out.push_str("ProtectionKey:      0\n");
            out.push_str("VmFlags: rd ex mr mw me sd\n");
        }
    }
    out.into_bytes()
}

/// Parse "START-END ..." from a maps header line (hex, unpadded).
fn re_header() -> fn(&str) -> Option<(usize, usize)> {
    |line: &str| -> Option<(usize, usize)> {
        let dash = line.find('-')?;
        let sp = line[dash..].find(' ')? + dash;
        let start = usize::from_str_radix(&line[..dash], 16).ok()?;
        let end = usize::from_str_radix(&line[dash + 1..sp], 16).ok()?;
        Some((start, end))
    }
}

/// Generate /proc/[pid]/maps content
///
/// Format: start-end perms offset dev inode pathname
/// e.g.: 00010000-00020000 r-xp 00000000 00:00 0 [exe]
pub fn generate_maps(pid: u64) -> Vec<u8> {
    use crate::process::{current_task, current_pid, find_task_by_pid};
    use crate::mm::vma::{VmaFlags, VmaType};

    let mut pinned;
    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        pinned = match PinnedTask::lookup(pid as u32) {
            Some(mut g) => g,
            None => return Vec::new(),
        };
        Some(pinned.get())
    };

    let task = match task {
        Some(t) => t,
        None => return Vec::new(),
    };

    let addr_space = match task.address_space_arc() {
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
            // Linux resolves the maps path through vm_file — the file
            // pinned on the VMA, which follows it across fd close, fork
            // and MAP_FIXED splits. Try the pin first (exec-time segment
            // VMAs carry fd=-1 with the identity on the pin; loader
            // mappings outlive their fd once ld.so closes it), then fall
            // back to the caller's fdtable for VMAs that only know the
            // fd number.
            let mut resolved: Option<(String, u64)> = None;
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace)
                && addr_space.get_vma_file(start).is_none()
            {
                crate::pr_info!(
                    "LVMA-DBG maps pin-miss start={:#x} fd={} off={:#x}",
                    start, vma.file_fd(), vma.offset()
                );
            }
            if let Some(file) = addr_space.get_vma_file(start) {
                // SAFETY: read-only dentry access on the pinned file.
                let dentry_opt = unsafe { &*file.dentry.get() };
                if let Some(dentry) = dentry_opt {
                    // d_path()-style: relative to the READING task's root.
                    let path = crate::fs::vfs::dentry_user_path(dentry);
                    let ino = dentry
                        .get_inode()
                        .map(|inode| inode.ino)
                        .unwrap_or(0);
                    resolved = Some((path, ino));
                }
            }
            if resolved.is_none() {
                let fd = vma.file_fd();
                if fd >= 0 {
                    // Look up file from fdtable to get path and inode
                    // A pinned-but-exiting task may have torn its fdtable
                    // down already; maps paths then show no path (Linux
                    // shows the empty path for such VMAs too).
                    let file = match task.try_fdtable() {
                        Some(ft) => unsafe { ft.get_file(fd as usize) },
                        None => None,
                    };
                    if let Some(file) = file {
                        // SAFETY: read-only dentry access on the fdtable file.
                        let dentry_opt = unsafe { &*file.dentry.get() };
                        if let Some(dentry) = dentry_opt {
                            // /proc/<pid>/maps paths are d_path()-style:
                            // relative to the READING task's root.
                            let path =
                                crate::fs::vfs::dentry_user_path(dentry);
                            let ino = dentry.get_inode()
                                .map(|inode| inode.ino)
                                .unwrap_or(0);
                            resolved = Some((path, ino));
                        }
                    }
                }
            }
            match resolved {
                Some(r) => r,
                None => (String::new(), 0),
            }
        } else {
            (String::new(), 0)
        };

        // Linux prints maps addresses UNPADDED (%lx, no zero fill) —
        // the zero-padded form broke scanf prefix matching in userspace
        // parsers (LTP mmap04 builds its pattern with PRIxPTR and got 0
        // conversions).
        if pathname.is_empty() {
            content.push_str(&format!(
                "{:x}-{:x} {}{}{}{} {:08x} 00:00 {} \n",
                start, end, r, w, x, s, offset, inode
            ));
        } else {
            content.push_str(&format!(
                "{:x}-{:x} {}{}{}{} {:08x} 00:00 {} {}\n",
                start, end, r, w, x, s, offset, inode, pathname
            ));
        }
    }

    content.into_bytes()
}

/// Fetch the `File` backing a descriptor of a task.
///
/// Shared backend for the /proc/[pid]/fd magic-link machinery: readlink
/// content (generate_fd_link), fd-dir lookup existence checks, and the
/// VFS magic-link redirect (follow_symlink) all resolve through here so
/// they can never disagree about whether a descriptor exists.
pub fn get_fd_file(pid: u64, fd: u32) -> Option<alloc::sync::Arc<crate::fs::File>> {
    use crate::process::{current_task, current_pid, find_task_by_pid};

    let task = if current_pid() as u64 == pid {
        current_task()
    } else {
        find_task_by_pid(pid as u32)
    };

    let task = task?;
    let fdtable = task.try_fdtable()?;
    fdtable.get_file(fd as usize)
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
        // /proc/<pid>/fd/N links are d_path()-style: relative to the
        // READING task's root (Linux readlink on these uses the reader's
        // fs->root), so a chrooted reader sees jail-relative names.
        Some(dentry) => crate::fs::vfs::dentry_user_path(dentry),
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

// ============================================================================
// /proc/[pid]/attr/* — SELinux label files (OH Phase 2)
// ============================================================================

/// Per-process SELinux labels written through /proc/[pid]/attr/exec and
/// /proc/[pid]/attr/current. Rux has no SELinux enforcement, but OH's
/// init *writes* the service context through libselinux's setexeccon()
/// (open+write+close on /proc/self/attr/exec) before exec'ing every
/// service — ENOENT there made init abort the child with
/// INIT_EEXEC_CONTENT, so composer_host/render_service/bootanimation
/// never started. The stored strings are just kept readable back.
use alloc::collections::BTreeMap;
use crate::sync::spinlock::Spinlock;

/// Attr kind indices (match ATTR_FILE_INO_OFF layout in mod.rs).
pub const ATTR_EXEC: usize = 0;
pub const ATTR_CURRENT: usize = 1;
pub const ATTR_FSCREATE: usize = 2;
pub const ATTR_SOCKCREATE: usize = 3;
pub const ATTR_PREV: usize = 4;
pub const ATTR_KEYCREATE: usize = 5;

/// Labels per pid: exec/current/fscreate/sockcreate/keycreate slots
/// (prev is synthesized as empty — see attr_get).
static ATTR_LABELS: Spinlock<BTreeMap<u32, [Option<alloc::vec::Vec<u8>>; 6]>> =
    Spinlock::new(BTreeMap::new());

/// Trim NUL/newline separators libselinux appends.
fn trim_label(buf: &[u8]) -> alloc::vec::Vec<u8> {
    let end = buf
        .iter()
        .position(|&b| b == 0 || b == b'\n')
        .unwrap_or(buf.len());
    buf[..end].to_vec()
}

/// Read one label slot. Empty content is valid (Linux: unset → empty
/// read — this exact case is what libselinux probes for).
///
/// `ATTR_CURRENT` falls back to `ATTR_EXEC` when the user has not
/// written `current` directly. Linux's `selinux_bprm_set_creds()` does
/// this implicitly at exec time (copies the stored exec label into the
/// new task's current label); Rux has no LSM hook to do that, so the
/// read path has to remember instead. OH's first-stage init calls
/// setexeccon() then exec()'s them — without this fallback every
/// service child reads empty from /proc/self/attr/current,
/// libselinux's getcon() returns "" and the SA gate's `callingSid`
/// comes back empty, so `Unknown class samgr_class` + `deny_unknown`
/// still fail closed (the SAMGR side cannot recover a SID it never
/// sees). Explicit writes to current (Some(empty) after a clear) win
/// over the fallback — only the unset slot triggers it.
/// The label reported for a task that never stored one. An
/// SELinux-enabled Linux owns every task's label — reading
/// /proc/self/attr/current NEVER returns empty there. libselinux's
/// getprocattrcon_raw maps an empty read to `*context = NULL` while
/// still returning SUCCESS (procattr.c: `if (ret == 0) { *context =
/// NULL; goto out2; }`), and OH's HapDomainSetcontext (appspawn's
/// pre-exec SetSelinuxCon) passes that NULL straight into
/// context_new(), which segfaults — this took down every appspawn
/// child (systemui/launcher/settingsdata) right after the
/// "SetSelinuxCon IsDeveloperModeOn" log in sc49/sc55. appspawn's
/// forked children inherit no slot (their ATTR_EXEC fallback is unset
/// too), so they hit exactly this path. A well-formed unlabeled
/// context keeps the parser alive; HapDomainSetcontext then rewrites
/// the type from sehap_contexts and setcon()s it.
const DEFAULT_CURRENT_LABEL: &[u8] = b"u:r:unlabeled:s0";

pub fn attr_get(pid: u64, kind: usize) -> Vec<u8> {
    if kind == ATTR_PREV {
        return Vec::new();
    }
    let map = ATTR_LABELS.lock_irqsave();
    let slots = match map.get(&(pid as u32)) {
        Some(s) => s,
        None => {
            // Only `current` must never read empty; the other slots keep
            // the unset->empty semantics libselinux probes rely on
            // (getexeccon/getsockcreatecon treat an empty slot as "no
            // label set", a valid state).
            if kind == ATTR_CURRENT {
                return DEFAULT_CURRENT_LABEL.to_vec();
            }
            return Vec::new();
        }
    };
    let primary = slots.get(kind).cloned().flatten();
    if kind == ATTR_CURRENT {
        match primary {
            Some(v) => v, // explicit current write (incl. empty) wins
            None => {
                let exec = slots.get(ATTR_EXEC).cloned().flatten().unwrap_or_default();
                if exec.is_empty() {
                    DEFAULT_CURRENT_LABEL.to_vec()
                } else {
                    exec
                }
            }
        }
    } else {
        primary.unwrap_or_default()
    }
}

/// Write one label slot (procfs_file_write path).
pub fn attr_set(pid: u64, kind: usize, buf: &[u8]) {
    if kind >= 6 {
        return;
    }
    let label = trim_label(buf);
    let mut map = ATTR_LABELS.lock_irqsave();
    let entry = map.entry(pid as u32).or_insert([None, None, None, None, None, None]);
    entry[kind] = Some(label);
}

/// Legacy bool-flavored accessor (exec vs current) for existing callers.
pub fn attr_label_get(pid: u64, exec: bool) -> Vec<u8> {
    attr_get(pid, if exec { ATTR_EXEC } else { ATTR_CURRENT })
}

/// Write one label file (procfs_file_write path).
pub fn attr_label_set(pid: u64, exec: bool, buf: &[u8]) {
    attr_set(pid, if exec { ATTR_EXEC } else { ATTR_CURRENT }, buf);
}

/// Generate /proc/[pid]/fdinfo/<fd> content (fdinfo(5)).
///
/// Generic header (pos/flags/mnt_id) plus per-descriptor-type lines; the
/// inotify table carries one "inotify wd:..." record per watch — LTP
/// inotify12 parses the mask back out of it.
pub fn generate_fdinfo(pid: u64, fd: u32) -> Vec<u8> {
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

    let file = match fdtable.get_file(fd as usize) {
        Some(f) => f,
        None => return Vec::new(),
    };

    let mut out = alloc::string::String::new();
    out.push_str(&alloc::format!("pos:\t{}\n", file.get_pos()));
    out.push_str(&alloc::format!("flags:\t{:o}\n", file.flags_bits()));
    out.push_str("mnt_id:\t0\n");

    if let Some(watches) = crate::fs::inotify::fdinfo_watches(&file) {
        for (wd, sdev, ino, mask) in watches {
            out.push_str(&alloc::format!(
                "inotify wd:{:x} ino:{:x} sdev:{:x} mask:{:x} ignored_mask:0 \n",
                wd, ino, sdev, mask
            ));
        }
    }

    out.into_bytes()
}
