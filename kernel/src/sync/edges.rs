//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sync subsystem's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L1 (2026-11-20), framekernel doctrine: the sync
//! POLICY files (futex.rs, futex_pi.rs) are safe by construction; the
//! genuinely-unsafe memory edges they need live here with INV-SY
//! contracts. Task pointer→reference derivations reuse
//! [`crate::sched::edges::task_shared`] (same INV-SCE-2 contract).
//!
//! INV registry:
//! - INV-SY-1 `*_via_task` — cross-task USER-memory diagnostic reads:
//!   walk the (foreign) task's page tables, translate, read volatile.
//!   `task` is live (walker pin / GRQ / census caller); the reads are
//!   diagnostic (racy by nature, exactly like the raw reads replaced);
//!   the mapped user page cannot be unmapped for the duration of the
//!   single volatile read that follows the walk (the owning task is
//!   parked on the futex being diagnosed).
//! - INV-SY-2 `entry_frame_*` — the task's trap frame at the top of
//!   its own kernel stack (the pt_regs window), read-only: same layout
//!   contract as `Task::pt_regs_ref`.
//! - INV-SY-3 `word_read`/`word_cas` — the futex user word through the
//!   arch uaccess accessors (fault-isolated); caller holds the hash
//!   bucket that serializes kernel-side accessors of the word.

use crate::process::task::Task;

// ---------------------------------------------------------------------------
// INV-SY-1: cross-task user-memory diagnostic reads
// ---------------------------------------------------------------------------

/// Read up to `buf.len()` bytes in `task`'s user address space via its
/// page tables, stopping at the first unmapped page (KILLDUMP sc97:
/// iovec arrays, path strings, log payload heads of the wedged task).
/// Returns how many bytes were actually read.
pub fn read_bytes_via_task(task: *const Task, va: usize, buf: &mut [u8]) -> usize {
    let tk = crate::sched::edges::task_shared(task);
    let aspace = match tk.address_space() {
        Some(a) => a,
        None => return 0,
    };
    let root = aspace.root_ppn();
    let mut done = 0usize;
    while done < buf.len() {
        let cur = va + done;
        // SAFETY: INV-SY-1 — the page-table walk is read-only.
        let (ppn, _) = match unsafe {
            crate::arch::mm::mm_ops::PageTableWalker::walk(root, cur as u64)
        } {
            Some(p) => p,
            None => break,
        };
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64))
            .bits() as usize
            + (cur & 0xfff);
        let chunk = core::cmp::min(buf.len() - done, 0x1000 - (cur & 0xfff));
        for i in 0..chunk {
            // SAFETY: INV-SY-1 — one volatile byte read of the mapped
            // user page the walk just resolved.
            buf[done + i] = unsafe { core::ptr::read_volatile((kv + i) as *const u8) };
        }
        done += chunk;
    }
    done
}

/// Printable snapshot of a NUL-terminated user string in `task`'s space.
pub fn read_cstr_via_task(task: *const Task, va: usize, max: usize) -> alloc::string::String {
    if va == 0 {
        return alloc::string::String::from("(null)");
    }
    let mut raw = alloc::vec![0u8; max];
    let n = read_bytes_via_task(task, va, &mut raw);
    let end = raw[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut s = alloc::string::String::new();
    for &b in &raw[..end] {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push('.');
        }
    }
    s
}

/// Read one u32 in `task`'s user address space via its page tables
/// (khungtaskd's get_user would read through the wrong mm).
pub fn read_u32_via_task(task: *const Task, va: usize) -> Option<u32> {
    let tk = crate::sched::edges::task_shared(task);
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    // SAFETY: INV-SY-1 — read-only page-table walk.
    let (ppn, _) = unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va as u64)? };
    let pa = (ppn as usize) << 12;
    let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
        as usize
        + (va & 0xfff);
    // SAFETY: INV-SY-1 — one volatile u32 read of the mapped page.
    Some(unsafe { core::ptr::read_volatile(kv as *const u32) })
}

// ---------------------------------------------------------------------------
// INV-SY-3: the futex user word
// ---------------------------------------------------------------------------

/// Read the futex user word at `uaddr` (fault-isolated; `None` on a bad
/// pointer). Caller holds the hash bucket that serializes kernel-side
/// accessors of the word (the plain side's WAKE_OP contract).
pub fn word_read(uaddr: usize) -> Option<u32> {
    // SAFETY: INV-SY-3 — uaddr came from the futex syscall's validated
    // user pointer; get_user is fault-isolated.
    unsafe { crate::arch::uaccess::get_user(uaddr as *const u32) }
}

/// Compare-and-swap the user word: writes `new` only when it reads
/// `expect`. Returns the observed pre-value; callers compare against
/// `expect` to learn whether the swap happened.
pub fn word_cas(uaddr: usize, expect: u32, new: u32) -> Option<u32> {
    let cur = word_read(uaddr)?;
    // SAFETY: INV-SY-3 — the fault-isolated store; a `false` return
    // (write faulted) is treated as a lost CAS.
    if cur == expect && !unsafe { crate::arch::uaccess::put_user(uaddr as *mut u32, new) } {
        return None;
    }
    Some(cur)
}
