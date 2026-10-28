//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process execution (execve) implementation
//!
//! - do_execve_elf: Load ELF binary and set up user execution context

use alloc::string::String;
use core::slice;

/// Base virtual address for loading the ELF interpreter (dynamic linker)
const INTERP_BASE: usize = 0x3FBF000000;

// FORENSIC: ppn of the just-exec'd image's GOT page (0x17208), recorded
// at exec completion for crash/watchpoint correlation.
pub static LAST_GOT_PPN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// FORENSIC: initial-stack zeroing ledger (see the zeroing site below).
pub struct ZeroEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub base: core::sync::atomic::AtomicU64,
    pub len: core::sync::atomic::AtomicU64,
}
impl ZeroEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            base: core::sync::atomic::AtomicU64::new(0),
            len: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const ZERO_NEW: ZeroEntry = ZeroEntry::new();
pub static STACKZERO_RING: [ZeroEntry; 128] = [ZERO_NEW; 128];
pub static STACKZERO_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Kernel-side write into a freshly mapped USER address range, resolving
/// each page through the page tables. The image mapping may span multiple
/// buddy blocks (alloc_and_map_to_user_table chunks big requests), so
/// physical contiguity must NOT be assumed.
///
/// Returns false when a page of the range has no mapping (the caller
/// treats that as ENOMEM — the range was just mapped, so a miss is a
/// kernel bug, not a malformed binary).
///
/// # Safety
/// `root_ppn` must be a valid user page-table root whose range
/// [vaddr, vaddr+src.len()) is fully mapped and exclusively owned by this
/// in-construction address space.
unsafe fn copy_user_image(root_ppn: u64, vaddr: u64, src: &[u8]) -> bool {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::{phys_to_virt, PhysAddr, PAGE_SHIFT, PAGE_SIZE};

    let mut done = 0usize;
    while done < src.len() {
        let va = vaddr + done as u64;
        let in_page = (va as usize) & (PAGE_SIZE as usize - 1);
        let Some((ppn, pte)) = PageTableWalker::walk(root_ppn, va) else {
            return false;
        };
        if pte & 1 == 0 {
            return false;
        }
        let chunk = core::cmp::min(src.len() - done, PAGE_SIZE as usize - in_page);
        let kva = phys_to_virt(PhysAddr::new(ppn << PAGE_SHIFT)).bits() as usize + in_page;
        // SAFETY: kva is the linear-map address of a page-table-resolved
        // frame of this address space; chunk is bounded by the page.
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr().add(done), kva as *mut u8, chunk);
        }
        done += chunk;
    }
    true
}

/// Execute ELF loading (execve internal function)
///
/// This function will:
/// 1. Create new address space
/// 2. Load ELF segments
/// 3. Load interpreter (dynamic linker) if present
/// 4. Set up user stack (argc, argv, envp, auxv)
/// 5. Update process context (address space, stack pointer, trap frame)
///
pub(crate) fn do_execve_elf(
    task_ptr: *mut crate::process::task::Task,
    program_data: &[u8],
    argv: &[String],
    envp: &[String],
    entry: u64,
    phdr_count: usize,
    ehdr: &crate::fs::elf::Elf64Ehdr,
    pathname: &str,
    interp_data: Option<&[u8]>,
    secure_exec: bool,
    exe_file: Option<&alloc::sync::Arc<crate::fs::file::File>>,
    interp_file: Option<&alloc::sync::Arc<crate::fs::file::File>>,
) -> Result<(), i32> {
    use crate::arch::mm::{
        create_user_address_space,
        PAGE_SIZE, PageTableEntry, phys_to_virt, PhysAddr,
    };
    use crate::mm::alloc_and_map_user_table as alloc_and_map_to_user_table;

    // EXECCOST (dfx=execcost): image-side stage stamps, raw read_time()
    // ticks. Paired with the ladder line (EXECCOST) from do_execve — same
    // switch, its own rate-limit counter (approximate lockstep; both lines
    // carry pid+path for pairing).
    static EXECCOST2_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let ec2 = crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ExecCost);
    let ec2_n = if ec2 {
        EXECCOST2_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    } else {
        u64::MAX
    };
    let ec2_on = ec2 && (ec2_n < 64 || ec2_n % 512 == 0);
    let ec2_t0: u64 = if ec2_on { crate::arch::cpu::read_time() } else { 0 };
    let (mut ec2_as, mut ec2_map, mut ec2_stack, mut ec2_copy, mut ec2_tig, mut ec2_it, mut ec2_fill, mut ec2_vma) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);

    // Capture the trap frame ONCE, before any blocking I/O below — but
    // derive it from THE TASK, never from the per-CPU current_pt_regs()
    // slot. That slot can be STALE for the syscall frame (observed:
    // ENTRY2 wrote the new-image entry into a frame on a DIFFERENT task's
    // kernel stack 0x...16afee0 while this task's stack top was
    // 0x...1690000; the real syscall frame at 0x...168fee0 kept the OLD
    // image's epc and sret resumed the parent image on the new mm — the
    // 0x1c29c NOVMA family). task.pt_regs() is the canonical outermost
    // user frame on THIS task's stack.
    let current_regs = unsafe { (*task_ptr).pt_regs() };

    // Close file descriptors with close-on-exec flag
    // SAFETY: task_ptr points to the current task which is valid throughout execve.
    if let Some(fdtable) = unsafe { (*task_ptr).try_fdtable() } {
        fdtable.close_cloexec_fds();
    }

    // Linux setup_new_exec(): comm becomes the executable's basename (max
    // TASK_COMM_LEN-1 = 15 chars). Without this every post-exec task keeps
    // the FORK-inherited comm — /proc/[pid]/comm, ps and the DFX task dumps
    // showed empty names for nearly every process (only PR_SET_NAME users
    // like glib's gdbus/InputThread threads were identifiable).
    {
        let base = pathname.rsplit('/').next().unwrap_or(pathname);
        let mut name = [0u8; 16];
        let len = core::cmp::min(base.len(), 15);
        name[..len].copy_from_slice(&base.as_bytes()[..len]);
        // SAFETY: task_ptr is the current task, valid throughout execve.
        unsafe { (*task_ptr).set_comm(&name); }
    }

    // POSIX exec signal cleanup:
    // 1. Reset signal handlers to SIG_DFL (preserve SIG_IGN per POSIX)
    // 2. Clear sigaltstack
    // 3. Flush pending signals
    // 4. Reset signal mask
    // 5. Clear any in-progress signal frame — a handler that longjmp'd out
    //    or is exec'ing from within a handler must not leave the kernel
    //    thinking a frame is still active (that would block ALL later
    //    signal delivery, including SIGKILL — review H15).
    // SAFETY: task_ptr is the current task; signal struct may be Arc-shared for threads,
    // but exec replaces the whole process image so this is safe.
    unsafe {
        if let Some(signal_struct) = (*task_ptr).signal.as_ref() {
            signal_struct.flush_handlers(false);
        }
        (*task_ptr).sigstack = crate::signal::SignalStack::new();
        (*task_ptr).pending.clear();
        // POSIX: execve PRESERVES the blocked signal mask (only caught
        // signal dispositions are reset). Clearing it unblocked anything
        // the program had deliberately blocked before exec (review P15).
        (*task_ptr).sigframe = None;
        (*task_ptr).sigframe_addr = 0;
    }

    // Find virtual address range
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    for i in 0..phdr_count {
        // SAFETY: program_data is a valid ELF file slice and i < phdr_count is bounded
        // by the ELF header's e_phnum, so the program header index is in range.
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let mem_size = phdr.p_memsz;

            if virt_addr < min_vaddr {
                min_vaddr = virt_addr;
            }
            if let Some(end) = virt_addr.checked_add(mem_size) {
                if end > max_vaddr {
                    max_vaddr = end;
                }
            } else {
                return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
            }
        }
    }

    // Page align
    let virt_start = min_vaddr & !(PAGE_SIZE - 1);
    let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Linux binfmt_elf load bias: ET_DYN executables (PIE) load at a
    // NONZERO base — ELF_ET_DYN_BASE = (TASK_SIZE/3*2) page-aligned, plus
    // ASLR on Linux — never at their raw p_vaddr. We mapped PIEs at bias
    // 0, so the image started at VA 0 and musl computed app.map =
    // AT_PHDR - PT_PHDR.p_vaddr + min_vaddr == 0; OH musl's cross-DSO CFI
    // then treats the main exe as "the dso has no data! map[0]" and skips
    // filling its shadow slice — every indirect call into exe code read
    // the still-PROT_NONE CFI shadow page and SIGSEGV'd in
    // __cfi_slowpath (the samgr/hilogd "Permission denied at ld.so+0x89ac6"
    // family that killed first pixels). ET_EXEC keeps absolute p_vaddr
    // (bias 0). The base is clear of the fixed INTERP_BASE (0x3FBF000000
    // = 17GB) and of MMAP_START (TASK_SIZE-64GB) on both arches.
    let exec_load_bias: u64 = if ehdr.e_type == crate::fs::elf::ElfType::ET_DYN as u16 {
        let task_size = crate::arch::mm::user_addr::TASK_SIZE as u64;
        (task_size / 3 * 2) & !(PAGE_SIZE - 1)
    } else {
        0
    };

    // Calculate initial stack size needed
    let argc = argv.len() as u64;
    let argv_count = argv.len();
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout size (same as below)
    let auxv_slots: usize = 32;  // 16 auxv entries * 2 (incl. AT_SYSINFO_EHDR)
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
    let execfn_space: usize = ((pathname.len() + 1 + 7) / 8) * 8;
    // +4 covers the AT_NULL pair AND the 2-slot AT_RANDOM buffer (this
    // sizing must mirror the writer's layout exactly).
    let total_slots: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots + 4 + (phdr_space + 7) / 8 + (string_space + 7) / 8 + (env_string_space + 7) / 8 + (execfn_space + 7) / 8;
    let args_size = (total_slots * 8) as u64;

    // Initial stack size: args + 128KB
    const STACK_EXPAND: u64 = 128 * 1024;
    let initial_stack_size = match args_size.checked_add(STACK_EXPAND).and_then(|v| v.checked_add(PAGE_SIZE - 1)) {
        Some(v) => v & !(PAGE_SIZE - 1),
        None => return Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
    };

    // Maximum stack size (8MB)
    const STACK_MAX_SIZE: u64 = 8 * 1024 * 1024;

    // Image size only. The stack is mapped separately near the top of the
    // user address space — Linux-style. The old layout placed the stack
    // directly above the image (virt_end + 128KB); for small PIEs the brk
    // heap starts right below it, so glibc's first malloc sbrk grew
    // straight into the stack and smashed canaries in every dynamically
    // linked heap user (ls/grep/dpkg "*** stack smashing detected ***").
    let total_size = match virt_end.checked_sub(virt_start) {
        Some(v) => v,
        None => return Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
    };

    // Create new user address space
    let user_ppn = create_user_address_space().ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // RAII guard: if anything fails after this point, auto-free the user page tables.
    struct UserAddrSpaceGuard(u64);
    impl Drop for UserAddrSpaceGuard {
        fn drop(&mut self) {
            // SAFETY: user_ppn was allocated by create_user_address_space;
            // on error path, free all page tables to avoid physical page
            // leaks. FRESH_TREE_PTE_LOCK: this tree is unpublished (no
            // MmStruct owns it yet), no mm-side writers exist (INV-LOCK-7).
            unsafe {
                crate::arch::mm::mmu_init::free_user_page_tables(
                    &crate::mm::mm_struct::FRESH_TREE_PTE_LOCK,
                    self.0,
                );
            }
        }
    }
    let _guard = UserAddrSpaceGuard(user_ppn);
    if ec2_on { ec2_as = crate::arch::cpu::read_time(); }

    // Start with RW (no X) — permissions will be tightened per-segment below.
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::A | PageTableEntry::D;

    // SAFETY: user_ppn is a freshly allocated page table root, image_start..image_start+total_size
    // is a valid range, and flags contains valid PTE bits for user pages.
    // NOTE: with the chunked allocator the image may span MULTIPLE physical
    // blocks — never write through phys_base + vaddr offset; the segment
    // loader below walks the page tables page by page.
    let image_start = virt_start + exec_load_bias;
    let _image_phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, image_start, total_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;
    if ec2_on { ec2_map = crate::arch::cpu::read_time(); }

    // Initial stack at the top of user space (one guard page below
    // USER_END). Mapped RW here; the VMA below marks it GROWSDOWN. The
    // mapped range may span multiple floor-pow2 blocks — the args block
    // is installed through the page-table walk below, never through the
    // returned phys base.
    let user_end = crate::arch::mm::user_addr::USER_END as u64;
    let stack_top = user_end - PAGE_SIZE as u64;
    let stack_bottom = stack_top - initial_stack_size;
    let _stack_phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, stack_bottom, initial_stack_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // NOTE (abi-review session 1): no explicit zeroing here. BOTH arch
    // alloc_and_map_to_user_table implementations zero every page before
    // mapping it (the "Zero BEFORE mapping" leak-window fix) — the old
    // full-stack zero_user_range re-walked and re-zeroed all 33+ pages
    // through the page tables on every exec (measured ~30-40 us of the
    // ~73 us stack-map stage on the p4_fork_exec shape, EXECCOST2 'stk').
    // The forensic ring below still records the initial-stack range for
    // crash correlation (its value is the (pid, range) record, not the
    // act of zeroing).
    {
        use core::sync::atomic::Ordering::Relaxed;
        let i = STACKZERO_CURSOR.fetch_add(1, Relaxed) % STACKZERO_RING.len();
        STACKZERO_RING[i].pid.store(crate::process::current_pid(), Relaxed);
        STACKZERO_RING[i].base.store(stack_bottom, Relaxed);
        STACKZERO_RING[i].len.store(initial_stack_size as u64, Relaxed);
    }

    // Signal-return trampoline (R+X page): RISC-V glibc installs handlers
    // without sa_restorer and returns through this fixed page (the Linux
    // kernel uses the vDSO __vdso_rt_sigreturn stub for the same purpose;
    // Rux's vDSO is disabled). Contents: li a7,139 (rt_sigreturn); ecall.
    // x86_64 needs no trampoline page: glibc always passes SA_RESTORER.
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: user_ppn is a fresh user page-table root; RX U flags are
        // valid PTE bits. The returned frame is freshly allocated and
        // exclusively owned by this address space.
        let tp = unsafe {
            alloc_and_map_to_user_table(
                user_ppn,
                crate::signal::SIGTRAMP_BASE,
                4096,
                PageTableEntry::V | PageTableEntry::U | PageTableEntry::R
                    | PageTableEntry::X | PageTableEntry::A,
            )
        };
        if let Some(tp) = tp {
            let kva = phys_to_virt(PhysAddr::new(tp)).bits() as usize;
            unsafe {
                core::ptr::write_volatile(kva as *mut u32, 0x08b0_0893);
                core::ptr::write_volatile((kva + 4) as *mut u32, 0x0000_0073);
            }
        }
    }

    // Load each segment
    if ec2_on { ec2_stack = crate::arch::cpu::read_time(); }
    for i in 0..phdr_count {
        // SAFETY: Same as above — i < phdr_count and program_data is a valid ELF slice.
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = exec_load_bias + phdr.p_vaddr;
            let file_size = phdr.p_filesz;
            let mem_size = phdr.p_memsz;
            let offset = phdr.p_offset as usize;

            // Validate the segment against the file image and the mapped
            // range: p_offset/p_filesz/p_vaddr come from the (possibly
            // attacker-controlled) ELF file; slicing or writing past the
            // mapped region without these checks panics or corrupts memory.
            if file_size > mem_size
                || virt_addr < image_start
                || virt_addr.saturating_add(mem_size) > virt_end + exec_load_bias
                || offset.checked_add(file_size as usize).map_or(true, |end| end > program_data.len())
            {
                return Err(crate::errno::Errno::ExecFormatError.as_neg_i32());
            }

            // The image mapping may span multiple physical blocks (chunked
            // allocator) — resolve every page through the page tables
            // instead of phys_base + offset. The old arithmetic worked
            // only while the whole image happened to be contiguous AND
            // the mapping never overran the buddy block (see
            // alloc_and_map_to_user_table).
            // Copy file data page by page.
            if file_size > 0 {
                let src = &program_data[offset..offset + file_size as usize];
                if !unsafe { copy_user_image(user_ppn, virt_addr, src) } {
                    return Err(crate::errno::Errno::OutOfMemory.as_neg_i32());
                }
            }

            // BSS: no explicit zeroing — the whole image range was
            // pre-zeroed by alloc_and_map_to_user_table before mapping
            // (both arches; the old per-segment zero_user_range re-walked
            // and re-zeroed the same pages through the page tables).
        }
    }

    // Tighten PTE permissions per-segment based on ELF p_flags.
    // Initial mapping was RW (no X); now set exact R/W/X from program headers.
    if ec2_on { ec2_copy = crate::arch::cpu::read_time(); }
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() && phdr.p_memsz > 0 {
            let seg_start = exec_load_bias + phdr.p_vaddr;
            let seg_end = (seg_start + phdr.p_memsz + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

            // Build per-segment PTE flags
            let mut seg_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A;
            if phdr.is_readable() { seg_flags |= PageTableEntry::R; }
            if phdr.is_writable() { seg_flags |= PageTableEntry::W | PageTableEntry::D; }
            if phdr.is_executable() { seg_flags |= PageTableEntry::X; }

            // Walk page table for each page in the segment and update PTE flags.
            // SAFETY: We are walking the freshly created user page tables (user_ppn)
            // and modifying PTE permission bits only — PPN (physical page number) is preserved.
                #[cfg(feature = "riscv64")]
                {
                let mut vaddr = seg_start;
                while vaddr < seg_end {
                    let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
                    let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
                    let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    
                    // SAFETY: user_ppn is a valid page table root allocated above.
                    unsafe {
                        let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                            user_ppn << crate::arch::mm::PAGE_SHIFT,
                        );
                        let pte2 = (*root_table).get(vpn2);
                        if !pte2.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                        let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                            pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                        );
                        let pte1 = (*table1).get(vpn1);
                        if !pte1.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                        let table0 = crate::arch::mm::mmu_init::get_page_table_virt(
                            pte1.ppn() << crate::arch::mm::PAGE_SHIFT,
                        );
                        let old_pte = (*table0).get(vpn0);
                        if !old_pte.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                        // Preserve PPN, replace permission bits
                        let new_bits = (old_pte.bits() & !(0xFFu64)) | seg_flags;
                        (*table0).set(vpn0, PageTableEntry::from_bits(new_bits));
                    }
    
                    vaddr += PAGE_SIZE as u64;
                }
                }

                #[cfg(feature = "x86_64")]
                {
                    let mut vaddr = seg_start;
                    while vaddr < seg_end {
                        // 4-level x86_64 walk (PML4/PDPT/PD/PT)
                        let i4 = ((vaddr >> 39) & 0x1FF) as usize;
                        let i3 = ((vaddr >> 30) & 0x1FF) as usize;
                        let i2 = ((vaddr >> 21) & 0x1FF) as usize;
                        let i1 = ((vaddr >> 12) & 0x1FF) as usize;

                        // SAFETY: walking the freshly created user page tables
                        // (user_ppn); permission-bit updates only, PPN preserved.
                        unsafe {
                            let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                                user_ppn << crate::arch::mm::PAGE_SHIFT,
                            );
                            let pte4 = (*root_table).get(i4);
                            if !pte4.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
                            let table3 = crate::arch::mm::mmu_init::get_page_table_virt(
                                pte4.ppn() << crate::arch::mm::PAGE_SHIFT,
                            );
                            let pte3 = (*table3).get(i3);
                            if !pte3.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
                            let table2 = crate::arch::mm::mmu_init::get_page_table_virt(
                                pte3.ppn() << crate::arch::mm::PAGE_SHIFT,
                            );
                            let pte2 = (*table2).get(i2);
                            if !pte2.is_valid() || pte2.is_leaf() { vaddr += PAGE_SIZE as u64; continue; }
                            let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                                pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                            );
                            let old_pte = (*table1).get(i1);
                            if !old_pte.is_valid() { vaddr += PAGE_SIZE as u64; continue; }

                            // x86 permission bits: clear W|U|NX, then set from the
                            // semantic aliases (V=P, W=RW, U=US; R/X are no-ops —
                            // non-exec is expressed via NX).
                            let clear = PageTableEntry::W | PageTableEntry::U | PageTableEntry::NX;
                            let mut new_bits = (old_pte.bits() & !clear) | seg_flags;
                            if !phdr.is_executable() {
                                new_bits |= PageTableEntry::NX;
                            }
                            (*table1).set(i1, PageTableEntry::from_bits(new_bits));
                        }
                        vaddr += PAGE_SIZE as u64;
                    }
                }

        }
    }

    // P2 vDSO mapping is DISABLED (data page lives outside the linear
    // map; see vdso.rs). The block below tried to map anyway and spewed
    // virt_to_phys warnings on every exec while AT_SYSINFO_EHDR stays
    // commented out — skipped until the vDSO pages come from linear-mapped
    // memory.
    // (riscv64-only: the block encodes the riscv map_user_region ABI;
    // x86_64 has no vDSO mapping — AT_SYSINFO_EHDR stays absent.)
    #[cfg(feature = "riscv64")]
    #[allow(unused_variables)]
    if false {
    // P2 vDSO: map the shared time-data page (RW) and the vDSO ELF page
    // (RX) at the fixed VDSO_BASE, two pages total. The mapping aliases
    // GLOBAL kernel pages — the data page stays live (tick-refreshed) in
    // every process; a fork that copies the page tables keeps either the
    // shared frame (no CoW fault ever fires — users only read it) or a
    // snapshot, both of which interpolate correctly from rdcycle.
    {
        let (data_paddr, code_paddr) = crate::mm::vdso::vdso_pages_phys();
        // SAFETY: user_ppn is the freshly created page-table root; the
        // physical frames are the aligned 4 KiB vDSO statics; flags are
        // valid user PTE bits.
        unsafe {
            crate::arch::mm::mm_ops::map_user_region(
                user_ppn,
                crate::mm::vdso::VDSO_BASE,
                data_paddr,
                4096,
                PageTableEntry::V | PageTableEntry::U | PageTableEntry::R
                    | PageTableEntry::W | PageTableEntry::A | PageTableEntry::D,
            );
            crate::arch::mm::mm_ops::map_user_region(
                user_ppn,
                crate::mm::vdso::VDSO_BASE + 4096,
                code_paddr,
                4096,
                PageTableEntry::V | PageTableEntry::U | PageTableEntry::R
                    | PageTableEntry::X | PageTableEntry::A,
            );
        }
    }

    }

    // Load interpreter (dynamic linker) if present
    if ec2_on { ec2_tig = crate::arch::cpu::read_time(); }
    let (actual_entry, at_base) = if let Some(interp_bytes) = interp_data {        let interp_base: u64 = INTERP_BASE as u64;  // mmap_start - 16MB

        // SAFETY: interp_bytes is a validated ELF interpreter file loaded earlier.
        let interp_ehdr = unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(interp_bytes) }
            .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;

        let mut interp_min_vaddr: u64 = u64::MAX;
        let mut interp_max_vaddr: u64 = 0;
        for i in 0..interp_ehdr.e_phnum as usize {
            // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
            if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                if phdr.is_load() {
                    if phdr.p_vaddr < interp_min_vaddr { interp_min_vaddr = phdr.p_vaddr; }
                    if let Some(end) = phdr.p_vaddr.checked_add(phdr.p_memsz) {
                        if end > interp_max_vaddr { interp_max_vaddr = end; }
                    } else {
                        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
                    }
                }
            }
        }
        let interp_size = (interp_max_vaddr - interp_min_vaddr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        // SAFETY: user_ppn is valid page table root, interp_base/interp_size describe the
        // interpreter's virtual memory range, flags are valid user PTE bits.
        // The range may span multiple floor-pow2 blocks (a 300-page ld.so
        // maps as 256+32+16+4) — the segment copies below go through the
        // page-table walk, never through a phys base (abi-review session 2:
        // this retires ElfLoader::load_dynamic_to, whose linear
        // dest_kva + (p_vaddr - min) writes assumed a single block).
        let _interp_phys = unsafe {
            alloc_and_map_to_user_table(user_ppn, interp_base, interp_size, flags)
        }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

        // Copy each PT_LOAD through the walker. BSS needs no explicit
        // zeroing: the allocator pre-zeroes every mapped page.
        for i in 0..interp_ehdr.e_phnum as usize {
            // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
            if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                if phdr.is_load() && phdr.p_filesz > 0 {
                    let offset = phdr.p_offset as usize;
                    let filesz = phdr.p_filesz as usize;
                    if !offset.checked_add(filesz).map_or(false, |end| end <= interp_bytes.len()) {
                        return Err(crate::errno::Errno::ExecFormatError.as_neg_i32());
                    }
                    let dst = interp_base + (phdr.p_vaddr - interp_min_vaddr);
                    // SAFETY: user_ppn is this fresh address space's root;
                    // dst lies inside the freshly mapped interp range.
                    if !unsafe {
                        copy_user_image(user_ppn, dst, &interp_bytes[offset..offset + filesz])
                    } {
                        return Err(crate::errno::Errno::OutOfMemory.as_neg_i32());
                    }
                }
            }
        }

        let entry_offset = interp_ehdr
            .e_entry
            .checked_sub(interp_min_vaddr)
            .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;
        let interp_entry = interp_base + entry_offset;

        // READBACK probe: the installed entry word through the USER PTE
        // path must equal the source image's entry word (the copy and the
        // readback share one translation, so a mismatch means the copy
        // itself corrupted — the old linear-vs-PTE IDIVERGE window is
        // gone by construction). The entry's FILE offset is resolved
        // through the PT_LOAD that contains its vaddr (p_offset +
        // (e_entry - p_vaddr)), never assumed to equal the vaddr.
        {
            use crate::arch::mm::mm_ops::PageTableWalker;
            let entry_va = interp_base + entry_offset;
            let mut src_off = None;
            for i in 0..interp_ehdr.e_phnum as usize {
                // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
                if let Some(ph) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                    if ph.is_load()
                        && interp_ehdr.e_entry >= ph.p_vaddr
                        && interp_ehdr.e_entry + 8 <= ph.p_vaddr + ph.p_filesz
                    {
                        src_off = Some((ph.p_offset + (interp_ehdr.e_entry - ph.p_vaddr)) as usize);
                        break;
                    }
                }
            }
            if let Some(src_off) = src_off {
                let mut wb = [0u8; 8];
                wb.copy_from_slice(&interp_bytes[src_off..src_off + 8]);
                let want = u64::from_le_bytes(wb);
                match unsafe { PageTableWalker::walk(user_ppn, entry_va) } {
                    Some((ppn, _)) => {
                        let pva = phys_to_virt(PhysAddr::new(ppn as u64 * 4096)).bits()
                            + (entry_va & 0xFFF);
                        let ptev = unsafe { core::ptr::read_volatile(pva as *const u64) };
                        if ptev != want {
                            crate::pr_err!(
                                "IDIVERGE pid={} entry={:#x} want={:#x} pte={:#x} ppn={:#x}",
                                crate::process::current_pid(), entry_va, want, ptev, ppn
                            );
                        }
                    }
                    None => crate::pr_err!("IDIVERGE pid={} entry UNMAPPED", crate::process::current_pid()),
                }
            }
        }

        // Tighten PTE permissions for interpreter segments.
        for i in 0..interp_ehdr.e_phnum as usize {
            if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                if phdr.is_load() && phdr.p_memsz > 0 {
                    let seg_start = interp_base + phdr.p_vaddr - interp_min_vaddr;
                    let seg_end = (seg_start + phdr.p_memsz + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

                    let mut seg_flags = PageTableEntry::V | PageTableEntry::U | PageTableEntry::A;
                    if phdr.is_readable() { seg_flags |= PageTableEntry::R; }
                    if phdr.is_writable() { seg_flags |= PageTableEntry::W | PageTableEntry::D; }
                    if phdr.is_executable() { seg_flags |= PageTableEntry::X; }

                    #[cfg(feature = "riscv64")]
                    {
                        let mut vaddr = seg_start;
                        while vaddr < seg_end {
                            let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
                            let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
                            let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
    
                            // SAFETY: walking the user page tables for interpreter PTE updates.
                            unsafe {
                                let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                                    user_ppn << crate::arch::mm::PAGE_SHIFT,
                                );
                                let pte2 = (*root_table).get(vpn2);
                                if !pte2.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                                let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                                    pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                                );
                                let pte1 = (*table1).get(vpn1);
                                if !pte1.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                                let table0 = crate::arch::mm::mmu_init::get_page_table_virt(
                                    pte1.ppn() << crate::arch::mm::PAGE_SHIFT,
                                );
                                let old_pte = (*table0).get(vpn0);
                                if !old_pte.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                                let new_bits = (old_pte.bits() & !(0xFFu64)) | seg_flags;
                                (*table0).set(vpn0, PageTableEntry::from_bits(new_bits));
                            }
    
                            vaddr += PAGE_SIZE as u64;
                        }
                    }

                    #[cfg(feature = "x86_64")]
                    {
                        let mut vaddr = seg_start;
                        while vaddr < seg_end {
                            // 4-level x86_64 walk (PML4/PDPT/PD/PT)
                            let i4 = ((vaddr >> 39) & 0x1FF) as usize;
                            let i3 = ((vaddr >> 30) & 0x1FF) as usize;
                            let i2 = ((vaddr >> 21) & 0x1FF) as usize;
                            let i1 = ((vaddr >> 12) & 0x1FF) as usize;
    
                            // SAFETY: walking the freshly created user page tables
                            // (user_ppn); permission-bit updates only, PPN preserved.
                            unsafe {
                                let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                                    user_ppn << crate::arch::mm::PAGE_SHIFT,
                                );
                                let pte4 = (*root_table).get(i4);
                                if !pte4.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
                                let table3 = crate::arch::mm::mmu_init::get_page_table_virt(
                                    pte4.ppn() << crate::arch::mm::PAGE_SHIFT,
                                );
                                let pte3 = (*table3).get(i3);
                                if !pte3.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
                                let table2 = crate::arch::mm::mmu_init::get_page_table_virt(
                                    pte3.ppn() << crate::arch::mm::PAGE_SHIFT,
                                );
                                let pte2 = (*table2).get(i2);
                                if !pte2.is_valid() || pte2.is_leaf() { vaddr += PAGE_SIZE as u64; continue; }
                                let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                                    pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                                );
                                let old_pte = (*table1).get(i1);
                                if !old_pte.is_valid() { vaddr += PAGE_SIZE as u64; continue; }
    
                                // x86 permission bits: clear W|U|NX, then set from the
                                // semantic aliases (V=P, W=RW, U=US; R/X are no-ops —
                                // non-exec is expressed via NX).
                                let clear = PageTableEntry::W | PageTableEntry::U | PageTableEntry::NX;
                                let mut new_bits = (old_pte.bits() & !clear) | seg_flags;
                                if !phdr.is_executable() {
                                    new_bits |= PageTableEntry::NX;
                                }
                                (*table1).set(i1, PageTableEntry::from_bits(new_bits));
                            }
                            vaddr += PAGE_SIZE as u64;
                        }
                    }
    
                }
            }
        }

        crate::pr_debug!("execve: loaded interpreter at {:#x}, entry={:#x}, size={:#x}",
            interp_base, interp_entry, interp_size);

        (interp_entry, interp_base)
    } else {
        // No interpreter (static or static-PIE): control goes straight to
        // the executable entry — apply its load bias for ET_DYN.
        (exec_load_bias + entry, 0)
    };

    // Set up stack (Linux reserves 256 bytes of slop below the raw top).
    if ec2_on { ec2_it = crate::arch::cpu::read_time(); }
    let stack_top = stack_top - 256;

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
    /// P2 vDSO: address of the mapped vDSO ELF header page.
    const AT_SYSINFO_EHDR: u64 = 33;

    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout
    let auxv_slots: usize = 32;  // 16 auxv entries * 2 (incl. AT_SYSINFO_EHDR)
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
    // Pathname string for AT_EXECFN (aligned to 8 bytes).
    let execfn_space: usize = ((pathname.len() + 1 + 7) / 8) * 8;

    // +2 skips the AT_NULL pair: AT_RANDOM gets its own 16 bytes here.
    // It used to share the AT_NULL slots (canary always read as zero)
    // while the two random words were written 16 bytes further on, over
    // the phdr table copy (review PROC-P05).
    let random_offset: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots + 2;
    let phdr_offset: usize = random_offset + 2;
    let env_string_offset: usize = phdr_offset + (phdr_space + 7) / 8;
    let string_offset: usize = env_string_offset + (env_string_space + 7) / 8;
    let execfn_string_offset: usize = string_offset + (string_space + 7) / 8;
    let total_slots: usize = execfn_string_offset + (execfn_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_slots * 8) as u64);
    // SysV x86-64 ABI (and Linux for every ABI it supports): the process
    // entry rsp — the word holding argc — is 16-byte aligned. All slot
    // math above is 8-byte granular, so without this the entry rsp can
    // land ≡8 (mod 16) and ld.so's first aligned access (movaps on the
    // entry frame chain) takes a #GP. riscv64 hardware tolerates
    // misaligned accesses, which is why the twin never tripped.
    let adjusted_stack_top = adjusted_stack_top & !0xFu64;

    // Build the args block in a KERNEL buffer, then install it with ONE
    // page-table-walking copy (abi-review session 2): the allocator now
    // serves floor-pow2 chunks — a 33-page initial stack maps as 32+1,
    // two blocks, NOT physically contiguous — so the old
    // phys_base + offset linear writer is gone by necessity. Writing
    // through the walker also removes the phys-view-vs-PTE-view DIVERGE
    // window by construction (write and readback use the same
    // translation); the readback probe below now compares the installed
    // words against this buffer.
    let mut args_buf: alloc::vec::Vec<u8> = alloc::vec![0u8; total_slots * 8];
    let put_u64 = |buf: &mut [u8], byte_off: usize, v: u64| {
        buf[byte_off..byte_off + 8].copy_from_slice(&v.to_le_bytes());
    };

    // Saved auxv for core dumps (NT_AUXV): the (type, value) words exactly
    // as written to the user stack below, including the AT_NULL pair.
    // Handed to the new mm after it is created — Linux mm->saved_auxv.
    let mut auxv_words: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(auxv_slots + 2);

    // Linux semantics: AT_PHDR must point at the phdr table INSIDE the
    // mapped image (load bias + PT_PHDR.p_vaddr). musl's ld.so computes
    // the main program's load bias as AT_PHDR - PT_PHDR.p_vaddr; pointing
    // AT_PHDR at the stack copy made that bias a stack address, so every
    // PIE relocation landed in garbage and the loader died silently
    // (found via musl dynlink.c; review PROC-P05). Segments are mapped
    // at their p_vaddr (bias 0), so the mapped address IS p_vaddr.
    let mapped_phdr: Option<u64> = {
        let mut found = None;
        for i in 0..ehdr.e_phnum as usize {
            // SAFETY: get_program_header bounds-checks e_phoff/idx against
            // program_data (Wave 1 central check).
            if let Some(ph) = unsafe { ehdr.get_program_header(program_data, i) } {
                if ph.p_type == crate::fs::elf::ElfPtType::PT_PHDR as u32 {
                    found = Some(exec_load_bias + ph.p_vaddr);
                    break;
                }
            }
        }
        found
    };
    let phdr_addr = mapped_phdr
        .unwrap_or(adjusted_stack_top + (phdr_offset * 8) as u64);
    let random_vaddr = adjusted_stack_top + (random_offset * 8) as u64;
    let execfn_vaddr = adjusted_stack_top + (execfn_string_offset * 8) as u64;

    // Copy program header table (bounds validated by get_program_headers:
    // e_phoff + phsize <= program_data.len()).
    let phdr_src = ehdr.e_phoff as usize;
    args_buf[phdr_offset * 8..phdr_offset * 8 + phsize]
        .copy_from_slice(&program_data[phdr_src..phdr_src + phsize]);

    // Write argv strings
    let mut argv_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(argv_count);
    let mut current_string_offset = string_offset;

    for arg in argv.iter() {
        let string_pos = current_string_offset * 8;
        let arg_bytes = arg.as_bytes();
        args_buf[string_pos..string_pos + arg_bytes.len()].copy_from_slice(arg_bytes);
        // The NUL terminator (and the 8-byte padding) is already zero.
        argv_addrs.push(adjusted_stack_top + string_pos as u64);
        current_string_offset += ((arg.len() + 1 + 7) / 8);
    }

    // Write envp strings
    let mut envp_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(envp_count);
    let mut current_env_string_offset = env_string_offset;

    for env in envp.iter() {
        let string_pos = current_env_string_offset * 8;
        let env_bytes = env.as_bytes();
        args_buf[string_pos..string_pos + env_bytes.len()].copy_from_slice(env_bytes);
        envp_addrs.push(adjusted_stack_top + string_pos as u64);
        current_env_string_offset += ((env.len() + 1 + 7) / 8);
    }

    // Write pathname string for AT_EXECFN
    {
        let execfn_pos = execfn_string_offset * 8;
        let path_bytes = pathname.as_bytes();
        args_buf[execfn_pos..execfn_pos + path_bytes.len()].copy_from_slice(path_bytes);
    }

    // argc
    let mut offset: usize = 0;
    put_u64(&mut args_buf, offset * 8, argc);
    offset += 1;

    // argv
    for &addr in &argv_addrs {
        put_u64(&mut args_buf, offset * 8, addr);
        offset += 1;
    }

    // argv terminator
    put_u64(&mut args_buf, offset * 8, 0);
    offset += 1;

    // envp pointers
    for &addr in &envp_addrs {
        put_u64(&mut args_buf, offset * 8, addr);
        offset += 1;
    }

    // envp terminator
    put_u64(&mut args_buf, offset * 8, 0);
    offset += 1;

    // auxv
    // AT_UID/AT_EUID/AT_GID/AT_EGID come from the (possibly just
    // setuid-updated) credentials; AT_SECURE=1 makes musl/glibc scrub
    // unsafe environment variables on privileged execs.
    let (at_uid, at_euid, at_gid, at_egid) = unsafe {
        let cred = (*task_ptr).cred();
        (cred.uid as u64, cred.euid as u64, cred.gid as u64, cred.egid as u64)
    };
    let auxv = &[
        (AT_PHDR, phdr_addr),
        (AT_PHENT, phent),
        (AT_PHNUM, phnum),
        (AT_PAGESZ, PAGE_SIZE as u64),
        (AT_BASE, at_base),
        // Linux AT_ENTRY is the main executable's entry (load bias +
        // e_entry), even when control is first transferred to the
        // interpreter.
        (AT_ENTRY, exec_load_bias + entry),
        (AT_UID, at_uid),
        (AT_EUID, at_euid),
        (AT_GID, at_gid),
        (AT_EGID, at_egid),
        (AT_HWCAP, 0),
        (AT_CLKTCK, 100),
        (AT_SECURE, secure_exec as u64),
        (AT_RANDOM, random_vaddr),
        (AT_EXECFN, execfn_vaddr),
        // P2 vDSO: libc resolves __vdso_clock_gettime & co. through
        // this ELF mapping and skips the syscall for gettime paths.
        // (AT_SYSINFO_EHDR, crate::mm::vdso::vdso_ehdr()), // P2-fix: vDSO data page is outside the linear mapping — disable until the page is allocated from linear-mapped memory
    ];

    for (typ, val) in auxv {
        put_u64(&mut args_buf, offset * 8, *typ);
        put_u64(&mut args_buf, (offset + 1) * 8, *val);
        auxv_words.push(*typ);
        auxv_words.push(*val);
        offset += 2;
    }

    // AT_NULL
    put_u64(&mut args_buf, offset * 8, AT_NULL);
    put_u64(&mut args_buf, (offset + 1) * 8, 0);
    auxv_words.push(AT_NULL);
    auxv_words.push(0);

    // 16 random bytes AT AT_RANDOM's address (time-seeded LCG; a real
    // entropy source is a separate work item).
    let seed = crate::arch::cpu::read_time();
    let mut state = seed;
    state = state.wrapping_mul(1103515245).wrapping_add(12345);
    let rand0 = state;
    state = state.wrapping_mul(1103515245).wrapping_add(12345);
    let rand1 = state;
    put_u64(&mut args_buf, random_offset * 8, rand0);
    put_u64(&mut args_buf, (random_offset + 1) * 8, rand1);

    // Install the block through the page-table walk (one copy; the range
    // may span multiple physical blocks — copy_user_image resolves each
    // page).
    // SAFETY: user_ppn is this fresh address space's root and
    // [adjusted_stack_top, adjusted_stack_top + args_buf.len()) lies
    // inside the freshly mapped stack region.
    if !unsafe { copy_user_image(user_ppn, adjusted_stack_top, &args_buf) } {
        return Err(crate::errno::Errno::OutOfMemory.as_neg_i32());
    }

    // READBACK probe: the installed words through the USER PTE path must
    // equal the buffer we just copied (argc, AT_RANDOM, AT_EXECFN).
    let probe = |byte_off: usize, label: &str| unsafe {
        use crate::arch::mm::mm_ops::PageTableWalker;
        let mut wb = [0u8; 8];
        wb.copy_from_slice(&args_buf[byte_off..byte_off + 8]);
        let want = u64::from_le_bytes(wb);
        let vaddr = adjusted_stack_top + byte_off as u64;
        match PageTableWalker::walk(user_ppn, vaddr) {
            Some((ppn, _)) => {
                let va = phys_to_virt(PhysAddr::new(ppn as u64 * 4096)).bits();
                let v = core::ptr::read_volatile((va + (vaddr & 0xFFF)) as *const u64);
                if v != want {
                    crate::pr_err!(
                        "DIVERGE pid={} {}: want={:#x} pte={:#x} mapped_ppn={:#x}",
                        crate::process::current_pid(), label, want, v, ppn
                    );
                }
            }
            None => {
                crate::pr_err!("DIVERGE pid={} {}: walk FAILED {:#x}",
                    crate::process::current_pid(), label, vaddr);
            }
        }
    };
    probe(0, "argc");
    probe(random_offset * 8, "at_random");
    probe(execfn_string_offset * 8, "at_execfn");

    // Create new address space structure
    // SAFETY: user_ppn is a freshly allocated page table root with no prior users.
    if ec2_on { ec2_fill = crate::arch::cpu::read_time(); }
    let new_addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // Reset the heap to the NEW image: exec used to leave the task's brk
    // (and the mm default) inherited from the PARENT image, so the first
    // malloc in the exec'd program extended a heap at the old image's
    // break — a mapped-but-wrong region (observed crash: dash jumping
    // through a function pointer at 0x1c29c, a parent-brk heap address).
    // Linux resets start_brk/brk to the page-aligned end of the new
    // image's highest PT_LOAD.
    let image_end = virt_end + exec_load_bias;
    new_addr_space.set_start_brk(image_end as usize);
    new_addr_space.set_brk_val(image_end as usize);
    unsafe { (*task_ptr).set_brk(image_end); };

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

    // Save the auxv into the mm so a fatal-signal core dump can emit
    // NT_AUXV without depending on the (clobberable) user stack.
    new_addr_space.set_saved_auxv(&auxv_words);

    // Set up stack VMA with GROWSDOWN flag and stack limit.
    // stack_bottom here is the mapped bottom of the high stack (see the
    // stack mapping above); the GROWSDOWN VMA lets it extend further down.
    // RLIMIT_STACK: the stack growth bound is the exec'ing task's soft
    // limit, capped at the architectural 8MB default. setrlimit/prlimit64
    // keep mm.stack_limit in sync afterwards (sync_stack_limit in
    // syscall/process.rs uses the same formula).
    let stack_size_limit = {
        let rlim_cur = crate::sched::current()
            .map(|t| t.rlimit(crate::process::task::rlimit_res::STACK).0)
            .unwrap_or(STACK_MAX_SIZE);
        rlim_cur.min(STACK_MAX_SIZE)
    };
    let stack_limit = stack_top.saturating_sub(stack_size_limit) + PAGE_SIZE;
    new_addr_space.set_start_stack(stack_top as usize);
    new_addr_space.set_stack_limit(stack_limit as usize);

    // Add stack VMA with GROWSDOWN flag
    {
        use crate::mm::vma::{Vma, VmaFlags, VmaOrigin, VmaType};
        // `stack_top` here is the argv-slop-adjusted value (raw top - 256,
        // NOT page aligned). VirtAddr::new SILENTLY FLOORS to page
        // granularity, so the VMA used to end one page below the mapped
        // stack top: the top page (mapped at exec, holding argv/env/auxv
        // and the entry frame) was left mapped-but-VMA-less. Userspace
        // never noticed (present pages do not fault), but every VMA-based
        // query on that page was wrong — process_vm_writev into a stack
        // VLA crossing the boundary stopped one page short (LTP
        // process_vm_writev02: 97632 of 100000 bytes). Round the VMA end
        // UP to cover the whole mapped stack.
        let stack_top_page =
            (stack_top + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        let mut stack_vma = Vma::new(
            crate::mm::page::VirtAddr::new(stack_bottom as usize),
            crate::mm::page::VirtAddr::new(stack_top_page as usize),
            VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN),
        );
        stack_vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC_STACK));
        let mut vma_mgr = new_addr_space.vma_write();
        // Account the exec image: the reference kernel charges every VMA
        // it installs (mm->total_vm += pages), so the image, the stack
        // and the interpreter count towards RLIMIT_AS and show up in the
        // reported VmSize.
        let stack_pages = stack_vma.page_count() as u64;
        let _ = vma_mgr.add(stack_vma);
        new_addr_space.add_total_vm(stack_pages);

        // Add VMAs for each ELF PT_LOAD segment
        let mut first_exec_set = false;
        for i in 0..phdr_count {
            // SAFETY: i < phdr_count and program_data is a valid ELF file slice.
            if let Some(phdr) = unsafe { ehdr.get_program_header(program_data, i) } {
                if phdr.is_load() {
                    let seg_start = exec_load_bias + phdr.p_vaddr;
                    let seg_end = seg_start + phdr.p_memsz;
                    if seg_end <= seg_start {
                        continue;
                    }

                    // Page-align: start rounds down, end rounds up
                    let vma_start = seg_start & !(PAGE_SIZE as u64 - 1);
                    let vma_end = (seg_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                    if vma_end <= vma_start {
                        continue;
                    }

                    let mut vma_flags = VmaFlags::new();
                    if phdr.is_readable() { vma_flags.insert(VmaFlags::READ); }
                    if phdr.is_writable() { vma_flags.insert(VmaFlags::WRITE); }
                    if phdr.is_executable() { vma_flags.insert(VmaFlags::EXEC); }
                    vma_flags.insert(VmaFlags::PRIVATE);

                    // Mark first executable segment as VM_EXECUTABLE
                    if phdr.is_executable() && !first_exec_set {
                        vma_flags.insert(VmaFlags::EXECUTABLE);
                        first_exec_set = true;
                    }

                    // Linux binfmt_elf parity: the executable's LOADs are
                    // FILE VMAs (vm_file = the exe, vm_pgoff = ELF_PAGESTART
                    // file offset) — /proc/<pid>/maps prints the path and a
                    // per-segment offset, and forensic symbolization of
                    // in-image rips can compute file positions
                    // (file_off = vma_off + (rip - vma_start)). The image is
                    // still copied into private anon frames at exec (the
                    // mapping model is unchanged); fd stays -1 so no
                    // fd-gated fault path is activated, and the identity
                    // lives on the pinned vm_file instead of an fd number
                    // (which exec closes and userspace could reuse).
                    let mut vma = Vma::new(
                        crate::mm::page::VirtAddr::new(vma_start as usize),
                        crate::mm::page::VirtAddr::new(vma_end as usize),
                        vma_flags,
                    );
                    vma.set_type(VmaType::FileBacked);
                    vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC));
                    vma.set_file_fd(-1);
                    vma.set_file_size(program_data.len() as u64);
                    // File page mapped at vma_start (elf_map:
                    // off = p_offset - ELF_PAGEOFFSET(p_vaddr)).
                    vma.set_offset(
                        (phdr.p_offset - (phdr.p_vaddr & (PAGE_SIZE as u64 - 1))) as usize,
                    );
                    if let Some(f) = exe_file {
                        new_addr_space.pin_vma_file(vma_start as usize, f.clone());
                    }
                    let seg_pages = vma.page_count() as u64;
                    let _ = vma_mgr.add(vma);
                    new_addr_space.add_total_vm(seg_pages);
                }
            }
        }

        // Add VMA for interpreter (dynamic linker)
        if let Some(interp_bytes) = interp_data {
            let interp_base: u64 = 0x3FBF000000u64;
            // SAFETY: interp_bytes was already validated as a valid ELF file above.
            let interp_ehdr = unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(interp_bytes) }
                .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;
            let mut interp_min: u64 = u64::MAX;
            let mut interp_max: u64 = 0;
            for i in 0..interp_ehdr.e_phnum as usize {
                // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
                if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                    if phdr.is_load() {
                        if phdr.p_vaddr < interp_min { interp_min = phdr.p_vaddr; }
                        if let Some(end) = phdr.p_vaddr.checked_add(phdr.p_memsz) {
                            if end > interp_max { interp_max = end; }
                        } else {
                            return Err(crate::errno::Errno::ExecFormatError.as_neg_i32());
                        }
                    }
                }
            }
            if interp_max > interp_min {
                // Page-align the size
                let raw_size = interp_max - interp_min;
                let interp_size = (raw_size + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                // Linux load_elf_interp parity: the interpreter's LOAD
                // segments are FILE VMAs (vm_file = the interp file) with
                // per-segment permissions and pgoff. The old single flat
                // R-X anonymous VMA over [INTERP_BASE, INTERP_BASE+size)
                // lost fd/offset/path entirely — /proc/<pid>/maps never
                // showed the interp, and rip-level forensics inside the
                // ld.so span (the 0x3fbf08ad30/0x3fbf116dea families)
                // could not convert a rip into a file offset. The image is
                // still eagerly copied into private anon frames at exec;
                // fd stays -1 (no fd-gated fault path activates) and the
                // file identity lives on the pinned vm_file.
                let mut prev_end = interp_base;
                let mut added_any = false;
                for i in 0..interp_ehdr.e_phnum as usize {
                    // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
                    if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                        if !phdr.is_load() || phdr.p_memsz == 0 {
                            continue;
                        }
                        let seg_vaddr = interp_base + phdr.p_vaddr - interp_min;
                        let seg_start = seg_vaddr & !(PAGE_SIZE as u64 - 1);
                        let seg_end = (seg_vaddr + phdr.p_memsz + PAGE_SIZE as u64 - 1)
                            & !(PAGE_SIZE as u64 - 1);
                        if seg_end <= seg_start {
                            continue;
                        }
                        // Keep the whole span VMA-covered like the old flat
                        // mapping: inter-segment gaps become PROT_NONE
                        // anonymous fillers (Linux leaves holes; PROT_NONE
                        // faults identically for an access while preserving
                        // mprotect's whole-range coverage checks).
                        if seg_start > prev_end {
                            let mut filler = Vma::new(
                                crate::mm::page::VirtAddr::new(prev_end as usize),
                                crate::mm::page::VirtAddr::new(seg_start as usize),
                                VmaFlags::from_bits(VmaFlags::PRIVATE),
                            );
                            filler.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC));
                            let filler_pages = filler.page_count() as u64;
                            let _ = vma_mgr.add(filler);
                            new_addr_space.add_total_vm(filler_pages);
                        }
                        let mut seg_flags = VmaFlags::new();
                        if phdr.is_readable() { seg_flags.insert(VmaFlags::READ); }
                        if phdr.is_writable() { seg_flags.insert(VmaFlags::WRITE); }
                        if phdr.is_executable() { seg_flags.insert(VmaFlags::EXEC); }
                        seg_flags.insert(VmaFlags::PRIVATE);
                        let mut vma = Vma::new(
                            crate::mm::page::VirtAddr::new(seg_start as usize),
                            crate::mm::page::VirtAddr::new(seg_end as usize),
                            seg_flags,
                        );
                        vma.set_type(VmaType::FileBacked);
                        vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC));
                        vma.set_file_fd(-1);
                        vma.set_file_size(interp_bytes.len() as u64);
                        // elf_map: file page mapped at seg_start is
                        // p_offset - ELF_PAGEOFFSET(p_vaddr) — exact
                        // rip->file_off arithmetic for forensics.
                        vma.set_offset(
                            (phdr.p_offset - (phdr.p_vaddr & (PAGE_SIZE as u64 - 1))) as usize,
                        );
                        if let Some(f) = interp_file {
                            new_addr_space.pin_vma_file(seg_start as usize, f.clone());
                        }
                        let seg_pages = vma.page_count() as u64;
                        let _ = vma_mgr.add(vma);
                        new_addr_space.add_total_vm(seg_pages);
                        added_any = true;
                        prev_end = seg_end;
                    }
                }
                // Tail filler to the page-rounded image end (BSS tail).
                let span_end = interp_base + interp_size;
                if added_any && span_end > prev_end {
                    let mut filler = Vma::new(
                        crate::mm::page::VirtAddr::new(prev_end as usize),
                        crate::mm::page::VirtAddr::new(span_end as usize),
                        VmaFlags::from_bits(VmaFlags::PRIVATE),
                    );
                    filler.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC));
                    let filler_pages = filler.page_count() as u64;
                    let _ = vma_mgr.add(filler);
                    new_addr_space.add_total_vm(filler_pages);
                }
                // Degenerate case (no loadable segments parsed here but
                // interp_max > interp_min): keep the old flat coverage.
                if !added_any {
                    let interp_flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::EXEC | VmaFlags::PRIVATE);
                    let mut interp_vma = Vma::new(
                        crate::mm::page::VirtAddr::new(interp_base as usize),
                        crate::mm::page::VirtAddr::new(span_end as usize),
                        interp_flags,
                    );
                    interp_vma.set_origin(VmaOrigin::from_bits(VmaOrigin::EXEC));
                    let interp_pages = interp_vma.page_count() as u64;
                    let _ = vma_mgr.add(interp_vma);
                    new_addr_space.add_total_vm(interp_pages);
                }
            }
        }
    }

    // Update process
    // SAFETY: task_ptr is the current task, valid throughout this execve operation.
    // We are replacing the task's address space and trap frame for the new program.
    if ec2_on { ec2_vma = crate::arch::cpu::read_time(); }
    unsafe {
        // Capture the new mm's ASID before the Arc takes ownership.
        let new_asid = new_addr_space.asid();

        // Switch to the NEW address space BEFORE dropping the old mm.
        // The old order (set_address_space → switch_mm) had an SMP window:
        // set_address_space drops the old mm's last Arc → teardown frees the
        // old root/table frames → another CPU's exec immediately recycles
        // (and zeroes) one of them → THIS task still executes on the OLD
        // satp for the 17 lines until switch_mm ran → kernel-text fetch
        // fault with the kernel half gone (the pid-309/1842 warmup lockups:
        // 10s soft-lockups, task-list damage, do_wait children-list panics).
        // The exit path (exit_mm) already switches satp before dropping;
        // exec must too.
        // SAFETY: user_ppn is the new image's page-table root with the
        // kernel half mapped (create_user_address_space links it).
        unsafe { crate::mm::switch_address_space(user_ppn, new_asid) };

        // Set new address space (this will drop old Arc if no other
        // references — with satp already on the new root, freeing the old
        // tree is safe for this hart).
        (*task_ptr).set_address_space(Some(alloc::sync::Arc::new(new_addr_space)));

        // Update exe_path
        (*task_ptr).set_exe_path(pathname.as_bytes());

        // Set user stack pointer
        (*task_ptr).set_user_sp(adjusted_stack_top);

        // ===== Return to user mode immediately after successful execve =====
        // After execve returns, sret will jump to new program entry

        // Flush arch thread state (FP registers) before returning to the
        // new image — the old image's FP register values must not leak
        // into the exec'd program (review ARCH, FP residue; flush_thread
        // had zero callers). Sets thread.fs=Off and the live sstatus.FS=Off
        // so the new image's first FP instruction goes through the lazy
        // fpu_init path with zeroed registers.
        crate::arch::process::flush_thread();

        // R11-5: use the entry-captured frame (see fn head).
        if current_regs.is_null() {
            // No trap frame, this is the init process case
            // Need to return via ret_from_fork
            // Success path: the new address space is now owned by the init
            // task — the RAII guard MUST be defused here too. Its drop was
            // freeing PID 1's freshly built page tables straight back to
            // the zone; the frames then got reused by the first kernel/user
            // allocations while init kept executing on them (stale TLB),
            // seeding the whole recycled-frame corruption family.
            core::mem::forget(_guard);
            return Ok(());
        }

        // Directly modify current trap frame
        // SPP = 0 means return to user mode, SPIE = 1 means enable interrupts
        const SR_SPIE: u64 = 1 << 5;

        // SUM is deliberately NOT set (SUM convergence, review SEC): the
        // kernel runs with sstatus.SUM=0 and reaches user memory only via
        // the uaccess exception-table paths.
        //
        // SAFETY: current_regs was obtained from current_pt_regs() and checked for null above.
        // We are modifying the trap frame to set up the new program's entry point and stack.
        unsafe {
            // Entry-frame setup per arch. Both follow the ELF_PLAT_INIT
            // discipline: a fresh image must not inherit the old image's
            // register state — the old frame carried the CALLER's ra, gp,
            // and temporaries; startup code that consumes an unsaved slot
            // (rtld_fini in a5 to __libc_start_main, lazy prologues,
            // assertion backtraces) jumps through them — observed as fetch
            // faults at parent-image addresses right after a successful
            // exec. Zero everything except the ABI-defined inputs.
            #[cfg(feature = "riscv64")]
            {
                (*current_regs).epc = actual_entry;         // Entry point (interpreter or program)
                (*current_regs).sp = adjusted_stack_top;   // New user stack
                (*current_regs).status = SR_SPIE;          // Clear SPP, set SPIE
                (*current_regs).tp = 0;                   // Clear TLS pointer - musl libc will reinitialize
                (*current_regs).a0 = argc;                 // argc for C runtime

                let r = &mut *current_regs;
                r.ra = 0;
                r.gp = 0;
                r.t0 = 0; r.t1 = 0; r.t2 = 0; r.t3 = 0; r.t4 = 0; r.t5 = 0; r.t6 = 0;
                r.s0 = 0; r.s1 = 0;
                r.s2 = 0; r.s3 = 0; r.s4 = 0; r.s5 = 0;
                r.s6 = 0; r.s7 = 0; r.s8 = 0; r.s9 = 0; r.s10 = 0; r.s11 = 0;
                r.a1 = 0; r.a2 = 0; r.a3 = 0; r.a4 = 0;
                r.a5 = 0; r.a6 = 0; r.a7 = 0;
            }
            #[cfg(feature = "x86_64")]
            {
                let r = &mut *current_regs;
                // Fresh zeroed frame: entry, user stack, user segments,
                // rflags.IF, and argc in the System V first-argument slot.
                *r = crate::arch::pt_regs::PtRegs::new();
                r.rip = actual_entry;
                r.rsp = adjusted_stack_top;
                r.mark_user_frame();
                r.rflags = 1 << 9; // IF
                r.rdi = argc;
            }

            {
            use crate::arch::mm::mm_ops::PageTableWalker;
            if let Some((gp, _)) = unsafe { PageTableWalker::walk(user_ppn, 0x17208) } {
                LAST_GOT_PPN.store(gp, core::sync::atomic::Ordering::Relaxed);
            }
        }
        // Pin ti_kernel_sp to THIS frame: exec defines the task's only
            // user context from here on. If the syscall entered with a
            // stale ksp (frame pushed off-slot), every later user trap
            // and the final sret must anchor to the exec-updated frame,
            // not to wherever the entry slot happened to land.
            (*task_ptr).set_ti_kernel_sp(current_regs as u64 + core::mem::size_of::<crate::arch::pt_regs::PtRegs>() as u64);

            // Other registers remain 0

        }

        // Note: Do not free PtRegs memory here because trap frame is on stack
    }

    // Wake vfork parent — child has replaced its address space with a new
    // program, so the parent can safely resume execution.
    // SAFETY: task_ptr is the current task, valid throughout execve.  The
    // vfork_parent field (if set) points to the parent that created this
    // child via CLONE_VFORK; the parent is blocked in UNINTERRUPTIBLE sleep.
    unsafe {
        crate::process::task::vfork_wake_parent(task_ptr);
    }

    // Success — prevent RAII guard from freeing page tables
    if ec2_on {
        let t = crate::arch::cpu::read_time();
        crate::pr_info!(
            "EXECCOST2 n={} pid={} as={} map={} stk={} cpy={} tig={} itr={} fill={} vma={} sw={} tot={}",
            ec2_n,
            crate::process::current_pid(),
            ec2_as - ec2_t0,
            ec2_map - ec2_as,
            ec2_stack - ec2_map,
            ec2_copy - ec2_stack,
            ec2_tig - ec2_copy,
            ec2_it - ec2_tig,
            ec2_fill - ec2_it,
            ec2_vma - ec2_fill,
            t - ec2_vma,
            t - ec2_t0
        );
    }
    core::mem::forget(_guard);
    Ok(())
}
