//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 Memory Layout Constants and Address Types
//!
//! 48-bit VA, 4K granule, 4-level translation tables (L0..L3).
//! The address space follows the ARM split-translation convention:
//! - TTBR0 (lower half): user space
//! - TTBR1 (upper half): kernel — linear map, vmalloc/vmemmap/fixmap, image
//!
//! Geometry mirrors the riscv64 backend's scheme translated into the
//! 48-bit upper half, so the generic tree (phys_to_virt through
//! VA_PA_OFFSET, user_addr constants, mmap layout) sees the same shapes:
//! - image linked near the top (riscv64: 0xffff_ffff_8000_0000)
//! - linear map (PAGE_OFFSET) below the vmalloc/vmemmap bands
//! - user TASK_SIZE = 256GB (riscv64 parity; avoids touching any generic
//!   user-space layout logic during bring-up)

// ==================== Page Size Constants ====================

pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;
pub const VA_BITS: u64 = 48;
/// TTBR0 half size (low VA space): 2^47 = 128TB
pub const VA_MASK: u64 = (1 << 47) - 1;

// ==================== AArch64 4-level Page Table Constants ====================

/// Entries per table level (4K granule)
pub const PTRS_PER_PTE: u64 = 512;
pub const PTRS_PER_PMD: u64 = 512;
pub const PTRS_PER_PUD: u64 = 512;
pub const PTRS_PER_PGD: u64 = 512;

/// Walk shifts: L0 512GB, L1 1GB, L2 2MB, L3 4KB
pub const PGDIR_SHIFT: u64 = 39;   // L0 entry maps 512GB
pub const PUD_SHIFT: u64 = 30;     // L1 entry maps 1GB
pub const PMD_SHIFT: u64 = 21;     // L2 entry maps 2MB

pub const PGDIR_SIZE: u64 = 1 << PGDIR_SHIFT;  // 512GB
pub const PUD_SIZE: u64 = 1 << PUD_SHIFT;      // 1GB
pub const PMD_SIZE: u64 = 1 << PMD_SHIFT;      // 2MB

/// TASK_SIZE - maximum user space address. riscv64 parity: 256GB. All user
/// VAs then live inside TTBR0 L0 entry 0 — the aarch64 walker's user half.
pub const TASK_SIZE: usize = 0x100_0000_0000; // 256GB

/// USER_PTRS_PER_PGD - user L0 entries (TTBR0 table). With TASK_SIZE=256GB
/// and 512GB per L0 entry, only entry 0 can ever hold user mappings; keep
/// the riscv64-style constant for code that computes split points.
pub const USER_PTRS_PER_PGD: usize = 1;

/// KERNEL_PGD_START - first L0 index considered kernel (within a single
/// table view). TTBR0's own L0 serves user; kernel L0 is the separate
/// TTBR1 table. The constant is kept for interface parity.
pub const KERNEL_PGD_START: usize = USER_PTRS_PER_PGD;

// ==================== Kernel Virtual Memory Layout (TTBR1 half) ====================
//
//   0xffff_ffff_ffff_ffff +-------------------------------+
//                          |  kernel image (linked at top) |
//   0xffff_ffff_8000_0000  +-------------------------------+  KERNEL_LINK_ADDR
//   FIXADDR_TOP            +-------------------------------+
//                          |  fixmap (16MB)                |
//   VMEMMAP_START          +-------------------------------+
//                          |  vmemmap (struct page array)  |  4GB
//   VMALLOC_START          +-------------------------------+
//                          |  vmalloc (unused; floor hosts |
//                          |  the kernel-half MMIO alias)  |  32TB
//   PAGE_OFFSET            +-------------------------------+
//                          |  linear map of all RAM        |  64TB
//   0xffff_c000_0000_0000  +-------------------------------+

/// PAGE_OFFSET - start of the kernel linear mapping region.
/// phys_to_virt(phys) = phys + (PAGE_OFFSET - phys_ram_base)
pub const PAGE_OFFSET: usize = 0xffff_c000_0000_0000;

/// KERN_VIRT_SIZE - the linear-map band [PAGE_OFFSET, KERNEL_LINK_ADDR)
/// (~64TB). Bounded below the kernel image so PAGE_OFFSET + KERN_VIRT_SIZE
/// stays inside usize (generic code computes that sum under
/// cfg(not(x86_64)) — sched.rs's kernel_ptr_canonical window).
pub const KERN_VIRT_SIZE: usize = 0x3fff_8000_0000;

/// VMALLOC region (32TB band under PAGE_OFFSET; the MMIO alias lives at
/// its floor like on riscv64)
pub const VMALLOC_SIZE: usize = 0x2000_0000_0000;
pub const VMALLOC_END: usize = PAGE_OFFSET;
pub const VMALLOC_START: usize = PAGE_OFFSET - VMALLOC_SIZE;

/// vmemmap region (4GB — same size as riscv64; covers struct-page arrays
/// for any QEMU-virt RAM size)
pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;
pub const VMEMMAP_END: usize = VMALLOC_START;
pub const VMEMMAP_START: usize = VMALLOC_START - VMEMMAP_SIZE;

/// Kernel window base VA (TTBR1 top 2GB; the early tables map
/// [0xffff_ffff_8000_0000, +34MB) to phys [0x4000_0000, +34MB))
pub const KERNEL_LINK_ADDR: usize = 0xffff_ffff_8000_0000;

/// Kernel IMAGE link base — the actual .text VMA: the image sits 512KB
/// (the arm64 text_offset) inside the window, i.e. at VA
/// KERNEL_LINK_ADDR + 0x8_0000 ↔ phys KERNEL_ENTRY.
pub const KERNEL_IMAGE_VA: usize = 0xffff_ffff_8008_0000;

// ==================== Physical <-> Virtual Address Conversion ====================

/// Physical RAM base (QEMU virt aarch64)
pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;

/// VA-PA offset for the linear mapping
/// va_pa_offset = PAGE_OFFSET - phys_ram_base
/// The aarch64 linear map places phys P at PAGE_OFFSET + P (identity from
/// zero — see setup_linear_mapping), so the VA-PA offset is PAGE_OFFSET.
pub const VA_PA_OFFSET: usize = PAGE_OFFSET;

/// Check if address is in the linear mapping region.
///
/// The linear map spans [PAGE_OFFSET, KERNEL_LINK_ADDR) — everything in
/// the TTBR1 half below the kernel image. The nominal KERN_VIRT_SIZE
/// (to the top of the half) would overflow when added to PAGE_OFFSET in
/// const evaluation, and the image window must stay OUTSIDE the linear
/// map anyway (it translates through va_kernel_pa_offset instead).
#[inline]
pub const fn is_linear_mapping(virt: usize) -> bool {
    virt >= PAGE_OFFSET && virt < KERNEL_LINK_ADDR
}

// ==================== Physical Memory Layout (QEMU virt aarch64) ====================

/// Kernel entry point / load address (RAM base + 0x80000, the arm64
/// text_offset convention; QEMU -kernel ELF loads segments at their LMA)
pub const KERNEL_ENTRY: u64 = 0x4008_0000;

/// Kernel image mapping budget: 32MB window (the riscv64 LAYOUT-4 lesson —
/// generous headroom from day one; the linker ASSERTs the image fits).
pub const KERNEL_SIZE: u64 = 0x2000_000;

/// Heap start address (after the kernel window)
/// Must stay at/above the boot.S image-window end (0x4008_0000 + 32MB).
pub const HEAP_START: u64 = 0x4280_0000;

/// Slab start address (after heap; actual address depends on
/// KERNEL_HEAP_SIZE config)
pub const SLAB_START_DEFAULT: u64 = HEAP_START + (32 * 1024 * 1024);

// ==================== Device Addresses (QEMU virt aarch64) ====================

/// PL011 UART base address
pub const UART_BASE: u64 = 0x0900_0000;

/// VirtIO MMIO transport base (32 slots at 0x200 granularity)
pub const VIRTIO_MMIO_BASE: u64 = 0x0a00_0000;

/// GICv3 distributor base (re-distributors follow; DTB is authoritative)
pub const GICD_BASE: u64 = 0x0800_0000;
pub const GICR_BASE: u64 = 0x080A_0000;

/// PCIe ECAM base address (virt high MMIO)
pub const PCIE_ECAM_BASE: u64 = 0x4010_0000;

/// PCI MMIO base address (virt high MMIO window)
pub const PCI_MMIO_BASE: u64 = 0x5000_0000;

// ==================== Kernel-half MMIO alias ====================
//
// Same rationale as riscv64: user MAP_FIXED over a cloned low device
// window must never kill a kernel MMIO access. The alias re-maps the low
// device physical window (all MMIO on QEMU virt aarch64 sits below
// 0x4000_0000 where RAM starts) into the (currently unused) vmalloc band's
// floor. Runtime MMIO accessors translate through mmio_alias().

/// Base of the kernel-half alias of the low device window
pub const MMIO_ALIAS_BASE: usize = VMALLOC_START;

/// Alias window size: covers phys [0, 1GB) — every virt MMIO device
/// (UART 0x0900_0000, virtio-mmio 0x0a00_0000, GIC 0x0800_0000); RAM
/// starts at 0x4000_0000 so the alias never double-maps RAM.
pub const MMIO_ALIAS_SIZE: usize = 0x4000_0000;

/// Compile-time placement guards (canonical upper half, below linear map)
const _: () = assert!(MMIO_ALIAS_BASE >= 0xffff_0000_0000_0000_usize);
const _: () = assert!(MMIO_ALIAS_BASE + MMIO_ALIAS_SIZE <= PAGE_OFFSET);

/// Translate a low identity device address (va == phys, below
/// MMIO_ALIAS_SIZE) to its permanent kernel-half alias.
#[inline]
pub const fn mmio_alias(low: u64) -> u64 {
    MMIO_ALIAS_BASE as u64 + low
}

/// Device alias for DTB-probed MMIO (same translation; interface parity
/// with the riscv64 backend's device_alias).
#[inline]
pub const fn device_alias(low: u64) -> u64 {
    mmio_alias(low)
}

// ==================== Kernel Mapping ====================

/// Runtime kernel mapping information (populated by boot.S before
/// rust_main; same struct contract as the riscv64 backend)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct KernelMapping {
    /// Kernel virtual address (linked address)
    pub virt_addr: usize,
    /// KASLR offset (0 — no KASLR)
    pub virt_offset: usize,
    /// Kernel physical load address
    pub phys_addr: usize,
    /// Kernel image size
    pub size: usize,
    /// VA-PA offset for the linear mapping (phys_to_virt)
    pub va_pa_offset: usize,
    /// VA-PA offset for the kernel image mapping
    pub va_kernel_pa_offset: usize,
    /// PAGE_OFFSET value
    pub page_offset: usize,
}

/// Global kernel mapping structure, initialized by boot.S
extern "C" {
    pub static mut KERNEL_MAP: KernelMapping;
}

/// Physical RAM base address (runtime from device tree)
#[used]
#[link_section = ".data"]
pub static mut PHYS_RAM_BASE: usize = PHYS_MEMORY_BASE;

// ==================== mmap Constant definitions ====================
// Identical values to the riscv64 backend (generic ABI constants).

/// mmap protection flags (prot)
pub mod prot {
    pub const PROT_READ: u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
    pub const PROT_EXEC: u32 = 0x4;
    pub const PROT_NONE: u32 = 0x0;
    pub const PROT_MASK: u32 = 0x7;
}

/// mmap mapping flags (flags)
pub mod map {
    pub const MAP_SHARED: u32 = 0x01;
    pub const MAP_PRIVATE: u32 = 0x02;
    pub const MAP_TYPE_MASK: u32 = 0x0f;
    pub const MAP_FIXED: u32 = 0x10;
    pub const MAP_ANONYMOUS: u32 = 0x20;
    pub const MAP_STACK: u32 = 0x20000;
    pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;
    pub const MAP_HUGETLB: u32 = 0x40000;
    pub const MAP_LOCKED: u32 = 0x2000;
    pub const MAP_NORESERVE: u32 = 0x4000;
    pub const MAP_POPULATE: u32 = 0x8000;
    pub const MAP_NODUMP: u32 = 0x10000;
}

/// mmap error codes
pub mod mmap_error {
    pub const EINVAL: i64 = -22;
    pub const ENOMEM: i64 = -12;
    pub const EACCES: i64 = -13;
    pub const EFAULT: i64 = -14;
    pub const ENOSPC: i64 = -28;
    pub const ENODEV: i64 = -19;
    pub const EBADF: i64 = -9;
    pub const EOPNOTSUPP: i64 = -95;
    pub const EEXIST: i64 = -17;
}

/// User space address range (256GB, riscv64 parity)
pub mod user_addr {
    pub const USER_START: usize = 0x0000_0000;
    pub const USER_END: usize = super::TASK_SIZE;
    pub const TASK_SIZE: usize = super::TASK_SIZE;
    pub const TASK_UNMAPPED_BASE: usize = super::TASK_SIZE / 3;
    pub const MMAP_LEGACY_BASE: usize = super::TASK_SIZE / 3;
    pub const MMAP_START: usize = super::TASK_SIZE - (64 * 1024 * 1024 * 1024);
    pub const MMAP_END: usize = super::TASK_SIZE;
    /// brk default (clear of the first 512MB; matches riscv64's policy of
    /// staying above any low device identity window)
    pub const BRK_DEFAULT: usize = 0x2000_0000;
    pub const BRK_MAX: usize = TASK_UNMAPPED_BASE;
    pub const STACK_TOP: usize = super::TASK_SIZE - (super::PAGE_SIZE as usize);
    pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024;
    pub const STACK_MIN_SIZE: usize = 1 * 1024 * 1024;
    pub const HEAP_START: usize = BRK_DEFAULT;
    pub const HEAP_MAX_SIZE: usize = BRK_MAX - BRK_DEFAULT;
    pub const PAGE_ZERO_SIZE: usize = 4 * 1024;
    pub const MIN_MAP_ADDR: usize = PAGE_ZERO_SIZE;
}

// ==================== Address Types ====================

/// Virtual address type with AArch64 48-bit sign extension
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    /// Create virtual address with proper 48-bit sign extension:
    /// bit 47 selects the half (0 = TTBR0/user, 1 = TTBR1/kernel).
    #[inline]
    pub const fn new(addr: u64) -> Self {
        let bit47 = (addr >> 47) & 1;
        if bit47 == 1 {
            // Kernel address: sign extend bit 47 to bits 63:48
            Self(addr | 0xFFFF_0000_0000_0000)
        } else {
            // User address: clear bits 63:48
            Self(addr & 0x0000_FFFF_FFFF_FFFF)
        }
    }

    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    #[inline]
    pub fn page_offset(&self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    /// Index into the table at `level` (0 = L0/512GB .. 3 = L3/4KB)
    #[inline]
    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1FF
    }

    #[inline]
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}

/// Physical address type
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    #[inline]
    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    #[inline]
    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    /// Physical page number (output address field [47:12])
    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 >> PAGE_SHIFT
    }
}

/// Convert physical address to kernel virtual address (linear mapping)
#[inline]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    // SAFETY: KERNEL_MAP is a static extern initialized by boot.S before
    // rust_main; va_pa_offset is set once at boot and never modified.
    let va_pa_offset = unsafe { KERNEL_MAP.va_pa_offset };
    VirtAddr::new(phys.0.wrapping_add(va_pa_offset as u64))
}

/// Convert kernel virtual address to physical address (linear mapping)
#[inline]
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    let addr = virt.0;

    if is_linear_mapping(addr as usize) {
        // SAFETY: see phys_to_virt.
        let va_pa_offset = unsafe { KERNEL_MAP.va_pa_offset };
        PhysAddr::new(addr - va_pa_offset as u64)
    } else if (addr as usize) >= KERNEL_IMAGE_VA
        && addr < (KERNEL_IMAGE_VA as u64 + 0x2000_000)
    {
        // Kernel image mapping: linked at KERNEL_IMAGE_VA, loaded at
        // KERNEL_ENTRY. Static driver buffers (virtio queues in .bss)
        // translate through here.
        PhysAddr::new(addr - KERNEL_IMAGE_VA as u64 + KERNEL_ENTRY)
    } else if addr >= KERNEL_ENTRY && addr < 0x8000_0000 {
        // Legacy identity mapping (transition period)
        PhysAddr::new(addr)
    } else {
        crate::pr_warn!(
            "virt_to_phys: address {:#x} is outside the linear mapping",
            addr
        );
        PhysAddr::new(addr)
    }
}
