//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Boot initrd support — the framework-side half (root-cold lane,
//! 2026-11-24): the physical-range bookkeeping (`set_region` /
//! `region` / `loaded`), the `CpioStats` DTO (the row's return type)
//! and `load()` with its ONE audited unsafe slice build over the
//! reserved range. The DECOMPRESSOR (RFC 1951/1952) and the cpio
//! (newc) unpacker moved to the rux-fs service crate
//! (kernel/rux-fs/src/fs/initrd.rs) — boot-only cold code reached
//! through the two LeafBootHooks rows (`initrd_gunzip`,
//! `initrd_unpack_newc`), filled by the crate's registrar before the
//! framework boots.
//!
//! Boot flow (mirrors Linux's initramfs path, `root=/dev/ram0` semantics):
//!
//! 1. Early boot (before the zone allocator exists) the FDT /chosen node is
//!    parsed for `linux,initrd-start` / `linux,initrd-end` and the range is
//!    `memblock_reserve`d so the buddy allocator never hands those pages
//!    out (`set_region`).
//! 2. After the ramfs rootfs is mounted at `/`, `load()` decompresses the
//!    image when it carries the gzip magic (QEMU `-initrd` images of
//!    Ubuntu/OpenHarmony are gzip cpio) and unpacks the cpio archive into
//!    the rootfs — files, directories, symlinks, and hard links; device
//!    nodes are counted and skipped (rootfs is a memory fs without device
//!    node support; userspace inits create their own nodes on /dev).
//! 3. `root=/dev/ram0` in main.rs then skips the ext4 root mount: the
//!    initrd content IS the root filesystem.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Physical start address of the boot initrd (0 = none).
static INITRD_START: AtomicUsize = AtomicUsize::new(0);
/// Physical end address (exclusive) of the boot initrd.
static INITRD_END: AtomicUsize = AtomicUsize::new(0);
/// Set once the image was successfully unpacked into the rootfs.
static INITRD_LOADED: AtomicBool = AtomicBool::new(false);

/// Record the initrd's physical range (early boot, from /chosen).
///
/// `end` must be > `start`; both must be inside RAM (the caller parsed
/// them from the FDT). Calling this twice keeps the last range.
pub fn set_region(start: usize, end: usize) {
    INITRD_START.store(start, Ordering::Release);
    INITRD_END.store(end, Ordering::Release);
}

/// The recorded initrd physical range, if any.
pub fn region() -> Option<(usize, usize)> {
    let start = INITRD_START.load(Ordering::Acquire);
    let end = INITRD_END.load(Ordering::Acquire);
    if start != 0 && end > start {
        Some((start, end))
    } else {
        None
    }
}

/// Whether an initrd was already unpacked into the rootfs.
pub fn loaded() -> bool {
    INITRD_LOADED.load(Ordering::Acquire)
}

/// Result of a successful cpio unpack (the `initrd_unpack_newc` row's
/// return type; the crate-side unpacker constructs it).
#[derive(Debug, Clone, Copy, Default)]
pub struct CpioStats {
    pub files: usize,
    pub dirs: usize,
    pub symlinks: usize,
    pub hardlinks: usize,
    /// Device nodes / fifos / sockets present but not representable in
    /// rootfs (skipped; userspace creates its own /dev nodes).
    pub skipped_special: usize,
    pub total_bytes: usize,
}

/// Either a borrowed (uncompressed) or owned (decompressed) cpio image.
enum CpioSource<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl CpioSource<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            CpioSource::Borrowed(s) => s,
            CpioSource::Owned(v) => v.as_slice(),
        }
    }
}

/// Decompress (when gzipped) and unpack the initrd into the rootfs.
///
/// Must run after the RAM root is initialized — the superblock is
/// the unpack target. Safe to call only once; subsequent calls are no-ops
/// that report the already-loaded state.
pub fn load() -> Result<CpioStats, &'static str> {
    if loaded() {
        return Err("initrd already loaded");
    }
    let (start, end) = region().ok_or("no initrd image")?;

    // The early-boot FDT parse ran on the identity mapping; here the
    // permanent linear mapping is live, so go through phys_to_virt.
    let virt = crate::arch::mm::phys_to_virt(
        crate::arch::mm::PhysAddr::new(start as u64),
    ).bits();
    // SAFETY: [start, end) is reserved RAM owned by the initrd; the range
    // was validated (end > start, inside RAM) by the FDT parse.
    let raw: &[u8] = unsafe {
        core::slice::from_raw_parts(virt as *const u8, end - start)
    };

    let cpio: CpioSource = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        CpioSource::Owned(
            (crate::fs::leaf_hooks::boot_hooks().initrd_gunzip)(raw)?
        )
    } else {
        CpioSource::Borrowed(raw)
    };

    // The global-sb typed view through the hook row (the rootfs leaf
    // lives in the rux-fs crate; the trait is the seam).
    let sb = crate::fs::leaf_glue::rootfs::sb_view().ok_or("rootfs not initialized")?;

    let stats = (crate::fs::leaf_hooks::boot_hooks().initrd_unpack_newc)(
        cpio.as_slice(), sb,
    )?;
    INITRD_LOADED.store(true, Ordering::Release);
    Ok(stats)
}
