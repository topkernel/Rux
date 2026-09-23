//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory Layout Management
//!
//! This module manages kernel memory layout, providing dynamic calculation
//! of memory regions instead of hardcoding addresses.
//!
//! The is part of Phase 0 of the memory management refactoring plan.

extern crate alloc;

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Page size constant (4KB)
const PAGE_SIZE: usize = 4096;

/// Physical memory base address (QEMU virt platform)
/// This is the start of physical memory on RISC-V QEMU virt
pub const PHYS_MEMORY_BASE: usize = 0x80000000;

/// Kernel entry point (after OpenSBI)
/// This is where the kernel is loaded by the bootloader
pub const KERNEL_ENTRY: usize = 0x80200000;

// (DEFAULT_HEAP_SIZE removed — crate::config::KERNEL_HEAP_SIZE is the
// single source for the heap size; a second 32MB constant here silently
// drifted from the config, review 4.20.)

/// Default slab size (4MB)
pub const DEFAULT_SLAB_SIZE: usize = 4 * 1024 * 1024;

/// Kernel memory layout
///
/// This structure holds all the dynamically computed memory regions.
/// It layout is determined at memblock and initialization time.
#[derive(Clone, Copy)]
pub struct KernelMemoryLayout {
    /// Physical memory base address (from device tree)
    pub phys_base: usize,
    /// Total physical memory size (from device tree)
    pub phys_size: usize,
    /// Kernel code start address (from linker symbols)
    pub kernel_start: usize,
    /// Kernel code end address (from linker symbols)
    pub kernel_end: usize,
    /// Kernel heap start address
    pub heap_start: usize,
    /// Kernel heap size
    pub heap_size: usize,
    /// Slab allocator start address
    pub slab_start: usize,
    /// Slab allocator size
    pub slab_size: usize,
    /// User physical memory start address (dynamically calculated)
    pub user_phys_start: usize,
    /// User physical memory size (dynamically calculated)
    pub user_phys_size: usize,
    /// Frame allocator start address (from memblock)
    pub frame_alloc_start: usize,
    /// Frame Allocator size
    pub frame_alloc_size: usize,
}

impl KernelMemoryLayout {
    /// Create a new empty layout
    pub const fn new() -> Self {
        Self {
            phys_base: PHYS_MEMORY_BASE,
            phys_size: 0,
            kernel_start: 0,
            kernel_end: 0,
            heap_start: 0,
            heap_size: crate::config::KERNEL_HEAP_SIZE,
            slab_start: 0,
            slab_size: DEFAULT_SLAB_SIZE,
            user_phys_start: 0,
            user_phys_size: 0,
            frame_alloc_start: 0,
            frame_alloc_size: 0,
        }
    }

    /// Initialize from memblock
    ///
    /// This should be called after memblock is initialized with memory regions.
    pub fn init_from_memblock(
        phys_base: usize,
        phys_size: usize,
        kernel_start: usize,
        kernel_end: usize,
    ) -> Self {
        // Calculate heap region (after kernel)
        let _kernel_size = kernel_end - kernel_start;
        let heap_start = (kernel_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        // Single source of truth: the kernel heap buddy
        // (buddy_allocator.rs) is sized from crate::config::KERNEL_HEAP_SIZE;
        // deriving the layout from the same constant keeps memblock
        // reservations, the buddy's window and this layout from drifting
        // apart (assert_memblock_consistency enforces it at boot).
        let heap_size = crate::config::KERNEL_HEAP_SIZE;

        // Calculate slab region (after heap)
        let slab_start = heap_start + heap_size;
        let slab_size = DEFAULT_SLAB_SIZE;

        // Calculate user physical memory region
        // Use 25% of remaining memory for user processes, max 64MB
        let remaining_after_slab = phys_base + phys_size - slab_start - slab_size;
        let user_phys_size = (remaining_after_slab / 4).min(64 * 1024 * 1024);
        let user_phys_start = slab_start + slab_size;

        // Frame allocator starts after user physical region
        let frame_alloc_start = user_phys_start + user_phys_size;
        let frame_alloc_size = phys_base + phys_size - frame_alloc_start;

        Self {
            phys_base,
            phys_size,
            kernel_start,
            kernel_end,
            heap_start,
            heap_size,
            slab_start,
            slab_size,
            user_phys_start,
            user_phys_size,
            frame_alloc_start,
            frame_alloc_size,
        }
    }
}

// Global memory layout instance.
//
// KERNEL_LAYOUT is written once during early boot by kernel_layout_init() and
// read-only afterwards.  KERNEL_LAYOUT_INIT acts as a guard so that readers
// can check initialization without accessing the MaybeUninit.
static KERNEL_LAYOUT_INIT: AtomicBool = AtomicBool::new(false);
static mut KERNEL_LAYOUT: MaybeUninit<KernelMemoryLayout> = MaybeUninit::uninit();

/// Initialize kernel memory layout
///
/// This should be called once during boot after memblock is initialized.
pub fn kernel_layout_init(layout: KernelMemoryLayout) {
    if KERNEL_LAYOUT_INIT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        // SAFETY: first and only write — guarded by compare_exchange above.
        unsafe {
            KERNEL_LAYOUT.write(layout);
        }
    }
}

/// Get kernel memory layout
///
/// Panics if layout is not initialized.
pub fn kernel_layout() -> &'static KernelMemoryLayout {
    if !KERNEL_LAYOUT_INIT.load(Ordering::Acquire) {
        panic!("Kernel layout not initialized");
    }
    // SAFETY: KERNEL_LAYOUT_INIT is true, so kernel_layout_init() has
    // completed and the value is fully initialized.
    unsafe { KERNEL_LAYOUT.assume_init_ref() }
}

/// Check if kernel layout is initialized
pub fn is_kernel_layout_initialized() -> bool {
    KERNEL_LAYOUT_INIT.load(Ordering::Acquire)
}

// ==================== Accessor Functions ====================

/// Get physical memory base address
#[inline]
pub fn phys_memory_base() -> usize {
    kernel_layout().phys_base
}

/// Get physical memory size
#[inline]
pub fn phys_memory_size() -> usize {
    kernel_layout().phys_size
}

/// Get kernel start address
#[inline]
pub fn kernel_start() -> usize {
    kernel_layout().kernel_start
}

/// Get kernel end address
#[inline]
pub fn kernel_end() -> usize {
    kernel_layout().kernel_end
}

/// Get heap start address
#[inline]
pub fn heap_start() -> usize {
    kernel_layout().heap_start
}

/// Get heap size
#[inline]
pub fn heap_size() -> usize {
    kernel_layout().heap_size
}

/// Get slab start address
#[inline]
pub fn slab_start() -> usize {
    kernel_layout().slab_start
}

/// Get slab size
#[inline]
pub fn slab_size() -> usize {
    kernel_layout().slab_size
}

/// Get user physical memory start address
#[inline]
pub fn user_phys_start() -> usize {
    kernel_layout().user_phys_start
}

/// Get user physical memory size
#[inline]
pub fn user_phys_size() -> usize {
    kernel_layout().user_phys_size
}

/// Get frame allocator start address
#[inline]
pub fn frame_alloc_start() -> usize {
    kernel_layout().frame_alloc_start
}

/// Get frame allocator size
#[inline]
pub fn frame_alloc_size() -> usize {
    kernel_layout().frame_alloc_size
}

/// Get heap end address
#[inline]
pub fn heap_end() -> usize {
    heap_start() + heap_size()
}

/// Get slab end address
#[inline]
pub fn slab_end() -> usize {
    slab_start() + slab_size()
}

/// Get user physical memory end address
#[inline]
pub fn user_phys_end() -> usize {
    user_phys_start() + user_phys_size()
}

// ==================== Debug/Info Functions ====================

/// Boot-time consistency assertion between the kernel layout and memblock
/// (review 4.20).
///
/// The heap and slab regions this layout advertises must:
///  - lie inside physical memory,
///  - be exactly adjacent (slab starts where the heap ends), and
///  - be fully covered by memblock RESERVED regions — anything else means
///    the zone allocator (which seeds from memory minus reserved) would
///    hand those pages to a second owner.
///
/// Panics at boot (deterministic, single-threaded) on violation.
pub fn assert_memblock_consistency(layout: &KernelMemoryLayout) {
    let phys_end = layout.phys_base.saturating_add(layout.phys_size);

    assert!(
        layout.heap_start >= layout.phys_base
            && layout.heap_start.saturating_add(layout.heap_size) <= phys_end,
        "layout: heap {:#x}+{:#x} outside physical memory {:#x}-{:#x}",
        layout.heap_start,
        layout.heap_size,
        layout.phys_base,
        phys_end
    );
    assert!(
        layout.slab_start.saturating_add(layout.slab_size) <= phys_end,
        "layout: slab {:#x}+{:#x} outside physical memory",
        layout.slab_start,
        layout.slab_size
    );
    assert!(
        layout.slab_start == layout.heap_start + layout.heap_size,
        "layout: slab {:#x} not adjacent to heap end {:#x}",
        layout.slab_start,
        layout.heap_start + layout.heap_size
    );

    let mb = super::memblock::memblock();
    let regions = [
        ("heap", layout.heap_start, layout.heap_size),
        ("slab", layout.slab_start, layout.slab_size),
    ];
    for (name, base, size) in regions {
        if size == 0 {
            continue;
        }
        let first = base;
        let last = base + size - PAGE_SIZE;
        assert!(
            mb.is_reserved(first) && mb.is_reserved(last),
            "layout: {} region {:#x}-{:#x} not fully reserved in memblock \
             (first_reserved={} last_reserved={}) — double-ownership risk",
            name,
            base,
            base + size,
            mb.is_reserved(first),
            mb.is_reserved(last)
        );
    }
}

/// Print memory layout information
pub fn print_kernel_layout() {
    if !is_kernel_layout_initialized() {
        crate::println!("kernel_layout: not initialized");
        return;
    }

    let layout = kernel_layout();

    crate::println!("Memory Layout:");
    crate::println!("  Physical Memory: {:#x} - {:#x} ({:?} MB)",
        layout.phys_base,
        layout.phys_base + layout.phys_size,
        layout.phys_size / (1024 * 1024),
    );
    crate::println!("  Kernel:          {:#x} - {:#x} ({:?} MB)",
        layout.kernel_start,
        layout.kernel_end,
        (layout.kernel_end - layout.kernel_start) / (1024 * 1024),
    );
    crate::println!("  Heap:           {:#x} - {:#x} ({:?} MB)",
        layout.heap_start,
        layout.heap_start + layout.heap_size,
        layout.heap_size / (1024 * 1024),
    );
    crate::println!("  Slab:            {:#x} - {:#x} ({:?} MB)",
        layout.slab_start,
        layout.slab_start + layout.slab_size,
        layout.slab_size / (1024 * 1024),
    );
    crate::println!("  User Phys:      {:#x} - {:#x} ({:?} MB)",
        layout.user_phys_start,
        layout.user_phys_start + layout.user_phys_size,
        layout.user_phys_size / (1024 * 1024),
    );
    crate::println!("  Frame Alloc:    {:#x} - {:#x} ({:?} MB)",
        layout.frame_alloc_start,
        layout.frame_alloc_start + layout.frame_alloc_size,
        layout.frame_alloc_size / (1024 * 1024),
    );
}
