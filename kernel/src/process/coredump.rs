//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Core dump support (P1: crash diagnostics).
//!
//! When a fatal signal whose default action is "core dump" terminates a
//! process, `do_coredump()` writes an ELF core file ("core" in the dying
//! task's current working directory — /proc/sys/kernel/core_pattern is not
//! implemented, the name is fixed) containing:
//!
//! - ELF64 header (ET_CORE, EM_RISC-V)
//! - one PT_NOTE segment with NT_PRSTATUS (full GP register set + pid
//!   lineage) and NT_PRPSINFO (comm)
//! - one PT_LOAD segment per readable VMA, dumped through the target's
//!   page tables (present pages carry their contents, non-present pages
//!   are zero-filled)
//!
//! The dump is planned in two passes: segment file offsets and sizes are
//! computed first (under the RLIMIT_CORE budget) so the program headers
//! describe the file exactly, then the header block and the segment
//! contents are streamed through the created file.
//!
//! `signal_makes_core()` lists the core-dumping signals. On success the
//! task is marked `core_dumped`, which wait4 encodes as WCOREDUMP (0x80)
//! in the wstatus word.

use crate::fs::elf::{Elf64Ehdr, Elf64Phdr};
use crate::fs::file::FileFlags;
use crate::process::task::{rlimit_res, Task};
use alloc::vec::Vec;

/// ELF core file type.
const ET_CORE: u16 = 4;
/// RISC-V machine type.
const EM_RISCV: u16 = 243;

/// NT_PRSTATUS note type.
const NT_PRSTATUS: u32 = 1;
/// NT_PRPSINFO note type.
const NT_PRPSINFO: u32 = 3;

/// Core dump I/O chunk (one page).
const CHUNK: usize = 4096;

/// Signals whose default action produces a core dump (signal(7) subset —
/// the hardware-fault and abort family).
pub fn signal_makes_core(sig: i32) -> bool {
    matches!(
        sig,
        3 |  // SIGQUIT
        4 |  // SIGILL
        6 |  // SIGABRT
        7 |  // SIGBUS
        8 |  // SIGFPE
        11   // SIGSEGV
    )
}

// ==================== little-endian builders ====================

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn push_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// One note entry: Elf64_Nhdr { namesz, descsz, type }, "CORE\0" name
/// (padded to 4) and the descriptor (padded to 4).
fn push_note(out: &mut Vec<u8>, n_type: u32, desc: &[u8]) {
    push_u32(out, 5); // namesz: "CORE\0"
    push_u32(out, desc.len() as u32);
    push_u32(out, n_type);
    out.extend_from_slice(b"CORE\0");
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out.extend_from_slice(desc);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn push_phdr(out: &mut Vec<u8>, p: &Elf64Phdr) {
    push_u32(out, p.p_type);
    push_u32(out, p.p_flags);
    push_u64(out, p.p_offset);
    push_u64(out, p.p_vaddr);
    push_u64(out, p.p_paddr);
    push_u64(out, p.p_filesz);
    push_u64(out, p.p_memsz);
    push_u64(out, p.p_align);
}

// ==================== note descriptors ====================

/// Build the NT_PRSTATUS descriptor for the dying task.
///
/// Layout (glibc elf_prstatus, 64-bit):
///   0   elf_siginfo { si_signo, si_code, si_errno }   3 × i32
///   12  pr_cursig (i16 + 2 pad)
///   16  pr_pid, pr_ppid, pr_pgrp, pr_sid              4 × i32
///   32  pr_utime, pr_stime, pr_cutime, pr_cstime      4 × timeval(16)
///   96  pr_reg (elf_gregset_t: 32 × u64 = 256)
///   352 pr_fpvalid (i32) + 4 pad                      -> descsz 360
///
/// # Safety
/// `task` is the current, dying task; its trap frame is quiescent.
unsafe fn build_prstatus(task: *mut Task, sig: i32) -> Vec<u8> {
    let mut d = Vec::with_capacity(360);
    push_u32(&mut d, sig as u32); // si_signo
    push_u32(&mut d, 0); // si_code
    push_u32(&mut d, 0); // si_errno
    push_u32(&mut d, sig as u32); // pr_cursig (i16 + 2 pad)
    // SAFETY: pid/ppid/pgid/sid accessors on a valid task.
    unsafe {
        push_u32(&mut d, (*task).pid());
        push_u32(&mut d, (*task).ppid());
        push_u32(&mut d, (*task).pgid());
        push_u32(&mut d, (*task).sid());
    }
    // No CPU accounting yet: four zeroed timevals (64 bytes).
    for _ in 0..64 {
        d.push(0);
    }
    // pr_reg: the first 32 words of PtRegs are exactly the RISC-V
    // user_regs_struct (pc, ra, sp, ..., t6).
    // SAFETY: pt_regs() returns the saved trap frame or null.
    let regs = unsafe { (*task).pt_regs() };
    if !regs.is_null() {
        // SAFETY: 256 bytes of GP registers at the frame head.
        let src = unsafe { core::slice::from_raw_parts(regs as *const u8, 256) };
        d.extend_from_slice(src);
    } else {
        d.extend_from_slice(&[0u8; 256]);
    }
    push_u32(&mut d, 0); // pr_fpvalid (no FP note written)
    while d.len() < 360 {
        d.push(0);
    }
    d
}

/// Build the NT_PRPSINFO descriptor (state, ids, comm).
///
/// # Safety
/// `task` is the current, dying task.
unsafe fn build_prpsinfo(task: *mut Task) -> Vec<u8> {
    let mut d = Vec::with_capacity(136);
    // pr_state/pr_sname/pr_zomb/pr_nice.
    d.push(4);
    d.push(b'T');
    d.push(0);
    d.push(0);
    push_u64(&mut d, 0); // pr_flag
    // SAFETY: cred accessors on a valid task.
    unsafe {
        let cred = (*task).cred();
        push_u32(&mut d, cred.uid);
        push_u32(&mut d, cred.gid);
        push_u32(&mut d, (*task).pid());
        push_u32(&mut d, (*task).ppid());
        push_u32(&mut d, (*task).pgid());
        push_u32(&mut d, (*task).sid());
    }
    // pr_fname[16] (offset 36).
    // SAFETY: comm is a NUL-terminated 16-byte field.
    let comm = unsafe { (*task).comm() };
    let name_len = comm.iter().position(|&b| b == 0).unwrap_or(16);
    d.extend_from_slice(&comm[..name_len]);
    while d.len() < 52 {
        d.push(0);
    }
    // pr_psargs[80]: the executable path, truncated.
    // SAFETY: exe_path accessor on a valid task.
    let exe = unsafe { (*task).get_exe_path() };
    let args_len = exe.len().min(79);
    d.extend_from_slice(&exe[..args_len]);
    while d.len() < 132 {
        d.push(0);
    }
    d
}

// ==================== memory reader ====================

/// Read target user memory through its page tables into `out`
/// (page-granular; non-present pages read as zeros so a whole VMA can be
/// covered by one PT_LOAD).
///
/// # Safety
/// `root_ppn` is a valid page-table root PPN of the dumped mm.
unsafe fn dump_read(root_ppn: u64, va: u64, out: &mut [u8]) {
    use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

    for b in out.iter_mut() {
        *b = 0;
    }
    let page = va & !0xFFFu64;
    // SV39 walk to the leaf (4 KiB / 2 MiB / 1 GiB).
    let a2 = (root_ppn << 12) + ((page >> 30) & 0x1FF) * 8;
    // SAFETY: a2 addresses a valid PTE in the level-2 table.
    let mut pte = unsafe {
        core::ptr::read_volatile(phys_to_virt(PhysAddr::new(a2)).0 as *const u64)
    };
    if pte & 1 == 0 {
        return;
    }
    let mut level = 2u32;
    loop {
        let is_leaf = pte & 0xE != 0;
        if is_leaf || level == 0 {
            break;
        }
        let shift = 12 + 9 * (level - 1);
        let next_table = ((pte >> 10) & 0xFFF_FFFF_FFFF) << 12;
        let idx = (page >> shift) & 0x1FF;
        // SAFETY: valid page-table page in the dumped mm.
        pte = unsafe {
            core::ptr::read_volatile(
                phys_to_virt(PhysAddr::new(next_table + idx * 8)).0 as *const u64,
            )
        };
        if pte & 1 == 0 {
            return;
        }
        level -= 1;
    }
    let (page_phys, page_len) = if level == 0 {
        (((pte >> 10) & 0xFFF_FFFF_FFFF) << 12, 4096u64)
    } else {
        let shift = 12 + 9 * level;
        let mask: u64 = !((1u64 << shift) - 1);
        ((((pte >> 10) & 0xFFF_FFFF_FFFF) << 12) & mask, 1u64 << shift)
    };
    let off = (va & (page_len - 1)) as usize;
    let copy = core::cmp::min(out.len(), page_len as usize - off);
    // SAFETY: kernel linear map of a present user page.
    let src = phys_to_virt(PhysAddr::new(page_phys + off as u64)).0 as *const u8;
    // SAFETY: bounded copy from the linear-mapped user page.
    unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), copy) };
}

// ==================== main entry ====================

/// do_coredump — write the ELF core file for a dying task.
///
/// Called from do_exit() before the mm is torn down. Best-effort: every
/// failure path just gives up silently (the process is dying anyway).
/// Returns true when a core file was created (truncation counts — Linux
/// sets WCOREDUMP even for RLIMIT-truncated dumps).
///
/// # Safety
/// `task` is the current task in its exit path (fdtable + mm still alive).
pub unsafe fn do_coredump(task: *mut Task, sig: i32) -> bool {
    use crate::fs::vfs::file_open;
    use crate::mm::mm_struct::MmFlags;

    // Gate 1: dumpability — SUID_DUMP_DISABLE (setuid binaries, prctl)
    // suppresses the dump.
    // SAFETY: fields of the valid current task.
    unsafe {
        if (*task).dumpable == 0 {
            return false;
        }
        // Gate 2: RLIMIT_CORE == 0 disables dumping entirely.
        let (rlim_cur, _) = (*task).rlimit(rlimit_res::CORE);
        if rlim_cur == 0 {
            return false;
        }
    }

    // The mm must still be there (we are at the top of do_exit).
    // SAFETY: address_space_arc() clones the Arc, keeping the mm alive.
    let mm = unsafe { (*task).address_space_arc() };
    let mm = match mm {
        Some(m) => m,
        None => return false,
    };
    // Guard against double entry.
    if mm.has_flag(MmFlags::MMF_DUMPED) {
        return false;
    }
    mm.set_flags(mm.flags() | MmFlags::MMF_DUMPED);

    // ---- Pass 1: plan the dump ----
    // Snapshotted segments: (vaddr, size, p_flags) per readable VMA.
    let mut segs: Vec<(u64, u64, u32)> = Vec::new();
    {
        let vma_mgr = mm.vma_read();
        for vma in vma_mgr.iter() {
            let f = vma.flags();
            if !f.is_readable() {
                continue;
            }
            if vma.vma_type() == crate::mm::vma::VmaType::Device {
                continue;
            }
            let mut pf = crate::fs::elf::PF_R;
            if f.is_writable() {
                pf |= crate::fs::elf::PF_W;
            }
            if f.is_executable() {
                pf |= crate::fs::elf::PF_X;
            }
            segs.push((vma.start().as_usize() as u64, vma.size() as u64, pf));
        }
    }

    // Notes.
    let mut notes: Vec<u8> = Vec::new();
    // SAFETY: task is current and dying.
    unsafe {
        push_note(&mut notes, NT_PRSTATUS, &build_prstatus(task, sig));
        push_note(&mut notes, NT_PRPSINFO, &build_prpsinfo(task));
    }

    // File layout: ehdr(64) + phdrs + notes, then page-aligned segments.
    // Segment sizes are capped by the remaining RLIMIT_CORE budget so the
    // phdr table written below describes the file exactly.
    let phnum = 1 + segs.len();
    let note_off = 64 + phnum * 56;
    let mut data_off = (note_off + notes.len() + CHUNK - 1) & !(CHUNK - 1);
    let budget: u64 = unsafe { (*task).rlimit(rlimit_res::CORE) }.0;
    if budget == 0 {
        return false;
    }

    // Cap the budget so the header block itself always fits (a tiny
    // RLIMIT_CORE still produces a valid, register-only core).
    let head_len = note_off + notes.len();
    let mut planned: Vec<(u64, u64, u64, u32)> = Vec::with_capacity(segs.len()); // (off, vaddr, filesz, flags)
    for (vaddr, size, pflags) in segs {
        if budget <= head_len as u64 {
            break;
        }
        let avail = budget - head_len as u64;
        // already-committed bytes to earlier segments
        let committed: u64 = planned.iter().map(|p| p.2).sum();
        let seg_bytes = size.min(avail.saturating_sub(committed));
        if seg_bytes == 0 {
            break;
        }
        data_off = (data_off + CHUNK - 1) & !(CHUNK - 1);
        planned.push((data_off as u64, vaddr, seg_bytes, pflags));
        data_off += seg_bytes as usize;
    }
    // The dump file's phnum must match the actually-written segments:
    // rewrite the header counts accordingly.
    let real_phnum = 1 + planned.len();

    // ---- Open the core file (fixed name "core" in the cwd) ----
    let cwd = unsafe { (*task).get_cwd() };
    let cwd_str = alloc::string::String::from_utf8_lossy(&cwd).into_owned();
    let path = if cwd_str.ends_with('/') {
        alloc::format!("{}core", cwd_str)
    } else if cwd_str.is_empty() {
        alloc::string::String::from("/core")
    } else {
        alloc::format!("{}/core", cwd_str)
    };

    let open_flags = FileFlags::O_WRONLY | FileFlags::O_CREAT | FileFlags::O_TRUNC;
    let fd = match file_open(&path, open_flags, 0o600) {
        Ok(fd) => fd,
        Err(_) => return false,
    };
    // Take our own Arc reference and immediately free the fd slot so the
    // dying task's fd table is not perturbed.
    // SAFETY: fd was just installed in the current task's fdtable.
    let file = unsafe { crate::fs::get_file_fd(fd) };
    if let Some(fdt) = unsafe { (*task).try_fdtable() } {
        let _ = fdt.close_fd(fd);
    }
    let file = match file {
        Some(f) => f,
        None => return false,
    };

    // ---- Pass 2: write header + notes, then stream the segments ----
    let mut head: Vec<u8> = Vec::with_capacity(head_len);
    let ehdr = Elf64Ehdr {
        e_ident: [
            0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        e_type: ET_CORE,
        e_machine: EM_RISCV,
        e_version: 1,
        e_entry: 0,
        e_phoff: 64,
        e_shoff: 0,
        e_flags: 0,
        e_ehsize: 64,
        e_phentsize: 56,
        e_phnum: real_phnum as u16,
        e_shentsize: 0,
        e_shnum: 0,
        e_shstrndx: 0,
    };
    // SAFETY: ehdr is repr(C) with 64-byte size (asserted by ELF layout).
    unsafe {
        let p = &ehdr as *const Elf64Ehdr as *const u8;
        head.extend_from_slice(core::slice::from_raw_parts(p, core::mem::size_of::<Elf64Ehdr>()));
    }
    // PT_NOTE right after the phdr table.
    push_phdr(
        &mut head,
        &Elf64Phdr {
            p_type: crate::fs::elf::ElfPtType::PT_NOTE as u32,
            p_flags: 0,
            p_offset: note_off as u64,
            p_vaddr: 0,
            p_paddr: 0,
            p_filesz: notes.len() as u64,
            p_memsz: notes.len() as u64,
            p_align: 4,
        },
    );
    // One PT_LOAD per planned segment.
    for (off, vaddr, filesz, pflags) in &planned {
        push_phdr(
            &mut head,
            &Elf64Phdr {
                p_type: crate::fs::elf::ElfPtType::PT_LOAD as u32,
                p_flags: *pflags,
                p_offset: *off,
                p_vaddr: *vaddr,
                p_paddr: 0,
                p_filesz: *filesz,
                p_memsz: *filesz,
                p_align: CHUNK as u64,
            },
        );
    }
    // Pad the phdr area up to the note offset, then the notes themselves.
    while head.len() < note_off {
        head.push(0);
    }
    head.extend_from_slice(&notes);

    // SAFETY: write_at on the regular inode-backed file we just created.
    unsafe {
        if file.write_at(0, head.as_ptr(), head.len()) < 0 {
            return false;
        }
    }

    let root_ppn = mm.root_ppn();
    let mut page_buf = [0u8; CHUNK];
    for (off, vaddr, filesz, _) in &planned {
        let mut written: u64 = 0;
        while written < *filesz {
            let va = vaddr + written;
            let chunk = core::cmp::min(CHUNK, (*filesz - written) as usize);
            // SAFETY: root_ppn belongs to the pinned mm.
            unsafe { dump_read(root_ppn, va, &mut page_buf[..chunk]) };
            // SAFETY: write_at on the created file.
            let n = unsafe { file.write_at(off + written, page_buf.as_ptr(), chunk) };
            if n < 0 {
                // I/O failure mid-dump: keep what was written (Linux-like
                // best-effort) and stop streaming.
                break;
            }
            written += chunk as u64;
        }
    }

    // SAFETY: task is valid; set the WCOREDUMP bookkeeping bit.
    unsafe {
        (*task).set_core_dumped();
    }
    true
}
