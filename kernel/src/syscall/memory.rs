//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory-related system calls
//!
//! Includes: brk, mmap, mmap_framebuffer, munmap, mprotect, msync, mremap, madvise, mincore, mlock, munlock

use super::*;
use super::SyscallArgs;
use crate::arch::riscv64::mm::{get_page_table_virt, PAGE_SHIFT, PAGE_SIZE, PageTableEntry, VirtAddr};

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
    use crate::arch::riscv64::mm::{alloc_and_map_user_memory, PageTableEntry};

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
                    crate::arch::riscv64::mm::user_addr::BRK_DEFAULT as u64
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
                .unwrap_or(crate::arch::riscv64::mm::user_addr::BRK_DEFAULT as u64);
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
                let user_end = crate::arch::riscv64::mm::user_addr::USER_END as u64;
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

                // Calculate page range to map
                let current_page_start = current_brk & !(PAGE_SIZE as u64 - 1);
                let new_page_end = (new_brk + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

                // If need to map new pages
                if new_page_end > current_page_start {
                    // Get root page table of address space
                    let root_ppn = if let Some(addr_space) = current_task.address_space() {
                        addr_space.root_ppn()
                    } else {
                        return current_brk as i64;
                    };

                    // Map new heap pages
                    let size = new_page_end - current_page_start;

                    // Permissions: User + Read + Write + Valid + Accessed + Dirty
                    let pte_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W
                        | PageTableEntry::U | PageTableEntry::A | PageTableEntry::D;

                    // SAFETY: root_ppn is a valid page table root; alloc_and_map_user_memory
                    // handles page allocation and mapping within user address space.
                    unsafe {
                        let result = alloc_and_map_user_memory(root_ppn, current_page_start, size, pte_flags);
                        if result.is_none() {
                            return current_brk as i64;
                        }
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
pub fn sys_mmap(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::{VmaFlags, VmaType};
    use crate::mm::pagemap::Perm;
    use crate::arch::riscv64::mm::{prot, map, mmap_error};

    let mut addr = args[0] as usize;
    let length = args[1] as usize;
    let prot_flags = args[2] as u32;
    let map_flags = args[3] as u32;
    let fd = args[4] as i32;
    let offset = args[5] as u64;

    // length of 0 is invalid per POSIX
    if length == 0 {
        return mmap_error::EINVAL;
    }

    let actual_length = length;

    // Check protection flags
    if prot_flags & !prot::PROT_MASK != 0 {
        return mmap_error::EINVAL;
    }

    // Check mapping type (must specify MAP_SHARED or MAP_PRIVATE)
    let map_type = map_flags & map::MAP_TYPE_MASK;
    if map_type != map::MAP_SHARED && map_type != map::MAP_PRIVATE {
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
        return mmap_error::EINVAL;
    }

    // User-address-space limit. The user root page table shares the kernel
    // PGD entries (copy_kernel_mappings), so a fixed mapping at or above
    // USER_END would walk into the shared kernel L1/L0 tables and replace
    // kernel PTEs with user-accessible ones — a direct privilege hole.
    let user_end = crate::arch::riscv64::mm::user_addr::USER_END;
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

    // (review批次1) The "fd >= 1000 means framebuffer" special case is
    // GONE: an unrelated file that happens to get a high fd number was
    // silently mapped onto the framebuffer instead of its own contents.
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

                    // Default readable
                    vma_flags.insert(VmaFlags::READ);

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

                    // Set VMA type
                    let vma_type = if map_flags & map::MAP_ANONYMOUS != 0 {
                        VmaType::Anonymous
                    } else {
                        VmaType::FileBacked
                    };

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
    use crate::arch::riscv64::mm::mmap_error;

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
    use crate::arch::riscv64::mm::{PageTableEntry, PAGE_SIZE, PAGE_SHIFT, PageTable, VirtAddr};

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
    if prot & !(crate::arch::riscv64::mm::prot::PROT_MASK | 0x0100_0000 | 0x0200_0000) != 0 {
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
    use crate::arch::riscv64::mm::user_addr;
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
            let _pte_guard = crate::arch::riscv64::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
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
                core::arch::asm!("sfence.vma");
            }
            drop(_pte_guard);

            // Update VMA flags to reflect new permissions — for EVERY VMA
            // overlapping the range (R7-A10: only the VMA containing `addr`
            // was updated, so later demand faults in sibling VMAs silently
            // remapped pages with the old permissions).
            if let Some(addr_space) = current_task.address_space() {
                let mut vma_mgr = addr_space.vma_write();
                let mut starts: alloc::vec::Vec<crate::mm::page::VirtAddr> = alloc::vec::Vec::new();
                for v in vma_mgr.iter() {
                    if v.end().as_usize() > addr && v.start().as_usize() < addr + length {
                        starts.push(v.start());
                    }
                }
                for start in starts {
                    if let Some(vma) = vma_mgr.find_mut(start) {
                        let perm_bits = (prot & 0x1 != 0) as u32      // PROT_READ
                            | ((prot & 0x2 != 0) as u32) << 1         // PROT_WRITE
                            | ((prot & 0x4 != 0) as u32) << 2;        // PROT_EXEC
                        // Preserve non-permission flags (SHARED, PRIVATE, GROWS*)
                        let old_flags = vma.flags().bits();
                        let new_flags = (old_flags & !(0x7)) | perm_bits;
                        vma.set_flags(crate::mm::vma::VmaFlags::from_bits(new_flags));
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
    use crate::arch::riscv64::mm::mmap_error;

    let addr = args[0] as usize;
    let length = args[1] as usize;
    let flags = args[2] as u32;

    // msync flags
    const MS_ASYNC: u32 = 0x1;      // Async write
    const MS_SYNC: u32 = 0x2;       // Sync write
    const MS_INVALIDATE: u32 = 0x4; // Invalidate cache

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
                    // Check if it is a shared mapping (only shared mappings can be msynced)
                    // Simplified: we allow all mappings to msync
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
    // Note: Complete implementation should:
    // - For file mappings, write dirty pages back to file
    // - If MS_SYNC, wait for write to complete
    // - If MS_ASYNC, just mark as needing write
    // - If MS_INVALIDATE, invalidate other processes' cache
    //
    // Simplified implementation: since we currently mainly have anonymous mappings, no file mappings,
    // so just return success

    0  // Success
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
    use crate::arch::riscv64::mm::{
        PAGE_SIZE, PAGE_SHIFT, PhysAddr, VirtAddr, map_page, phys_to_virt, PageTableEntry,
    };
    use crate::mm::page_alloc::alloc_page;
    use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};
    use crate::mm::zone::GfpFlags;

    // R7-A5: serialize leaf-PTE mutations against fork's table walk and COW
    // faults — the same discipline as the demand-fault paths.
    let _pte_guard = crate::arch::riscv64::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    let mut offset = 0usize;
    while offset < size {
        let old_virt = (old_addr + offset) as u64;
        let new_virt = (new_addr + offset) as u64;

        // Walk old page table to get the source physical page
        if let Some((old_ppn, old_bits)) = crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(root_ppn, old_virt) {
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
                (*page).set_index(new_virt as usize / (PAGE_SIZE as usize));
                (*page).inc_mapcount();
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
    use crate::arch::riscv64::mm::{map, mmap_error};

    let old_addr = args[0] as usize;
    let old_size = args[1] as usize;
    let new_size = args[2] as usize;
    let flags = args[3] as u32;
    let new_addr_arg = args[4] as usize;

    // mremap flags
    const MREMAP_MAYMOVE: u32 = 0x1;  // Can move to new address
    const MREMAP_FIXED: u32 = 0x2;    // Must map to specified address

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
    use crate::arch::riscv64::mm::mmap_error;

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
            // MADV_REMOVE: Completely free mapping (equivalent to munmap)
            match address_space.munmap(VirtAddr::new(addr), length_aligned) {
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
    use crate::arch::riscv64::mm::{PageTableEntry, PageTable, mmap_error};

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

    // Validate vec pointer
    if vec_ptr.is_null() {
        return mmap_error::EINVAL;
    }

    // Calculate needed page count
    let page_count = (length + PAGE_SIZE - 1) / PAGE_SIZE;


    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(vec_ptr as usize, page_count) {
        return mmap_error::EFAULT;
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

    // 1. Validate that address range is covered by VMA
    {
        let vma_mgr = address_space.vma_read();
        let mut check_addr = addr;
        let end_addr = addr + page_count * PAGE_SIZE;

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

    // 2. Get page table root
    let root_ppn = address_space.root_ppn();

    // 3. Check if each page is in memory
    // SAFETY: root_ppn is a valid page table root; we traverse 3-level Sv39 page tables
    // via linear mapping. vec_ptr validated with access_ok(page_count).
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

            // Set result: lowest bit indicates if page is in memory
            *vec_ptr.add(i) = if page_in_memory { 1 } else { 0 };
        }
    }

    0  // Success
}
/// sys_mlock - Lock memory
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 228
///
/// # Description
/// mlock locks memory, preventing it from being swapped out
pub fn sys_mlock(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    // Validate arguments
    if length == 0 {
        return -22_i64;  // EINVAL
    }

    // Address must be page aligned
    if addr % crate::mm::page::PAGE_SIZE != 0 {
        return -22_i64;  // EINVAL
    }

    // P2 mlock (fake-success cleanup): set VM_LOCKED on every VMA covering
    // [addr, addr+len). The flag marks the range non-swappable for the
    // reclaim/swap paths (like Linux's vm_flags bit). Granularity is the
    // WHOLE VMA, not a split range — a partial mlock of a large mapping
    // pins more than requested (documented approximation; VMA splitting is
    // future work). RLIMIT_MEMLOCK is NOT enforced yet (rlimits are
    // stored-but-inactive; see gap analysis). Pages are not prefaulted —
    // with swap still unwired, nothing can evict them anyway.
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

    let mut cursor = addr;
    loop {
        let mut mgr = address_space.vma_write();
        let vma = match mgr.find_mut(VirtAddr::new(cursor)) {
            Some(v) => v,
            None => return -12_i64, // ENOMEM: unmapped hole in the range
        };
        let vma_end = vma.end().as_usize();
        let mut flags = vma.flags();
        flags.insert(VmaFlags::LOCKED);
        vma.set_flags(flags);
        if vma_end >= end {
            return 0;
        }
        cursor = vma_end;
    }
}
/// sys_munlock - Unlock memory
///
///
/// # Arguments
/// - args[0] (addr): starting address
/// - args[1] (length): length
///
/// # Returns
/// Returns 0 on success, negative error code on failure
///
/// - RISC-V: 229
///
/// # Description
/// munlock unlocks previously locked memory
pub fn sys_munlock(args: [u64; 6]) -> i64 {
    use crate::mm::page::VirtAddr;
    use crate::mm::vma::VmaFlags;

    let addr = args[0] as usize;
    let length = args[1] as usize;

    // Validate arguments
    if length == 0 {
        return -22_i64;  // EINVAL
    }

    // Address must be page aligned
    if addr % crate::mm::page::PAGE_SIZE != 0 {
        return -22_i64;  // EINVAL
    }

    // P2 mlock: clear VM_LOCKED on every VMA covering the range (same
    // whole-VMA granularity as sys_mlock).
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

    let mut cursor = addr;
    loop {
        let mut mgr = address_space.vma_write();
        let vma = match mgr.find_mut(VirtAddr::new(cursor)) {
            Some(v) => v,
            None => return 0, // hole: nothing to unlock there (POSIX munlock is advisory)
        };
        let vma_end = vma.end().as_usize();
        let mut flags = vma.flags();
        flags.remove(VmaFlags::LOCKED);
        vma.set_flags(flags);
        if vma_end >= end {
            return 0;
        }
        cursor = vma_end;
    }
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
    let address_space = match current_task.address_space_mut() {
        Some(a) => a,
        None => return -12_i64,
    };

    let user_start = crate::arch::riscv64::mm::user_addr::USER_START;
    let user_end = crate::arch::riscv64::mm::user_addr::USER_END;
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

    let user_start = crate::arch::riscv64::mm::user_addr::USER_START;
    let user_end = crate::arch::riscv64::mm::user_addr::USER_END;
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
    sys_mlock([addr as u64, length as u64, 0, 0, 0, 0])
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
        if !crate::arch::riscv64::uaccess::access_ok(_nodemask_ptr as usize, (_maxnode + 7) / 8) {
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
    if !crate::arch::riscv64::uaccess::access_ok(mode_ptr as usize, 4) {
        return -errno::EFAULT as i64;
    }

    // SAFETY: mode_ptr validated with access_ok(4); put_user is the
    // exception-table copy path.
    unsafe {
        // MPOL_DEFAULT = 0
        let _ = crate::arch::riscv64::uaccess::put_user(mode_ptr, 0i32);
    }

    // Fill nodemask with all nodes
    if !nodemask_ptr.is_null() && maxnode > 0 {
        if !crate::arch::riscv64::uaccess::access_ok(nodemask_ptr as usize, (maxnode + 7) / 8) {
            return -errno::EFAULT as i64;
        }
        let nwords = (maxnode + core::mem::size_of::<usize>() * 8 - 1) / (core::mem::size_of::<usize>() * 8);
        // SAFETY: nodemask_ptr validated with access_ok; nwords bounded by maxnode.
        unsafe {
            for i in 0..nwords {
                let _ = crate::arch::riscv64::uaccess::put_user(nodemask_ptr.add(i), usize::MAX);
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
        if !crate::arch::riscv64::uaccess::access_ok(_nodemask_ptr as usize, (_maxnode + 7) / 8) {
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
        if !crate::arch::riscv64::uaccess::access_ok(_status_ptr as usize, _count * 4) {
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
pub fn sys_fadvise64(args: [u64; 6]) -> i64 {
    let _fd = args[0] as i32;
    let _offset = args[1] as i64;
    let _len = args[2] as i64;
    let _advice = args[3] as i32;
    // Simplified: ignore advice, return success
    0
}

/// sys_remap_file_pages - Remap file pages (NR 234, deprecated)
pub fn sys_remap_file_pages(_args: [u64; 6]) -> i64 {
    0 // Deprecated, return success
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
