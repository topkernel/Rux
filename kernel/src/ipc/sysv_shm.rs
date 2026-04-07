//! System V Shared Memory
//!
//! Implements shmget, shmctl, shmat, shmdt following the Linux kernel design.

use crate::arch::riscv64::uaccess::{access_ok, copy_to_user};
use crate::arch::riscv64::mm::map_user_page;
use crate::arch::riscv64::mm::memory_layout::{VirtAddr as MmVirtAddr, PhysAddr as MmPhysAddr};
use crate::mm::page::{PAGE_SIZE, PAGE_MASK, VirtAddr};
use crate::mm::page_alloc::{free_pages, get_zeroed_page};
use crate::mm::zone::GfpFlags;
use crate::mm::vma::{Vma, VmaFlags, VmaType};
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use core::sync::atomic::{AtomicI32, AtomicI64, AtomicIsize, AtomicU32, Ordering};

use super::util::*;

// ============================================================================
// UAPI Structures
// ============================================================================

/// struct shmid64_ds — returned by IPC_STAT, IPC_SET
/// Must match asm-generic/shmbuf.h for RV64. Total: 112 bytes.
#[repr(C)]
pub struct ShmidDsUapi {
    pub shm_perm: IpcPermUapi,
    pub shm_segsz: u64,
    pub shm_atime: i64,
    pub shm_dtime: i64,
    pub shm_ctime: i64,
    pub shm_cpid: u32,
    pub shm_lpid: u32,
    pub shm_nattch: u64,
    pub __unused4: u64,
    pub __unused5: u64,
}

// ============================================================================
// Kernel Structures
// ============================================================================

/// Physical pages backing a shared memory segment.
struct ShmPages {
    /// Physical page addresses (page-aligned).
    pages: alloc::vec::Vec<usize>,
}

impl ShmPages {
    fn new(size: usize) -> Option<Self> {
        let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut pages = alloc::vec::Vec::with_capacity(page_count);
        for _ in 0..page_count {
            let phys = get_zeroed_page(GfpFlags::GFP_USER);
            if phys == 0 {
                for p in &pages {
                    free_pages(*p, 0);
                }
                return None;
            }
            pages.push(phys);
        }
        Some(Self { pages })
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn get_page(&self, idx: usize) -> Option<usize> {
        self.pages.get(idx).copied()
    }
}

impl Drop for ShmPages {
    fn drop(&mut self) {
        for p in &self.pages {
            free_pages(*p, 0);
        }
    }
}

/// Shared memory segment (the IPC object).
pub struct ShmSegment {
    pub perm: KernIpcPerm,
    /// Segment size in bytes (requested by user).
    segsz: u64,
    /// Physical pages backing this segment.
    pages: Spinlock<Option<ShmPages>>,
    /// PID of creator.
    cpid: AtomicU32,
    /// PID of last shmat.
    lpid: AtomicU32,
    /// Time of last shmat.
    shm_atime: AtomicI64,
    /// Time of last shmdt.
    shm_dtime: AtomicI64,
    /// Time of last shmctl that changed the segment.
    shm_ctime: AtomicI64,
    /// Number of current attaches.
    nattch: AtomicIsize,
    /// Whether the segment has been marked for deletion.
    marked_destroy: AtomicI32,
}

impl IpcObject for ShmSegment {
    fn get_perm(&self) -> &KernIpcPerm {
        &self.perm
    }
    fn get_perm_mut(&mut self) -> &mut KernIpcPerm {
        &mut self.perm
    }
}

impl ShmSegment {
    fn new(key: i32, size: u64, mode: u16) -> Option<Self> {
        let size = if size == 0 { PAGE_SIZE as u64 } else { size };
        let pages = ShmPages::new(size as usize)?;
        Some(Self {
            perm: KernIpcPerm::new(key, mode),
            segsz: size,
            pages: Spinlock::new(Some(pages)),
            cpid: AtomicU32::new(get_current_pid()),
            lpid: AtomicU32::new(0),
            shm_atime: AtomicI64::new(0),
            shm_dtime: AtomicI64::new(0),
            shm_ctime: AtomicI64::new(ipc_current_time()),
            nattch: AtomicIsize::new(0),
            marked_destroy: AtomicI32::new(0),
        })
    }
}

// ============================================================================
// Global shared memory registry
// ============================================================================

static SHM_IDS: IpcIds<ShmSegment> = IpcIds::new();

fn get_current_pid() -> u32 {
    crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0)
}

// ============================================================================
// Address allocation helper
// ============================================================================

/// Find a free virtual address range in the current process's address space.
fn find_free_shm_addr(size: usize) -> Option<VirtAddr> {
    let current = crate::sched::current()?;
    let addr_space = current.address_space()?;

    let mmap_base = addr_space.mmap_base();
    let vma_mgr = addr_space.vma_read();

    // Search downward from mmap_base
    let mut addr = mmap_base;
    let min_addr: usize = 0x1000000; // 16MB minimum user address

    while addr > min_addr && addr > size {
        addr -= size;
        addr &= !PAGE_MASK;

        // Check for conflicts
        let mut conflict = false;
        for vma in vma_mgr.iter() {
            if vma.start().as_usize() <= addr + size && vma.end().as_usize() > addr {
                addr = vma.start().as_usize();
                conflict = true;
                break;
            }
        }
        if !conflict {
            return Some(VirtAddr::new(addr));
        }
    }
    None
}

// ============================================================================
// Syscall Implementations
// ============================================================================

/// sys_shmget — Allocate or find a shared memory segment (NR 194)
pub fn sys_shmget(args: [u64; 6]) -> u64 {
    let key = args[0] as i32;
    let size = args[1] as u64;
    let shmflg = args[2] as i32;

    // Round up size to page boundary
    let size = if size == 0 { 0 } else { ((size + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1)) };

    let segment = match ShmSegment::new(key, size, (shmflg & 0o777) as u16) {
        Some(s) => s,
        None => return -errno::ENOMEM as u64,
    };

    match SHM_IDS.alloc(segment, key, shmflg) {
        Ok((id, _)) => id as u64,
        Err(e) => e as u64,
    }
}

/// sys_shmctl — Shared memory control operations (NR 195)
pub fn sys_shmctl(args: [u64; 6]) -> u64 {
    let shmid = args[0] as i32;
    let cmd = args[1] as i32;
    let buf = args[2];

    let idx = match SHM_IDS.find(shmid) {
        Some(i) => i,
        None => return -errno::EINVAL as u64,
    };

    match cmd {
        IPC_RMID => {
            // Owner check: only creator or CAP_IPC_OWNER can destroy
            {
                let slots = SHM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    let cred = crate::sched::current().map(|t| t.cred());
                    let allowed = match cred {
                        Some(ref c) => {
                            c.euid == entry.inner.perm.cuid
                                || crate::security::capable(crate::security::CAP_IPC_OWNER)
                        }
                        None => false,
                    };
                    if !allowed {
                        return -errno::EPERM as u64;
                    }
                }
            }
            let slots = SHM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx] {
                if entry.inner.nattch.load(Ordering::Relaxed) > 0 {
                    entry.inner.marked_destroy.store(1, Ordering::Relaxed);
                    return 0;
                }
            }
            drop(slots);
            let _ = SHM_IDS.remove(shmid);
            SHM_IDS.free_slot(shmid);
            0
        }
        IPC_STAT => {
            let buf_ptr = buf as *mut ShmidDsUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<ShmidDsUapi>()) {
                return -errno::EFAULT as u64;
            }
            let mut ds = ShmidDsUapi {
                shm_perm: IpcPermUapi::default(),
                shm_segsz: 0,
                shm_atime: 0,
                shm_dtime: 0,
                shm_ctime: 0,
                shm_cpid: 0,
                shm_lpid: 0,
                shm_nattch: 0,
                __unused4: 0,
                __unused5: 0,
            };
            {
                let slots = SHM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    ds.shm_perm = entry.inner.perm.to_uapi();
                    ds.shm_segsz = entry.inner.segsz;
                    ds.shm_atime = entry.inner.shm_atime.load(Ordering::Relaxed);
                    ds.shm_dtime = entry.inner.shm_dtime.load(Ordering::Relaxed);
                    ds.shm_ctime = entry.inner.shm_ctime.load(Ordering::Relaxed);
                    ds.shm_cpid = entry.inner.cpid.load(Ordering::Relaxed);
                    ds.shm_lpid = entry.inner.lpid.load(Ordering::Relaxed);
                    ds.shm_nattch = entry.inner.nattch.load(Ordering::Relaxed) as u64;
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for size_of::<ShmidDsUapi>() above;
            // ds is a stack-local copy of the shared memory segment metadata.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const ShmidDsUapi as *const u8,
                    core::mem::size_of::<ShmidDsUapi>(),
                );
            }
            0
        }
        IPC_SET => {
            let buf_ptr = buf as *const u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<ShmidDsUapi>()) {
                return -errno::EFAULT as u64;
            }
            let idx2 = match SHM_IDS.find_with_perms(shmid, 0o6) {
                Ok(i) => i,
                Err(e) => return e as u64,
            };
            let mut slots = SHM_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                // SAFETY: buf_ptr was access_ok-validated for size_of::<ShmidDsUapi>() (112 bytes) above;
                // offset 20 is within the shm_perm IPC_perm layout for the mode field.
                let new_mode = unsafe { core::ptr::read_volatile(buf_ptr.add(20) as *const u16) };
                entry.inner.perm.update_mode(new_mode);
                entry.inner.shm_ctime.store(ipc_current_time(), Ordering::Relaxed);
            }
            0
        }
        IPC_INFO => {
            let buf_ptr = buf as *mut u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, 48) {
                return -errno::EFAULT as u64;
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for 48 bytes above;
            // zeroing the entire buffer is within bounds.
            unsafe { core::ptr::write_bytes(buf_ptr, 0, 48) };
            // SAFETY: buf_ptr was access_ok-validated for 48 bytes; offset 0 is within bounds.
            unsafe { core::ptr::write_volatile(buf_ptr as *mut u64, 256 * 4096u64) };
            // SAFETY: buf_ptr + 8 is within the 48-byte access_ok-validated range.
            unsafe { core::ptr::write_volatile(buf_ptr.add(8) as *mut u64, 1u64) };
            // SAFETY: buf_ptr + 16 is within the 48-byte access_ok-validated range.
            unsafe { core::ptr::write_volatile(buf_ptr.add(16) as *mut u64, 256u64) };
            // SAFETY: buf_ptr + 24 is within the 48-byte access_ok-validated range.
            unsafe { core::ptr::write_volatile(buf_ptr.add(24) as *mut u64, 4096u64) };
            // SAFETY: buf_ptr + 32 is within the 48-byte access_ok-validated range.
            unsafe { core::ptr::write_volatile(buf_ptr.add(32) as *mut u64, 256 * 256u64) };
            SHM_IDS.count() as u64
        }
        11 => 0, // SHM_LOCK — no-op
        12 => 0, // SHM_UNLOCK — no-op
        13 => {
            // SHM_STAT — like IPC_STAT but uses raw kernel index, returns shmid
            let raw_idx = shmid as usize;
            let buf_ptr = buf as *mut ShmidDsUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<ShmidDsUapi>()) {
                return -errno::EFAULT as u64;
            }
            if raw_idx >= 256 {
                return -errno::EINVAL as u64;
            }
            let mut ds = ShmidDsUapi {
                shm_perm: IpcPermUapi::default(),
                shm_segsz: 0,
                shm_atime: 0,
                shm_dtime: 0,
                shm_ctime: 0,
                shm_cpid: 0,
                shm_lpid: 0,
                shm_nattch: 0,
                __unused4: 0,
                __unused5: 0,
            };
            let result_id: i32;
            {
                let slots = SHM_IDS.slots.lock();
                if let Some(ref entry) = slots[raw_idx] {
                    if entry.deleted {
                        return -errno::EINVAL as u64;
                    }
                    ds.shm_perm = entry.inner.perm.to_uapi();
                    ds.shm_segsz = entry.inner.segsz;
                    ds.shm_atime = entry.inner.shm_atime.load(Ordering::Relaxed);
                    ds.shm_dtime = entry.inner.shm_dtime.load(Ordering::Relaxed);
                    ds.shm_ctime = entry.inner.shm_ctime.load(Ordering::Relaxed);
                    ds.shm_cpid = entry.inner.cpid.load(Ordering::Relaxed);
                    ds.shm_lpid = entry.inner.lpid.load(Ordering::Relaxed);
                    ds.shm_nattch = entry.inner.nattch.load(Ordering::Relaxed) as u64;
                    result_id = super::util::ipc_build_id(raw_idx, entry.inner.perm.seq);
                } else {
                    return -errno::EINVAL as u64;
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for size_of::<ShmidDsUapi>() above;
            // ds is a stack-local copy of the shared memory segment metadata.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const ShmidDsUapi as *const u8,
                    core::mem::size_of::<ShmidDsUapi>(),
                );
            }
            result_id as u64
        }
        14 => {
            // SHM_INFO — returns struct shm_info (current usage)
            // struct shm_info: 8 fields × 8 bytes = 64 bytes on RV64
            let buf_ptr = buf as *mut u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, 64) {
                return -errno::EFAULT as u64;
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for 64 bytes above;
            // zeroing the entire buffer is within bounds.
            unsafe { core::ptr::write_bytes(buf_ptr, 0, 64) };
            // used_ids (offset 0)
            // SAFETY: buf_ptr was access_ok-validated for 64 bytes; offset 0 is within bounds.
            unsafe { core::ptr::write_volatile(buf_ptr as *mut u64, SHM_IDS.count() as u64) };
            // shm_tot (offset 8) — total shared memory pages
            let mut total_pages: u64 = 0;
            {
                let slots = SHM_IDS.slots.lock();
                for entry in slots.iter() {
                    if let Some(ref e) = entry {
                        if !e.deleted {
                            let pages = (e.inner.segsz + 4095) / 4096;
                            total_pages += pages;
                        }
                    }
                }
            }
            // SAFETY: buf_ptr + 8 is within the 64-byte access_ok-validated range.
            unsafe { core::ptr::write_volatile(buf_ptr.add(8) as *mut u64, total_pages) };
            // shm_rss (offset 16), shm_swp (offset 24) — no swap support, 0
            // swap_attempts (offset 32), swap_successes (offset 40) — 0
            // shm_tot is already written; remaining fields stay 0
            // Return: index of highest used entry + 1
            let mut max_idx: usize = 0;
            {
                let slots = SHM_IDS.slots.lock();
                for (i, entry) in slots.iter().enumerate().rev() {
                    if entry.is_some() {
                        max_idx = i + 1;
                        break;
                    }
                }
            }
            max_idx as u64
        }
        _ => -errno::EINVAL as u64,
    }
}

/// sys_shmat — Attach shared memory segment (NR 196)
pub fn sys_shmat(args: [u64; 6]) -> u64 {
    let shmid = args[0] as i32;
    let shmaddr = args[1] as usize;
    let shmflg = args[2] as i32;

    let shm_readonly = (shmflg & 0o10000) != 0; // SHM_RDONLY

    let idx = match SHM_IDS.find_with_perms(shmid, if shm_readonly { 0o4 } else { 0o6 }) {
        Ok(i) => i,
        Err(e) => return e as u64,
    };

    // Get segment info
    let segsz;
    {
        let slots = SHM_IDS.slots.lock();
        if let Some(ref entry) = slots[idx] {
            if entry.deleted {
                return -errno::EIDRM as u64;
            }
            segsz = entry.inner.segsz;
        } else {
            return -errno::EINVAL as u64;
        }
    }

    let size_aligned = ((segsz as usize) + PAGE_SIZE - 1) & !PAGE_MASK;

    // Determine attach address
    let attach_addr = if shmaddr != 0 {
        if shmaddr & PAGE_MASK != 0 {
            if (shmflg & 0o20000) != 0 { // SHM_RND
                shmaddr & !PAGE_MASK
            } else {
                return -errno::EINVAL as u64;
            }
        } else {
            shmaddr
        }
    } else {
        match find_free_shm_addr(size_aligned) {
            Some(a) => a.as_usize(),
            None => return -errno::ENOMEM as u64,
        }
    };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };
    let addr_space = match current.address_space() {
        Some(as_) => as_,
        None => return -errno::ENOMEM as u64,
    };

    let root_ppn = addr_space.root_ppn();

    // Map physical pages from the shared segment
    {
        let slots = SHM_IDS.slots.lock();
        if let Some(ref entry) = slots[idx] {
            if entry.deleted {
                return -errno::EIDRM as u64;
            }
            let pages_lock = entry.inner.pages.lock();
            if let Some(ref shm_pages) = *pages_lock {
                // Build PTE flags
                let mut pte_flags = crate::arch::riscv64::mm::PageTableEntry::V
                    | crate::arch::riscv64::mm::PageTableEntry::A
                    | crate::arch::riscv64::mm::PageTableEntry::D
                    | crate::arch::riscv64::mm::PageTableEntry::U
                    | crate::arch::riscv64::mm::PageTableEntry::R;
                if !shm_readonly {
                    pte_flags |= crate::arch::riscv64::mm::PageTableEntry::W;
                }

                // Map each page
                for i in 0..shm_pages.page_count() {
                    let phys = match shm_pages.get_page(i) {
                        Some(p) => p,
                        None => {
                            // Rollback: unmap already mapped pages using munmap
                            let rollback_size = i * PAGE_SIZE;
                            if rollback_size > 0 {
                                let _ = addr_space.munmap(
                                    VirtAddr::new(attach_addr),
                                    rollback_size,
                                );
                            }
                            return -errno::ENOMEM as u64;
                        }
                    };
                    // SAFETY: attach_addr + i*PAGE_SIZE is page-aligned and within the
                    // newly allocated VMA range; phys is a valid page from get_zeroed_page;
                    // root_ppn is the current process's page table root.
                    unsafe {
                        map_user_page(
                            root_ppn,
                            MmVirtAddr::new((attach_addr + i * PAGE_SIZE) as u64),
                            MmPhysAddr::new(phys as u64),
                            pte_flags,
                        );
                    }
                }
            } else {
                return -errno::EIDRM as u64;
            }
        } else {
            return -errno::EINVAL as u64;
        }
    }

    // Create VMA entry for this attachment
    let mut vma_flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::SHARED);
    if !shm_readonly {
        vma_flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::SHARED);
    }

    let mut vma = Vma::new(
        VirtAddr::new(attach_addr),
        VirtAddr::new(attach_addr + size_aligned),
        vma_flags,
    );
    vma.set_type(VmaType::SharedMemory);
    vma.set_file_fd(shmid);
    vma.set_file_size(segsz);

    if addr_space.add_vma(vma).is_err() {
        let _ = addr_space.munmap(VirtAddr::new(attach_addr), size_aligned);
        return -errno::ENOMEM as u64;
    }

    // Update segment metadata
    {
        let slots = SHM_IDS.slots.lock();
        if let Some(ref entry) = slots[idx] {
            entry.inner.nattch.fetch_add(1, Ordering::Relaxed);
            entry.inner.lpid.store(get_current_pid(), Ordering::Relaxed);
            entry.inner.shm_atime.store(ipc_current_time(), Ordering::Relaxed);
        }
    }

    // Flush TLB
    // SAFETY: sfence.vma is a RISC-V privileged instruction valid in S-mode;
    // required after modifying page table entries for the mapping to take effect.
    unsafe { core::arch::asm!("sfence.vma"); }

    attach_addr as u64
}

/// sys_shmdt — Detach shared memory segment (NR 197)
pub fn sys_shmdt(args: [u64; 6]) -> u64 {
    let shmaddr = args[0] as usize;

    if shmaddr == 0 {
        return -errno::EINVAL as u64;
    }

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::ESRCH as u64,
    };
    let addr_space = match current.address_space() {
        Some(as_) => as_,
        None => return -errno::EINVAL as u64,
    };

    // Find the VMA at shmaddr and get shm_id
    let shm_id;
    let vma_size;
    {
        let vma_mgr = addr_space.vma_read();
        let vma = match vma_mgr.find(VirtAddr::new(shmaddr)) {
            Some(v) => v,
            None => return -errno::EINVAL as u64,
        };
        if vma.vma_type() != VmaType::SharedMemory {
            return -errno::EINVAL as u64;
        }
        shm_id = vma.file_fd();
        vma_size = vma.end().as_usize() - vma.start().as_usize();
    }

    // munmap handles both VMA removal and page table unmapping
    let _ = addr_space.munmap(VirtAddr::new(shmaddr), vma_size);

    // Update segment metadata
    let idx = match SHM_IDS.find(shm_id) {
        Some(i) => i,
        None => return 0, // Already gone
    };
    let mut slots = SHM_IDS.slots.lock();
    if let Some(ref mut entry) = slots[idx] {
        let old_nattch = entry.inner.nattch.fetch_sub(1, Ordering::Relaxed);
        entry.inner.lpid.store(crate::process::current_pid(), Ordering::Relaxed);
        entry.inner.shm_dtime.store(ipc_current_time(), Ordering::Relaxed);

        // Free segment if marked for destruction and no more attaches
        if old_nattch == 1 && entry.inner.marked_destroy.load(Ordering::Relaxed) != 0 {
            drop(slots);
            let _ = SHM_IDS.remove(shm_id);
            SHM_IDS.free_slot(shm_id);
        }
    }

    // Flush TLB
    // SAFETY: sfence.vma is a RISC-V privileged instruction valid in S-mode;
    // required after unmapping page table entries to flush stale TLB entries.
    unsafe { core::arch::asm!("sfence.vma"); }

    0
}

/// Detach a shared memory segment (called from exit_mmap).
/// Decrements nattch and frees the segment if marked_destroy && nattch == 0.
pub fn shm_detach_vma(shmid: i32) {
    let idx = match SHM_IDS.find(shmid) {
        Some(i) => i,
        None => return,
    };
    let slots = SHM_IDS.slots.lock();
    if let Some(ref entry) = slots[idx] {
        let old_nattch = entry.inner.nattch.fetch_sub(1, Ordering::Relaxed);
        if old_nattch == 1 && entry.inner.marked_destroy.load(Ordering::Relaxed) != 0 {
            drop(slots);
            let _ = SHM_IDS.remove(shmid);
            SHM_IDS.free_slot(shmid);
        }
    }
}

/// Attach a shared memory segment (called from fork).
/// Increments nattch for the inherited attachment.
pub fn shm_attach_vma(shmid: i32) {
    let idx = match SHM_IDS.find(shmid) {
        Some(i) => i,
        None => return,
    };
    let slots = SHM_IDS.slots.lock();
    if let Some(ref entry) = slots[idx] {
        entry.inner.nattch.fetch_add(1, Ordering::Relaxed);
    }
}
