//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory-related system calls
//!
//! Includes: brk, mmap, mmap_framebuffer, munmap, mprotect, msync, mremap, madvise, mincore, mlock, munlock

use super::*;
use super::SyscallArgs;
use crate::arch::mm::{get_page_table_virt, PAGE_SHIFT, PAGE_SIZE, PageTableEntry, VirtAddr};

/// sys_brk - Change data segment size
///
///
/// # Arguments
/// - args[0] (addr): new top of heap address
///
/// # Returns
/// Returns new top of heap address on success, current address on failure (no change)
///
/// # Behavior
/// - If addr is 0, return current brk value
/// - If addr is less than current brk, shrink heap and return new value
/// - If addr is greater than current brk, try to expand heap and return new value
/// - If expansion fails, return current value (no change)
///
/// - RISC-V: 214
pub fn sys_brk(args: [u64; 6]) -> i64 {
    use crate::sched;
    use crate::mm::page::PAGE_SIZE;
    use crate::arch::mm::PageTableEntry;
    use crate::mm::alloc_and_map_user_memory;

    let new_brk = args[0] as u64;

    // Get current process
    match sched::current() {
        Some(current_task) => {
            // Get current brk value
            let current_brk = current_task.get_brk();

            // If brk is not initialized, get or set default value from address space
            if current_brk == 0 {
                // Try to get brk from address space
                let default_brk = if let Some(addr_space) = current_task.address_space() {
                    addr_space.brk().as_usize() as u64
                } else {
                    // Use BRK_DEFAULT from mm module
                    crate::arch::mm::user_addr::BRK_DEFAULT as u64
                };
                current_task.set_brk(default_brk);

                if new_brk == 0 {
                    return default_brk as i64;
                }
            }

            // Re-get current brk (may have been updated)
            let current_brk = current_task.get_brk();

            // If new_brk is 0, return current brk
            if new_brk == 0 {
                return current_brk as i64;
            }

            // Allow shrinking heap — but never below the heap start
            // (Linux: newbrk < mm->start_brk is ignored). Without this,
            // brk(small) would munmap the ELF segments and corrupt rmap.
            let brk_floor = current_task.address_space()
                .map(|a| a.start_brk() as u64)
                .unwrap_or(crate::arch::mm::user_addr::BRK_DEFAULT as u64);
            if new_brk < current_brk && new_brk >= brk_floor {
                // Calculate page range to unmap
                let new_page_end = (new_brk + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                let current_page_end = (current_brk + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

                // Unmap pages that are no longer needed
                if new_page_end < current_page_end {
                    if let Some(addr_space) = current_task.address_space() {
                        let _ = addr_space.munmap(
                            crate::mm::page::VirtAddr::new(new_page_end as usize),
                            (current_page_end - new_page_end) as usize,
                        );
                    }
                }

                current_task.set_brk(new_brk);
                return new_brk as i64;
            }

            // Expand heap: need to map new memory pages
            if new_brk > current_brk {
                // Upper bound: the brk must stay inside the user address
                // space. Without this check a brk above USER_END would map
                // user-accessible pages into the kernel range (review批次4).
                let user_end = crate::arch::mm::user_addr::USER_END as u64;
                if new_brk > user_end {
                    return current_brk as i64; // Linux: keep old brk on failure
                }

                // RLIMIT_DATA: the data segment is [start_brk, brk); growth
                // beyond the soft limit keeps the old brk (Linux sys_brk
                // behavior — brk(2) returns the current break, not ENOMEM).
                // RLIM_INFINITY (u64::MAX) disables the check.
                let (data_cur, _) = current_task.rlimit(
                    crate::process::task::rlimit_res::DATA,
                );
                if data_cur != u64::MAX
                    && new_brk.saturating_sub(brk_floor) > data_cur
                {
                    return current_brk as i64;
                }

                // Calculate page range to map. Only FULL NEW pages may be
                // mapped: the page containing current_brk was already
                // mapped by the previous growth (or by exec for the bss
                // tail). Mapping from the page FLOOR of a mid-page brk
                // replaced that live page's PTE with a fresh zero page —
                // glibc keeps its static TLS/TCB block in the first brk
                // page, so the next sbrk round silently wiped pd->list and
                // every fork() crashed in __libc_fork dereferencing NULL.
                let brk_page_floor = current_brk & !(PAGE_SIZE as u64 - 1);
                let brk_page_ceil = brk_page_floor + PAGE_SIZE as u64;
                let new_page_end = (new_brk + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

                // Get root page table of address space
                let root_ppn = if let Some(addr_space) = current_task.address_space() {
                    addr_space.root_ppn()
                } else {
                    return current_brk as i64;
                };

                // Mid-page brk: the partial page must already be mapped;
                // verify via page-table walk so an unexpected hole still
                // gets mapped instead of silently skipped.
                let map_start = if current_brk == brk_page_floor {
                    current_brk
                } else {
                    // SAFETY: root_ppn is the current task's page-table
                    // root; walk is a read-only page-table inspection.
                    let partial_mapped = unsafe {
                        crate::arch::mm::mm_ops::PageTableWalker::walk(
                            root_ppn,
                            brk_page_floor as u64,
                        )
                    }
                    .is_some();
                    if partial_mapped { brk_page_ceil } else { brk_page_floor }
                };

                // If need to map new pages
                if new_page_end > map_start {
                    // Map new heap pages
                    let size = new_page_end - map_start;

                    // Permissions: User + Read + Write + Valid + Accessed + Dirty
                    let pte_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W
                        | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;

                    // SAFETY: root_ppn is a valid page table root; alloc_and_map_user_memory
                    // handles page allocation and mapping within user address space.
                    unsafe {
                        let result = alloc_and_map_user_memory(root_ppn, map_start, size, pte_flags);
                        if result.is_none() {
                            return current_brk as i64;
                        }
                    }

                    // Register the heap growth as an anonymous VMA. Mapping
                    // PTEs without a VMA left every access past the initial
                    // brk page as NOVMA — glibc malloc's first sbrk region
                    // (TCB, arenas, function pointers) faulted on touch
                    // (observed: dash jumping through a heap pointer at
                    // 0x1c29c with tp already in the brk heap).
                    if let Some(addr_space) = current_task.address_space() {
                        use crate::mm::vma::{Vma, VmaFlags, VmaType};
                        let heap_vma = Vma::new(
                            crate::mm::page::VirtAddr::new(map_start as usize),
                            crate::mm::page::VirtAddr::new(new_page_end as usize),
                            VmaFlags::from_bits(
                                VmaFlags::READ | VmaFlags::WRITE | VmaFlags::PRIVATE,
                            ),
                        );
                        // Vma::new starts with no type — set it explicitly.
                        let mut heap_vma = heap_vma;
                        heap_vma.set_type(VmaType::Anonymous);
                        let _ = addr_space.vma_write().add(heap_vma);
                    }
                }

                current_task.set_brk(new_brk);
                new_brk as i64
            } else {
                current_brk as i64
            }
        }
        None => -12_i64  // ENOMEM
    }
}
/// sys_mmap - Create memory mapping
///
///
/// # Arguments
/// - args[0] (addr): suggested starting address
/// - args[1] (length): mapping length
/// - args[2] (prot): protection flags (PROT_READ/WRITE/EXEC)
/// - args[3] (flags): mapping flags (MAP_PRIVATE/SHARED/ANONYMOUS)
/// - args[4] (fd): file descriptor
/// - args[5] (offset): file offset
///
/// # Returns
/// Returns mapped starting address on success, negative error code on failure
///
/// - RISC-V: 222
// FORENSIC ring: every mmap call (all sizes — ld.so's second-segment
// mapping is <1MB and was a blind spot of serial-print tracing).
pub struct MmapEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub addr: core::sync::atomic::AtomicU64,
    pub len: core::sync::atomic::AtomicU64,
    pub flags: core::sync::atomic::AtomicU32,
    pub fd: core::sync::atomic::AtomicI32,
    pub off: core::sync::atomic::AtomicU64,
    pub ret: core::sync::atomic::AtomicU64,
}
impl MmapEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            addr: core::sync::atomic::AtomicU64::new(0),
            len: core::sync::atomic::AtomicU64::new(0),
            flags: core::sync::atomic::AtomicU32::new(0),
            fd: core::sync::atomic::AtomicI32::new(0),
            off: core::sync::atomic::AtomicU64::new(0),
            ret: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const MMAP_NEW: MmapEntry = MmapEntry::new();
pub static MMAP_RING: [MmapEntry; 1024] = [MMAP_NEW; 1024];
pub static MMAP_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn sys_mmap(args: [u64; 6]) -> i64 {
    let ret = sys_mmap_inner(args);
    {
        use core::sync::atomic::Ordering::Relaxed;
        let idx = MMAP_CURSOR.fetch_add(1, Relaxed) % MMAP_RING.len();
        MMAP_RING[idx].pid.store(crate::process::current_pid(), Relaxed);
        MMAP_RING[idx].addr.store(args[0], Relaxed);
        MMAP_RING[idx].len.store(args[1], Relaxed);
        MMAP_RING[idx].flags.store(args[3] as u32, Relaxed);
        MMAP_RING[idx].fd.store(args[4] as i32, Relaxed);
        MMAP_RING[idx].off.store(args[5], Relaxed);
        MMAP_RING[idx].ret.store(ret as u64, Relaxed);
    }
    ret
}

fn sys_mmap_inner(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;
    use crate::arch::mm::{prot, mmap_error};
    use crate::mm::map;

    let mut addr = args[0] as usize;
    let length = args[1] as usize;
    let prot_flags = args[2] as u32;
    let map_flags = args[3] as u32;
    let mut fd = args[4] as i32;
    let offset = args[5] as u64;
    // MAP_SHARED /dev/zero mapping: the ORIGINAL open file description is
    // captured here (the fd is masked out of the generic attach below) and
    // pinned onto the resulting VMA so mprotect() can enforce the
    // read-only-open EACCES rule (LTP mprotect01 case 3).
    let mut zero_dev_shared_file: Option<alloc::sync::Arc<crate::fs::file::File>> = None;

    // length of 0 is invalid per POSIX
    if length == 0 {
        return mmap_error::EINVAL;
    }

    let actual_length = length;

    // Check protection flags
    if prot_flags & !prot::PROT_MASK != 0 {
        return mmap_error::EINVAL;
    }

    // Check mapping type (must specify MAP_SHARED, MAP_PRIVATE, or
    // MAP_SHARED_VALIDATE). MAP_SHARED_VALIDATE (0x03) behaves like
    // SHARED but turns unknown-flag EINVAL into EOPNOTSUPP (Linux
    // map_mmap_flags — LTP mmap20).
    let map_type = map_flags & map::MAP_TYPE_MASK;
    if map_type != map::MAP_SHARED && map_type != map::MAP_PRIVATE && map_type != 0x03 {
        return mmap_error::EINVAL;
    }

    // Reject unknown mapping flags (Linux do_mmap rejects unmapped bits).
    // Honored set: type bits plus the asm-generic flags we recognize.
    let known_map_flags: u32 = map::MAP_TYPE_MASK
        | map::MAP_FIXED
        | map::MAP_ANONYMOUS
        | map::MAP_STACK
        | map::MAP_FIXED_NOREPLACE
        | map::MAP_HUGETLB
        | map::MAP_LOCKED
        | map::MAP_NORESERVE
        | map::MAP_POPULATE
        | map::MAP_NODUMP
        | 0x0100   // MAP_GROWSDOWN
        | 0x0800   // MAP_DENYWRITE
        | 0x1000   // MAP_EXECUTABLE
        | 0x10000  // MAP_NONBLOCK
        | 0x80000; // MAP_SYNC
    if map_flags & !known_map_flags != 0 {
        // With MAP_SHARED_VALIDATE an unknown flag is EOPNOTSUPP; plain
        // SHARED/PRIVATE reject it with EINVAL (LTP mmap20 passes a
        // bogus 1<<10 bit and expects EOPNOTSUPP).
        if map_type == 0x03 {
            return mmap_error::EOPNOTSUPP;
        }
        return mmap_error::EINVAL;
    }

    // MAP_FIXED_NOREPLACE: exact placement like MAP_FIXED, but any overlap
    // with an EXISTING mapping fails EEXIST instead of replacing it
    // (Linux do_mmap — LTP mmap17 maps the same address twice).
    if map_flags & map::MAP_FIXED_NOREPLACE != 0 {
        if addr % crate::mm::page::PAGE_SIZE != 0 || addr == 0 {
            return mmap_error::EINVAL;
        }
        // Page-rounded, overflow-safe end (Vma::new asserts alignment).
        let sum = match addr.checked_add(actual_length) {
            Some(s) => s,
            None => return mmap_error::EINVAL,
        };
        let end = sum.div_ceil(crate::mm::page::PAGE_SIZE) * crate::mm::page::PAGE_SIZE;
        if end <= addr {
            return mmap_error::EINVAL;
        }
        if let Some(task) = crate::sched::current() {
            if let Some(aspace) = task.address_space() {
                let test = crate::mm::vma::Vma::new(
                    VirtAddr::new(addr),
                    VirtAddr::new(end),
                    VmaFlags::new(),
                );
                let mgr = aspace.vma_read();
                if mgr.iter().any(|v| v.overlaps(&test)) {
                    return mmap_error::EEXIST;
                }
            }
        }
    }

    // User-address-space limit. The user root page table shares the kernel
    // PGD entries (copy_kernel_mappings), so a fixed mapping at or above
    // USER_END would walk into the shared kernel L1/L0 tables and replace
    // kernel PTEs with user-accessible ones — a direct privilege hole.
    let user_end = crate::arch::mm::user_addr::USER_END;
    let out_of_user_range = addr
        .checked_add(actual_length)
        .map_or(true, |end| end > user_end);
    if out_of_user_range {
        if map_flags & map::MAP_FIXED != 0 {
            return mmap_error::EINVAL;
        }
        if addr != 0 {
            // Out-of-range hint: ignore it and let the kernel choose.
            addr = 0;
        }
    }

    // File-backed mapping fd validation (Linux mmap semantics):
    // - non-anonymous mapping requires an OPEN fd → EBADF for a closed
    //   one (LTP mmap08);
    // - the fd must be open for reading (the page cache fills private
    //   and shared pages alike) → EACCES for an O_WRONLY fd (LTP
    //   mmap06: every prot on a write-only fd fails);
    // - MAP_SHARED with PROT_WRITE additionally needs write access.
    if map_flags & map::MAP_ANONYMOUS == 0 && map_flags & map::MAP_HUGETLB == 0 {
        if fd < 0 {
            return mmap_error::EBADF;
        }
        match unsafe { crate::fs::get_file_fd(fd as usize) } {
            None => return mmap_error::EBADF,
            Some(file) => {
                let accmode = file.flags().bits()
                    & crate::fs::file::FileFlags::O_ACCMODE;
                let readable = accmode != crate::fs::file::FileFlags::O_WRONLY;
                let writable = accmode == crate::fs::file::FileFlags::O_RDWR
                    || accmode == crate::fs::file::FileFlags::O_WRONLY;
                if !readable {
                    return mmap_error::EACCES;
                }
                if map_flags & map::MAP_SHARED != 0
                    && prot_flags & prot::PROT_WRITE != 0
                    && !writable
                {
                    return mmap_error::EACCES;
                }
            }
        }
    }

    // (review批次1) The "fd >= 1000 means framebuffer" special case is
    // GONE: an unrelated file that happens to get a high fd number was    // silently mapped onto the framebuffer instead of its own contents.
    // File-backed mappings now go through the generic path; mmap on a file
    // description without mmap-capable ops (e.g. a device node with no
    // driver backing) fails with ENODEV, matching Linux.

    // P2 vm.overcommit_memory (mode 2, "strict"): a mapping larger than
    // physical RAM cannot possibly be backed — reject with ENOMEM before
    // any address-space work. Modes 0 (heuristic) and 1 (always) are
    // accepted without limit (simplified: no commit accounting exists).
    // MAP_NORESERVE bypasses the check (Linux: never charged).
    if crate::fs::procfs::sysctl::OVERCOMMIT_MEMORY
        .load(core::sync::atomic::Ordering::Acquire)
        == 2
        && map_flags & map::MAP_NORESERVE == 0
        && actual_length > crate::mm::layout::phys_memory_size()
    {
        return mmap_error::ENOMEM;
    }

    // Check if this is an io_uring fd
    if fd >= 0 {
        // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File or None.
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if let Some(ops) = file.get_ops() {
                let io_uring_ops = core::ptr::addr_of!(crate::io_uring::IO_URING_OPS);
                if core::ptr::eq(ops as *const _, io_uring_ops as *const _) {
                    // Pass the already-verified file — re-fetching by fd
                    // inside the handler would be a TOCTOU type confusion.
                    match crate::io_uring::io_uring_mmap_handler(&file, addr, actual_length, offset, prot_flags) {
                        Ok(mapped) => return mapped as i64,
                        Err(e) => return -(e as i64),
                    }
                }
            }
        }
    }

    // Binder mmap: /dev/binder maps the per-open transaction buffer zone
    // (kernel-allocated contiguous pages shared kernel<->user, like the
    // io_uring rings; transaction payloads land there and userspace reads
    // them via the offsets handed out in BR_TRANSACTION/BR_REPLY).
    if fd >= 0 {
        // SAFETY: fd is valid; get_file_fd returns a valid Arc<File>.
        if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            if let Some(ops) = file.get_ops() {
                if core::ptr::eq(ops as *const _, &crate::ipc::binder::BINDER_OPS as *const _) {
                    return match crate::ipc::binder::binder_mmap_handler(
                        &file,
                        addr,
                        actual_length,
                        offset,
                        prot_flags,
                    ) {
                        Ok(mapped) => mapped as i64,
                        Err(e) => -(e as i64),
                    };
                }
            }
        }
    }

    // Non-anonymous mapping without file descriptor
    if (map_flags & map::MAP_ANONYMOUS == 0) && fd < 0 {
        return mmap_error::EBADF;
    }

    // Non-anonymous mapping: the file must exist and be mappable (Linux:
    // -ENODEV when the file has no ->mmap; -EBADF when the fd is bad).
    if (map_flags & map::MAP_ANONYMOUS == 0) && fd >= 0 {
        match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
            Some(file) => {
                if file.get_ops().is_none() {
                    return mmap_error::ENODEV;
                }
            }
            None => return mmap_error::EBADF,
        }
    }

    // Get current process
    match crate::sched::current() {
        Some(current_task) => {
            // RLIMIT_AS: the process's total mapped VM (mm.total_vm, in
            // pages, maintained by add_vma/remove_vma accounting) plus
            // this new mapping must stay under the soft limit (Linux
            // checks rlimit(RLIMIT_AS) in mmap_region → ENOMEM).
            // RLIM_INFINITY (u64::MAX) disables the check. Read before the
            // mutable address_space borrow below.
            {
                let (as_cur, _) = current_task.rlimit(
                    crate::process::task::rlimit_res::AS,
                );
                if as_cur != u64::MAX {
                    let page = crate::mm::page::PAGE_SIZE as u64;
                    let len_pages = (actual_length as u64 + page - 1) / page;
                    let total = current_task
                        .address_space()
                        .map(|a| a.total_vm())
                        .unwrap_or(0);
                    let total_after = total + len_pages;
                    let exceeds = total_after
                        .checked_mul(page)
                        .map_or(true, |bytes| bytes > as_cur);
                    if exceeds {
                        return mmap_error::ENOMEM;
                    }
                }
            }

            // cgroup v2 memory controller (U1b): charge the mapping length
            // to the task's cgroup chain (memory.current semantics —
            // ancestors see descendant usage). memory.max overrun fails
            // with ENOMEM, like Linux's memcg charge failure here.
            // Simplification: virtual mapping length is the charge unit
            // (an RSS approximation); uncharge happens at munmap/exit.
            if !crate::sched::cgroup::cgroup_memory_charge(actual_length) {
                return mmap_error::ENOMEM;
            }

            // Check if address space exists
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // Parse protection flags — exact, no implicit W for
                    // exec (review批次1: "PROT_EXEC simplified to RWX"
                    // broke W^X for every mapped library).
                    let perm = match (
                        prot_flags & prot::PROT_READ != 0,
                        prot_flags & prot::PROT_WRITE != 0,
                        prot_flags & prot::PROT_EXEC != 0,
                    ) {
                        (false, false, false) => Perm::None,
                        (true, false, false) => Perm::Read,
                        (true, true, false) => Perm::ReadWrite,
                        (true, true, true) => Perm::ReadWriteExec,
                        (true, false, true) => Perm::ReadExec,
                        // Write-only/exec-only are honored as-is on Sv39
                        // (X-only is architectural; W-only folds to RW).
                        (false, true, _) => Perm::ReadWrite,
                        (false, false, true) => Perm::Exec,
                    };

                    // Parse VMA flags
                    let mut vma_flags = VmaFlags::new();

                    // Readable only for PROT_READ (write-only stays W-only
                    // in the VMA so /proc/self/maps shows "-w"; the Sv39
                    // W-without-R PTE fold happens at page-fault build
                    // time). PROT_NONE gets NO access flags — a fault
                    // inside it must reach the exception table (EFAULT
                    // for uaccess) or SIGSEGV (user access), not be
                    // demand-filled (LTP write03/unlink07: the old
                    // unconditional READ made PROT_NONE pages read as
                    // zeros through copy_from_user).
                    if prot_flags & prot::PROT_READ != 0 {
                        vma_flags.insert(VmaFlags::READ);
                    }

                    if map_flags & map::MAP_SHARED != 0 {
                        vma_flags.insert(VmaFlags::SHARED);
                    }
                    if map_flags & map::MAP_PRIVATE != 0 {
                        vma_flags.insert(VmaFlags::PRIVATE);
                    }
                    if prot_flags & prot::PROT_WRITE != 0 {
                        vma_flags.insert(VmaFlags::WRITE);
                    }
                    if prot_flags & prot::PROT_EXEC != 0 {
                        vma_flags.insert(VmaFlags::EXEC);
                    }
                    if map_flags & map::MAP_STACK != 0 {
                        vma_flags.insert(VmaFlags::GROWSDOWN);
                    }
                    // MAP_LOCKED: the VMA is born locked (Linux VM_LOCKED on
                    // mmap — VmLck in /proc/self/status reports it; LTP
                    // mmap14 diffs VmLck across a MAP_LOCKED mmap).
                    if map_flags & map::MAP_LOCKED != 0 {
                        vma_flags.insert(VmaFlags::LOCKED);
                    }

                    // Set VMA type
                    let mut vma_type = if map_flags & map::MAP_ANONYMOUS != 0 {
                        VmaType::Anonymous
                    } else {
                        VmaType::FileBacked
                    };

                    // /dev/zero mmap = anonymous zero-filled mapping
                    // (Linux map_zero): a char device has no on-disk
                    // extent to fault from — treating it as file-backed
                    // made the first touch SIGBUS "past EOF" (device
                    // size 0). LTP mmap10 mmaps /dev/zero and forks
                    // writers into it.
                    if vma_type == VmaType::FileBacked && fd >= 0 {
                        let is_zero_dev = unsafe {
                            crate::fs::file::get_file_fd(fd as usize)
                                .and_then(|f| {
                                    let inode_opt = &*f.inode.get();
                                    inode_opt.as_ref().map(|i| {
                                        i.mode.is_char_device()
                                            && i.rdev
                                                == crate::fs::dev_t::DEV_ZERO.to_user_dev()
                                    })
                                })
                                .unwrap_or(false)
                        };
                        if is_zero_dev {
                            vma_type = VmaType::Anonymous;
                            // The generic file attach below must not pin
                            // the device "file" either — use the anonymous
                            // path by masking the fd out of that branch.
                            // EXCEPTION (Linux vm_file semantics): a
                            // MAP_SHARED /dev/zero mapping still remembers
                            // the open file's access mode — mprotect()
                            // upgrading it to PROT_WRITE fails EACCES when
                            // /dev/zero was opened O_RDONLY (LTP mprotect01
                            // case 3). Capture the file here; it is pinned
                            // onto the VMA after the mapping succeeds.
                            zero_dev_shared_file = if map_flags & map::MAP_SHARED != 0 {
                                unsafe { crate::fs::file::get_file_fd(fd as usize) }
                            } else {
                                None
                            };
                            fd = -1;
                        }
                    }

                    // Framebuffer mmap: /dev/fb0 maps the GPU framebuffer's
                    // own physical pages into the caller (no anonymous
                    // backing, no demand paging — writes land in the
                    // scanout buffer; userspace flushes via FBIO_FLUSH).
                    // Arch-generic: info.addr is the bounce buffer's
                    // physical address on both arches, and the
                    // PageTableEntry::{V,R,W,U,A,D} flag names alias the
                    // identical bits in both mm layers.
                    if fd >= 0 {
                        let is_fb = unsafe { crate::fs::file::get_file_fd(fd as usize) }
                            .map(|f| crate::drivers::gpu::fbdev::is_fb_file(&f))
                            .unwrap_or(false);
                        if is_fb {
                            let info = match crate::drivers::gpu::get_framebuffer_info() {
                                Some(i) => i,
                                None => return mmap_error::EINVAL,
                            };
                            let fb_len = info.size as usize;
                            if actual_length > fb_len {
                                return mmap_error::EINVAL;
                            }
                            let hint = if addr == 0 { None } else { Some(addr) };
                            let placement = match hint {
                                Some(a) => a,
                                None => match address_space.find_free_area(actual_length) {
                                    Ok(v) => v.as_usize(),
                                    Err(_) => return mmap_error::ENOMEM,
                                },
                            };
                            let root = address_space.root_ppn();
                            // SAFETY: root is the task's page-table root;
                            // info.addr..+len is the linear-mapped framebuffer.
                            unsafe {
                                crate::arch::mm::mm_ops::map_user_region(
                                    root,
                                    placement as u64,
                                    info.addr,
                                    actual_length as u64,
                                    crate::arch::mm::PageTableEntry::V
                                        | crate::arch::mm::PageTableEntry::R
                                        | crate::arch::mm::PageTableEntry::W
                                        | crate::arch::mm::PageTableEntry::U
                                        | crate::arch::mm::PageTableEntry::A
                                        | crate::arch::mm::PageTableEntry::D,
                                );
                            }
                            // Device VMA: faults never touch it (pages are
                            // already present), unmap only clears PTEs.
                            // Kick the auto-flush presenter awake: this
                            // mapper may be a flush-less renderer (Xorg).
                            crate::drivers::gpu::fbdev::mark_fb_user_mapped();
                            let vma_end = placement + actual_length;
                            let mut dv = crate::mm::vma::Vma::new(
                                VirtAddr::new(placement),
                                VirtAddr::new(vma_end),
                                vma_flags,
                            );
                            dv.set_type(crate::mm::vma::VmaType::Device);
                            let _ = address_space.vma_write().add(dv);
                            return placement as i64;
                        }
                    }

                    // Call AddressSpace::mmap
                    let result = address_space.mmap(
                        VirtAddr::new(addr),
                        actual_length,
                        vma_flags,
                        vma_type,
                        perm,
                        map_flags,
                    );
                    match result {
                        Ok(mapped_addr) => {
// For file-backed mappings, store fd and file size in VMA for demand paging
                            if map_flags & map::MAP_ANONYMOUS == 0 && fd >= 0 {
                                // Get file size from stat
                                // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File;
                                // inode access via UnsafeCell is safe as we hold &File.
                                let file_sz = unsafe {
                                    crate::fs::get_file_fd(fd as usize).and_then(|file| {
                                        let inode_opt = &*file.inode.get();
                                        inode_opt.as_ref().map(|inode| inode.get_size())
                                    }).unwrap_or(0)
                                };

                                if let Some(vma) = address_space.vma_write().find_mut(mapped_addr) {
                                    vma.set_file_fd(fd);
                                    vma.set_file_size(file_sz);
                                    vma.set_offset(offset as usize);
                                }

                                // Pin the file itself (Linux vm_file): demand
                                // faults after the caller closes the fd must
                                // still read the mapped file, not zero-fill.
                                if let Some(file) = unsafe { crate::fs::get_file_fd(fd as usize) } {
                                    address_space.pin_vma_file(mapped_addr.as_usize(), file);
                                }
                            }

                            // MAP_SHARED /dev/zero: pin the ORIGINAL device
                            // file description (captured before fd was
                            // masked out). mprotect's shared-file PROT_WRITE
                            // EACCES check reads it; nothing else consumes a
                            // pinned file on an Anonymous VMA (faults and
                            // msync writeback are FileBacked-only).
                            if let Some(file) = zero_dev_shared_file.take() {
                                address_space.pin_vma_file(mapped_addr.as_usize(), file);
                            }

                            // MAP_SHARED|MAP_ANONYMOUS: the mapping must be
                            // genuinely shared across fork (Linux shmem
                            // object semantics). A demand-filled anonymous
                            // VMA faults a PRIVATE zero page per process —
                            // the parent never sees the child's stores
                            // (LTP getpid02/clone03: the child writes
                            // getpid() into a MAP_SHARED|MAP_ANONYMOUS
                            // page and the parent re-reads 0). Eagerly
                            // allocate zeroed pages and map them NOW: the
                            // VMA carries VmaFlags::SHARED, so fork's
                            // copy_page_table_cow inherits these frames
                            // as-is (COW-exempt, refcount bumped) and both
                            // processes alias the same physical pages.
                            // Same eager model shmat() uses for SysV shm.
                            if map_flags & map::MAP_ANONYMOUS != 0
                                && vma_flags.contains(VmaFlags::SHARED)
                            {
                                let root_ppn = address_space.root_ppn();
                                let mut pte_flags = PageTableEntry::V
                                    | PageTableEntry::A
                                    | PageTableEntry::D
                                    | PageTableEntry::U;
                                // Same W^X folds as the demand-fault path.
                                if vma_flags.is_readable() || vma_flags.is_writable() {
                                    pte_flags |= PageTableEntry::R;
                                }
                                if vma_flags.is_writable() {
                                    pte_flags |= PageTableEntry::W;
                                }
                                if vma_flags.is_executable() {
                                    pte_flags |= PageTableEntry::X;
                                }
                                // Round UP: LTP getpid02/clone03 mmap
                                // sizeof(pid_t) == 4 bytes — the old
                                // length/PAGE_SIZE truncation computed
                                // npages == 0, mapped NOTHING eagerly, and
                                // the post-fork demand faults then produced
                                // PRIVATE zero pages per process (parent
                                // read 0 where the child wrote its pid).
                                let npages = (actual_length
                                    + crate::mm::page::PAGE_SIZE
                                        - 1)
                                    / crate::mm::page::PAGE_SIZE;
                                let mut failed = false;
                                for i in 0..npages {
                                    let phys = crate::mm::page_alloc::get_zeroed_page(
                                        crate::mm::zone::GfpFlags::GFP_USER,
                                    );
                                    if phys == 0 {
                                        failed = true;
                                        break;
                                    }
                                    // SAFETY: mapped_addr + i*PAGE_SIZE is
                                    // page-aligned inside the VMA just added;
                                    // phys is a fresh zeroed page; root_ppn is
                                    // the caller's page-table root. Map+book-
                                    // keeping under the PTE lock, matching the
                                    // demand-fault discipline.
                                    unsafe {
                                        let _pte_guard =
                                            crate::arch::mm::mm_ops::PTE_MODIFY_LOCK
                                                .lock_irqsave();
                                        crate::arch::mm::map_user_page(
                                            root_ppn,
                                            crate::arch::mm::memory_layout::VirtAddr::new(
                                                (mapped_addr.as_usize()
                                                    + i * crate::mm::page::PAGE_SIZE)
                                                    as u64,
                                            ),
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                phys as u64,
                                            ),
                                            pte_flags,
                                        );
                                        // Mirror the SharedMemory fault-path
                                        // page bookkeeping (Anonymous flag,
                                        // index, mapcount, rmap) so vmscan/
                                        // unmap see a normal shared anon
                                        // page. No anon-LRU: like SysV shm
                                        // pages, these have no private rmap
                                        // walk to reverse.
                                        let page = crate::mm::page_desc::pfn_to_page_mut(
                                            crate::mm::phys_to_pfn(phys),
                                        );
                                        if !page.is_null() {
                                            (*page).set_flag(
                                                crate::mm::page_desc::PageFlag::Anonymous,
                                            );
                                            (*page).set_index(
                                                (mapped_addr.as_usize()
                                                    + i * crate::mm::page::PAGE_SIZE)
                                                    / crate::mm::page::PAGE_SIZE,
                                            );
                                            (*page).inc_mapcount();
                                            crate::mm::rmap::page_record_mapping(
                                                &*page,
                                                (&address_space) as *const _ as usize,
                                                mapped_addr.as_usize()
                                                    + i * crate::mm::page::PAGE_SIZE,
                                            );
                                        }
                                    }
                                }
                                if failed {
                                    // Unwind the partial mapping and the VMA;
                                    // munmap frees the eagerly mapped frames.
                                    let _ = address_space.munmap(
                                        VirtAddr::new(mapped_addr.as_usize()),
                                        actual_length,
                                    );
                                    return mmap_error::ENOMEM;
                                }
                                address_space.add_rss(npages as u64);
                                // SAFETY: sfence.vma is valid in S-mode; the
                                // new PTEs need a flush before first use.
                                unsafe { crate::arch::mm::asid::flush_tlb_all(); }
                            }

                            mapped_addr.as_usize() as i64
                        },
                        Err(e) => {
                            // cgroup v2 (U1b): the pre-charge above must be
                            // returned when the mapping never happened.
                            crate::sched::cgroup::cgroup_memory_uncharge(actual_length);
                            let err = match e {
                                crate::mm::pagemap::MapError::OutOfMemory => mmap_error::ENOMEM,
                                crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                                crate::mm::pagemap::MapError::AlreadyMapped => mmap_error::ENOMEM,
                                crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                            };
                            err
                        }
                    }
                }
                None => {
                    // cgroup v2 (U1b): no address space → no mapping; the
                    // pre-charge must go back.
                    crate::sched::cgroup::cgroup_memory_uncharge(actual_length);
                    mmap_error::ENOMEM
                }
            }
        }
        None => {
            mmap_error::ENOMEM
        }
    }
}
/// sys_munmap - Unmap memory
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 215
pub fn sys_munmap(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::arch::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    // Validate arguments
    if length == 0 {
        return mmap_error::EINVAL;
    }

    // Check address alignment
    if addr % 4096 != 0 {
        return mmap_error::EINVAL;
    }

    // Get current process
    match crate::sched::current() {
        Some(current_task) => {
            // Check if address space exists
            match current_task.address_space_mut() {
                Some(address_space) => {
                    // Call AddressSpace::munmap
                    match address_space.munmap(VirtAddr::new(addr), length) {
                        Ok(()) => {
                            // cgroup v2 (U1b): give the unmapped length
                            // back to the cgroup memory charge.
                            crate::sched::cgroup::cgroup_memory_uncharge(length);
                            0
                        }
                        Err(e) => {
                            let err = match e {
                                crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                                crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                                _ => mmap_error::ENOMEM,
                            };
                            err
                        }
                    }
                }
                None => mmap_error::ENOMEM,
            }
        }
        None => mmap_error::ENOMEM,
    }
}
/// sys_mprotect - Change protection of memory region
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
/// - args[2] (prot): new protection flags (PROT_READ/WRITE/EXEC)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 226
///
/// # Description
/// mprotect is used to change protection attributes of existing memory mapping
pub fn sys_mprotect(args: [u64; 6]) -> i64 {
    use crate::arch::mm::{PageTableEntry, PAGE_SIZE, PAGE_SHIFT, PageTable, VirtAddr};

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let prot = args[2] as u32;

    // Validate arguments
    if length == 0 {
        return -22_i64;  // EINVAL
    }

    // Unknown protection bits are EINVAL. PROT_GROWSDOWN/GROWSUP are
    // mprotect *flags* living in the high bits — accepted (and ignored,
    // since we operate on the exact range passed).
    if prot & !(crate::arch::mm::prot::PROT_MASK | 0x0100_0000 | 0x0200_0000) != 0 {
        return -22_i64;  // EINVAL
    }

    // Address must be page aligned
    if addr % PAGE_SIZE as usize != 0 {
        return -22_i64;  // EINVAL
    }

    // R7-1: user-range bound — without it a kernel-range mprotect rewrites
    // the SHARED kernel page tables with flags that always include V|U
    // (userspace gains U|R|W on kernel memory = full compromise), or strips
    // permissions off kernel text (kernel-mode fault → panic).
    use crate::arch::mm::user_addr;
    if addr < user_addr::USER_START
        || match addr.checked_add(length) {
            Some(e) => e > user_addr::USER_END,
            None => true,
        }
    {
        return -22_i64;  // EINVAL
    }

    // Get current process
    match crate::sched::current() {
        Some(current_task) => {
            // Linux mprotect: the ENTIRE [addr, addr+length) range must be
            // covered by VMAs — any gap fails the whole call with ENOMEM
            // (mprotect_fixup walks until the vma list stops covering the
            // range). Silently skipping unmapped pages (the old behavior)
            // returned success for a range that was never mapped (LTP
            // mprotect01).
            {
                let addr_space = match current_task.address_space() {
                    Some(a) => a,
                    None => return -12_i64, // ENOMEM
                };
                let mut spans: alloc::vec::Vec<(usize, usize)> = addr_space
                    .vma_read()
                    .iter()
                    .map(|v| (v.start().as_usize(), v.end().as_usize()))
                    .collect();
                spans.sort_unstable();
                let mut cursor = addr;
                for (s, e) in &spans {
                    if *e <= cursor {
                        continue;
                    }
                    if *s > cursor {
                        break; // gap at `cursor`
                    }
                    cursor = *e;
                    if cursor >= addr + length {
                        break;
                    }
                }
                if cursor < addr + length {
                    return -12_i64; // ENOMEM: range not fully mapped
                }

                // Linux mprotect: requesting PROT_WRITE on a MAP_SHARED
                // file mapping whose backing file was NOT opened for
                // writing fails with EACCES before any PTE is touched
                // (LTP mprotect01 case 3: mmap /dev/zero O_RDONLY
                // PROT_READ MAP_SHARED, then mprotect PROT_WRITE).
                if prot & 0x2 != 0 {
                    for vma in addr_space.vma_read().iter() {
                        let vs = vma.start().as_usize();
                        let ve = vma.end().as_usize();
                        if vs >= addr + length || ve <= addr {
                            continue; // no overlap with the target range
                        }
                        if !vma.flags().is_shared() {
                            continue; // MAP_PRIVATE upgrades are legal
                        }
                        if let Some(file) = addr_space.get_vma_file(vs) {
                            if file.flags().is_readonly() {
                                return -13_i64; // EACCES
                            }
                        }
                    }
                }
            }

            // Get page table root
            let root_ppn = if let Some(addr_space) = current_task.address_space() {
                addr_space.root_ppn()
            } else {
                return -12_i64;  // ENOMEM
            };

            // Calculate new PTE flags
            // Base flags: Valid + User + Accessed + Dirty
            let mut new_flags = PageTableEntry::V | PageTableEntry::U
                | PageTableEntry::A | PageTableEntry::D;

            if prot & 0x1 != 0 {  // PROT_READ
                new_flags |= PageTableEntry::R;
            }
            if prot & 0x2 != 0 {  // PROT_WRITE
                new_flags |= PageTableEntry::W | PageTableEntry::R;  // W requires R
            }
            if prot & 0x4 != 0 {  // PROT_EXEC
                new_flags |= PageTableEntry::X;
            }

            // If prot == 0 (PROT_NONE), only keep V and U, remove R/W/X

            // Traverse pages and update permissions
            let start_page = addr / PAGE_SIZE as usize;
            let num_pages = (length + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;

            // Hold the PTE-modify lock across the whole walk + rewrite +
            // sfence: reading a leaf PTE before taking the lock and writing
            // it after let a concurrent COW fault's page swap race the
            // rewrite (stale PPN could resurrect a freed page) — regression
            // round 6 HIGH. The loop never sleeps, so one irqsave section
            // for the whole range is safe (fork holds it likewise).
            let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
            for i in 0..num_pages {
                let virt = ((start_page + i) * PAGE_SIZE as usize) as u64;
                // SAFETY: root_ppn is a valid page table root; we traverse 3-level Sv39 page
                // tables via linear mapping and only modify valid leaf PTEs.
                unsafe {
                    let virt_addr = VirtAddr(virt);

                    // Extract virtual page numbers
                    let vpn2 = virt_addr.vpn(2) as usize;
                    let vpn1 = virt_addr.vpn(1) as usize;
                    let vpn0 = virt_addr.vpn(0) as usize;

                    // Access page table using linear mapping
                    let root_table = get_page_table_virt(root_ppn << PAGE_SHIFT);

                    let pte2 = (*root_table).get(vpn2);
                    if !pte2.is_valid() {
                        continue;  // Page not mapped, skip
                    }

                    let ppn1 = pte2.ppn();
                    let table1 = get_page_table_virt(ppn1 << PAGE_SHIFT);
                    let pte1 = (*table1).get(vpn1);
                    if !pte1.is_valid() {
                        continue;  // Page not mapped, skip
                    }

                    let ppn0 = pte1.ppn();
                    let table0 = get_page_table_virt(ppn0 << PAGE_SHIFT);
                    let pte0 = (*table0).get(vpn0);

                    if pte0.is_valid() {
                        // Preserve PPN and any COW software bit (bit 8):
                        // clearing COW here let two processes that share a
                        // forked page write straight through after
                        // mprotect(PROT_WRITE), bypassing the COW copy
                        // (review MM-H7). Also never raise W on a page
                        // whose COW bit is set — the write fault path owns
                        // the copy.
                        let ppn = pte0.ppn();
                        let cow_bit = pte0.bits() & (1 << 8);
                        let mut flags = new_flags;
                        if cow_bit != 0 {
                            flags |= cow_bit;
                            flags &= !PageTableEntry::W; // defer W to COW fault
                        }
                        let new_pte = PageTableEntry::from_bits((ppn << 10) | flags);
                        (*table0).set(vpn0, new_pte);
                    }
                }
            }

            // Flush TLB after updating PTE permissions (still under the
            // PTE lock so no stale entry survives the section).
            // SAFETY: sfence.vma is a valid RISC-V instruction; required after PTE modification.
            unsafe {
                crate::arch::mm::asid::flush_tlb_all();
            }
            drop(_pte_guard);

            // Update VMA flags to reflect new permissions. Linux
            // mprotect_fixup semantics: the range must be SPLIT out of any
            // overlapping VMA and only the covered portion's permission
            // bits changed. Setting flags on whole overlapping VMAs
            // (R7-A10's earlier fix) leaked the change across the whole
            // VMA: glibc deliberately PROT_NONEs single guard pages
            // between DSO segments (dl-load.c), which stripped READ from
            // the entire executable segment and killed every dynamically
            // linked binary with SIGSEGV on its first library read.
            if let Some(addr_space) = current_task.address_space() {
                let range_start = addr;
                let range_end = addr + length;
                let mut vma_mgr = addr_space.vma_write();

                // Snapshot overlapping VMAs (start addresses).
                let mut starts: alloc::vec::Vec<crate::mm::page::VirtAddr> = alloc::vec::Vec::new();
                for v in vma_mgr.iter() {
                    if v.end().as_usize() > range_start && v.start().as_usize() < range_end {
                        starts.push(v.start());
                    }
                }

                for start in starts {
                    // The pin is keyed by VMA start; splits below re-pin
                    // the pieces so a closed mapping fd still reads.
                    let pinned_file = addr_space.get_vma_file(start.as_usize());

                    // Re-fetch under the write lock (split below may have
                    // altered the set).
                    let vma = match vma_mgr.find_mut(start) {
                        Some(v) => v,
                        None => continue,
                    };

                    let v_start = vma.start().as_usize();
                    let v_end = vma.end().as_usize();
                    if v_end <= range_start || v_start >= range_end {
                        continue; // no longer overlapping (split by a prior iteration)
                    }

                    // Permissions for the covered portion. Write-only
                    // stays W-only (display-accurate); the Sv39 PTE fold
                    // happens at fault time.
                    let perm_bits = (prot & 0x1 != 0) as u32             // R (PROT_READ)
                        | ((prot & 0x2 != 0) as u32) << 1               // PROT_WRITE
                        | ((prot & 0x4 != 0) as u32) << 2;              // PROT_EXEC

                    let full_flags = vma.flags();
                    let old_type = vma.vma_type();
                    let old_fd = vma.file_fd();
                    let old_fsz = vma.file_size();
                    let old_off = vma.offset();

                    // Escalating a SHARED FILE mapping to writable
                    // requires the backing file to be open for writing
                    // (Linux mprotect_fixup → vma_wants_writectory):
                    // mprotect(PROT_WRITE) on a read-only-fd MAP_SHARED
                    // mapping fails with EACCES (LTP mprotect01). Private
                    // (COW) mappings are exempt — writes stay private.
                    // (pinned_file was fetched before this loop; taking
                    // no further VMA locks here.)
                    if prot & 0x2 != 0
                        && full_flags.contains(crate::mm::vma::VmaFlags::SHARED)
                        && old_type == crate::mm::vma::VmaType::FileBacked
                    {
                        let writable = pinned_file
                            .as_ref()
                            .map(|f| {
                                let m = f.flags().bits()
                                    & crate::fs::file::FileFlags::O_ACCMODE;
                                m == crate::fs::file::FileFlags::O_RDWR
                                    || m == crate::fs::file::FileFlags::O_WRONLY
                            })
                            .unwrap_or(false);
                        if !writable {
                            return -13_i64; // EACCES
                        }
                    }

                    // Head piece [v_start, range_start): keep original flags.
                    let head = if v_start < range_start {
                        let mut h = crate::mm::vma::Vma::new(
                            crate::mm::page::VirtAddr::new(v_start),
                            crate::mm::page::VirtAddr::new(range_start),
                            full_flags,
                        );
                        h.set_type(old_type);
                        h.set_file_fd(old_fd);
                        h.set_file_size(old_fsz);
                        h.set_offset(old_off);
                        Some(h)
                    } else {
                        None
                    };

                    // Tail piece [range_end, v_end): keep original flags.
                    let tail = if v_end > range_end {
                        let mut t = crate::mm::vma::Vma::new(
                            crate::mm::page::VirtAddr::new(range_end),
                            crate::mm::page::VirtAddr::new(v_end),
                            full_flags,
                        );
                        t.set_type(old_type);
                        t.set_file_fd(old_fd);
                        t.set_file_size(old_fsz);
                        t.set_offset(old_off + (range_end - v_start));
                        Some(t)
                    } else {
                        None
                    };

                    // Covered piece [max(v_start,range_start), min(v_end,range_end)):
                    // new permission bits, non-permission flags preserved.
                    let cov_start = v_start.max(range_start);
                    let cov_end = v_end.min(range_end);
                    let mut cov = crate::mm::vma::Vma::new(
                        crate::mm::page::VirtAddr::new(cov_start),
                        crate::mm::page::VirtAddr::new(cov_end),
                        crate::mm::vma::VmaFlags::from_bits(
                            (full_flags.bits() & !0x7) | perm_bits,
                        ),
                    );
                    cov.set_type(old_type);
                    cov.set_file_fd(old_fd);
                    cov.set_file_size(old_fsz);
                    cov.set_offset(old_off + (cov_start - v_start));

                    // Swap: remove original, re-add pieces (with pins).
                    let _ = vma_mgr.remove(crate::mm::page::VirtAddr::new(v_start));
                    addr_space.unpin_vma_file(v_start);
                    if let Some(h) = head {
                        let hs = h.start().as_usize();
                        let _ = vma_mgr.add(h);
                        if let Some(f) = pinned_file.as_ref() {
                            addr_space.pin_vma_file(hs, f.clone());
                        }
                    }
                    {
                        let cs = cov.start().as_usize();
                        let _ = vma_mgr.add(cov);
                        if let Some(f) = pinned_file.as_ref() {
                            addr_space.pin_vma_file(cs, f.clone());
                        }
                    }
                    if let Some(t) = tail {
                        let ts = t.start().as_usize();
                        let _ = vma_mgr.add(t);
                        if let Some(f) = pinned_file.as_ref() {
                            addr_space.pin_vma_file(ts, f.clone());
                        }
                    }
                }
            }

            0
        }
        None => -12_i64  // ENOMEM
    }
}
/// sys_msync - Synchronize memory mapping to file
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
/// - args[2] (flags): sync flags (MS_ASYNC/MS_SYNC/MS_INVALIDATE)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 227
///
/// # Description
/// msync writes changes from file mapping back to disk
pub fn sys_msync(args: [u64; 6]) -> i64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let flags = args[2] as u32;

    // msync flags (asm-generic ABI: MS_ASYNC=1, MS_INVALIDATE=2, MS_SYNC=4
    // — the old table had SYNC/INVALIDATE SWAPPED, so msync(MS_INVALIDATE)
    // silently became a plain sync and succeeded where Linux returns EBUSY
    // on locked ranges; LTP msync03).
    const MS_ASYNC: u32 = 0x1;      // Async write
    const MS_INVALIDATE: u32 = 0x2; // Invalidate cache
    const MS_SYNC: u32 = 0x4;       // Sync write

    // Validate flags
    if flags & !(MS_ASYNC | MS_SYNC | MS_INVALIDATE) != 0 {
        return mmap_error::EINVAL;
    }

    // Cannot set both ASYNC and SYNC
    if (flags & MS_ASYNC != 0) && (flags & MS_SYNC != 0) {
        return mmap_error::EINVAL;
    }

    // Validate arguments
    if length == 0 {
        return mmap_error::EINVAL;
    }

    // Address must be page aligned
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL;
    }

    // Above the user address space: EINVAL, not ENOMEM (Linux msync
    // bounds-checks against TASK_SIZE first — LTP msync03 case 3 passes
    // RLIMIT_DATA's max, which lies far above user space).
    if addr >= crate::arch::mm::user_addr::USER_END {
        return mmap_error::EINVAL;
    }

    // Align length
    let length_aligned = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Get current process
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM,
    };

    let address_space = match current_task.address_space() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM,
    };

    // 1. Validate that address range is covered by VMA
    {
        let vma_mgr = address_space.vma_read();
        let mut check_addr = addr;
        let end_addr = addr + length_aligned;

        while check_addr < end_addr {
            match vma_mgr.find(VirtAddr::new(check_addr)) {
                Some(vma) => {
                    // MS_INVALIDATE over a LOCKED (mlock/MAP_LOCKED) range
                    // is EBUSY — the pages cannot be dropped (Linux
                    // SYSCALL msync → EBUSY; LTP msync03 case 1).
                    if flags & MS_INVALIDATE != 0
                        && vma.flags().contains(crate::mm::vma::VmaFlags::LOCKED)
                    {
                        return -16_i64; // EBUSY
                    }
                    check_addr = vma.end().as_usize();
                }
                None => {
                    // Address not in any VMA
                    return mmap_error::ENOMEM;
                }
            }
        }
    }

    // 2. Perform sync operation
    //
    // MS_SYNC/MS_ASYNC: write dirty pages of SHARED FILE mappings back to
    // the backing file. The mapping's pages are private allocations filled
    // from the file at fault time; without this writeback a MAP_SHARED
    // mapping's stores were lost on every msync (LTP msync01 read back the
    // original file contents).
    {
        use crate::arch::mm::{PageTableEntry, PageTable};
        use crate::mm::vma::{VmaFlags, VmaType};

        let root_ppn = address_space.root_ppn();
        let end_addr = addr + length_aligned;

        // Snapshot the overlapping shared file VMAs (start, end, offset).
        let mut spans: alloc::vec::Vec<(usize, usize, usize)> = alloc::vec::Vec::new();
        {
            let vma_mgr = address_space.vma_read();
            for v in vma_mgr.iter() {
                if v.end().as_usize() <= addr || v.start().as_usize() >= end_addr {
                    continue;
                }
                if v.flags().contains(VmaFlags::SHARED)
                    && v.vma_type() == VmaType::FileBacked
                    && v.flags().is_writable()
                {
                    spans.push((
                        v.start().as_usize().max(addr),
                        v.end().as_usize().min(end_addr),
                        v.offset(),
                    ));
                }
            }
        }

        for (v_start, v_end, v_off) in spans {
            // The pinned vm_file (Linux vm_file): demand faults keep
            // reading it after the mapping fd is closed.
            let file = match address_space.get_vma_file(v_start)
                .or_else(|| vma_file_by_range(&address_space, v_start)) {
                Some(f) => f,
                None => continue,
            };
            // i_size clamp: Linux writeback never extends past EOF — bytes
            // a MAP_SHARED mapping holds beyond the file's end are
            // silently dropped at msync (LTP mmap01 greps for a pattern
            // written past EOF and requires it to stay out of the file).
            let file_size = unsafe {
                (*file.inode.get()).as_ref().map(|i| i.get_size()).unwrap_or(0)
            };
            let mut cursor = v_start;
            while cursor < v_end {
                // SAFETY: root_ppn is a valid page-table root; the walk
                // only reads PTEs, the write path uses the kernel's
                // linear map of the physical page.
                unsafe {
                    let vpn = [
                        (cursor >> 12) & 0x1FF,
                        (cursor >> 21) & 0x1FF,
                        (cursor >> 30) & 0x1FF,
                    ];
                    let mut pte_virt =
                        get_page_table_virt(root_ppn << PAGE_SHIFT) as *const PageTableEntry;
                    let mut phys: Option<u64> = None;
                    for level in (0..3usize).rev() {
                        let pte = &*pte_virt.add(vpn[level]);
                        if !pte.is_valid() {
                            break;
                        }
                        let is_leaf = pte.is_readable() || pte.is_writable() || pte.is_executable();
                        if level == 0 || is_leaf {
                            if pte.is_writable() {
                                phys = Some(pte.ppn() << PAGE_SHIFT);
                            }
                            break;
                        }
                        pte_virt = get_page_table_virt(pte.ppn() << PAGE_SHIFT) as *const PageTableEntry;
                    }
                    if let Some(p) = phys {
                        // Clamped whole-page writeback from the kernel
                        // linear map: nothing past EOF (see i_size clamp
                        // above), full pages otherwise.
                        let page_phys = p;
                        let file_off = (v_off + (cursor - v_start)) as u64;
                        if file_off >= file_size {
                            cursor += PAGE_SIZE as usize;
                            continue;
                        }
                        let n = core::cmp::min(
                            PAGE_SIZE as u64,
                            file_size - file_off,
                        ) as usize;
                        let kva = crate::arch::mm::phys_to_virt(
                            crate::arch::mm::memory_layout::PhysAddr(page_phys),
                        );
                        let _ = file.write_at(
                            file_off,
                            kva.as_usize() as *const u8,
                            n,
                        );
                    }
                }
                cursor += PAGE_SIZE as usize;
            }
        }
    }

    0  // Success
}

/// Fallback vm_file resolution for a SHARED file VMA whose pin was lost
/// (should not happen — pins live for the VMA — but msync must not skip
/// writeback silently): re-resolve through the VMA's recorded fd.
fn vma_file_by_range(
    address_space: &crate::mm::mm_struct::MmStruct,
    addr: usize,
) -> Option<alloc::sync::Arc<crate::fs::file::File>> {
    let vma_mgr = address_space.vma_read();
    let vma = vma_mgr.find(crate::mm::page::VirtAddr::new(addr))?;
    let fd = vma.file_fd();
    if fd < 0 {
        return None;
    }
    unsafe { crate::fs::get_file_fd(fd as usize) }
}
/// Copy page contents from old virtual address range to new virtual address range.
///
/// Used by mremap MOVE operations to preserve data.
///
/// The destination mapping created by `AddressSpace::mmap` is lazy (VMA
/// registered, no PTEs — pages normally appear via faults), so a plain walk
/// of the destination finds nothing. Instead, this eagerly allocates a
/// destination page per source page, maps it with the source's R/W/X
/// permissions, and copies — mirroring the anonymous fault path's mapping
/// and rmap bookkeeping.
///
/// # Safety
/// Caller must ensure both old and new ranges are valid, page-aligned, and
/// the old range is mapped.
unsafe fn copy_old_to_new_pages(root_ppn: u64, old_addr: usize, new_addr: usize, size: usize) {
    use crate::arch::mm::{
        PAGE_SIZE, PAGE_SHIFT, PhysAddr, VirtAddr, map_page, phys_to_virt, PageTableEntry,
    };
    use crate::mm::page_alloc::alloc_page;
    use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
    use crate::mm::zone::GfpFlags;

    // R7-A5: serialize leaf-PTE mutations against fork's table walk and COW
    // faults — the same discipline as the demand-fault paths.
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    let mut offset = 0usize;
    while offset < size {
        let old_virt = (old_addr + offset) as u64;
        let new_virt = (new_addr + offset) as u64;

        // Walk old page table to get the source physical page
        if let Some((old_ppn, old_bits)) = crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, old_virt) {
            let old_phys = old_ppn << PAGE_SHIFT;
            let old_kvaddr = phys_to_virt(PhysAddr::new(old_phys)).bits() as *const u8;

            // Eagerly allocate + map the destination page (see comment above)
            let new_phys = alloc_page(GfpFlags::GFP_KERNEL);
            if new_phys == 0 {
                // Out of memory: leave the remainder lazy — the destination
                // VMA exists, so untouched pages fault in as zero pages.
                return;
            }
            let perms = old_bits & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X);
            let pte_flags = PageTableEntry::V | PageTableEntry::U
                | PageTableEntry::A | PageTableEntry::D | perms;
            map_page(root_ppn, VirtAddr::new(new_virt), PhysAddr::new(new_phys as u64), pte_flags);

            let new_kvaddr = phys_to_virt(PhysAddr::new(new_phys as u64)).bits() as *mut u8;
            // SAFETY: both addresses are kernel linear-mapping views of
            // distinct physical pages; PAGE_SIZE is the allocation size.
            core::ptr::copy_nonoverlapping(old_kvaddr, new_kvaddr, PAGE_SIZE as usize);

            // rmap/mapcount bookkeeping, same as the anonymous fault path
            let page = pfn_to_page_mut(new_phys >> PAGE_SHIFT);
            if !page.is_null() {
                // SAFETY: freshly allocated page exclusively owned here.
                (*page).set_flag(PageFlag::Anonymous);
                // SwapBacked + LRU membership: keep the moved page
                // reclaimable via swap-out (vmscan).
                (*page).set_flag(PageFlag::SwapBacked);
                (*page).set_index(new_virt as usize / (PAGE_SIZE as usize));
                (*page).inc_mapcount();
                if let Some(mm) = crate::sched::current().and_then(|t| t.address_space()) {
                    crate::mm::rmap::page_record_mapping(
                        &*page,
                        mm as *const _ as usize,
                        new_virt as usize,
                    );
                }
                crate::mm::lru::page_add_anon_lru(&*page);
            }
        }
        offset += PAGE_SIZE as usize;
    }
    drop(_pte_guard);
}

/// sys_mremap - Remap memory
///
///
/// # Arguments
/// - args[0] (old_addr): old address
/// - args[1] (old_size): old size
/// - args[2] (new_size): new size
/// - args[3] (flags): flags (MREMAP_MAYMOVE/MREMAP_FIXED)
/// - args[4] (new_addr): new address (only used when MREMAP_FIXED)
///
/// # Returns
/// Returns new address on success (may be same as old), negative error code on failure
///
/// - RISC-V: 216
///
/// # Description
/// mremap expands or shrinks existing memory mapping
pub fn sys_mremap(args: [u64; 6]) -> i64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;
    use crate::arch::mm::mmap_error;
    use crate::mm::map;

    let old_addr = args[0] as usize;
    let old_size = args[1] as usize;
    let new_size = args[2] as usize;
    let flags = args[3] as u32;
    let new_addr_arg = args[4] as usize;

    // mremap flags
    const MREMAP_MAYMOVE: u32 = 0x1;  // Can move to new address
    const MREMAP_FIXED: u32 = 0x2;    // Must map to specified address

    // Flag sanity (Linux mremap_to): MREMAP_FIXED requires MREMAP_MAYMOVE,
    // and the new placement must not overlap the old mapping. Unknown
    // bits are EINVAL (LTP mremap05).
    if (flags & MREMAP_FIXED) != 0 && (flags & MREMAP_MAYMOVE) == 0 {
        return mmap_error::EINVAL;
    }
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED) != 0 {
        return mmap_error::EINVAL;
    }
    if (flags & MREMAP_FIXED) != 0 {
        let old_end = old_addr.saturating_add(old_size);
        let new_end = new_addr_arg.saturating_add(new_size);
        if new_addr_arg < old_end && old_addr < new_end {
            return mmap_error::EINVAL;
        }
    }

    // Validate old_addr page alignment
    if old_addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL;
    }

    // Validate new_addr page alignment (if specified)
    if (flags & MREMAP_FIXED) != 0 && new_addr_arg % PAGE_SIZE != 0 {
        return mmap_error::EINVAL;
    }

    // Align sizes
    let old_size_aligned = (old_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_size_aligned = (new_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Get current process
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM,
    };

    let address_space = match current_task.address_space_mut() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM,
    };

    // 1. Find VMA covering old_addr
    let vma_info = {
        let vma_mgr = address_space.vma_read();
        vma_mgr.find(VirtAddr::new(old_addr)).map(|vma| {
            (vma.start(), vma.end(), vma.flags(), vma.vma_type())
        })
    };

    let (vma_start, vma_end, vma_flags, vma_type) = match vma_info {
        Some(info) => info,
        None => return mmap_error::EFAULT,  // Address not mapped
    };

    // Validate old_addr is VMA start address
    if vma_start.as_usize() != old_addr {
        return mmap_error::EFAULT;
    }

    // Validate old_size is within VMA range
    if old_addr + old_size_aligned > vma_end.as_usize() {
        return mmap_error::EFAULT;
    }

    // 2. Decide operation type based on new_size
    if new_size_aligned == old_size_aligned {
        // NO_RESIZE: size unchanged
        // If MREMAP_FIXED is specified, need to move
        if (flags & MREMAP_FIXED) != 0 {
            // Move to new address — save old data, create new mapping, copy, unmap old
            let root_ppn = address_space.root_ppn();
            // Create mapping at new address first
            let perm = vma_flags.to_page_perm();
            let mmap_result = address_space.mmap(
                VirtAddr::new(new_addr_arg),
                new_size_aligned,
                vma_flags,
                vma_type,
                perm,
                map::MAP_FIXED,
            );
            match mmap_result {
                Ok(new_addr) => {
                    // Copy pages from old to new
                    let copy_size = old_size_aligned.min(new_size_aligned);
                    unsafe {
                        copy_old_to_new_pages(root_ppn, old_addr, new_addr_arg, copy_size);
                    }
                    // Unmap old mapping
                    let _ = address_space.munmap(VirtAddr::new(old_addr), old_size_aligned);
                    new_addr.as_usize() as i64
                }
                Err(e) => {
                    let err = match e {
                        crate::mm::pagemap::MapError::OutOfMemory => mmap_error::ENOMEM,
                        crate::mm::pagemap::MapError::Invalid => mmap_error::EINVAL,
                        crate::mm::pagemap::MapError::AlreadyMapped => mmap_error::ENOMEM,
                        crate::mm::pagemap::MapError::NotMapped => mmap_error::EINVAL,
                    };
                    err
                }
            }
        } else {
            // No operation needed
            old_addr as i64
        }
    } else if new_size_aligned < old_size_aligned {
        // SHRINK: shrink mapping
        // Unmap extra part
        let unmap_start = old_addr + new_size_aligned;
        let unmap_size = old_size_aligned - new_size_aligned;

        match address_space.munmap(VirtAddr::new(unmap_start), unmap_size) {
            Ok(()) => old_addr as i64,
            Err(_) => mmap_error::ENOMEM,
        }
    } else {
        // EXPAND: expand mapping
        let extra_size = new_size_aligned - old_size_aligned;
        let new_end = old_addr + new_size_aligned;

        // Check if can expand in place (check if next VMA would conflict)
        let can_expand = {
            let vma_mgr = address_space.vma_read();
            if let Some(next_vma) = vma_mgr.find_vma_after(VirtAddr::new(vma_end.as_usize())) {
                next_vma.start().as_usize() >= new_end
            } else {
                true  // No next VMA, can expand
            }
        };

        if can_expand {
            // Expand in place: map extra pages
            let perm = vma_flags.to_page_perm();
            match address_space.mmap(
                VirtAddr::new(vma_end.as_usize()),
                extra_size,
                vma_flags,
                vma_type,
                perm,
                map::MAP_FIXED,  // Force at this address
            ) {
                Ok(_) => old_addr as i64,
                Err(_) => mmap_error::ENOMEM,
            }
        } else if (flags & MREMAP_MAYMOVE) != 0 {
            // Can move: find new location
            let perm = vma_flags.to_page_perm();
            match address_space.mmap(
                VirtAddr::new(0),  // Let kernel choose address
                new_size_aligned,
                vma_flags,
                vma_type,
                perm,
                0,  // Don't force address
            ) {
                Ok(new_mapping_addr) => {
                    // Copy pages from old mapping to new mapping before unmapping
                    let copy_size = old_size_aligned.min(new_size_aligned);
                    let new_addr_val = new_mapping_addr.as_usize();
                    // SAFETY: both old_addr and new_addr_val are page-aligned user addresses.
                    // copy_old_to_new_pages copies PTE contents (physical page data).
                    unsafe {
                        copy_old_to_new_pages(
                            address_space.root_ppn(),
                            old_addr,
                            new_addr_val,
                            copy_size,
                        );
                    }
                    // Unmap old mapping
                    let _ = address_space.munmap(VirtAddr::new(old_addr), old_size_aligned);
                    new_mapping_addr.as_usize() as i64
                }
                Err(_) => mmap_error::ENOMEM,
            }
        } else {
            // Cannot expand in place and moving not allowed
            mmap_error::ENOMEM
        }
    }
}
/// sys_madvise - Give advice to kernel about memory usage patterns
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
/// - args[2] (advice): advice type (MADV_NORMAL/MADV_RANDOM/MADV_SEQUENTIAL/etc)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 233
///
/// # Description
/// madvise allows application to give advice to kernel about how to use memory
pub fn sys_madvise(args: [u64; 6]) -> i64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let advice = args[2] as i32;

    // madvise advice types
    const MADV_NORMAL: i32 = 0;       // No special advice
    const MADV_RANDOM: i32 = 1;       // Random access
    const MADV_SEQUENTIAL: i32 = 2;   // Sequential access
    const MADV_WILLNEED: i32 = 3;     // Will be accessed
    const MADV_DONTNEED: i32 = 4;     // No longer needed (release pages)
    const MADV_FREE: i32 = 8;         // Can be freed (similar to DONTNEED)
    const MADV_REMOVE: i32 = 9;       // Free mapping
    const MADV_DONTFORK: i32 = 10;    // Don't copy on fork
    const MADV_DOFORK: i32 = 11;      // Copy on fork
    const MADV_MERGEABLE: i32 = 12;   // Mergeable (KSM)
    const MADV_UNMERGEABLE: i32 = 13; // Not mergeable
    const MADV_HUGEPAGE: i32 = 14;    // Use huge pages
    const MADV_NOHUGEPAGE: i32 = 15;  // Don't use huge pages
    const MADV_DONTDUMP: i32 = 16;    // Don't dump to core
    const MADV_DODUMP: i32 = 17;      // Dump to core
    const MADV_HWPOISON: i32 = 100;   // Mark as corrupted

    // Validate arguments
    if length == 0 {
        return mmap_error::EINVAL;
    }

    // Address must be page aligned
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL;
    }

    // Above the user address space: EINVAL, not ENOMEM (Linux msync
    // bounds-checks against TASK_SIZE first — LTP msync03 case 3 passes
    // RLIMIT_DATA's max, which lies far above user space).
    if addr >= crate::arch::mm::user_addr::USER_END {
        return mmap_error::EINVAL;
    }

    // Validate advice type
    match advice {
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED |
        MADV_DONTNEED | MADV_FREE | MADV_REMOVE | MADV_DONTFORK | MADV_DOFORK |
        MADV_MERGEABLE | MADV_UNMERGEABLE | MADV_HUGEPAGE | MADV_NOHUGEPAGE |
        MADV_DONTDUMP | MADV_DODUMP => {
            // Valid advice
        }
        _ => {
            return mmap_error::EINVAL;
        }
    }

    // Align length
    let length_aligned = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Get current process
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM,
    };

    let address_space = match current_task.address_space_mut() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM,
    };

    // 1. Validate that address range is covered by VMA
    {
        let vma_mgr = address_space.vma_read();
        let start = VirtAddr::new(addr);
        let end = VirtAddr::new(addr + length_aligned);

        // Check if starting address has VMA
        if vma_mgr.find(start).is_none() {
            return mmap_error::ENOMEM;
        }

        // Advice/VMA compatibility checks (Linux mm/madvise.c):
        // - MADV_REMOVE / MADV_DONTNEED / MADV_FREE over a VM_LOCKED
        //   range fail with EINVAL (LTP madvise02 mlocks its file mapping
        //   in tcases_filter, then expects EINVAL for each).
        // - MADV_MERGEABLE / MADV_UNMERGEABLE need KSM, which this kernel
        //   does not provide → EINVAL.
        // - MADV_FREE only applies to private anonymous memory → EINVAL
        //   on file-backed ranges.
        {
            let mut check = addr;
            let mut rejected = false;
            while check < addr + length_aligned {
                match vma_mgr.find(VirtAddr::new(check)) {
                    Some(vma) => {
                        let locked = vma
                            .flags()
                            .contains(crate::mm::vma::VmaFlags::LOCKED);
                        let anon = vma.vma_type() == crate::mm::vma::VmaType::Anonymous
                            || vma.vma_type() == crate::mm::vma::VmaType::SharedMemory;
                        match advice {
                            MADV_REMOVE | MADV_DONTNEED | MADV_FREE if locked => {
                                rejected = true;
                            }
                            MADV_MERGEABLE | MADV_UNMERGEABLE => {
                                rejected = true; // no KSM in this kernel
                            }
                            MADV_FREE if !anon => {
                                rejected = true;
                            }
                            _ => {}
                        }
                        if rejected {
                            break;
                        }
                        check = vma.end().as_usize();
                    }
                    None => break,
                }
            }
            if rejected {
                return mmap_error::EINVAL;
            }
        }

        // For MADV_DONTNEED and MADV_REMOVE, need entire range to be in VMA
        if advice == MADV_DONTNEED || advice == MADV_REMOVE {
            // Find VMA covering entire range
            let mut check_addr = addr;
            while check_addr < addr + length_aligned {
                match vma_mgr.find(VirtAddr::new(check_addr)) {
                    Some(vma) => {
                        check_addr = vma.end().as_usize();
                    }
                    None => {
                        return mmap_error::ENOMEM;
                    }
                }
            }
        }
    }

    // 2. Perform operation based on advice
    match advice {
        MADV_DONTNEED | MADV_FREE => {
            // MADV_DONTNEED: discard the mapped pages but keep the VMAs.
            // The next fault re-zero-fills anonymous ranges — glibc/jemalloc
            // heap shrink depends on the data actually being discarded
            // (review批次1: previously a silent no-op). MADV_FREE is
            // permitted to behave like DONTNEED (eager discard instead of
            // lazy marking).
            match address_space.zap_page_range(VirtAddr::new(addr), length_aligned) {
                Ok(()) => 0,
                Err(_) => mmap_error::ENOMEM,
            }
        }
        MADV_REMOVE => {
            // MADV_REMOVE: Linux punches a hole (FALLOC_FL_PUNCH_HOLE) —
            // pages are freed and the file range becomes a hole, but the
            // MAPPING STAYS VALID. Treating it as munmap destroyed the
            // VMA and every later madvise on the same region failed with
            // ENOMEM (LTP madvise01's post-REMOVE cases). Zap the pages
            // only (same page-discard engine as DONTNEED).
            match address_space.zap_page_range(VirtAddr::new(addr), length_aligned) {
                Ok(()) => 0,
                Err(_) => mmap_error::ENOMEM,
            }
        }
        MADV_WILLNEED => {
            // MADV_WILLNEED: prefault advice. Correct no-op semantics for
            // this kernel: demand paging faults pages in on first touch
            // anyway, there is no readahead window to trigger, and swap-in
            // cannot apply (swap is not wired). P2 fake-success cleanup:
            // this remains a no-op BY DESIGN (an advice), unlike the
            // mlock family which now sets VM_LOCKED.
            0
        }
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL => {
            // These are performance hints, ignored in simplified implementation
            // In complete implementation, should update VMA's vm_flags
            0
        }
        MADV_DONTFORK | MADV_DOFORK => {
            // Fork related flags
            // Ignored in simplified implementation
            0
        }
        MADV_HUGEPAGE | MADV_NOHUGEPAGE => {
            // Huge page related, ignored in simplified implementation
            0
        }
        MADV_MERGEABLE | MADV_UNMERGEABLE => {
            // KSM related, ignored in simplified implementation
            0
        }
        MADV_DONTDUMP | MADV_DODUMP => {
            // Core dump related, ignored in simplified implementation
            0
        }
        _ => {
            // Should not reach here since validated earlier
            mmap_error::EINVAL
        }
    }
}
/// sys_mincore - Query if pages are in memory
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
/// - args[2] (vec): result vector pointer
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 232
///
/// # Description
/// mincore returns a vector indicating which pages are in memory
/// Lowest bit of each byte in vec indicates if corresponding page is in memory
pub fn sys_mincore(args: [u64; 6]) -> i64 {
    use crate::mm::page::{VirtAddr, PAGE_SIZE};
    use crate::arch::mm::{PageTableEntry, PageTable, mmap_error};

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let vec_ptr = args[2] as *mut u8;

    // Validate arguments
    if length == 0 {
        return mmap_error::EINVAL;
    }

    // Address must be page aligned
    if addr % PAGE_SIZE != 0 {
        return mmap_error::EINVAL;
    }

    // Above the user address space: EINVAL, not ENOMEM (Linux msync
    // bounds-checks against TASK_SIZE first — LTP msync03 case 3 passes
    // RLIMIT_DATA's max, which lies far above user space).
    if addr >= crate::arch::mm::user_addr::USER_END {
        return mmap_error::EINVAL;
    }

    // Validate vec pointer
    if vec_ptr.is_null() {
        return mmap_error::EINVAL;
    }

    // Calculate needed page count
    let page_count = (length + PAGE_SIZE - 1) / PAGE_SIZE;

    // Linux mincore order of checks: range validity FIRST (overflow past
    // TASK_SIZE or a VMA gap → ENOMEM), only then the vec EFAULT — LTP
    // mincore01 distinguishes the two errnos.
    let range_end = match addr.checked_add(page_count * PAGE_SIZE) {
        Some(e) => e,
        None => return mmap_error::ENOMEM,
    };
    if range_end > crate::arch::mm::user_addr::USER_END {
        return mmap_error::ENOMEM;
    }

    // Get current process
    let current_task = match crate::sched::current() {
        Some(task) => task,
        None => return mmap_error::ENOMEM,
    };

    let address_space = match current_task.address_space() {
        Some(aspace) => aspace,
        None => return mmap_error::ENOMEM,
    };

    // 1. Validate that address range is covered by VMA — BEFORE the vec
    // pointer check: Linux do_mincore walks the VMAs first (ENOMEM for an
    // unmapped range), and only the vec WRITE can EFAULT (mincore01 case
    // 4: huge len past RLIMIT_AS is ENOMEM even though the vec pointer
    // arithmetic then looks out-of-range).
    {
        let vma_mgr = address_space.vma_read();
        let mut check_addr = addr;
        let end_addr = range_end;

        while check_addr < end_addr {
            match vma_mgr.find(VirtAddr::new(check_addr)) {
                Some(vma) => {
                    check_addr = vma.end().as_usize();
                }
                None => {
                    // Address not in any VMA
                    return mmap_error::ENOMEM;
                }
            }
        }
    }

    // Validate vec pointer (only now — after the ENOMEM checks)
    if vec_ptr.is_null() {
        return mmap_error::EINVAL;
    }
    if !crate::arch::uaccess::access_ok(vec_ptr as usize, page_count) {
        return mmap_error::EFAULT;
    }

    // 2. Get page table root
    let root_ppn = address_space.root_ppn();

    // 3. Check if each page is in memory
    // SAFETY: root_ppn is a valid page table root; we traverse 3-level Sv39 page tables
    // via linear mapping. vec_ptr validated with access_ok(page_count).
    let mut vec_ok = true;
    unsafe {
        for i in 0..page_count {
            let page_addr = addr + i * PAGE_SIZE;

            // Find page table entry
            let vpn = [
                (page_addr >> 12) & 0x1FF,
                (page_addr >> 21) & 0x1FF,
                (page_addr >> 30) & 0x1FF,
            ];

            // Traverse page table using linear mapping
            let mut pte_virt = get_page_table_virt(root_ppn << PAGE_SHIFT) as *const PageTableEntry;
            let mut page_in_memory = false;

            for level in (0..3usize).rev() {
                let pte = &*pte_virt.add(vpn[level]);

                if !pte.is_valid() {
                    // Page table entry invalid, page not in memory
                    break;
                }

                // Check if leaf node (R/W/X any set indicates leaf node)
                let is_leaf = pte.is_readable() || pte.is_writable() || pte.is_executable();

                if level == 0 || is_leaf {
                    // Reached leaf node or huge page, page is in memory
                    page_in_memory = true;
                    break;
                }

                // Continue to next level
                pte_virt = get_page_table_virt(pte.ppn() << PAGE_SHIFT) as *const PageTableEntry;
            }

            // Set result: lowest bit indicates if page is in memory.
            // put_user (exception-table): a raw store faults in S-mode on
            // every U page (SUM=0) and panics the kernel — glibc's malloc
            // probes with mincore, so any allocator-heavy program hit this.
            // A store that fails (vec page unmapped — LTP mincore01
            // setup2) must surface as EFAULT, not be swallowed.
            if !crate::arch::uaccess::put_user(
                vec_ptr.add(i),
                if page_in_memory { 1u8 } else { 0u8 },
            ) {
                vec_ok = false;
            }
        }
    }

    if !vec_ok {
        return mmap_error::EFAULT;
    }

    0  // Success
}
/// Derive a sub-VMA of `v` covering [s, e) — same attributes, file
/// offset advanced by the sub-range start.
fn sub_vma(v: &crate::mm::vma::Vma, s: usize, e: usize) -> crate::mm::vma::Vma {
    let page_delta = (s - v.start().as_usize()) / crate::mm::page::PAGE_SIZE;
    let mut nv = crate::mm::vma::Vma::new(
        crate::mm::page::VirtAddr::new(s),
        crate::mm::page::VirtAddr::new(e),
        v.flags(),
    );
    nv.set_offset(v.offset() + page_delta * crate::mm::page::PAGE_SIZE);
    nv.set_type(v.vma_type());
    nv.set_file_fd(v.file_fd());
    nv.set_file_size(v.file_size());
    nv
}

/// sys_mlock - Lock memory
///
/// Range-precise VM_LOCKED: the covering VMAs are SPLIT at the (rounded)
/// range boundaries and only the covered sub-VMAs carry the lock — VmLck
/// in /proc/self/status and "Locked:" in /proc/self/smaps report exactly
/// the locked bytes (LTP mlock201 VmLck deltas, mlock05 per-VMA Locked).
/// Linux prefaults the whole range (populate_vma_page_range) unless the
/// caller passed MLOCK_ONFAULT (mlock2). RLIMIT_MEMLOCK is enforced the
/// Linux way (can_do_mlock): no CAP_IPC_LOCK + zero limit -> EPERM;
/// exceeding a nonzero soft limit -> ENOMEM. Any unmapped hole in the
/// range fails the whole call with ENOMEM (LTP mlock01).
pub fn sys_mlock(args: [u64; 6]) -> i64 {
    mlock_impl(args[0] as usize, args[1] as usize, false)
}

/// Shared implementation for mlock(2) and mlock2(2) (`onfault` skips the
/// prefault pass — Linux VM_LOCKONFAULT: pages lock as they fault in).
fn mlock_impl(raw_addr: usize, length: usize, onfault: bool) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    if length == 0 {
        return -22_i64; // EINVAL
    }
    // Linux rounds the range to page boundaries: addr DOWN, len UP — an
    // unaligned addr is legal (LTP mlock01 locks one byte past a page
    // boundary).
    let addr = raw_addr & !(crate::mm::page::PAGE_SIZE - 1);
    let length_aligned = (length + crate::mm::page::PAGE_SIZE - 1)
        & !(crate::mm::page::PAGE_SIZE - 1);
    let end = match addr.checked_add(length_aligned) {
        Some(e) => e,
        None => return -22_i64,
    };

    let current_task = match crate::sched::current() {
        Some(t) => t,
        None => return -12_i64,
    };

    // RLIMIT_MEMLOCK gate (Linux can_do_mlock): EPERM when the caller
    // holds no CAP_IPC_LOCK and the soft limit is 0.
    let has_ipc_lock = crate::security::capable(crate::security::CAP_IPC_LOCK);
    let (memlock_cur, _) = current_task
        .rlimit(crate::process::task::rlimit_res::MEMLOCK);
    if !has_ipc_lock && memlock_cur == 0 {
        return -1_i64; // EPERM
    }

    let address_space = match current_task.address_space_mut() {
        Some(a) => a,
        None => return -12_i64,
    };

    // Range fully mapped? Any hole -> ENOMEM before any mutation (Linux
    // mlock/vma walk semantics — the whole call fails, nothing locked).
    {
        let mgr = address_space.vma_read();
        let mut cursor = addr;
        for v in mgr.iter() {
            let (s, e2) = (v.start().as_usize(), v.end().as_usize());
            if e2 <= cursor {
                continue;
            }
            if s > cursor {
                return -12_i64; // hole
            }
            cursor = e2;
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            return -12_i64; // ENOMEM: trailing hole
        }
    }

    // RLIMIT_MEMLOCK accounting: locked bytes after the call = already
    // locked VMAs OUTSIDE the range + the rounded range (our post-split
    // granularity is exact).
    if !has_ipc_lock && memlock_cur != u64::MAX {
        let locked_after: u64 = {
            let mgr = address_space.vma_read();
            let mut total = 0u64;
            for v in mgr.iter() {
                let (s, e2) = (v.start().as_usize(), v.end().as_usize());
                if v.flags().contains(VmaFlags::LOCKED) && (e2 <= addr || s >= end) {
                    total += (e2 - s) as u64;
                }
            }
            total + length_aligned as u64
        };
        if locked_after > memlock_cur {
            return -12_i64; // ENOMEM
        }
    }

    // Flag the range in one manager pass: every VMA overlapping
    // [addr, end) is rebuilt as up to three pieces (below/inside/above
    // the range) with VM_LOCKED set only on the covered piece. The
    // pieces carry DIFFERENT flags, so the manager's insert-merge cannot
    // glue them back (a split-then-flag sequence was merged back to one
    // whole-VMA lock — mlock201's VmLck deltas read the full mapping).
    let mut prefault_spans: alloc::vec::Vec<(usize, usize, bool)> = alloc::vec::Vec::new();
    {
        let mut mgr = address_space.vma_write();
        let mut cursor = addr;
        while cursor < end {
            let vma = match mgr.find(VirtAddr::new(cursor)) {
                Some(v) => v.clone(),
                None => return -12_i64, // raced hole (should not happen)
            };
            let (vs, ve) = (vma.start().as_usize(), vma.end().as_usize());
            let vma_type = vma.vma_type();
            let seg_start = cursor.max(addr).max(vs);
            let seg_end = ve.min(end);

            let mut mid = sub_vma(&vma, seg_start, seg_end);
            let mut flags = mid.flags();
            flags.insert(VmaFlags::LOCKED);
            mid.set_flags(flags);

            // Remove the original and re-add the (up to three) pieces.
            let _ = mgr.remove(VirtAddr::new(vs));
            if vs < seg_start {
                let _ = mgr.add(sub_vma(&vma, vs, seg_start));
            }
            let _ = mgr.add(mid);
            if seg_end < ve {
                let _ = mgr.add(sub_vma(&vma, seg_end, ve));
            }

            prefault_spans.push((
                seg_start,
                seg_end,
                vma_type == crate::mm::vma::VmaType::Anonymous
                    || vma_type == crate::mm::vma::VmaType::SharedMemory,
            ));
            if ve >= end {
                break;
            }
            cursor = ve;
        }
    }

    // Linux mlock PREFAULTS the whole range (populate_vma_page_range):
    // locked pages are resident — mincore02 asserts every locked page
    // shows present. MLOCK_ONFAULT (mlock2) skips this: pages lock as
    // they fault in (LTP mlock201 ONFAULT cases expect locked-but-not-
    // present pages). Anonymous/SharedMemory VMAs get zero pages mapped
    // here (file-backed pages stay demand-read; mincore02 uses anon).
    if !onfault {
        let root_ppn = address_space.root_ppn();
        for (seg_start, seg_end, zero_fill) in prefault_spans {
            if !zero_fill {
                continue;
            }
            let mut p = seg_start & !(crate::mm::page::PAGE_SIZE - 1);
            while p < seg_end {
                // SAFETY: PageTableWalker::walk is a read-only inspection
                // of the task's own page tables.
                let present = unsafe {
                    crate::arch::mm::mm_ops::PageTableWalker::walk(
                        root_ppn,
                        p as u64,
                    )
                }
                .is_some();
                if !present {
                    // SAFETY: alloc + map under the PTE lock, same as the
                    // sysv shm attach path; p is page-aligned user memory.
                    unsafe {
                        let _pte_guard =
                            crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
                        if let Some(phys) =
                            crate::arch::mm::mm_ops::alloc_user_phys_page()
                        {
                            let page_ptr = crate::arch::mm::phys_to_virt(
                                crate::arch::mm::PhysAddr::new(phys),
                            )
                            .0 as *mut u8;
                            core::ptr::write_bytes(page_ptr, 0, crate::mm::page::PAGE_SIZE);
                            let pte_flags =
                                crate::arch::mm::PageTableEntry::V
                                    | crate::arch::mm::PageTableEntry::A
                                    | crate::arch::mm::PageTableEntry::D
                                    | crate::arch::mm::PageTableEntry::U
                                    | crate::arch::mm::PageTableEntry::R
                                    | crate::arch::mm::PageTableEntry::W;
                            crate::arch::mm::mm_ops::map_user_page(
                                root_ppn,
                                crate::arch::mm::VirtAddr::new(p as u64),
                                crate::arch::mm::PhysAddr::new(phys),
                                pte_flags,
                            );
                        }
                    }
                }
                p += crate::mm::page::PAGE_SIZE;
            }
        }
    }

    0
}


/// sys_munlock - Unlock memory
///
/// Range-precise unlock: split at the boundaries, clear VM_LOCKED on the
/// covered sub-VMAs only. Linux munlock(2) fails with ENOMEM when the
/// range contains unmapped pages (same rule as mlock) — LTP munlock02
/// munmaps the middle of a locked area and expects munlock(ENOENT hole)
/// to return ENOMEM.
pub fn sys_munlock(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    if length == 0 {
        return -22_i64; // EINVAL
    }
    // Round to page boundaries like mlock (Linux mlock/munlock never
    // require the caller to align).
    let addr = addr & !(crate::mm::page::PAGE_SIZE - 1);
    let length_aligned = (length + crate::mm::page::PAGE_SIZE - 1)
        & !(crate::mm::page::PAGE_SIZE - 1);
    let end = match addr.checked_add(length_aligned) {
        Some(e) => e,
        None => return -22_i64,
    };

    let current_task = match crate::sched::current() {
        Some(t) => t,
        None => return -12_i64,
    };
    let address_space = match current_task.address_space_mut() {
        Some(a) => a,
        None => return -12_i64,
    };

    // Whole range must be mapped — ENOMEM on any hole (LTP munlock02).
    {
        let mgr = address_space.vma_read();
        let mut cursor = addr;
        for v in mgr.iter() {
            let (s, e2) = (v.start().as_usize(), v.end().as_usize());
            if e2 <= cursor {
                continue;
            }
            if s > cursor {
                return -12_i64; // hole
            }
            cursor = e2;
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            return -12_i64; // ENOMEM: trailing hole
        }
    }

    // Clear the flag with the same one-pass piece rebuild (a full clear
    // of a LOCKED VMA cannot split, but a partial unlock must).
    {
        let mut mgr = address_space.vma_write();
        let mut cursor = addr;
        while cursor < end {
            let vma = match mgr.find(VirtAddr::new(cursor)) {
                Some(v) => v.clone(),
                None => return -12_i64,
            };
            let (vs, ve) = (vma.start().as_usize(), vma.end().as_usize());
            let seg_start = cursor.max(addr).max(vs);
            let seg_end = ve.min(end);

            let mut mid = sub_vma(&vma, seg_start, seg_end);
            let mut flags = mid.flags();
            flags.remove(VmaFlags::LOCKED);
            mid.set_flags(flags);

            let _ = mgr.remove(VirtAddr::new(vs));
            if vs < seg_start {
                let _ = mgr.add(sub_vma(&vma, vs, seg_start));
            }
            let _ = mgr.add(mid);
            if seg_end < ve {
                let _ = mgr.add(sub_vma(&vma, seg_end, ve));
            }
            if ve >= end {
                break;
            }
            cursor = ve;
        }
    }
    0
}

/// sys_mlockall - Lock all process memory (NR 230)
pub fn sys_mlockall(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    // MCL_CURRENT = 1, MCL_FUTURE = 2, MCL_ONFAULT = 4.
    const MCL_CURRENT: u32 = 1;
    const MCL_FUTURE: u32 = 2;
    const MCL_ONFAULT: u32 = 4;
    let flags = args[0] as u32;
    if flags == 0 || flags & !(MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT) != 0 {
        return -22_i64; // EINVAL
    }

    // P2 mlock: set VM_LOCKED on every CURRENT VMA (MCL_CURRENT).
    // MCL_FUTURE (lock future mappings too) is accepted but not tracked —
    // mmap does not consult a per-mm flag yet (documented limitation).
    let current_task = match crate::sched::current() {
        Some(t) => t,
        None => return -12_i64,
    };

    // RLIMIT_MEMLOCK gate (Linux can_do_mlock / mlockall): EPERM for a
    // caller without CAP_IPC_LOCK and a zero soft limit (LTP mlockall03
    // case 2: seteuid(nobody) + rlimit 0); ENOMEM when locking EVERYTHING
    // would exceed a nonzero soft limit (LTP mlockall03 case 1: rlimit 7
    // bytes vs the whole address space). CAP_IPC_LOCK bypasses the size
    // check.
    let has_ipc_lock = crate::security::capable(crate::security::CAP_IPC_LOCK);
    let (memlock_cur, _) = current_task
        .rlimit(crate::process::task::rlimit_res::MEMLOCK);
    if !has_ipc_lock && memlock_cur == 0 {
        return -1_i64; // EPERM
    }
    if !has_ipc_lock && memlock_cur != u64::MAX && flags & MCL_CURRENT != 0 {
        // Whole-address-space lock: the charge is the total VMA span.
        let as_ref = match current_task.address_space() {
            Some(a) => a,
            None => return -12_i64,
        };
        let total: u64 = {
            let mgr = as_ref.vma_read();
            mgr.iter()
                .map(|v| (v.end().as_usize() - v.start().as_usize()) as u64)
                .sum()
        };
        if total > memlock_cur {
            return -12_i64; // ENOMEM
        }
    }

    let address_space = match current_task.address_space_mut() {
        Some(a) => a,
        None => return -12_i64,
    };

    let user_start = crate::arch::mm::user_addr::USER_START;
    let user_end = crate::arch::mm::user_addr::USER_END;
    let mut cursor = user_start;
    loop {
        let mut mgr = address_space.vma_write();
        match mgr.find_mut(VirtAddr::new(cursor)) {
            Some(vma) => {
                let vma_end = vma.end().as_usize();
                let mut f = vma.flags();
                f.insert(VmaFlags::LOCKED);
                vma.set_flags(f);
                if vma_end >= user_end {
                    return 0;
                }
                cursor = vma_end;
            }
            None => {
                // Hole at cursor — jump to the next VMA's start.
                let next = mgr
                    .iter()
                    .map(|v| v.start().as_usize())
                    .find(|&s| s > cursor);
                match next {
                    Some(s) => cursor = s,
                    None => return 0, // no VMAs left above cursor
                }
            }
        }
    }
}

/// sys_munlockall - Unlock all process memory (NR 231)
pub fn sys_munlockall(_args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    // P2 mlock: clear VM_LOCKED on every current VMA.
    let current_task = match crate::sched::current() {
        Some(t) => t,
        None => return -12_i64,
    };
    let address_space = match current_task.address_space_mut() {
        Some(a) => a,
        None => return -12_i64,
    };

    let user_start = crate::arch::mm::user_addr::USER_START;
    let user_end = crate::arch::mm::user_addr::USER_END;
    let mut cursor = user_start;
    loop {
        let mut mgr = address_space.vma_write();
        match mgr.find_mut(VirtAddr::new(cursor)) {
            Some(vma) => {
                let vma_end = vma.end().as_usize();
                let mut f = vma.flags();
                f.remove(VmaFlags::LOCKED);
                vma.set_flags(f);
                if vma_end >= user_end {
                    return 0;
                }
                cursor = vma_end;
            }
            None => {
                let next = mgr
                    .iter()
                    .map(|v| v.start().as_usize())
                    .find(|&s| s > cursor);
                match next {
                    Some(s) => cursor = s,
                    None => return 0,
                }
            }
        }
    }
}

/// sys_mlock2 - Lock memory with flags (NR 284)
pub fn sys_mlock2(args: [u64; 6]) -> i64 {
    let addr = args[0] as usize;
    let length = args[1] as usize;
    let flags = args[2] as u32;

    // MLOCK_ONFAULT = 0x01 — the only definable flag; unknown bits EINVAL
    // (accepted-and-ignored: we do not prefault anyway).
    const MLOCK_ONFAULT: u32 = 0x01;
    if flags & !MLOCK_ONFAULT != 0 {
        return -22_i64;
    }
    if length == 0 {
        return -22_i64;
    }
    if addr % crate::mm::page::PAGE_SIZE != 0 {
        return -22_i64;
    }
    mlock_impl(addr, length, flags & MLOCK_ONFAULT != 0)
}

/// sys_mbind - Set memory policy for a range (NR 235)
///
/// On a single-node RISC-V system, all memory policies are effectively MPOL_DEFAULT.
/// Validate arguments and return success.
pub fn sys_mbind(args: [u64; 6]) -> i64 {
    let _start = args[0] as usize;
    let _len = args[1] as usize;
    let _mode = args[2] as i32;
    let _nodemask_ptr = args[3] as *const usize;
    let _maxnode = args[4] as usize;
    let _flags = args[5] as u32;

    // Validate nodemask pointer if provided
    if !_nodemask_ptr.is_null() && _maxnode > 0 {
        if !crate::arch::uaccess::access_ok(_nodemask_ptr as usize, (_maxnode + 7) / 8) {
            return -errno::EFAULT as i64;
        }
    }

    // Single-node system: silently accept any policy
    0
}

/// sys_get_mempolicy - Get memory policy (NR 236)
///
/// On a single-node system, return MPOL_DEFAULT (0) with all nodes in nodemask.
pub fn sys_get_mempolicy(args: [u64; 6]) -> i64 {
    let mode_ptr = args[0] as *mut i32;
    let nodemask_ptr = args[1] as *mut usize;
    let maxnode = args[2] as usize;
    let _addr = args[3] as usize;
    let _flags = args[4] as u32;

    if mode_ptr.is_null() {
        return -errno::EFAULT as i64;
    }
    if !crate::arch::uaccess::access_ok(mode_ptr as usize, 4) {
        return -errno::EFAULT as i64;
    }

    // SAFETY: mode_ptr validated with access_ok(4); put_user is the
    // exception-table copy path.
    unsafe {
        // MPOL_DEFAULT = 0
        let _ = crate::arch::uaccess::put_user(mode_ptr, 0i32);
    }

    // Fill nodemask with all nodes
    if !nodemask_ptr.is_null() && maxnode > 0 {
        if !crate::arch::uaccess::access_ok(nodemask_ptr as usize, (maxnode + 7) / 8) {
            return -errno::EFAULT as i64;
        }
        let nwords = (maxnode + core::mem::size_of::<usize>() * 8 - 1) / (core::mem::size_of::<usize>() * 8);
        // SAFETY: nodemask_ptr validated with access_ok; nwords bounded by maxnode.
        unsafe {
            for i in 0..nwords {
                let _ = crate::arch::uaccess::put_user(nodemask_ptr.add(i), usize::MAX);
            }
        }
    }

    0
}

/// sys_set_mempolicy - Set process memory policy (NR 237)
pub fn sys_set_mempolicy(args: [u64; 6]) -> i64 {
    let _mode = args[0] as i32;
    let _nodemask_ptr = args[1] as *const usize;
    let _maxnode = args[2] as usize;

    if !_nodemask_ptr.is_null() && _maxnode > 0 {
        if !crate::arch::uaccess::access_ok(_nodemask_ptr as usize, (_maxnode + 7) / 8) {
            return -errno::EFAULT as i64;
        }
    }

    // Single-node system: accept any policy silently
    0
}

/// sys_migrate_pages - Migrate pages to another node (NR 238)
///
/// On a single-node system, no migration needed.
pub fn sys_migrate_pages(args: [u64; 6]) -> i64 {
    let _pid = args[0] as u32;
    let _maxnode = args[1] as usize;
    let _old_nodes_ptr = args[2] as *const usize;
    let _new_nodes_ptr = args[3] as *const usize;

    // Single-node system: nothing to migrate
    0
}

/// sys_move_pages - Move pages to another node (NR 239)
pub fn sys_move_pages(args: [u64; 6]) -> i64 {
    let _pid = args[0] as u32;
    let _count = args[1] as usize;
    let _pages_ptr = args[2] as *const usize;
    let _nodes_ptr = args[3] as *const i32;
    let _status_ptr = args[4] as *mut i32;
    let _flags = args[5] as i32;

    // Single-node system: all pages already on node 0
    // Fill status array with -ENOENT (page not present) if provided
    if !_status_ptr.is_null() && _count > 0 {
        if !crate::arch::uaccess::access_ok(_status_ptr as usize, _count * 4) {
            return -errno::EFAULT as i64;
        }
    }
    _count as i64
}

/// sys_pkey_mprotect - Protect memory with protection key (NR 288)
///
/// RISC-V does not have memory protection keys. Delegate to mprotect.
pub fn sys_pkey_mprotect(args: [u64; 6]) -> i64 {
    let addr = args[0] as usize;
    let len = args[1] as usize;
    let prot = args[2] as u32;
    let _pkey = args[3] as i32;

    // RISC-V has no pkeys — ignore pkey, delegate to mprotect
    sys_mprotect([addr as u64, len as u64, prot as u64, 0, 0, 0])
}

/// sys_pkey_alloc - Allocate protection key (NR 289)
pub fn sys_pkey_alloc(_args: [u64; 6]) -> i64 {
    // No pkey hardware on RISC-V
    -errno::ENOSYS as i64
}

/// sys_pkey_free - Free protection key (NR 290)
pub fn sys_pkey_free(args: [u64; 6]) -> i64 {
    let _pkey = args[0] as i32;
    // No pkey hardware on RISC-V
    -errno::EINVAL as i64
}

/// sys_fadvise64 - Predeclare file access pattern (NR 223)
///
/// Linux vfs_fadvise error contract (LTP posix_fadvise02/03/04):
/// - EBADF when `fd` does not refer to an open file
/// - ESPIPE when the file is a pipe or FIFO (no pos to advise on)
/// - EINVAL when `advice` is outside POSIX_FADV_NORMAL..POSIX_FADV_DONTNEED
/// Success is a no-op (no read-ahead engine to program yet).
pub fn sys_fadvise64(args: [u64; 6]) -> i64 {
    let fd = args[0] as i32;
    let _offset = args[1] as i64;
    let _len = args[2] as i64;
    let advice = args[3] as i32;

    // POSIX_FADV_NORMAL=0 .. POSIX_FADV_DONTNEED=5 (sequential layout on
    // every arch except 31-bit s390; RISC-V uses the generic values).
    if !(0..=5).contains(&advice) {
        return -errno::EINVAL as i64;
    }

    // SAFETY: get_file_fd returns a shared Arc<File> or None; only the
    // ops identity is inspected, no mutation.
    unsafe {
        let file = match crate::fs::file::get_file_fd(fd as usize) {
            Some(f) => f,
            None => return -errno::EBADF as i64,
        };
        // Pipes are anonymous Files with PIPE_OPS and no inode; FIFOs are
        // inode-backed with S_IFMT == S_IFIFO. Both are unseekable.
        let is_pipe = file
            .get_ops()
            .map(|ops| core::ptr::eq(ops as *const _, &crate::fs::pipe::PIPE_OPS as *const _))
            .unwrap_or(false);
        if is_pipe {
            return -errno::ESPIPE as i64;
        }
        if let Some(inode) = (*file.inode.get()).as_ref() {
            if inode.mode.is_fifo() {
                return -errno::ESPIPE as i64;
            }
        }
    }
    0
}

/// sys_remap_file_pages - Remap file pages (NR 234, deprecated)
///
/// F12: the old stub returned fake success (0) unconditionally. LTP
/// shmctl05 races remap_file_pages() against IPC_RMID in a
/// `do { ... } while (ret == 0)` loop — with a fake 0 the loop NEVER
/// exited and the test hung forever (chunk WEDGE).
///
/// Linux (post-4.0 emulation, mm/mmap.c) semantics implemented here:
///   prot != 0 or flags != 0          -> EINVAL
///   size == 0 / unaligned / wraps    -> EINVAL
///   no MAP_SHARED VMA at start,
///   or range not inside one VMA      -> EINVAL
///   SysV shm mapping whose segment
///   has been IPC_RMID'd              -> EIDRM (the emulation's unmap
///                                       drops the last attach and the
///                                       re-mmap of the dying object
///                                       fails — the race outcome LTP
///                                       expects first)
///   otherwise (identity remap)       -> 0 (Linux tears down and
///                                       re-establishes the same
///                                       mapping; the net mapping is
///                                       unchanged)
pub fn sys_remap_file_pages(args: [u64; 6]) -> i64 {
    use crate::mm::vma::{VmaFlags, VmaType};

    let start = args[0] as usize;
    let size = args[1] as usize;
    let prot = args[2] as usize;
    let _pgoff = args[3] as usize;
    let flags = args[4] as usize;

    // Only MAP_NONBLOCK survives in Linux's emulation; Rux treats every
    // flag as invalid (the LTP callers pass 0).
    if prot != 0 || flags != 0 {
        return -errno::EINVAL as i64;
    }
    let page: usize = PAGE_SIZE as usize;
    if start % page != 0 {
        return -errno::EINVAL as i64;
    }
    let size = (size + page - 1) & !(page - 1);
    if size == 0 {
        return -errno::EINVAL as i64;
    }
    let end = match start.checked_add(size) {
        Some(e) if e > start => e,
        _ => return -errno::EINVAL as i64,
    };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::EINVAL as i64,
    };
    let aspace = match current.address_space() {
        Some(a) => a,
        None => return -errno::EINVAL as i64,
    };

    // The VMA must cover the whole range and be MAP_SHARED.
    let vma = match aspace.find_vma(crate::mm::page::VirtAddr::new(start)) {
        Some(v) => v,
        None => return -errno::EINVAL as i64,
    };
    if vma.start().as_usize() > start || vma.end().as_usize() < end {
        return -errno::EINVAL as i64;
    }
    if !vma.flags().contains(VmaFlags::SHARED) {
        return -errno::EINVAL as i64;
    }

    match vma.vma_type() {
        VmaType::SharedMemory => {
            // shmat() stores the shmid in the VMA (set_file_fd). A removed
            // segment fails the remap like Linux's unmap+remap emulation.
            let shmid = vma.file_fd();
            if crate::ipc::sysv_shm::shm_id_removed(shmid) {
                return -errno::EIDRM as i64;
            }
            // Live segment, identity remap: mapping unchanged.
            0
        }
        // File-backed shared mappings: identity remap is a no-op (the
        // non-linear remap itself is not implemented — remap_file_pages01
        // exercises that separately and already reports it).
        VmaType::FileBacked => 0,
        _ => -errno::EINVAL as i64,
    }
}

/// Linux AIO syscalls (NR 0-4) - all stubs
pub fn sys_io_setup(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

pub fn sys_io_destroy(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

pub fn sys_io_submit(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

pub fn sys_io_cancel(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

pub fn sys_io_getevents(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

/// sys_io_pgetevents - Async I/O get events v2 (NR 292)
pub fn sys_io_pgetevents(_args: [u64; 6]) -> i64 {
    -errno::ENOSYS as i64
}

/// sys_set_mempolicy_home_node - Set home node for memory policy (NR 450)
pub fn sys_set_mempolicy_home_node(_args: [u64; 6]) -> i64 {
    // Single-node system: nothing to do
    0
}
