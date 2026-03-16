//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory Descriptor
//!
//!
//! This module implements the mm_struct abstraction for describing process address spaces.
//!
//! Key fields:
//! - pgd: Page table base address
//! - mmap: VMA manager
//! - start_code/end_code: Code segment range
//! - start_data/end_data: Data segment range
//! - start_brk/brk: Heap region
//! - start_stack: Stack start address
//! - arg_start/arg_end: Command line arguments
//! - env_start/env_end: Environment variables
//! - total_vm: Total virtual memory page count
//! - locked_vm: Locked memory page count
//!
//! # Architecture Design
//!
//! `MmStruct` is a platform-independent data structure containing all mm_struct fields.
//! Architecture-specific operations (such as page table mapping) are implemented through:
//!
//! 1. Platform-independent methods defined in `impl MmStruct` (e.g., field accessors)
//! 2. Architecture-specific methods added in `arch/*/mm.rs` via extension traits or impl blocks
//!
//! This design follows a layered architecture:
//! - mm_struct is the generic memory descriptor
//! - Architecture-specific pte/pmd/pud/p4d/pgd operations are implemented in arch directory

extern crate alloc;

use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use spin::RwLock;

use crate::mm::vma::{VmaManager, Vma, VmaFlags, VmaType};
use crate::mm::page::VirtAddr;
use crate::mm::pagemap::{MapError, Perm, PageTableType};

/// Memory Descriptor
///
/// Structure describing a process's complete address space.
pub struct MmStruct {
    // ==================== Page Table Management ====================
    /// Page table root PPN (Page Global Directory)
    pub pgd: u64,

    /// VMA manager (protected by RwLock for interior mutability)
    vma_manager: RwLock<VmaManager>,

    /// Address space type
    space_type: PageTableType,

    // ==================== Segment Ranges ====================
    /// Code segment start address
    start_code: AtomicUsize,

    /// Code segment end address
    end_code: AtomicUsize,

    /// Data segment start address
    start_data: AtomicUsize,

    /// Data segment end address
    end_data: AtomicUsize,

    // ==================== Heap Management ====================
    /// Heap start address (minimum brk value)
    start_brk: AtomicUsize,

    /// Current heap pointer (current brk value)
    brk: AtomicUsize,

    // ==================== Stack Management ====================
    /// Stack start address (stack top)
    start_stack: AtomicUsize,

    // ==================== Arguments and Environment Variables ====================
    /// Command line arguments start address
    arg_start: AtomicUsize,

    /// Command line arguments end address
    arg_end: AtomicUsize,

    /// Environment variables start address
    env_start: AtomicUsize,

    /// Environment variables end address
    env_end: AtomicUsize,

    // ==================== Virtual Memory Statistics ====================
    /// Total virtual memory page count
    total_vm: AtomicU64,

    /// Locked memory page count
    locked_vm: AtomicU64,

    /// Pinned memory page count
    pinned_vm: AtomicU64,

    /// Data segment page count
    data_vm: AtomicU64,

    /// Executable segment page count
    exec_vm: AtomicU64,

    /// Stack page count
    stack_vm: AtomicU64,

    // ==================== mmap Region Management ====================
    /// mmap region base address
    mmap_base: AtomicUsize,

    /// mmap region legacy base address
    mmap_legacy_base: AtomicUsize,

    /// Highest virtual memory end address
    highest_vm_end: AtomicUsize,

    // ==================== Reference Counting ====================
    /// User count: number of threads sharing this mm
    mm_users: AtomicI32,

    /// Reference count: lifetime reference of mm_struct
    mm_count: AtomicI32,

    // ==================== Other Fields ====================
    /// Flags
    flags: AtomicU64,

    /// Owning task (optional)
    owner_pid: AtomicI32,
}

/// MmStruct flags
pub struct MmFlags;
impl MmFlags {
    /// Core dump in progress
    pub const MMF_DUMP_CORE: u64 = 0x00000001;
    /// Skip shared mappings
    pub const MMF_DUMP_SKIP_SHARED: u64 = 0x00000002;
    /// Skip private mappings
    pub const MMF_DUMP_SKIP_PRIVATE: u64 = 0x00000004;
    /// Dumped
    pub const MMF_DUMPED: u64 = 0x00000008;
    /// OOM notification disabled
    pub const MMF_OOM_DISABLE: u64 = 0x00000010;
    /// OOM score adjustment
    pub const MMF_OOM_SCORE_ADJ: u64 = 0x00000020;
}

impl MmStruct {
    /// Create a new memory descriptor
    ///
    /// # Arguments
    /// - `pgd`: Page table root PPN
    /// - `space_type`: Address space type
    ///
    /// # Safety
    /// Caller must ensure `pgd` points to a valid page table
    pub unsafe fn new(pgd: u64, space_type: PageTableType) -> Self {
        use super::vma::RiscVAddressSpaceLayout;
        use super::vma::AddressSpaceLayout;

        let vma_manager = VmaManager::new();
        let brk_default = if space_type == PageTableType::User {
            super::vma::RiscVAddressSpaceLayout::heap_start()
        } else {
            0
        };

        let mmap_base = if space_type == PageTableType::User {
            // mmap region starts after heap region
            super::vma::RiscVAddressSpaceLayout::heap_end()
        } else {
            0
        };

        Self {
            pgd,
            vma_manager: RwLock::new(vma_manager),
            space_type,
            // Segment ranges
            start_code: AtomicUsize::new(0),
            end_code: AtomicUsize::new(0),
            start_data: AtomicUsize::new(0),
            end_data: AtomicUsize::new(0),
            // Heap management
            start_brk: AtomicUsize::new(brk_default),
            brk: AtomicUsize::new(brk_default),
            // Stack management
            start_stack: AtomicUsize::new(0),
            // Arguments and environment variables
            arg_start: AtomicUsize::new(0),
            arg_end: AtomicUsize::new(0),
            env_start: AtomicUsize::new(0),
            env_end: AtomicUsize::new(0),
            // Virtual memory statistics
            total_vm: AtomicU64::new(0),
            locked_vm: AtomicU64::new(0),
            pinned_vm: AtomicU64::new(0),
            data_vm: AtomicU64::new(0),
            exec_vm: AtomicU64::new(0),
            stack_vm: AtomicU64::new(0),
            // mmap region
            mmap_base: AtomicUsize::new(mmap_base),
            mmap_legacy_base: AtomicUsize::new(mmap_base),
            highest_vm_end: AtomicUsize::new(0),
            // Reference counting
            mm_users: AtomicI32::new(1),
            mm_count: AtomicI32::new(1),
            // Other
            flags: AtomicU64::new(0),
            owner_pid: AtomicI32::new(-1),
        }
    }

    /// Create a shared page table memory descriptor (for fork)
    pub unsafe fn new_shared(pgd: u64, space_type: PageTableType, brk: VirtAddr) -> Self {
        let mut mm = Self::new(pgd, space_type);
        mm.start_brk.store(brk.as_usize(), Ordering::Release);
        mm.brk.store(brk.as_usize(), Ordering::Release);
        mm
    }

    /// Create a memory descriptor with specified type
    ///
    /// This is an alias for `new()`, providing an interface compatible with the old `AddressSpace::new_with_type`
    pub unsafe fn new_with_type(pgd: u64, space_type: PageTableType) -> Self {
        Self::new(pgd, space_type)
    }

    /// Create kernel address space (convenience method)
    pub unsafe fn new_kernel(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::Kernel)
    }

    /// Create user address space (convenience method)
    pub unsafe fn new_user(pgd: u64) -> Self {
        Self::new(pgd, PageTableType::User)
    }

    // ==================== Basic Accessors ====================

    /// Get page table root PPN
    #[inline]
    pub fn pgd(&self) -> u64 {
        self.pgd
    }

    /// Get page table root PPN (compatible alias)
    #[inline]
    pub fn root_ppn(&self) -> u64 {
        self.pgd
    }

    /// Get address space type
    #[inline]
    pub fn space_type(&self) -> PageTableType {
        self.space_type
    }

    // ==================== Segment Range Accessors ====================

    /// Get code segment start address
    #[inline]
    pub fn start_code(&self) -> usize {
        self.start_code.load(Ordering::Acquire)
    }

    /// Set code segment start address
    #[inline]
    pub fn set_start_code(&self, addr: usize) {
        self.start_code.store(addr, Ordering::Release);
    }

    /// Get code segment end address
    #[inline]
    pub fn end_code(&self) -> usize {
        self.end_code.load(Ordering::Acquire)
    }

    /// Set code segment end address
    #[inline]
    pub fn set_end_code(&self, addr: usize) {
        self.end_code.store(addr, Ordering::Release);
    }

    /// Get data segment start address
    #[inline]
    pub fn start_data(&self) -> usize {
        self.start_data.load(Ordering::Acquire)
    }

    /// Set data segment start address
    #[inline]
    pub fn set_start_data(&self, addr: usize) {
        self.start_data.store(addr, Ordering::Release);
    }

    /// Get data segment end address
    #[inline]
    pub fn end_data(&self) -> usize {
        self.end_data.load(Ordering::Acquire)
    }

    /// Set data segment end address
    #[inline]
    pub fn set_end_data(&self, addr: usize) {
        self.end_data.store(addr, Ordering::Release);
    }

    // ==================== Heap Management ====================

    /// Get heap start address
    #[inline]
    pub fn start_brk(&self) -> usize {
        self.start_brk.load(Ordering::Acquire)
    }

    /// Set heap start address
    #[inline]
    pub fn set_start_brk(&self, addr: usize) {
        self.start_brk.store(addr, Ordering::Release);
    }

    /// Get current brk value
    #[inline]
    pub fn brk(&self) -> VirtAddr {
        VirtAddr::new(self.brk.load(Ordering::Acquire))
    }

    /// Set brk value
    #[inline]
    pub fn set_brk_val(&self, addr: usize) {
        self.brk.store(addr, Ordering::Release);
    }

    // ==================== Stack Management ====================

    /// Get stack start address
    #[inline]
    pub fn start_stack(&self) -> usize {
        self.start_stack.load(Ordering::Acquire)
    }

    /// Set stack start address
    #[inline]
    pub fn set_start_stack(&self, addr: usize) {
        self.start_stack.store(addr, Ordering::Release);
    }

    // ==================== Arguments and Environment Variables ====================

    /// Get command line arguments start address
    #[inline]
    pub fn arg_start(&self) -> usize {
        self.arg_start.load(Ordering::Acquire)
    }

    /// Set command line arguments start address
    #[inline]
    pub fn set_arg_start(&self, addr: usize) {
        self.arg_start.store(addr, Ordering::Release);
    }

    /// Get command line arguments end address
    #[inline]
    pub fn arg_end(&self) -> usize {
        self.arg_end.load(Ordering::Acquire)
    }

    /// Set command line arguments end address
    #[inline]
    pub fn set_arg_end(&self, addr: usize) {
        self.arg_end.store(addr, Ordering::Release);
    }

    /// Get environment variables start address
    #[inline]
    pub fn env_start(&self) -> usize {
        self.env_start.load(Ordering::Acquire)
    }

    /// Set environment variables start address
    #[inline]
    pub fn set_env_start(&self, addr: usize) {
        self.env_start.store(addr, Ordering::Release);
    }

    /// Get environment variables end address
    #[inline]
    pub fn env_end(&self) -> usize {
        self.env_end.load(Ordering::Acquire)
    }

    /// Set environment variables end address
    #[inline]
    pub fn set_env_end(&self, addr: usize) {
        self.env_end.store(addr, Ordering::Release);
    }

    // ==================== Virtual Memory Statistics ====================

    /// Get total virtual memory page count
    #[inline]
    pub fn total_vm(&self) -> u64 {
        self.total_vm.load(Ordering::Acquire)
    }

    /// Add to total virtual memory page count
    #[inline]
    pub fn add_total_vm(&self, pages: u64) {
        self.total_vm.fetch_add(pages, Ordering::AcqRel);
    }

    /// Subtract from total virtual memory page count
    #[inline]
    pub fn sub_total_vm(&self, pages: u64) {
        self.total_vm.fetch_sub(pages, Ordering::AcqRel);
    }

    /// Get locked memory page count
    #[inline]
    pub fn locked_vm(&self) -> u64 {
        self.locked_vm.load(Ordering::Acquire)
    }

    /// Get pinned memory page count
    #[inline]
    pub fn pinned_vm(&self) -> u64 {
        self.pinned_vm.load(Ordering::Acquire)
    }

    /// Get data segment page count
    #[inline]
    pub fn data_vm(&self) -> u64 {
        self.data_vm.load(Ordering::Acquire)
    }

    /// Get executable segment page count
    #[inline]
    pub fn exec_vm(&self) -> u64 {
        self.exec_vm.load(Ordering::Acquire)
    }

    /// Get stack page count
    #[inline]
    pub fn stack_vm(&self) -> u64 {
        self.stack_vm.load(Ordering::Acquire)
    }

    // ==================== mmap Region ====================

    /// Get mmap base address
    #[inline]
    pub fn mmap_base(&self) -> usize {
        self.mmap_base.load(Ordering::Acquire)
    }

    /// Set mmap base address
    #[inline]
    pub fn set_mmap_base(&self, addr: usize) {
        self.mmap_base.store(addr, Ordering::Release);
    }

    /// Get highest virtual memory end address
    #[inline]
    pub fn highest_vm_end(&self) -> usize {
        self.highest_vm_end.load(Ordering::Acquire)
    }

    /// Update highest virtual memory end address
    #[inline]
    pub fn update_highest_vm_end(&self, addr: usize) {
        let current = self.highest_vm_end.load(Ordering::Acquire);
        if addr > current {
            self.highest_vm_end.store(addr, Ordering::Release);
        }
    }

    // ==================== Reference Counting ====================

    /// Increment user count (mm_users)
    /// Returns the value after increment
    #[inline]
    pub fn mm_users_inc(&self) -> i32 {
        self.mm_users.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement user count (mm_users)
    /// Returns the value after decrement
    #[inline]
    pub fn mm_users_dec(&self) -> i32 {
        self.mm_users.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Get user count
    #[inline]
    pub fn mm_users(&self) -> i32 {
        self.mm_users.load(Ordering::Acquire)
    }

    /// Increment reference count (mm_count)
    #[inline]
    pub fn mm_count_inc(&self) -> i32 {
        self.mm_count.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Decrement reference count (mm_count)
    #[inline]
    pub fn mm_count_dec(&self) -> i32 {
        self.mm_count.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// Get reference count
    #[inline]
    pub fn mm_count(&self) -> i32 {
        self.mm_count.load(Ordering::Acquire)
    }

    // ==================== Flags ====================

    /// Get flags
    #[inline]
    pub fn flags(&self) -> u64 {
        self.flags.load(Ordering::Acquire)
    }

    /// Set flags
    #[inline]
    pub fn set_flags(&self, flags: u64) {
        self.flags.store(flags, Ordering::Release);
    }

    /// Check if specified flag is set
    #[inline]
    pub fn has_flag(&self, flag: u64) -> bool {
        self.flags.load(Ordering::Acquire) & flag != 0
    }

    // ==================== Owner ====================

    /// Get owner PID
    #[inline]
    pub fn owner_pid(&self) -> i32 {
        self.owner_pid.load(Ordering::Acquire)
    }

    /// Set owner PID
    #[inline]
    pub fn set_owner_pid(&self, pid: i32) {
        self.owner_pid.store(pid, Ordering::Release);
    }

    // ==================== VMA Operations ====================

    /// Acquire VMA read lock
    #[inline]
    pub fn vma_read(&self) -> spin::RwLockReadGuard<'_, VmaManager> {
        self.vma_manager.read()
    }

    /// Acquire VMA write lock
    #[inline]
    pub fn vma_write(&self) -> spin::RwLockWriteGuard<'_, VmaManager> {
        self.vma_manager.write()
    }

    /// Find VMA
    pub fn find_vma(&self, addr: VirtAddr) -> Option<Vma> {
        let vma_mgr = self.vma_read();
        vma_mgr.find(addr).cloned()
    }

    /// Add VMA and update statistics
    pub fn add_vma(&self, vma: Vma) -> Result<(), MapError> {
        let pages = vma.page_count() as u64;

        let mut vma_mgr = self.vma_write();
        vma_mgr.add(vma).map_err(|_| MapError::Invalid)?;

        // Update statistics
        self.add_total_vm(pages);

        // Update highest end address
        self.update_highest_vm_end(vma.end().as_usize());

        Ok(())
    }

    /// Remove VMA and update statistics
    pub fn remove_vma(&self, start: VirtAddr) -> Result<(), MapError> {
        let mut vma_mgr = self.vma_write();
        let vma = vma_mgr.get(start).cloned();

        if let Some(vma) = vma {
            let pages = vma.page_count() as u64;
            vma_mgr.remove(start).map_err(|_| MapError::NotMapped)?;
            self.sub_total_vm(pages);
            Ok(())
        } else {
            Err(MapError::NotMapped)
        }
    }

    // ==================== ELF Loading Helpers ====================

    /// Set code/data segment ranges based on ELF segment type
    ///
    /// Called during ELF loading to set start_code, end_code, start_data, end_data
    pub fn setup_segment_layout(
        &self,
        code_start: usize,
        code_end: usize,
        data_start: usize,
        data_end: usize,
        entry_point: usize,
    ) {
        self.set_start_code(code_start);
        self.set_end_code(code_end);
        self.set_start_data(data_start);
        self.set_end_data(data_end);

        // Update executable segment page count
        let code_pages = ((code_end - code_start + 4095) / 4096) as u64;
        self.exec_vm.store(code_pages, Ordering::Release);

        // Update data segment page count
        let data_pages = ((data_end - data_start + 4095) / 4096) as u64;
        self.data_vm.store(data_pages, Ordering::Release);

        // Update total page count
        self.add_total_vm(code_pages + data_pages);
    }

    /// Set up stack layout
    ///
    /// Set stack address and statistics
    pub fn setup_stack(&self, stack_top: usize, stack_size: usize) {
        self.set_start_stack(stack_top);

        let stack_pages = ((stack_size + 4095) / 4096) as u64;
        self.stack_vm.store(stack_pages, Ordering::Release);
        self.add_total_vm(stack_pages);

        // Update highest end address
        self.update_highest_vm_end(stack_top);
    }

    /// Set up command line arguments layout
    pub fn setup_argv(&self, arg_start: usize, arg_end: usize) {
        self.set_arg_start(arg_start);
        self.set_arg_end(arg_end);
    }

    /// Set up environment variables layout
    pub fn setup_envp(&self, env_start: usize, env_end: usize) {
        self.set_env_start(env_start);
        self.set_env_end(env_end);
    }

    /// Free all physical pages in this address space
    ///
    /// This walks the page table and frees all user physical pages,
    /// including the page table pages themselves.
    ///
    /// For COW pages, decrements reference count instead of freeing.
    pub fn free_page_tables(&self) {
        use crate::arch::riscv64::mm::{free_user_phys_page, ppn_to_virt_ptr, PAGE_SIZE};
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};

        let root_ppn = self.pgd;
        if root_ppn == 0 {
            return;
        }

        crate::println!("free_page_tables: START, root_ppn={:#x}", root_ppn);  // Keep this one for debugging

        let mut user_pages_freed = 0usize;
        let mut shared_pages_decref = 0usize;
        let mut kernel_pages_skipped = 0usize;

        // Helper function to free a data page with COW handling
        // IMPORTANT: Only free USER pages! Kernel pages are shared, not owned by this process.
        let mut free_data_page = |ppn: u64, vaddr: u64, is_user: bool| {
            // Skip kernel pages - they are shared between all processes
            if !is_user {
                kernel_pages_skipped += 1;
                return;
            }

            let page = pfn_to_page_mut(ppn as usize);
            if !page.is_null() {
                unsafe {
                    let page_ref = &*page;
                    // Check if this page has a refcount (COW or shared)
                    // Pages with refcount > 0 are shared and should not be freed directly
                    let refcount = page_ref.refcount();

                    // Debug: track vaddr=0x1f000 only
                    if vaddr == 0x1f000 {
                        crate::println!("free: vaddr={:#x} ppn={:#x} refcount={}", vaddr, ppn, refcount);
                    }

                    if refcount > 0 {
                        // Shared page (COW or read-only shared), decrement refcount
                        shared_pages_decref += 1;
                        let new_count = page_ref.put_page();
                        // Debug: for vaddr=0x1f000 only
                        if vaddr == 0x1f000 {
                            crate::println!("free: vaddr={:#x} decref {}->{}", vaddr, refcount, new_count);
                        }
                        if new_count <= 0 {
                            // Refcount reached 0, safe to free
                            page_ref.clear_flag(PageFlag::Cow);
                            free_user_phys_page(ppn << 12);
                        }
                    } else {
                        // Not a shared page, free directly - THIS IS SUSPICIOUS for COW pages!
                        crate::println!("WARNING: freeing user page with refcount=0! vaddr={:#x}, ppn={:#x}", vaddr, ppn);
                        user_pages_freed += 1;
                        free_user_phys_page(ppn << 12);
                    }
                }
            }
            // No page descriptor means it's a page table page, skip silently
        };

        // Walk the page table and free all pages
        // CRITICAL: Use ppn_to_virt_ptr to access page tables!
        // User page tables are in user physical memory (0x84000000-0x88000000)
        // which must be accessed via high kernel virtual addresses.
        unsafe {
            let root_table = ppn_to_virt_ptr::<u64>(root_ppn);

            // PTE U flag (bit 4) - indicates user-accessible page
            const PTE_U: u64 = 0x10;

            // Walk L2 entries (VPN2)
            for vpn2 in 0..512 {
                let pte2 = core::ptr::read(root_table.add(vpn2));
                if pte2 & 0x1 == 0 {  // !V
                    continue;
                }

                // Check if leaf (2MB huge page)
                let is_leaf = (pte2 & 0xA) != 0;  // R | X
                if is_leaf {
                    // 2MB huge page - free the data page
                    let ppn = pte2 >> 10;
                    let vaddr = (vpn2 as u64) << 30;
                    let is_user = (pte2 & PTE_U) != 0;
                    free_data_page(ppn, vaddr, is_user);
                    continue;
                }

                // Non-leaf: walk L1 entries
                let ppn1 = pte2 >> 10;
                let l1_table = ppn_to_virt_ptr::<u64>(ppn1);

                for vpn1 in 0..512 {
                    let pte1 = core::ptr::read(l1_table.add(vpn1));
                    if pte1 & 0x1 == 0 {  // !V
                        continue;
                    }

                    // Check if leaf
                    let is_leaf = (pte1 & 0xA) != 0;  // R | X
                    if is_leaf {
                        // Leaf at L1 - could be 2MB page
                        let ppn = pte1 >> 10;
                        let vaddr = (vpn2 as u64) << 30 | (vpn1 as u64) << 21;
                        let is_user = (pte1 & PTE_U) != 0;
                        free_data_page(ppn, vaddr, is_user);
                        continue;
                    }

                    // Non-leaf: walk L0 entries
                    let ppn0 = pte1 >> 10;
                    let l0_table = ppn_to_virt_ptr::<u64>(ppn0);

                    // Debug: print L0 table address and L1 PTE for vaddr base < 0x200000
                    let base_vaddr = (vpn2 as u64) << 30 | (vpn1 as u64) << 21;
                    if base_vaddr < 0x200000 {
                        crate::println!("free: L1 PTE {:#x} -> L0 table at {:#x} (ppn0={:#x})",                            pte1, l0_table as u64, ppn0);
                        // Debug: dump first few L0 PTEs
                        for i in 0..5 {
                            let debug_pte = core::ptr::read(l0_table.add(i));
                            crate::println!("free: L0[{}] = {:#x}", i, debug_pte);
                        }
                    }

                    for vpn0 in 0..512 {
                        let pte0 = core::ptr::read(l0_table.add(vpn0));
                        if pte0 & 0x1 == 0 {  // !V
                            continue;
                        }

                        // User page - free with COW handling
                        let ppn = pte0 >> 10;
                        let vaddr = (vpn2 as u64) << 30 | (vpn1 as u64) << 21 | (vpn0 as u64) << 12;
                        let is_user = (pte0 & PTE_U) != 0;

                        // Debug: print L0 PTE for vaddr=0x1f000
                        if vaddr == 0x1f000 {
                            crate::println!("free: vaddr={:#x} pte0={:#x} ppn={:#x} is_user={}",
                                vaddr, pte0, ppn, is_user);
                        }
                        free_data_page(ppn, vaddr, is_user);
                    }

                    // Free L0 page table page (convert PPN to physical address)
                    free_user_phys_page(ppn0 << 12);
                }

                // Free L1 page table page (convert PPN to physical address)
                free_user_phys_page(ppn1 << 12);
            }

            // Free root page table page (convert PPN to physical address)
            free_user_phys_page(root_ppn << 12);
        }

        crate::println!("free_page_tables: DONE, user_freed={}, shared_decref={}, kernel_skipped={}",
            user_pages_freed, shared_pages_decref, kernel_pages_skipped);
    }

    /// Free only user data pages, keeping page table structure intact
    /// This is called before execve allocates new memory to prevent
    /// page table pages from being reused and corrupted
    pub fn free_user_pages_only(&self) {
        use crate::arch::riscv64::mm::{ppn_to_virt_ptr, PAGE_SIZE};
        use crate::mm::page_desc::{pfn_to_page_mut, PageFlag};

        let root_ppn = self.pgd;
        if root_ppn == 0 {
            crate::println!("free_user_pages_only: root_ppn is 0, skipping");
            return;
        }

        crate::println!("free_user_pages_only: START, root_ppn={:#x}", root_ppn);

        // Debug: dump L0 table for vaddr 0x1f000 BEFORE freeing
        // Also verify L1 PTE content
        // CRITICAL: Use ppn_to_virt_ptr to access page tables!
        unsafe {
            let root_table = ppn_to_virt_ptr::<u64>(root_ppn);

            // Get L2 PTE for VPN2=0
            let pte2 = core::ptr::read(root_table);
            crate::println!("free: L2 PTE[0]={:#x} (ppn1={:#x})", pte2, pte2 >> 10);
            if pte2 & 0x1 != 0 {
                let ppn1 = pte2 >> 10;
                let l1_table = ppn_to_virt_ptr::<u64>(ppn1);

                // Get L1 PTE for VPN1=0
                let pte1 = core::ptr::read(l1_table);
                crate::println!("free: L1 PTE[0]={:#x} (ppn0={:#x}) at L1 table {:#x}",
                    pte1, pte1 >> 10, l1_table as u64);
                if pte1 & 0x1 != 0 {
                    let ppn0 = pte1 >> 10;
                    let l0_table = ppn_to_virt_ptr::<u64>(ppn0);

                    // Read L0 PTE for vaddr 0x1f000 (vpn0=0x1f)
                    let l0_pte_1f = core::ptr::read(l0_table.add(0x1f));
                    crate::println!("free: L0 table at {:#x}, entry[0x1f]={:#x} (ppn={:#x})",
                        l0_table as u64, l0_pte_1f, l0_pte_1f >> 10);

                    // CRITICAL: Dump ALL non-zero L0 entries to understand table content
                    let mut non_zero_count = 0;
                    for i in 0..512 {
                        let pte = core::ptr::read(l0_table.add(i));
                        if pte != 0 {
                            non_zero_count += 1;
                            if non_zero_count <= 20 {  // Only print first 20
                                crate::println!("free: L0[{:#x}]={:#x} (ppn={:#x})", i, pte, pte >> 10);
                            }
                        }
                    }
                    crate::println!("free: L0 table has {} non-zero entries", non_zero_count);
                }
            }
        }

        let mut user_pages_freed = 0usize;
        let mut shared_pages_decref = 0usize;

        // Helper function to free a data page with COW handling
        let mut free_data_page = |ppn: u64, vaddr: u64| {
            let page = pfn_to_page_mut(ppn as usize);
            if !page.is_null() {
                unsafe {
                    let page_ref = &*page;
                    let refcount = page_ref.refcount();

                    if refcount > 0 {
                        // Shared page (COW or read-only shared), decrement refcount
                        shared_pages_decref += 1;
                        let new_count = page_ref.put_page();
                        if new_count <= 0 {
                            // Refcount reached 0, safe to free
                            page_ref.clear_flag(PageFlag::Cow);
                            crate::arch::riscv64::mm::free_user_phys_page(ppn << 12);
                        }
                    } else {
                        // Not a shared page, free directly
                        user_pages_freed += 1;
                        crate::arch::riscv64::mm::free_user_phys_page(ppn << 12);
                    }
                }
            }
        };

        // Walk the page table and free only user data pages
        // CRITICAL: Use ppn_to_virt_ptr to access page tables!
        unsafe {
            let root_table = ppn_to_virt_ptr::<u64>(root_ppn);

            // PTE U flag (bit 4) - indicates user-accessible page
            const PTE_U: u64 = 0x10;

            // Walk L2 entries (VPN2)
            for vpn2 in 0..512 {
                let pte2 = core::ptr::read(root_table.add(vpn2));
                if pte2 & 0x1 == 0 {  // !V
                    continue;
                }

                // Check if leaf (2MB huge page)
                let is_leaf = (pte2 & 0xA) != 0;  // R | X
                if is_leaf {
                    // Only free if it's a user page
                    if (pte2 & PTE_U) != 0 {
                        let ppn = pte2 >> 10;
                        free_data_page(ppn, (vpn2 as u64) << 30);
                    }
                    continue;
                }

                // Non-leaf: walk L1 entries
                let ppn1 = pte2 >> 10;
                let l1_table = ppn_to_virt_ptr::<u64>(ppn1);

                for vpn1 in 0..512 {
                    let pte1 = core::ptr::read(l1_table.add(vpn1));
                    if pte1 & 0x1 == 0 {  // !V
                        continue;
                    }

                    // Check if leaf
                    let is_leaf = (pte1 & 0xA) != 0;  // R | X
                    if is_leaf {
                        // Only free if it's a user page
                        if (pte1 & PTE_U) != 0 {
                            let ppn = pte1 >> 10;
                            let vaddr = (vpn2 as u64) << 30 | (vpn1 as u64) << 21;
                            free_data_page(ppn, vaddr);
                        }
                        continue;
                    }

                    // Non-leaf: walk L0 entries
                    let ppn0 = pte1 >> 10;
                    let l0_table = ppn_to_virt_ptr::<u64>(ppn0);

                    for vpn0 in 0..512 {
                        let pte0 = core::ptr::read(l0_table.add(vpn0));
                        if pte0 & 0x1 == 0 {  // !V
                            continue;
                        }

                        // Only free user pages
                        if (pte0 & PTE_U) != 0 {
                            let ppn = pte0 >> 10;
                            let vaddr = (vpn2 as u64) << 30 | (vpn1 as u64) << 21 | (vpn0 as u64) << 12;
                            free_data_page(ppn, vaddr);
                        }
                    }
                    // Do NOT free L0 page table - keep it intact
                }
                // Do NOT free L1 page table - keep it intact
            }
            // Do NOT free root page table - keep it intact
        }
    }
}

impl Drop for MmStruct {
    fn drop(&mut self) {
        // Only free pages for user address spaces
        if self.space_type == PageTableType::User {
            crate::println!("MmStruct::drop: freeing address space, pgd={:#x}", self.pgd);
            self.free_page_tables();
        }
    }
}

unsafe impl Send for MmStruct {}
unsafe impl Sync for MmStruct {}

// ============================================================================
// Helper type aliases (backward compatibility)
// ============================================================================

/// Address space type alias (backward compatibility)
///
/// Recommend using MmStruct directly
pub type AddressSpace = MmStruct;
