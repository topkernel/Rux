//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V MMU Initialization and Page Mapping
//!
//! This module contains:
//! - Page table allocation (early/fixmap/late stages)
//! - MMU initialization functions
//! - Page mapping functions (map_page, map_region, etc.)
//! - Linear mapping setup
//! - Device mapping functions

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use super::memory_layout::*;
use super::pagetable::*;
use crate::mm::{MmStruct, alloc_pages, free_pages, GfpFlags};
use crate::mm::page::{PAGE_SIZE as PAGE_SIZE_USIZE, VirtAddr as PageVirtAddr};

// ==================== Assembly Page Tables (defined in boot.S) ====================

/// Trampoline page directory - maps only first 2MB of kernel
extern "C" {
    /// Trampoline PGD - minimal mapping for MMU enable
    pub static trampoline_pg_dir: [u8; 4096];
    /// Trampoline PMD - contains 2MB kernel mapping
    pub static trampoline_pmd: [u8; 4096];
    /// Early PGD - full early mapping
    pub static early_pg_dir: [u8; 4096];
    /// Early PMD for kernel region
    pub static early_pmd: [u8; 4096];
    /// Early PMD for device region
    pub static early_pmd_dev: [u8; 4096];
}

// ==================== Root Page Table ====================

#[link_section = ".bss"]
pub static mut ROOT_PAGE_TABLE: PageTable = PageTable::new();

static MMU_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[link_section = ".bss"]
static mut TRAP_STACKS: [[u8; 16384]; 4] = [[0; 16384]; 4];  // 4 CPUs

/// Get trap stack for current CPU
pub unsafe fn get_trap_stack() -> u64 {
    let cpu_id = crate::arch::riscv64::smp::cpu_id() as usize;
    if cpu_id >= 4 {
        panic!("mm: Invalid CPU ID {}", cpu_id);
    }
    let stack_base = &mut TRAP_STACKS[cpu_id] as *mut [u8; 16384] as *mut u8;
    stack_base.add(16384) as u64  // stack top
}

// ==================== Page Table Allocation (Linux-style three-stage) ====================
//
// Linux uses three stages for page table allocation:
// 1. Early: Static arrays (MMU not enabled yet, identity mapping)
// 2. Fixmap: memblock allocation (MMU enabled, but buddy not ready)
// 3. Late: Buddy allocator (full memory management available)

/// Number of early page tables
const NUM_EARLY_PMD: usize = 4;   // L1 page tables (covers 4GB virtual space)
const NUM_EARLY_PTE: usize = 48;  // L0 page tables (covers 96MB mapped space)

/// Early page tables for boot (like Linux's early_pmd, early_pte)
#[link_section = ".bss"]
static mut EARLY_PMD: [PageTable; NUM_EARLY_PMD] = [PageTable::new(); NUM_EARLY_PMD];
#[link_section = ".bss"]
static mut EARLY_PTE: [PageTable; NUM_EARLY_PTE] = [PageTable::new(); NUM_EARLY_PTE];

/// Counter for early page table allocation
static EARLY_PMD_NEXT: AtomicUsize = AtomicUsize::new(0);
static EARLY_PTE_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocation stage tracking
#[derive(Debug, Clone, Copy, PartialEq)]
enum AllocStage {
    /// Early boot: MMU not fully enabled, use static arrays with identity mapping
    Early,
    /// Fixmap stage: MMU enabled, use memblock allocation
    Fixmap,
    /// Late stage: Buddy allocator ready, use normal page allocation
    Late,
}

impl AllocStage {
    const fn from_u8(v: u8) -> Self {
        match v {
            0 => AllocStage::Early,
            1 => AllocStage::Fixmap,
            2 => AllocStage::Late,
            _ => AllocStage::Late,
        }
    }
}

/// Current allocation stage
static ALLOC_STAGE: AtomicU8 = AtomicU8::new(AllocStage::Early as u8);

/// Get current allocation stage
fn get_alloc_stage() -> AllocStage {
    AllocStage::from_u8(ALLOC_STAGE.load(Ordering::Acquire))
}

/// Transition to fixmap stage (MMU enabled, can use memblock)
pub fn pt_ops_set_fixmap() {
    ALLOC_STAGE.store(AllocStage::Fixmap as u8, Ordering::Release);
}

/// Transition to late stage (buddy allocator ready)
pub fn pt_ops_set_late() {
    ALLOC_STAGE.store(AllocStage::Late as u8, Ordering::Release);
}

/// Check if frame allocator is ready
#[inline]
fn is_frame_allocator_ready() -> bool {
    get_alloc_stage() == AllocStage::Late
}

/// Allocate a page table and return its physical address
///
/// Three-stage allocation like Linux:
/// - Early: static arrays (identity mapped)
/// - Fixmap: memblock allocation (linear mapped)
/// - Late: buddy allocator (linear mapped)
pub unsafe fn alloc_page_table() -> Option<u64> {
    let stage = get_alloc_stage();
    match stage {
        AllocStage::Early => {
            // Early boot: use static arrays with identity mapping
            let pmd_idx = EARLY_PMD_NEXT.load(Ordering::Acquire);
            if pmd_idx < NUM_EARLY_PMD {
                EARLY_PMD_NEXT.store(pmd_idx + 1, Ordering::Release);
                let table_ptr = &EARLY_PMD[pmd_idx] as *const PageTable as u64;
                core::ptr::write_bytes(table_ptr as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_ptr);
            }

            let pte_idx = EARLY_PTE_NEXT.load(Ordering::Acquire);
            if pte_idx < NUM_EARLY_PTE {
                EARLY_PTE_NEXT.store(pte_idx + 1, Ordering::Release);
                let table_ptr = &EARLY_PTE[pte_idx] as *const PageTable as u64;
                core::ptr::write_bytes(table_ptr as *mut u8, 0, PAGE_SIZE as usize);
                return Some(table_ptr);
            }

            panic!("mm: Out of early page tables (PMD: {}, PTE: {})", pmd_idx, pte_idx);
        }
        AllocStage::Fixmap => {
            // Fixmap stage: use memblock allocation
            let phys_addr = crate::mm::memblock::memblock_phys_alloc()?;

            // Use identity mapping only for actually identity-mapped regions
            let virt_addr = if phys_addr >= 0x80000000 && phys_addr < 0x88000000 {
                phys_addr as *mut u8  // Identity mapping
            } else {
                // For other regions, use linear mapping
                phys_to_virt(PhysAddr::new(phys_addr as u64)).bits() as *mut u8
            };
            core::ptr::write_bytes(virt_addr, 0, PAGE_SIZE as usize);
            Some(phys_addr as u64)
        }
        AllocStage::Late => {
            // Late stage: use zone allocator
            use crate::mm::zone::ZoneType;

            let phys_addr = if let Some(node) = crate::mm::pglist::first_online_node_mut() {
                if let Some(zone) = node.zone_mut(ZoneType::ZoneNormal) {
                    if zone.is_initialized() {
                        if let Some(pfn) = zone.alloc_pages(0) {
                            let page = crate::mm::page_desc::pfn_to_page_mut(pfn);
                            if !page.is_null() {
                                unsafe {
                                    (*page).set_refcount(1);
                                    (*page).set_order(0);
                                    (*page).set_flag(crate::mm::page_desc::PageFlag::Referenced);
                                }
                            }
                            crate::mm::zone::pfn_to_phys(pfn) as u64
                        } else {
                            0
                        }
                    } else {
                        0
                    }
                } else {
                    0
                }
            } else {
                0
            };

            if phys_addr == 0 {
                return None;
            }

            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            core::ptr::write_bytes(virt_addr.bits() as *mut u8, 0, PAGE_SIZE as usize);
            Some(phys_addr)
        }
    }
}

/// Get virtual address for accessing a page table given its physical address
#[inline]
pub unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    match get_alloc_stage() {
        AllocStage::Early => {
            // Identity mapping during early boot
            phys_addr as *mut PageTable
        }
        AllocStage::Fixmap => {
            // Use identity mapping only for actually identity-mapped regions
            if phys_addr >= 0x80000000 && phys_addr < 0x88000000 {
                phys_addr as *mut PageTable  // Identity mapping
            } else {
                // For other regions, use linear mapping
                let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
                virt_addr.bits() as *mut PageTable
            }
        }
        AllocStage::Late => {
            // Use linear mapping after buddy allocator is ready
            let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
            virt_addr.bits() as *mut PageTable
        }
    }
}

/// Free a page table (only valid for late stage allocations)
unsafe fn free_page_table(phys_addr: u64) {
    if get_alloc_stage() != AllocStage::Late {
        return;
    }

    // Check if it's from early static region
    let early_pmd_start = &EARLY_PMD as *const _ as u64;
    let early_pmd_end = early_pmd_start + (NUM_EARLY_PMD * PAGE_SIZE as usize) as u64;
    let early_pte_start = &EARLY_PTE as *const _ as u64;
    let early_pte_end = early_pte_start + (NUM_EARLY_PTE * PAGE_SIZE as usize) as u64;

    if (phys_addr >= early_pmd_start && phys_addr < early_pmd_end) ||
       (phys_addr >= early_pte_start && phys_addr < early_pte_end) {
        // Don't free early page tables
        return;
    }

    // Use zone allocator to free
    crate::mm::page_alloc::free_pages(phys_addr as usize, 0);
}

/// Free all page tables and user data pages used by a user address space
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    use crate::mm::{pfn_to_page, phys_to_pfn, phys_valid, page_desc::PageFlag};

    let root_phys = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_phys);

    // Walk and free all levels (only user space: VPN2 0-255)
    for vpn2 in 0..256 {
        let pte2 = (*root_table).get(vpn2);
        if pte2.is_valid() {
            // Check if L2 is a leaf (1GB huge page)
            if pte2.is_leaf() {
                let phys_addr = pte2.ppn() << PAGE_SHIFT;
                let pfn = phys_to_pfn(phys_addr as usize);
                let page = pfn_to_page(pfn);
                if !page.is_null() {
                    if (*page).test_flag(PageFlag::Cow) {
                        (*page).put_page();
                    } else {
                        free_pages(phys_addr as usize, 0);
                    }
                }
                continue;
            }

            let ppn1 = pte2.ppn();
            let table1_phys = ppn1 << PAGE_SHIFT;

            if !phys_valid(table1_phys as usize) {
                continue;
            }

            if table1_phys == root_phys {
                continue;
            }

            let table1 = get_page_table_virt(table1_phys);

            for vpn1 in 0..512 {
                let pte1 = (*table1).get(vpn1);
                if pte1.is_valid() {
                    if pte1.is_leaf() {
                        let phys_addr = pte1.ppn() << PAGE_SHIFT;
                        let pfn = phys_to_pfn(phys_addr as usize);
                        let page = pfn_to_page(pfn);
                        if !page.is_null() {
                            if (*page).test_flag(PageFlag::Cow) {
                                (*page).put_page();
                            } else {
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                        continue;
                    }

                    let ppn0 = pte1.ppn();
                    let table0_phys = ppn0 << PAGE_SHIFT;

                    if !phys_valid(table0_phys as usize) {
                        continue;
                    }

                    let table0 = get_page_table_virt(table0_phys);

                    for vpn0 in 0..512 {
                        let pte0 = (*table0).get(vpn0);
                        if pte0.is_valid() && pte0.is_leaf() {
                            let phys_addr = pte0.ppn() << PAGE_SHIFT;
                            let pfn = phys_to_pfn(phys_addr as usize);
                            let page = pfn_to_page(pfn);

                            if page.is_null() || phys_addr < 0x80000000 {
                                continue;
                            }

                            if (*page).test_flag(PageFlag::Cow) {
                                (*page).put_page();
                            } else {
                                free_pages(phys_addr as usize, 0);
                            }
                        }
                    }
                    free_page_table(table0_phys);
                }
            }
            free_page_table(table1_phys);
        }
    }

    // Free root table (L2)
    free_page_table(root_phys);
}

// ==================== Page Mapping Functions ====================

/// Map a single page in page table
///
/// # Arguments
/// - root_ppn: Root page table physical page number
/// - virt: Virtual address
/// - phys: Physical address
/// - flags: Page table entry flags
pub unsafe fn map_page(root_ppn: u64, virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let virt_addr = virt.bits();
    let phys_addr = phys.bits();

    // Extract virtual page numbers (VPN2, VPN1, VPN0)
    let vpn2 = ((virt_addr >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt_addr >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt_addr >> 12) & 0x1FF) as usize;

    // Get root page table (L2)
    let root_table_addr = root_ppn << PAGE_SHIFT;
    let root_table = get_page_table_virt(root_table_addr);
    let root = &mut *root_table;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 1 -> Level 0
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        pte1.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_page: failed to allocate L0 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        ppn
    };

    // Level 0 -> Physical page
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);
    let table0_ref = &mut *table0;
    let ppn: u64 = phys_addr >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));

    // Flush TLB
    asm!("sfence.vma");
}

/// Map a region with identity mapping
pub(crate) unsafe fn map_region(root_ppn: u64, start: u64, size: u64, flags: u64) {
    let virt_start = VirtAddr::new(start);
    let phys_start = PhysAddr::new(start);
    let virt_end = VirtAddr::new(start + size);

    let mut virt = virt_start.floor();
    let end = virt_end.ceil();

    while virt.bits() < end.bits() {
        let offset = virt.bits() - virt_start.bits();
        let phys = PhysAddr::new(phys_start.bits() + offset);
        map_page(root_ppn, virt, phys, flags);
        virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
    }
}

/// Map a 2MB huge page using PMD leaf entry
unsafe fn map_pmd_huge_page(virt: usize, phys: usize, flags: u64) {
    let vpn2 = (virt >> 30) & 0x1FF;
    let vpn1 = (virt >> 21) & 0x1FF;

    // Get root page table (L2)
    let root = &mut ROOT_PAGE_TABLE;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_pmd_huge_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Create PMD leaf entry (2MB huge page)
    let ppn = (phys >> 12) as u64;
    let entry_bits = (ppn << 10) | flags;

    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    (*table1).set(vpn1, PageTableEntry::from_bits(entry_bits));
}

/// Map a kernel virtual page to a physical page
///
/// Used for vmemmap and other kernel mappings that need 4KB page granularity.
pub unsafe fn map_kernel_page(virt: u64, phys: u64, flags: u64) {
    let vpn2 = ((virt >> 30) & 0x1FF) as usize;
    let vpn1 = ((virt >> 21) & 0x1FF) as usize;
    let vpn0 = ((virt >> 12) & 0x1FF) as usize;

    // Get root page table from current satp
    let satp: u64;
    asm!("csrr {}, satp", out(reg) satp);
    let root_ppn = satp & 0xFFFFFFFFFFFFF;
    let root_phys = root_ppn << PAGE_SHIFT;
    let root = get_page_table_virt(root_phys) as *mut PageTable;
    let root = &mut *root;

    // Level 2 -> Level 1
    let pte2 = root.get(vpn2);
    let ppn1 = if pte2.is_valid() {
        pte2.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_kernel_page: failed to allocate L1 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        root.set(vpn2, PageTableEntry::new_table(ppn));
        asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Level 1 -> Level 0
    let table1_phys = ppn1 << PAGE_SHIFT;
    let table1 = get_page_table_virt(table1_phys);
    let table1_ref = &mut *table1;
    let pte1 = table1_ref.get(vpn1);
    let ppn0 = if pte1.is_valid() {
        pte1.ppn()
    } else {
        let table_phys = alloc_page_table().expect("map_kernel_page: failed to allocate L0 page table");
        let ppn = table_phys >> PAGE_SHIFT;
        table1_ref.set(vpn1, PageTableEntry::new_table(ppn));
        asm!("sfence.vma zero, zero", options(nomem, nostack));
        ppn
    };

    // Level 0 -> Physical page
    let table0_phys = ppn0 << PAGE_SHIFT;
    let table0 = get_page_table_virt(table0_phys);
    let table0_ref = &mut *table0;
    let ppn: u64 = phys >> PAGE_SHIFT;
    let pte_bits: u64 = (ppn << 10) | flags;

    table0_ref.set(vpn0, PageTableEntry::from_bits(pte_bits));

    asm!("sfence.vma zero, zero", options(nomem, nostack));
}

/// Map device memory page to user space
pub fn map_device_page(virt: usize, phys: usize, flags: u64) {
    let vpn2 = (virt >> 30) & 0x1FF;
    let ppn = (phys >> 12) as u64;
    let entry_bits = (ppn << 10) | flags;

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2 as usize, PageTableEntry::from_bits(entry_bits));
    }

    unsafe {
        asm!("sfence.vma", options(nomem, nostack));
    }
}

/// Select best mapping size (Linux-style)
#[inline]
fn best_map_size(pa: usize, va: usize, size: usize) -> usize {
    const PMD_MASK: usize = (PMD_SIZE as usize) - 1;

    // For 64-bit: use PMD_SIZE (2MB) if aligned
    if (pa & PMD_MASK) == 0 && (va & PMD_MASK) == 0 && size >= PMD_SIZE as usize {
        PMD_SIZE as usize
    } else {
        PAGE_SIZE as usize
    }
}

// ==================== MMU Initialization ====================

/// Setup early page tables for MMU enable (Linux-style)
///
/// This function is called from boot.S before relocate_enable_mmu.
#[no_mangle]
pub unsafe extern "C" fn setup_vm() {
    // Get the VA-PA offset by comparing linked address with runtime address
    let va_pa_offset: u64;
    asm!(
        "1:",
        "auipc {offset}, 0",
        "la {virt}, 1b",
        "sub {offset}, {virt}, {offset}",
        offset = out(reg) va_pa_offset,
        virt = out(reg) _,
        options(nostack),
    );

    extern "C" {
        static mut early_pg_dir: [u8; 4096];
    }

    let early_pg_dir_va = &raw mut early_pg_dir as *mut u8 as u64;
    let early_pg_dir_pa = early_pg_dir_va - va_pa_offset;
    let early_pg_dir_ptr = early_pg_dir_pa as *mut PageTable;
    let early_pg_dir_ref = &mut *early_pg_dir_ptr;

    early_pg_dir_ref.zero();

    let early_ppn = early_pg_dir_pa / PAGE_SIZE;

    let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;
    let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                       PageTableEntry::A | PageTableEntry::D;

    // Map kernel with identity mapping
    let kernel_phys = KERNEL_ENTRY;
    let kernel_virt = KERNEL_ENTRY + VA_PA_OFFSET as u64;
    let kernel_size = KERNEL_SIZE;

    let mut phys = kernel_phys;
    let mut virt = kernel_phys;
    let end_phys = kernel_phys + kernel_size;

    while phys < end_phys {
        map_page(early_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
        phys += PAGE_SIZE;
        virt += PAGE_SIZE;
    }

    // Map kernel at virtual address
    phys = kernel_phys;
    virt = kernel_virt;
    while phys < end_phys {
        map_page(early_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
        phys += PAGE_SIZE;
        virt += PAGE_SIZE;
    }

    // Map UART (identity mapping)
    map_region(early_ppn, UART_BASE, 0x1000, device_flags);

    // Map DTB area (identity mapping)
    map_region(early_ppn, DTB_BASE, 0x100000, device_flags);
}

/// Initialize MMU
pub fn init() {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);

        // Check if MMU is already enabled
        if satp >> 60 != 0 {
            return;
        }

        // Try to acquire initialization lock
        if !MMU_INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            while !MMU_INITIALIZED.load(Ordering::Acquire) {
                asm!("nop", options(nomem, nostack));
            }

            let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
            let addr_space = MmStruct::new_kernel(root_ppn);
            addr_space.enable();

            return;
        }

        // Initialize root page table
        ROOT_PAGE_TABLE.zero();

        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;

        let kernel_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::X | PageTableEntry::A | PageTableEntry::D;

        // Map kernel code
        let kernel_phys = KERNEL_ENTRY;
        let kernel_virt = KERNEL_ENTRY + VA_PA_OFFSET as u64;
        let kernel_size = KERNEL_SIZE;

        let mut phys = kernel_phys;
        let mut virt = kernel_virt;
        let end_phys = kernel_phys + kernel_size;

        while phys < end_phys {
            map_page(root_ppn, VirtAddr::new(virt), PhysAddr::new(phys), kernel_flags);
            phys += PAGE_SIZE;
            virt += PAGE_SIZE;
        }

        // Keep identity mapping for early boot
        map_region(root_ppn, KERNEL_ENTRY, KERNEL_SIZE, kernel_flags);

        // Map heap space
        let heap_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                        PageTableEntry::A | PageTableEntry::D;
        let heap_virt_start = HEAP_START;
        let heap_phys_start = HEAP_START;
        let heap_size = crate::config::KERNEL_HEAP_SIZE as u64;

        let virt_start = VirtAddr::new(heap_virt_start);
        let phys_start = PhysAddr::new(heap_phys_start);
        let virt_end = VirtAddr::new(heap_virt_start + heap_size);
        let mut virt = virt_start.floor();
        let end = virt_end.ceil();

        while virt.bits() < end.bits() {
            let offset = virt.bits() - virt_start.bits();
            let phys = PhysAddr::new(phys_start.bits() + offset);
            map_page(root_ppn, virt, phys, heap_flags);
            virt = VirtAddr::new(virt.bits() + PAGE_SIZE);
        }

        // Map Slab allocator area
        let slab_virt_start = HEAP_START + crate::config::KERNEL_HEAP_SIZE as u64;
        let slab_size = 4 * 1024 * 1024u64;
        map_region(root_ppn, slab_virt_start, slab_size, heap_flags);

        // Map gap region
        let gap_start = slab_virt_start + slab_size;
        let gap_end = 0x84000000u64;
        let gap_size = gap_end - gap_start;
        map_region(root_ppn, gap_start, gap_size, heap_flags);

        // Map UART device
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;
        map_region(root_ppn, UART_BASE, 0x1000, device_flags);

        // Map DTB area
        map_region(root_ppn, DTB_BASE, 0x100000, device_flags);

        // Initialize UART fixmap
        super::fixmap::init_uart_fixmap();

        // Enable MMU
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

/// Setup device mappings (called after fixmap stage is ready)
#[allow(dead_code)]
pub fn setup_device_mappings() {
    unsafe {
        let satp: u64;
        asm!("csrr {}, satp", out(reg) satp);
        let root_ppn = ((satp & 0xFFFFFFFFFFFFF) as usize) as u64;
        let device_flags = PageTableEntry::V | PageTableEntry::R | PageTableEntry::W |
                          PageTableEntry::A | PageTableEntry::D;

        map_region(root_ppn, VIRTIO_MMIO_BASE, 0x100000, device_flags);
        map_region(root_ppn, PLIC_BASE, 0x200000, device_flags);
        map_region(root_ppn, CLINT_BASE, 0x10000, device_flags);
        map_region(root_ppn, PCIE_ECAM_BASE, 0x100000, device_flags);
        map_region(root_ppn, PCI_MMIO_BASE, 0x10000000, device_flags);
    }
}

/// Setup linear mapping for physical memory (Linux-style)
pub fn setup_linear_mapping(memory_regions: &[crate::cmdline::MemoryRegion]) {
    unsafe {
        let linear_flags = PageTableEntry::V | PageTableEntry::R |
                          PageTableEntry::W | PageTableEntry::A | PageTableEntry::D;

        for region in memory_regions {
            let phys_start = region.base;
            let size = region.size;
            let phys_end = phys_start + size;

            let virt_start = phys_start + VA_PA_OFFSET;

            let mut phys = phys_start;
            let mut virt = virt_start;

            while phys < phys_end {
                let remaining = phys_end - phys;
                let map_size = best_map_size(phys, virt, remaining);

                if map_size == PMD_SIZE as usize {
                    map_pmd_huge_page(virt, phys, linear_flags);
                } else {
                    map_kernel_page(virt as u64, phys as u64, linear_flags);
                }

                phys += map_size;
                virt += map_size;
            }
        }

        asm!("sfence.vma zero, zero", options(nomem, nostack));
    }
}

/// Enable MMU (secondary function)
pub fn enable() {
    unsafe {
        let root_ppn = (&raw mut ROOT_PAGE_TABLE as *mut PageTable as u64) / PAGE_SIZE;
        let addr_space = MmStruct::new_kernel(root_ppn);
        addr_space.enable();
    }
}

/// Map identity mapping
pub fn map_identity(virt: VirtAddr, phys: PhysAddr, flags: u64) {
    let vpn2 = virt.vpn(2) as usize;
    let ppn = phys.ppn();

    unsafe {
        ROOT_PAGE_TABLE.set(vpn2, PageTableEntry::from_bits((ppn << 10) | flags));
    }
}

/// Get kernel page table PPN
pub fn get_kernel_page_table_ppn() -> u64 {
    unsafe {
        let root_addr = &raw mut ROOT_PAGE_TABLE as *mut PageTable as u64;
        root_addr / PAGE_SIZE
    }
}
