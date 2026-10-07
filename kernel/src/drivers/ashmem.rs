//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /dev/ashmem — Anonymous Shared Memory (Android/OpenHarmony ABI).
//!
//! Port of the classic Linux `drivers/staging/android/ashmem.c` interface
//! (the ABI OpenHarmony's libc buffer management uses):
//!
//! - open("/dev/ashmem") creates a fresh, unnamed, zero-sized area
//! - ASHMEM_SET_NAME / ASHMEM_GET_NAME — advisory name (frozen at mmap)
//! - ASHMEM_SET_SIZE / ASHMEM_GET_SIZE — byte size (frozen at mmap)
//! - ASHMEM_SET_PROT_MASK / ASHMEM_GET_PROT_MASK — allowed PROT_* subset
//!   (starts PROT_READ|PROT_WRITE|PROT_EXEC, can only narrow)
//! - ASHMEM_PIN / ASHMEM_UNPIN / ASHMEM_GET_PIN_STATUS — page-range pin
//!   bookkeeping (ranges are tracked exactly like the Linux driver's
//!   ashmem_range list; nothing is ever actually reclaimed here, so PIN
//!   always reports ASHMEM_NOT_PURGED)
//! - ASHMEM_PURGE_ALL_CACHES — CAP_SYS_ADMIN only, no-op returning 0
//!   freed pages (no shrinker exists)
//! - mmap maps the area's private page pool into the caller; MAP_SHARED
//!   mappings alias the pool pages (fork/COW-exempt via VmaFlags::SHARED,
//!   the SysV shmat model), MAP_PRIVATE mappings are read-only+COW.
//!
//! Implementation model (Phase 2 prep, OH port plan §6 item 2): each open
//! file description owns an `AshmemArea` (Box in File::private_data).
//! The backing store is an eagerly allocated physical page pool created
//! at the FIRST mmap — the same discipline SysV shm uses (ShmPages): the
//! pool holds one reference per page, every mapping takes one per PTE
//! (get_page at map, put_page at unmap/exit), so pages survive until the
//! last mapping is gone even if every fd was closed first. Linux hangs
//! the same lifetime off the shmem backing file's vm_file refcount.

use crate::arch::mm::map_user_page;
use crate::arch::mm::memory_layout::{PhysAddr as MmPhysAddr, VirtAddr as MmVirtAddr};
use crate::arch::mm::PageTableEntry;
use crate::fs::dev_t::{DevNo, MISC_MAJOR};
use crate::mm::page::{PAGE_MASK, PAGE_SIZE};
use crate::mm::page_alloc::{free_pages, get_zeroed_page};
use crate::mm::vma::{Vma, VmaFlags, VmaType};
use crate::mm::zone::GfpFlags;
use crate::sync::spinlock::Spinlock;

/// ASHMEM_NAME_LEN (UAPI).
pub const ASHMEM_NAME_LEN: usize = 256;

/// Default name reported by ASHMEM_GET_NAME for unnamed areas.
const ASHMEM_NAME_DEF: &[u8] = b"dev/ashmem\0";

/// ASHMEM_PIN / ASHMEM_UNPIN result: content survived.
pub const ASHMEM_NOT_PURGED: i64 = 0;
/// ASHMEM_PIN result: content was purged while unpinned.
pub const ASHMEM_WAS_PURGED: i64 = 1;

/// ASHMEM_GET_PIN_STATUS result.
pub const ASHMEM_IS_UNPINNED: i64 = 0;
pub const ASHMEM_IS_PINNED: i64 = 1;

/// struct ashmem_pin { u32 offset; u32 len; } (8 bytes on every ABI).
#[repr(C)]
struct AshmemPin {
    offset: u32,
    len: u32,
}

// ioctl command words, asm-generic encoding: dir<<30 | size<<16 | 0x77<<8 | nr
// (char[256] payload -> size field 0x100; verified against _IOW() on the
// riscv64 toolchain)
const ASHMEM_SET_NAME: u32 = 0x4100_7701; // _IOW(0x77, 1, char[256])
const ASHMEM_GET_NAME: u32 = 0x8100_7702; // _IOR(0x77, 2, char[256])
const ASHMEM_SET_SIZE: u32 = 0x4008_7703; // _IOW(0x77, 3, size_t)
const ASHMEM_GET_SIZE: u32 = 0x0000_7704; // _IO(0x77, 4)
const ASHMEM_SET_PROT_MASK: u32 = 0x4008_7705; // _IOW(0x77, 5, unsigned long)
const ASHMEM_GET_PROT_MASK: u32 = 0x0000_7706; // _IO(0x77, 6)
const ASHMEM_PIN: u32 = 0x4008_7707; // _IOW(0x77, 7, struct ashmem_pin)
const ASHMEM_UNPIN: u32 = 0x4008_7708; // _IOW(0x77, 8, struct ashmem_pin)
const ASHMEM_GET_PIN_STATUS: u32 = 0x0000_7709; // _IO(0x77, 9)
const ASHMEM_PURGE_ALL_CACHES: u32 = 0x0000_770a; // _IO(0x77, 10)

/// PROT_READ | PROT_WRITE | PROT_EXEC (asm-generic values).
const PROT_MASK: u32 = 0x1 | 0x2 | 0x4;

/// /dev/ashmem device number (misc 10:62 — no other misc minor in this
/// kernel occupies 62; Linux allocates the real ashmem minor
/// dynamically and userspace only ever opens by path).
pub const DEV_ASHMEM: DevNo = DevNo::new(MISC_MAJOR, 62);

// ============================================================================
// Page pool
// ============================================================================

/// Physical page pool backing one ashmem area (SysV ShmPages discipline).
struct AshmemPages {
    pages: alloc::vec::Vec<usize>,
}

impl AshmemPages {
    fn new(size: usize) -> Option<Self> {
        let page_count = size.div_ceil(PAGE_SIZE);
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

impl Drop for AshmemPages {
    fn drop(&mut self) {
        // Drop the pool's owner reference per page. Live mappings hold
        // their own reference (taken at map time), so a page still mapped
        // by any process survives until its last unmap's put_page.
        for p in &self.pages {
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(*p));
            let freed_by_us = if page.is_null() {
                true
            } else {
                // SAFETY: descriptor exists for this in-RAM page.
                unsafe { (*page).put_page() == 0 }
            };
            if freed_by_us {
                free_pages(*p, 0);
            }
        }
    }
}

/// One unpinned page range [pgstart, pgend] (inclusive page indices).
#[derive(Clone, Copy)]
struct UnpinRange {
    pgstart: usize,
    pgend: usize,
    purged: u32,
}

/// Per-open-file-description ashmem area (Linux struct ashmem_area).
pub struct AshmemArea {
    inner: Spinlock<AreaInner>,
}

struct AreaInner {
    /// Advisory name (NUL-terminated, at most ASHMEM_NAME_LEN bytes).
    name: [u8; ASHMEM_NAME_LEN],
    /// True once a custom name was set via ASHMEM_SET_NAME.
    name_set: bool,
    /// Size in bytes, frozen once the pool exists.
    size: usize,
    /// Allowed PROT_* bits, starts full, can only narrow.
    prot_mask: u32,
    /// Backing pages, created at first mmap.
    pages: Option<AshmemPages>,
    /// Sorted, non-overlapping unpinned ranges.
    unpinned: alloc::vec::Vec<UnpinRange>,
}

impl AshmemArea {
    fn new() -> Self {
        Self {
            inner: Spinlock::new(AreaInner {
                name: [0; ASHMEM_NAME_LEN],
                name_set: false,
                size: 0,
                prot_mask: PROT_MASK,
                pages: None,
                unpinned: alloc::vec::Vec::new(),
            }),
        }
    }
}

// ============================================================================
// Range algebra (port of ashmem_pin/ashmem_unpin/ashmem_get_pin_status)
// ============================================================================

/// page_range_in_range: does [rs,re] interact with [s,e]?
fn range_in_range(r: UnpinRange, s: usize, e: usize) -> bool {
    (r.pgstart <= s && r.pgend >= s)
        || (r.pgstart <= e && r.pgend >= e)
        || (r.pgstart >= s && r.pgend <= e)
}

/// Pin [pgstart,pgend]: remove/split/trim the overlapping unpinned ranges.
/// Returns the OR of purged flags of the removed parts (never purged here,
/// but the bookkeeping mirrors the Linux driver exactly).
fn pin_ranges(unpinned: &mut alloc::vec::Vec<UnpinRange>, pgstart: usize, pgend: usize) -> u32 {
    let mut ret = 0u32;
    let mut i = 0;
    while i < unpinned.len() {
        let r = unpinned[i];
        if r.pgend < pgstart {
            // Sorted list: ranges before our window.
            i += 1;
            continue;
        }
        if !range_in_range(r, pgstart, pgend) {
            i += 1;
            continue;
        }
        ret |= r.purged;
        if r.pgstart >= pgstart && r.pgend <= pgend {
            // Case 1: request subsumes the range — remove it whole.
            unpinned.remove(i);
            continue;
        }
        if r.pgstart >= pgstart {
            // Case 2: overlap from the front — trim the range's start.
            unpinned[i].pgstart = pgend + 1;
            i += 1;
            continue;
        }
        if r.pgend <= pgend {
            // Case 3: overlap from the rear — trim the range's end.
            unpinned[i].pgend = pgstart - 1;
            i += 1;
            continue;
        }
        // Case 4: the pin punches a hole — split into two ranges.
        let second = UnpinRange {
            pgstart: pgend + 1,
            pgend: r.pgend,
            purged: r.purged,
        };
        unpinned[i].pgend = pgstart - 1;
        unpinned.insert(i + 1, second);
        break;
    }
    ret
}

/// Unpin [pgstart,pgend]: merge with overlapping ranges, insert sorted.
fn unpin_ranges(unpinned: &mut alloc::vec::Vec<UnpinRange>, mut pgstart: usize, mut pgend: usize) {
    let mut purged = 0u32;
    let mut i = 0;
    while i < unpinned.len() {
        let r = unpinned[i];
        if r.pgend < pgstart {
            i += 1;
            continue;
        }
        // Entirely covered by an existing range: nothing to do.
        if r.pgstart <= pgstart && r.pgend >= pgend {
            return;
        }
        if range_in_range(r, pgstart, pgend) {
            // Widen the request and absorb the range (Linux's restart).
            pgstart = pgstart.min(r.pgstart);
            pgend = pgend.max(r.pgend);
            purged |= r.purged;
            unpinned.remove(i);
            i = 0;
            continue;
        }
        i += 1;
    }
    let pos = unpinned.partition_point(|x| x.pgstart < pgstart);
    unpinned.insert(
        pos,
        UnpinRange {
            pgstart,
            pgend,
            purged,
        },
    );
}

/// GET_PIN_STATUS: any unpinned range touching [pgstart,pgend]?
fn any_unpinned(unpinned: &[UnpinRange], pgstart: usize, pgend: usize) -> bool {
    unpinned.iter().any(|r| range_in_range(*r, pgstart, pgend))
}

// ============================================================================
// File operations
// ============================================================================

/// Extract the &AshmemArea a File's private_data points at.
// SAFETY: private_data was installed by devfs_open as a Box<AshmemArea>
// and is freed by ashmem_file_close (ops.close, exactly once per open
// file description). Callers hold an Arc<File> from the fd table, so the
// close op cannot have run yet.
unsafe fn area_of(file: &crate::fs::file::File) -> Option<&'static AshmemArea> {
    let cell = file.private_data.get();
    (*cell).map(|p| &*(p as *const AshmemArea))
}

fn ashmem_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    // SAFETY: see area_of.
    let area = match unsafe { area_of(file) } {
        Some(a) => a,
        None => return -9, // EBADF
    };
    let mut inner = area.inner.lock_irqsave();
    if inner.size == 0 {
        return 0; // EOF, like Linux ashmem_read_iter
    }
    let Some(ref pages) = inner.pages else {
        return -9; // EBADF: no backing store until the first mmap
    };
    let pos = file.get_pos() as usize;
    if pos >= inner.size {
        return 0;
    }
    let want = buf.len().min(inner.size - pos);
    let mut done = 0;
    while done < want {
        let off = pos + done;
        let pg = off / PAGE_SIZE;
        let in_pg = off % PAGE_SIZE;
        let chunk = (want - done).min(PAGE_SIZE - in_pg);
        let phys = match pages.get_page(pg) {
            Some(p) => p,
            None => break,
        };
        let src = crate::arch::mm::memory_layout::phys_to_virt(MmPhysAddr::new(phys as u64))
            .bits() as *const u8;
        // SAFETY: src is the kernel linear map of a live pool page; the
        // destination is the syscall-validated user buffer slice.
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(in_pg), buf.as_mut_ptr().add(done), chunk);
        }
        done += chunk;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

fn ashmem_file_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see area_of.
    let area = match unsafe { area_of(file) } {
        Some(a) => a,
        None => return -9, // EBADF
    };
    let inner = area.inner.lock_irqsave();
    if inner.size == 0 {
        return -22; // EINVAL, like Linux ashmem_llseek
    }
    if inner.pages.is_none() {
        return -9; // EBADF
    }
    let cur = file.get_pos() as i64;
    let size = inner.size as i64;
    let new_pos = match whence {
        0 => offset as i64,
        1 => cur + offset as i64,
        2 => size + offset as i64,
        _ => return -22,
    };
    if new_pos < 0 {
        return -22;
    }
    file.set_pos(new_pos as u64);
    new_pos as isize
}

fn ashmem_file_close(file: &crate::fs::file::File) -> i32 {
    // Linux ashmem_release: mappings do NOT pin the area — it dies on
    // close. Mapped pages survive on their own refcounts (AshmemPages
    // drop only releases the pool's owner references).
    // SAFETY: ops.close runs exactly once, on the final reference drop
    // (close_fd / fdtable teardown / deferred close_pending).
    unsafe {
        let cell = file.private_data.get();
        if let Some(p) = (*cell).take() {
            drop(alloc::boxed::Box::from_raw(p as *mut AshmemArea));
        }
    }
    0
}

/// /dev/ashmem file operations.
pub static ASHMEM_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(ashmem_file_read),
    write: None, // the Linux driver has no .write_iter either
    lseek: Some(ashmem_file_lseek),
    close: Some(ashmem_file_close),
    poll: None,
};

/// True when `file` is an open /dev/ashmem description.
pub fn is_ashmem_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &ASHMEM_OPS as *const _),
        None => false,
    }
}

/// devfs_open hook: allocate the per-open area.
pub fn ashmem_open(file: &crate::fs::file::File) -> i32 {
    file.set_private_data(
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new(AshmemArea::new())) as *mut u8,
    );
    0
}

// ============================================================================
// ioctl
// ============================================================================

/// Per-fd ioctl dispatch (ops identity, the loop/evdev pattern).
/// Returns Some(ret) when the file is ashmem, None otherwise.
pub fn ashmem_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    if !is_ashmem_file(file) {
        return None;
    }
    Some(unsafe { ashmem_ioctl(file, request, arg) })
}

/// SAFETY: `arg` is the raw syscall argument; every user-pointer use is
/// validated through the uaccess helpers before dereference.
unsafe fn ashmem_ioctl(file: &crate::fs::file::File, request: u32, arg: usize) -> i64 {
    use crate::arch::uaccess::{access_ok, copy_to_user, strncpy_from_user};

    const EINVAL: i64 = 22;
    const ENOTTY: i64 = 25;
    const EPERM: i64 = 1;
    const EFAULT: i64 = 14;

    // SAFETY: see area_of.
    let area = match area_of(file) {
        Some(a) => a,
        None => return -EINVAL,
    };

    match request {
        ASHMEM_SET_NAME => {
            if arg == 0 || !access_ok(arg, ASHMEM_NAME_LEN) {
                return -EFAULT;
            }
            let mut local = [0u8; ASHMEM_NAME_LEN];
            // SAFETY: arg validated above; strncpy_from_user goes through
            // the exception-tabled byte fetcher.
            let name = match strncpy_from_user(arg as *const u8, ASHMEM_NAME_LEN, &mut local) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let len = name.len().min(ASHMEM_NAME_LEN - 1);
            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_some() {
                return -EINVAL; // cannot change name after mmap
            }
            inner.name = [0; ASHMEM_NAME_LEN];
            inner.name[..len].copy_from_slice(&name[..len]);
            inner.name_set = true;
            0
        }
        ASHMEM_GET_NAME => {
            if arg == 0 || !access_ok(arg, ASHMEM_NAME_LEN) {
                return -EFAULT;
            }
            let inner = area.inner.lock_irqsave();
            // Only the meaningful bytes are copied out (never the whole
            // 256-byte stack-buffer content).
            let (bytes, len): (&[u8], usize) = if inner.name_set {
                let len = inner
                    .name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(ASHMEM_NAME_LEN - 1)
                    + 1;
                (&inner.name, len)
            } else {
                (ASHMEM_NAME_DEF, ASHMEM_NAME_DEF.len())
            };
            // SAFETY: arg validated above for ASHMEM_NAME_LEN bytes and
            // len <= ASHMEM_NAME_LEN.
            if copy_to_user(arg as *mut u8, bytes.as_ptr(), len) != 0 {
                return -EFAULT;
            }
            0
        }
        ASHMEM_SET_SIZE => {
            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_some() {
                return -EINVAL; // cannot resize after mmap
            }
            inner.size = arg;
            0
        }
        ASHMEM_GET_SIZE => {
            let inner = area.inner.lock_irqsave();
            inner.size as i64
        }
        ASHMEM_SET_PROT_MASK => {
            let prot = arg as u32 & PROT_MASK;
            let mut inner = area.inner.lock_irqsave();
            // The mask can only narrow (Linux set_prot_mask).
            if inner.prot_mask & prot != prot {
                return -EINVAL;
            }
            inner.prot_mask = prot;
            0
        }
        ASHMEM_GET_PROT_MASK => {
            let inner = area.inner.lock_irqsave();
            inner.prot_mask as i64
        }
        ASHMEM_PIN | ASHMEM_UNPIN | ASHMEM_GET_PIN_STATUS => {
            // All three commands take a struct ashmem_pin from the user —
            // GET_PIN_STATUS is _IO-encoded but the Linux driver copies
            // the struct for it too (ashmem_pin_unpin handles all three).
            let mut pin = AshmemPin { offset: 0, len: 0 };
            if arg == 0 || !access_ok(arg, core::mem::size_of::<AshmemPin>()) {
                return -EFAULT;
            }
            // SAFETY: arg validated for the 8-byte struct.
            if crate::arch::uaccess::copy_from_user(
                &mut pin as *mut AshmemPin as *mut u8,
                arg as *const u8,
                core::mem::size_of::<AshmemPin>(),
            ) != 0
            {
                return -EFAULT;
            }

            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_none() {
                return -EINVAL; // Linux: no backing file yet
            }
            let aligned_size = (inner.size + PAGE_SIZE - 1) & !PAGE_MASK;
            // len == 0 means "everything onward".
            let mut len = pin.len as usize;
            if len == 0 {
                len = aligned_size - pin.offset as usize;
            }
            let offset = pin.offset as usize;
            if (offset | len) & PAGE_MASK != 0 {
                return -EINVAL; // not page-aligned
            }
            if u32::MAX as usize - offset < len {
                return -EINVAL;
            }
            if aligned_size < offset + len {
                return -EINVAL;
            }
            let pgstart = offset / PAGE_SIZE;
            let pgend = pgstart + len / PAGE_SIZE - 1;
            match request {
                ASHMEM_PIN => pin_ranges(&mut inner.unpinned, pgstart, pgend) as i64,
                ASHMEM_UNPIN => {
                    unpin_ranges(&mut inner.unpinned, pgstart, pgend);
                    0
                }
                _ => {
                    if any_unpinned(&inner.unpinned, pgstart, pgend) {
                        ASHMEM_IS_UNPINNED
                    } else {
                        ASHMEM_IS_PINNED
                    }
                }
            }
        }
        ASHMEM_PURGE_ALL_CACHES => {
            // CAP_SYS_ADMIN only, like the Linux driver; nothing is ever
            // reclaimed (no shrinker), so 0 pages were purged.
            if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
                return -EPERM;
            }
            0
        }
        _ => -ENOTTY,
    }
}

// ============================================================================
// mmap
// ============================================================================

/// ashmem mmap handler, called from sys_mmap once the file was verified
/// to be /dev/ashmem (is_ashmem_file).
///
/// `addr` 0 means "kernel chooses". `offset` is the file offset in bytes
/// (must be page-aligned; mapped pool pages come from that index).
///
/// Returns the mapping address or negative errno.
pub fn ashmem_mmap(
    file: &crate::fs::file::File,
    addr: usize,
    length: usize,
    offset: u64,
    prot: u32,
    map_shared: bool,
) -> Result<usize, i64> {
    const EINVAL: i64 = 22;
    const EPERM: i64 = 1;
    const ENOMEM: i64 = 12;

    if offset as usize % PAGE_SIZE != 0 {
        return Err(-EINVAL);
    }

    // SAFETY: see area_of.
    let area = unsafe { area_of(file) }.ok_or(-EINVAL)?;

    // Validation + pool creation under the area lock; the actual PTE work
    // happens outside it (snapshot the page list first, shmat discipline).
    let (phys_pages, size): (alloc::vec::Vec<usize>, usize) = {
        let mut inner = area.inner.lock_irqsave();
        if inner.size == 0 {
            return Err(-EINVAL); // SET_SIZE required before mmap
        }
        let aligned_size = (inner.size + PAGE_SIZE - 1) & !PAGE_MASK;
        if length > aligned_size {
            return Err(-EINVAL); // mapping larger than the area
        }
        let off = offset as usize;
        if off + length > aligned_size {
            return Err(-EINVAL); // window past the end (stricter than the
                                 // classic driver, which SIGBUSes later)
        }
        // Requested protection must be within the allowed mask.
        if prot & PROT_MASK & !inner.prot_mask != 0 {
            return Err(-EPERM);
        }
        if inner.pages.is_none() {
            let pool = AshmemPages::new(inner.size).ok_or(-ENOMEM)?;
            inner.pages = Some(pool);
        }
        // SAFETY: pages is Some here (just created or pre-existing).
        let pages = inner.pages.as_ref().unwrap();
        let base_pg = off / PAGE_SIZE;
        let npages = length.div_ceil(PAGE_SIZE);
        let mut v = alloc::vec::Vec::with_capacity(npages);
        for i in 0..npages {
            match pages.get_page(base_pg + i) {
                Some(p) => v.push(p),
                None => return Err(-EINVAL),
            }
        }
        (v, inner.size)
    };

    let task = crate::sched::current().ok_or(-ENOMEM)?;
    let address_space = task.address_space().ok_or(-ENOMEM)?;
    let root_ppn = address_space.root_ppn();

    // Placement: honor a page-aligned hint, else find a free area.
    if addr != 0 && addr % PAGE_SIZE != 0 {
        return Err(-EINVAL);
    }
    let placement = if addr != 0 {
        addr
    } else {
        match address_space.find_free_area(length) {
            Ok(v) => v.as_usize(),
            Err(_) => return Err(-ENOMEM),
        }
    };

    // PTE flags: MAP_SHARED maps pool pages writable (when PROT_WRITE);
    // MAP_PRIVATE maps them read-only with the COW marker so the first
    // store breaks to a private copy through the standard COW fault path.
    let mut pte_flags = PageTableEntry::V
        | PageTableEntry::A
        | PageTableEntry::D
        | PageTableEntry::U
        | PageTableEntry::R;
    if map_shared {
        if prot & 0x2 != 0 {
            pte_flags |= PageTableEntry::W;
        }
        if prot & 0x4 != 0 {
            pte_flags |= PageTableEntry::X;
        }
    } else if prot & 0x2 != 0 {
        pte_flags |= crate::arch::mm::mm_ops::cow_flags::COW;
    }

    let npages = phys_pages.len();
    for (i, &phys) in phys_pages.iter().enumerate() {
        // SAFETY: placement + i*PAGE_SIZE is page-aligned inside the range
        // being established; phys is a live pool page; root_ppn is the
        // caller's page-table root. Map + per-mapping reference under the
        // PTE lock (shmat discipline).
        unsafe {
            let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
            map_user_page(
                root_ppn,
                MmVirtAddr::new((placement + i * PAGE_SIZE) as u64),
                MmPhysAddr::new(phys as u64),
                pte_flags,
            );
            let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(phys));
            if !page.is_null() {
                (*page).get_page();
            }
        }
    }

    // VMA bookkeeping: SHARED maps use the SysV-shm VMA type so fork
    // inherits the frames as-is (VmaFlags::SHARED is the COW exemption
    // key in copy_page_table_cow). file_fd stays unset — the SysV shm
    // detach accounting keys on it and must not see ashmem VMAs.
    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ); // shmat discipline: mapping is readable
    if prot & 0x2 != 0 {
        vma_flags.insert(VmaFlags::WRITE);
    }
    if prot & 0x4 != 0 {
        vma_flags.insert(VmaFlags::EXEC);
    }
    if map_shared {
        vma_flags.insert(VmaFlags::SHARED);
    } else {
        vma_flags.insert(VmaFlags::PRIVATE);
    }
    let mut vma = Vma::new(
        crate::mm::page::VirtAddr::new(placement),
        crate::mm::page::VirtAddr::new(placement + npages * PAGE_SIZE),
        vma_flags,
    );
    vma.set_type(if map_shared {
        VmaType::SharedMemory
    } else {
        VmaType::Anonymous
    });
    vma.set_file_size(size as u64);
    vma.set_offset(offset as usize);
    if address_space.add_vma(vma).is_err() {
        // Roll back the eager mapping.
        let _ = address_space.munmap(
            crate::mm::page::VirtAddr::new(placement),
            npages * PAGE_SIZE,
        );
        return Err(-ENOMEM);
    }
    address_space.add_rss(npages as u64);
    // SAFETY: fresh user PTEs need a flush before first use on any hart.
    unsafe { crate::arch::mm::asid::flush_tlb_all(); }
    Ok(placement)
}
