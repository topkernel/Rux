//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The mm subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem lives here, together with the constants those definitions
//! name (array lengths, and the constants those derive from); every
//! `impl` block stays with the functions that use it — an impl is code,
//! a definition is data layout. The exceptions are definitions local to
//! code: a type declared inside a function or inside a private inline
//! module.
//!
//! This file holds no `unsafe`. A field that was private to its old file
//! is `pub(super)` here because the code that touches it stayed in that
//! file: the visibility boundary is the mm directory, the smallest one
//! Rust offers for a definition that lives in a sibling module.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU32, AtomicU64, AtomicUsize};

use crate::sync::rwlock::RwSpinlock;
use crate::sync::rwsem::Rwsem;
use crate::sync::spinlock::{Spinlock, SpinlockGuard, SpinlockIrqGuard};

use super::PAGE_SIZE;

// ---------------------------------------------------------------------------
// kernel-heap buddy allocator: BuddyAllocator and its metadata, the layout
// constants its definitions name, the stats snapshot, the meta-audit types
// and the combined GlobalAlloc front
// ---------------------------------------------------------------------------

pub(super) const BUDDY_MAX_ORDER: usize = crate::config::BUDDY_MAX_ORDER;

pub(super) const HEAP_SIZE: usize = crate::config::KERNEL_HEAP_SIZE;

/// Maximum number of pages (for metadata array size)
pub(super) const BUDDY_MAX_PAGES: usize = HEAP_SIZE / PAGE_SIZE;

/// Words in the per-page free bitmap (INV-HEAP-3).
pub(super) const FREE_BITMAP_WORDS: usize = (BUDDY_MAX_PAGES + 63) / 64;

/// Block metadata (stored separately, not mixed with user data)
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct BlockMeta {
    /// Block size order (2^order * PAGE_SIZE)
    pub(super) order: u8,
    /// Whether the block is free
    pub(super) free: u8,
    /// Previous index (index in metadata array, u32::MAX means null)
    pub(super) prev: u32,
    /// Next index (index in metadata array, u32::MAX means null)
    pub(super) next: u32,
}

/// Metadata array wrapper (uses UnsafeCell for interior mutability)
pub(super) struct MetaArray {
    pub(super) data: UnsafeCell<[BlockMeta; BUDDY_MAX_PAGES]>,
}

pub struct BuddyAllocator {
    /// Magic number (for corruption detection)
    pub(super) magic: AtomicUsize,
    /// Heap start address (user data area)
    pub(super) heap_start: AtomicUsize,
    /// Heap end address
    pub(super) heap_end: AtomicUsize,
    /// Free block lists (one list per order, stores page indices)
    pub(super) free_lists: [AtomicUsize; BUDDY_MAX_ORDER + 1],
    /// Whether initialized
    pub(super) initialized: AtomicUsize,
    /// Metadata area (stores metadata for each page)
    pub(super) meta: MetaArray,
    /// Per-page free bitmap (INV-HEAP-3): bit (page_idx % 64) of word
    /// (page_idx / 64). Maintained by the same locked paths that used to
    /// fill the per-page `BlockMeta.free` bytes; word-strided so an
    /// order-scale fill costs 2^order/64 word RMWs instead of 2^order
    /// per-page `get_mut` iterations under the irqsave lock.
    pub(super) free_bits: [AtomicU64; FREE_BITMAP_WORDS],
    /// Lock for buddy operations
    pub(super) lock: Spinlock<()>,
}

/// Buddy allocator statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct BuddyStats {
    /// Heap start address
    pub heap_start: usize,
    /// Heap end address
    pub heap_end: usize,
    /// Heap total size (bytes)
    pub heap_size: usize,
    /// Used size (bytes)
    pub used_bytes: usize,
    /// Free size (bytes)
    pub free_bytes: usize,
    /// Number of free blocks per order
    pub free_blocks: [usize; BUDDY_MAX_ORDER + 1],
    /// Total allocation count
    pub alloc_count: usize,
    /// Total free count
    pub free_count: usize,
}

/// One shadow-audit mismatch. `kind` classifies it; `idx`/`prev`/`next`
/// carry the offending block's leader meta, `word`/`bits`/`derived` the
/// offending free_bits word and the audit-derived shadow value for the
/// bitmap kinds.
#[derive(Clone, Copy, PartialEq)]
pub struct MetaFinding {
    pub kind: MetaFindKind,
    pub order: usize,
    pub idx: usize,
    pub prev: u32,
    pub next: u32,
    pub word: usize,
    pub bits: u64,
    pub derived: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MetaFindKind {
    /// A list head or next-link escaped [0, MAX_PAGES), or a block's
    /// span ran past the end of the page array.
    LinkOob,
    /// Listed block's leader order byte disagrees with the list order.
    OrderMismatch,
    /// Listed block's leader free byte is not 1.
    FreeByteClear,
    /// Listed block leader is not 2^order-aligned.
    Unaligned,
    /// prev link does not mirror the walk position.
    PrevBroken,
    /// free_bits has set bits where the lists say nothing is free.
    BitmapSetUnlisted,
    /// A listed free block's span has clear bits in free_bits.
    BitmapClearListed,
    /// The same page is reachable from two list positions.
    DoubleListed,
    /// Walk exceeded the page-count budget (list cycle / mega-list).
    HopOverflow,
}

/// MetaArray-style interior-mutable scratch (see MetaArray's SAFETY
/// comments): only touched by `meta_audit_inner`, which every caller
/// reaches while holding the META_AUDIT_BUSY claim.
pub(super) struct AuditScratch {
    pub(super) data: UnsafeCell<[u64; FREE_BITMAP_WORDS]>,
}

/// Outcome of one shadow scan: what pass 1 (lock-free) saw and what the
/// under-lock confirmation made of it. `confirmed > 0` is corruption.
#[derive(Clone, Copy)]
pub struct MetaScanOutcome {
    pub lockfree_findings: usize,
    pub confirmed_findings: usize,
}

/// Combined allocator - prioritizes Slab for small objects, falls back to Buddy for large objects
///
/// This design can reduce memory fragmentation and improve small object allocation efficiency
pub struct CombinedAllocator;

// ---------------------------------------------------------------------------
// the legacy alias of BuddyAllocator
// ---------------------------------------------------------------------------

#[deprecated(note = "Use BuddyAllocator instead")]
pub type BumpAllocator = BuddyAllocator;

// ---------------------------------------------------------------------------
// the kernel memory layout and the clamped static carve
// ---------------------------------------------------------------------------

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

/// The clamped static carve — heap and slab windows — that
/// `layout::effective_static_regions` hands back on a machine smaller
/// than the config assumes (both windows come back empty when even the
/// minimum does not fit).
pub struct EffectiveStaticRegions {
    pub heap_start: usize,
    pub heap_size: usize,
    pub slab_start: usize,
    pub slab_size: usize,
}

// ---------------------------------------------------------------------------
// page and address newtypes
// ---------------------------------------------------------------------------

pub type PhysFrameNr = usize;

pub type VirtPageNr = usize;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PhysFrame {
    pub number: PhysFrameNr,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct VirtPage {
    pub number: VirtPageNr,
}

// ---------------------------------------------------------------------------
// the page descriptor: Page, its flag word and bits, its type, the diagnostic
// record and the stats snapshot
// ---------------------------------------------------------------------------

/// Page flags
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    /// Page is locked, not accessible
    Locked = 1 << 0,
    /// Page is being written back
    Writeback = 1 << 1,
    /// Page has been accessed (for LRU)
    Referenced = 1 << 2,
    /// Page data is valid (read from disk)
    UpToDate = 1 << 3,
    /// Page has been modified (needs writeback)
    Dirty = 1 << 4,
    /// Page is in LRU list
    Lru = 1 << 5,
    /// Head page of compound page
    Head = 1 << 6,
    /// Page has waiters
    Waiters = 1 << 7,
    /// Page is in active LRU list
    Active = 1 << 8,
    /// Reserved page (kernel use, not swappable)
    Reserved = 1 << 9,
    /// Page has private data (stored in private field)
    Private = 1 << 10,
    /// Page will be reclaimed
    Reclaim = 1 << 11,
    /// Page is backed by swap space
    SwapBacked = 1 << 12,
    /// Page is unevictable
    Unevictable = 1 << 13,
    /// Copy-on-write page (Rux extension)
    Cow = 1 << 14,
    /// Anonymous page (Rux extension)
    Anonymous = 1 << 15,
    /// Page is currently a free-list LEADER in its zone (R15-6: the
    /// authoritative linked-state bit — next_free is NOT reliable, it
    /// survives both power-on-0 and historical-leader residue).
    OnFreelist = 1 << 16,
    /// The page has more distinct (mm, vaddr) mappings than the fixed rmap
    /// slot array can record; reverse mapping falls back to slot 0 plus a
    /// full task scan (review 4.10).
    RmapOverflow = 1 << 17,
    /// Page is in the middle of a compaction migration (review 4.16): its
    /// PTEs hold migration-entry markers and the fault path must wait for
    /// the migration to finish instead of installing a zero page.
    Migrating = 1 << 21,
    /// Page sits in a per-CPU pageset (pcp) cache: refcount 0, NOT on any
    /// buddy freelist. Together with OnFreelist this keeps every order-0
    /// frame in one of three mutually exclusive states — buddy freelist
    /// (OnFreelist), pcp cache (OnPcp), or in use (neither). The pcp
    /// free path uses this bit as its double-free tripwire, exactly the
    /// role OnFreelist plays for the buddy (R15-3).
    OnPcp = 1 << 22,
}

/// Page flags collection
#[derive(Debug, Default)]
pub struct PageFlags(pub(super) AtomicU32);

/// Page type
///
/// Used for special page identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageType {
    /// Normal page
    Normal = 0,
    /// Buddy system free page
    Buddy = 1,
    /// Slab allocator page
    Slab = 2,
    /// Page cache page
    PageCache = 3,
    /// Anonymous page
    Anonymous = 4,
}

/// Page descriptor
///
/// Each physical page frame corresponds to a Page structure, used to track page usage.
///
/// Memory layout (128 bytes, 64-byte aligned):
/// - flags: 4 bytes (atomic flags)
/// - _mapcount: 4 bytes (map count, -1 means unmapped)
/// - _refcount: 4 bytes (reference count)
/// - private: 8 bytes (private data)
/// - mapping: 8 bytes (page-cache eviction key for cache frames)
/// - index: 8 bytes (rmap slot 0: virtual page number)
/// - rmap_alt: 48 bytes (rmap slots 1..3: (mm ptr, vpn) pairs)
/// - _type: 4 bytes (page type)
/// - next_free: 8 bytes (free list pointer, for allocator)
/// - lru_next: 8 bytes (LRU next PFN)
/// - lru_prev: 8 bytes (LRU prev PFN — doubly-linked LRU, O(1) unlink)
///
#[repr(C, align(64))]
pub struct Page {
    /// Atomic flags
    pub(super) flags: PageFlags,

    /// Map count: how many page table entries directly reference this page
    /// -1 means unmapped, 0 means mapped once, etc.
    /// Note: initial value is -1 (PAGE_MAPCOUNT_BIAS)
    pub(super) _mapcount: AtomicI32,

    /// Reference count: number of references to this page
    /// 0 means free, > 0 means in use
    ///
    /// NOTE (compound convention, review 4.4): unlike Linux, Rux keeps a
    /// per-member refcount on every page of a high-order allocation — the
    /// arch teardown path put_page()s each member individually via its
    /// PTE.  Do not free an order>0 block while members are still mapped.
    pub(super) _refcount: AtomicI32,

    /// Private data
    /// - Buddy system: stores order
    /// - Slab: stores slab management structure
    /// - File system: stores buffer_head
    pub(super) private: AtomicUsize,

    /// Page-cache eviction key for cache-owned frames (the combined
    /// (fs_id, ino) cache_key from fs::page_cache; set by insert/
    /// insert_batch, cleared by release_page_frame). NOT an rmap slot:
    /// page_record_mapping deliberately never writes this field — the
    /// mm-side release path (release_cache_mapping_pin) and eviction
    /// reverse-resolve the cache entry through it. Zero for non-cache
    /// frames. (The legacy page_add_file_rmap would clobber it, but has
    /// no live callers — fault paths set their fields directly.)
    pub(super) mapping: AtomicUsize,

    /// rmap slot 0: virtual page number in the owning mm. (rmap's slot-0
    /// record writes ONLY this field; see rmap.rs page_record_mapping.
    /// The page cache uses the `mapping` field above as its eviction key —
    /// cache-owned frames therefore take no rmap slot-0 record, see the
    /// PCACHE arms in the fault paths.)
    pub(super) index: AtomicUsize,

    /// rmap slots 1..3: (mm pointer, virtual page number) pairs for
    /// additional mappings (MAP_FIXED / shared anon).  `rmap_alt[2*k]` is
    /// the mm pointer, `rmap_alt[2*k+1]` the virtual page number.  All
    /// zero = unused slot.  When a 4th distinct mapping arrives the slots
    /// are flushed and the `RmapOverflow` flag is set — reverse-map
    /// lookups then fall back to the slot-0 + full-task-scan path.
    pub(super) rmap_alt: [AtomicUsize; 6],

    /// Page type (for special pages)
    pub(super) _type: AtomicU32,

    /// Free list pointer (for allocator internal use)
    pub(super) next_free: AtomicUsize,

    /// LRU next pointer (PFN of next page in LRU list, 0 = end of list)
    /// Used for doubly-linked LRU lists; separate from mapping/index (rmap).
    pub(super) lru_next: AtomicUsize,

    /// LRU prev pointer (PFN of previous page in LRU list, 0 = head/end)
    /// Companion to lru_next — makes unlink O(1) instead of a full list
    /// walk per reclaim (review 4.11).
    pub(super) lru_prev: AtomicUsize,
}

/// Lifecycle event ring for STAMPED frames (gnome-sigill hunt): one entry
/// per interesting event on a stamped frame, so the report printed when a
/// foreign free fires can replay the frame's whole history — who filled
/// it, which forks took references, and every put that dropped one.
/// Site chars: 'F' fault-fill, 'K' fork get, 'T' teardown put,
/// 'U' unmap put, 'C' cow put, 'R' refused(leak), 'S' stale-stamp freed.
pub struct TextFrameEvent {
    pub pfn: AtomicU64,
    pub pid: AtomicU32,
    pub site: AtomicU64,
    pub refc: AtomicI32,
}

/// Page descriptor statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PageDescStats {
    /// Total page count
    pub total_pages: usize,
    /// Free page count (refcount == 0)
    pub free_pages: usize,
    /// In-use page count (refcount > 0)
    pub used_pages: usize,
    /// Reserved page count (Reserved flag)
    pub reserved_pages: usize,
    /// Mapped page count (mapcount > PAGE_MAPCOUNT_BIAS)
    pub mapped_pages: usize,
    /// Dirty page count (Dirty flag)
    pub dirty_pages: usize,
    /// COW page count (Cow flag)
    pub cow_pages: usize,
    /// Anonymous page count (Anonymous flag)
    pub anonymous_pages: usize,
}

// ---------------------------------------------------------------------------
// the VMA record, its flags, type, origin and error, and the tree manager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags(pub(super) u32);

#[derive(Clone, Copy)]
pub struct Vma {
    /// Start virtual address (inclusive)
    pub(super) start: VirtAddr,

    /// End virtual address (exclusive)
    pub(super) end: VirtAddr,

    /// Access permissions and attributes
    pub(super) flags: VmaFlags,

    /// VMA offset (for file mapping)
    pub(super) offset: usize,

    /// VMA type
    pub(super) vma_type: VmaType,

    /// File descriptor for file-backed mappings (-1 for anonymous)
    pub(super) file_fd: i32,

    /// File size for file-backed mappings (0 for anonymous)
    pub(super) file_size: u64,

    /// Origin stamp — which kernel path created this record (see
    /// [`VmaOrigin`). Forensic-only; fits existing struct padding.
    pub(super) origin: VmaOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    /// Anonymous mapping (heap, stack, private data)
    Anonymous,
    /// File mapping
    FileBacked,
    /// Device mapping (MMIO)
    Device,
    /// Shared memory
    SharedMemory,
}

/// Origin stamp: which kernel path CREATED this Vma record.
///
/// Forensic attribute for fault-path VMA dumps (leg-31B render_service
/// adjudication). A fault into an unexpected permission shape — e.g. a
/// PROT_NONE piece sitting on top of a thread's RW stack — has exactly two
/// possible owners: the image itself established the shape (its own
/// mprotect guard, its own PROT_NONE mmap reserve) or a kernel-side
/// split/copy produced it (fork snapshot, mprotect surgery bug). The
/// origin names the establishing path in one line at the dump site, with
/// no new print traffic — it rides the existing fault-only dump lines.
///
/// Semantics: the stamp records the path that created THIS Vma node —
/// after mprotect splits a mapping, the covered piece carries `Mprotect`
/// while head/tail pieces inherit the original stamp (splits never
/// change permissions of the pieces they preserve). On fork the child's
/// copies inherit the parent's stamp plus the `FORKED` modifier bit, so
/// "mprotect+FORK" reads as "an mprotect established the shape, fork
/// carried it into this process" — the stamp survives inheritance
/// without erasing its creator. VMA merges keep the left survivor's
/// stamp (a merge only extends `end`), so a stamp can under-report the
/// newest contributor of a merged range — acceptable for adjudication:
/// permission *changes* always split first, so the changed piece's stamp
/// is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaOrigin(pub(super) u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaError {
    /// VMA overlap
    Overlap,
    /// No space (reserved for compatibility)
    NoSpace,
    /// Not found
    NotFound,
    /// Invalid parameter
    Invalid,
}

/// VMA Manager
///
/// Uses BTreeMap to store VMAs, sorted by start address
/// - O(log n) lookup, insert, delete
/// - Dynamic expansion, no limit on count
pub struct VmaManager {
    /// VMA map (sorted by start address)
    pub(super) vmas: BTreeMap<VirtAddr, Vma>,

    /// Cached maximum end address (for fast overlap detection)
    pub(super) max_end: VirtAddr,
}

/// Address space layout driven by the arch interface's `user_addr`
/// constants (riscv64: Sv39 256GB user space; x86_64: 128TB 4-level user
/// space — each backend publishes the same constant set).
///
/// User space layout:
/// - 0x0 ~ 0x1000: Null page (unmapped, null pointer guard)
/// - 0x1000+: ELF code/data segments
/// - brk area: follows ELF segments, grows up to TASK_SIZE/3
/// - mmap area: TASK_SIZE/3 ~ TASK_SIZE (top-down allocation)
/// - Stack: TASK_SIZE - stack_size, grows down
pub struct KernelAddressSpaceLayout;

/// Historical name (Sv39 era) — same layout source.
pub type RiscVAddressSpaceLayout = KernelAddressSpaceLayout;

// ---------------------------------------------------------------------------
// the mapping API's error, permission and table-type enums
// ---------------------------------------------------------------------------

// Map error types (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// Already mapped
    AlreadyMapped,
    /// Not mapped
    NotMapped,
    /// Out of memory
    OutOfMemory,
    /// Invalid parameter
    Invalid,
}

// Page permissions (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Perm {
    /// No access
    None = 0,
    /// Read only
    Read = 1,
    /// Read and write
    ReadWrite = 2,
    /// Read, write and execute
    ReadWriteExec = 3,
    /// Read and execute (R=1, W=0, X=1)
    ReadExec = 4,
    /// Execute only (R=0, W=0, X=1) — valid on Sv39
    Exec = 5,
}

// Page table types (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageTableType {
    /// Kernel page table
    Kernel = 0,
    /// User page table
    User = 1,
}

// ---------------------------------------------------------------------------
// the slab allocator's structures
// ---------------------------------------------------------------------------

/// Object size array
pub(super) const OBJECT_SIZES: [usize; 10] = [
    8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096
];

/// Number of slab caches - derived from object size array
pub(super) const NUM_CACHES: usize = OBJECT_SIZES.len();

/// Slab state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SlabState {
    /// Completely free
    Free,
    /// Partially used
    Partial,
    /// Completely used
    Full,
}

/// Slab header (stored at the beginning of each slab page)
///
/// `repr(align(8))` + explicit padding pin the header at exactly 16 bytes:
/// objects start at `header_size + i * object_size`, and with a 16-byte
/// header and 8-multiple object sizes every handed-out pointer is at least
/// 8-aligned. The unpadded 14-byte layout left odd-index 8-byte objects on
/// a 2-mod-8 address — fine for the old u16 free-list writes, fatal once
/// arbitrary GlobalAlloc layouts (Box<u64>, ArcInner<...>) flow through.
#[repr(C, align(8))]
pub(super) struct SlabHeader {
    /// Cache index this slab belongs to
    pub(crate) cache_idx: u8,
    /// Object size
    pub(super) object_size: u16,
    /// Total object count
    pub(super) total_objects: u16,
    /// Free object count
    pub(super) free_objects: u16,
    /// First free object index
    pub(super) free_index: u16,
    /// Next slab page index (in slab_pages array)
    pub(super) next: u16,
    /// Previous slab page index (in slab_pages array)
    pub(super) prev: u16,
    /// Pad to 16 bytes (see the struct comment)
    pub(super) _pad: u16,
}

/// Slab cache
pub struct SlabCache {
    /// Object size
    pub(super) object_size: usize,
    /// Objects per slab
    pub(super) objects_per_slab: usize,
    /// Free slab list head (page index)
    pub(super) free_list: u16,
    /// Partially used slab list head
    pub(super) partial_list: u16,
    /// Fully used slab list head
    pub(super) full_list: u16,
    /// Statistics: allocation count
    pub(super) alloc_count: AtomicUsize,
    /// Statistics: free count
    pub(super) free_count: AtomicUsize,
}

/// Slab page management
///
/// All methods take `&self` (not `&mut self`). Interior mutability is
/// achieved through:
///   - `allocated_pages`: AtomicUsize for lock-free page allocation
///   - `get_header_mut()`: returns raw `&mut SlabHeader` via pointer
///     arithmetic — each slab header lives at a unique page address,
///     and concurrent access to *different* pages is safe because the
///     underlying memory regions don't overlap.
///
/// This design allows `SlabPages` to be shared (`&self`) across CPUs
/// without a global lock, while per-cache spinlocks serialize access
/// to each cache's slab lists.
pub struct SlabPages {
    /// Slab page base address (immutable after init)
    pub(super) base_addr: usize,
    /// Allocated page count (atomic for lock-free concurrent page allocation)
    pub(super) allocated_pages: AtomicUsize,
    /// Maximum page count (immutable after init)
    pub(super) max_pages: usize,
}

/// Slab allocator global state.
///
/// All fields are `Sync`:
///   - `caches`: per-cache `Spinlock` — only one CPU can hold a given cache lock
///   - `pages`: `SlabPages` (Sync proof in mm::unsafe, atomic page counter)
///   - `initialized`: `AtomicUsize` — lock-free read
///
/// This allows `SLAB_ALLOCATOR` to be a plain `static` (no `mut`), eliminating
/// the `&mut` aliasing UB that existed when multiple CPUs simultaneously called
/// `kmalloc` for different cache indices.
pub struct SlabAllocator {
    /// Slab cache array
    pub(super) caches: [Spinlock<SlabCache>; NUM_CACHES],
    /// Slab page management (UnsafeCell for one-time init write through &self)
    pub(super) pages: UnsafeCell<SlabPages>,
    /// Whether initialized
    pub(super) initialized: AtomicUsize,
}

/// Cache statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub object_size: usize,
    pub alloc_count: usize,
    pub free_count: usize,
}

/// Slab statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct SlabStats {
    pub cache_stats: [CacheStats; NUM_CACHES],
    pub total_pages: usize,
}

// ---------------------------------------------------------------------------
// the memory-info snapshot, its summary and the Display helper
// ---------------------------------------------------------------------------

/// Memory statistics info (similar to /proc/meminfo)
#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    // ========== Physical Memory ==========
    /// Total physical memory (bytes)
    pub mem_total: usize,
    /// Free physical memory (bytes)
    pub mem_free: usize,
    /// Available physical memory (bytes) = mem_free + reclaimable memory
    pub mem_available: usize,
    /// Used physical memory (bytes)
    pub mem_used: usize,

    // ========== Heap Memory (Buddy Allocator) ==========
    /// Heap total size (bytes)
    pub heap_total: usize,
    /// Heap used (bytes)
    pub heap_used: usize,
    /// Heap free (bytes)
    pub heap_free: usize,

    // ========== Slab Allocator ==========
    /// Slab page count
    pub slab_pages: usize,
    /// Slab allocation count
    pub slab_allocs: usize,
    /// Slab free count
    pub slab_frees: usize,

    // ========== Per-CPU Pages ==========
    /// PCP page count for each CPU — frames cached in that CPU's
    /// per-CPU pageset (mm/pcp.rs); counted free by zone.free_pages.
    pub pcp_pages: [usize; crate::config::MAX_CPUS],

    // ========== Page Descriptor Statistics ==========
    /// Free page count
    pub pages_free: usize,
    /// In-use page count
    pub pages_used: usize,
    /// Reserved page count
    pub pages_reserved: usize,
    /// Mapped page count
    pub pages_mapped: usize,
    /// Dirty page count
    pub pages_dirty: usize,
    /// COW page count
    pub pages_cow: usize,
    /// Anonymous page count
    pub pages_anon: usize,
}

/// Memory info formatter
pub struct MemoryInfoFormatter<'a> {
    pub(super) info: &'a MemoryInfo,
}

/// Memory usage summary (for quick checking)
#[derive(Debug, Clone, Copy, Default)]
pub struct MemorySummary {
    /// Total physical memory (MB)
    pub total_mb: usize,
    /// Used physical memory (MB)
    pub used_mb: usize,
    /// Free physical memory (MB)
    pub free_mb: usize,
    /// Heap usage percentage
    pub heap_usage_percent: usize,
}

// ---------------------------------------------------------------------------
// the address space: MmStruct, its flags and alias, and the leaf-PTE lock family
// ---------------------------------------------------------------------------

/// The global PTE epoch — the type of each arch's `PTE_MODIFY_LOCK`
/// static (arch/*/mm/mm_ops.rs).
///
/// * Exclusive holders (`lock` / `lock_irqsave`) get the OLD single-lock
///   semantics: they exclude against every leaf-PTE writer on the system.
///   On riscv64 (global mode) ALL PTE sections go through here; on x86_64
///   the remaining direct users are the ipc install paths (binder /
///   sysv_shm — IPC line's files), `map_user_region` and kernel-root /
///   unpublished-tree maps, none of which can reach the owning mm.
/// * Shared holders are per-mm PTE sections (x86_64 split mode, `PteLock`)
///   — they exclude against exclusive holders and against each other only
///   through the per-mm spinlock, so PTE writers on DIFFERENT mms run
///   concurrently.
pub struct GlobalPteLock {
    pub(crate) raw: RwSpinlock<()>,
}

/// Per-mm leaf-PTE exclusion lock (x86_64 split mode; see `GlobalPteLock`).
///
/// On non-x86_64 builds (`pte_lock_for` global mode) `lock*()` degrade to
/// the global exclusive section — the per-mm spin field is unused there.
pub struct PteLock {
    pub(super) spin: Spinlock<()>,
}

/// Guard of a `PteLock::lock()` section. Fields drop innermost-first
/// (declaration order): the mm spin releases before the epoch.
/// Split mode on all architectures (see PteLock::lock).
pub struct PteGuard<'a> {
    pub(super) _spin: SpinlockGuard<'a, ()>,
    pub(super) _epoch: crate::sync::rwlock::RwSpinlockReadGuard<'a, ()>,
}

/// Guard of a `PteLock::lock_irqsave()` section.
pub struct PteIrqGuard<'a> {
    pub(super) _spin: SpinlockIrqGuard<'a, ()>,
    pub(super) _epoch: crate::sync::rwlock::RwSpinlockIrqReadGuard<'a, ()>,
}

/// Memory Descriptor
///
/// Structure describing a process's complete address space.
pub struct MmStruct {
    // ==================== Page Table Management ====================
    /// Page table root PPN (Page Global Directory)
    pub pgd: u64,

    /// Address Space ID for TLB management
    /// 0 means not allocated or kernel ASID
    pub(super) asid: AtomicU16,

    /// Leaf-PTE exclusion lock for THIS address space (INV-LOCK-7; the
    /// vestigial RwSpinlock `pgd_lock` that never had a caller, replaced
    /// by the per-mm PTE lock). Acquired only through `pte_lock()` /
    /// the arch hook `arch::mm::mm_ops::pte_lock_for(&mm)`.
    pub(super) pte_lock: PteLock,

    /// Whole-syscall surgery lock for THIS address space (hp89 hunt, the
    /// mremap-move race): mm syscalls that run as MULTI-STEP sequences
    /// with dropped internal guards (mremap's mmap-new -> copy -> munmap-
    /// old move; munmap's multi-piece cursor walk; brk's shrink; madvise's
    /// zap; mmap's MAP_FIXED teardown+install) take this for the WHOLE
    /// call, so sibling CLONE_VM threads can never interleave their own
    /// surgery into the gaps. This is the reference kernel's mmap_lock
    /// (write) role, scoped to the syscall layer; the internal vma/pte
    /// guards stay as they are (the copy loop's pte section already
    /// excludes same-mm leaf writers — the residual hole was the gap
    /// BEFORE the copy, where a sibling munmap/mremap cleared the move
    /// SOURCE and copy_old_to_new_pages then silently installed NOTHING
    /// at the destination, fault-filling ZERO pages: the mallocng
    /// zeroed-metadata signature).
    pub(super) mm_surgery: Rwsem<()>,

    /// VMA manager. SLEEPABLE rwsem (the mmap_lock shape, linux-6.6
    /// kernel/locking/rwsem.c — see sync/rwsem.rs): mm paths hold
    /// this across slow work (pagefault COW under a read guard while
    /// page allocation crawls; VmaManager::add under the write
    /// guard), and the former pure spinner monopolized CPUs whenever
    /// a slow reader met a pending writer (the 52nd's §6b evidence).
    /// Waiters now SLEEP; held sections are preemptible. Position in
    /// the lock graph is unchanged (INV-LOCK-7: vma lock above the
    /// PTE section, above page_desc).
    pub(super) vma_manager: Rwsem<VmaManager>,

    /// Pinned file references for file-backed VMAs, keyed by VMA start
    /// address. Linux's VMA holds vm_file (a refcounted struct file) that
    /// keeps the file alive after the mapping fd is closed; Rux VMAs only
    /// store the fd NUMBER, and demand faults resolved it through the
    /// process fd table — after the dynamic linker maps a DSO and closes
    /// its fd, every later demand fill silently produced ZERO pages (the
    /// get_file_fd lookup failed) and ld.so died parsing zero ELF data.
    /// This pin table is the vm_file equivalent.
    pub(super) vma_files: RwSpinlock<alloc::collections::BTreeMap<usize, alloc::sync::Arc<crate::fs::file::File>>>,

    /// Address space type
    pub(super) space_type: PageTableType,

    // ==================== Segment Ranges ====================
    /// Code segment start address
    pub(super) start_code: AtomicUsize,

    /// Code segment end address
    pub(super) end_code: AtomicUsize,

    /// Data segment start address
    pub(super) start_data: AtomicUsize,

    /// Data segment end address
    pub(super) end_data: AtomicUsize,

    // ==================== Heap Management ====================
    /// Heap start address (minimum brk value)
    pub(super) start_brk: AtomicUsize,

    /// Current heap pointer (current brk value) — the SINGLE authority
    /// for the break (GNOME trim face, 2026-11-07/c127e4bc1): one break
    /// per address space, shared by every thread, like the reference's
    /// mm->brk. Task::brk is only a synced copy for fork/exec readers.
    pub(super) brk: AtomicUsize,

    /// Per-mm break lock (GNOME trim face, 2026-11-07): serializes the
    /// whole sys_brk read-modify-write (break value + VMA surgery).
    /// The reference kernel does the same under mmap_lock. Without it
    /// two threads of one process could interleave a grow and a shrink:
    /// the shrink's value store could land AFTER the grow's, leaving
    /// the break value behind the heap VMA's end — the next shrink
    /// would then release live pages. Lock order: brk_lock is the
    /// OUTERMOST (taken before the mm-surgery section and the VMA write
    /// lock); nothing takes it while holding those.
    pub(super) brk_lock: Spinlock<()>,

    // ==================== Stack Management ====================
    /// Stack start address (stack top)
    pub(super) start_stack: AtomicUsize,

    /// Stack limit (minimum stack address, stack can grow down to this)
    /// This is the maximum size limit for stack expansion
    pub(super) stack_limit: AtomicUsize,

    // ==================== Arguments and Environment Variables ====================
    /// Command line arguments start address
    pub(super) arg_start: AtomicUsize,

    /// Command line arguments end address
    pub(super) arg_end: AtomicUsize,

    /// Environment variables start address
    pub(super) env_start: AtomicUsize,

    /// Environment variables end address
    pub(super) env_end: AtomicUsize,

    /// Saved auxiliary vector (core dump NT_AUXV source).
    ///
    /// Linux keeps the exec-time auxv in mm->saved_auxv so a later core
    /// dump can emit NT_AUXV without walking the (possibly clobbered)
    /// user stack. AT_VECTOR_SIZE-sized word array: (type, value) pairs,
    /// terminated by the AT_NULL pair written by copy_strings; entries
    /// past the terminator stay zero.
    pub(super) saved_auxv: RwSpinlock<[u64; AT_VECTOR_SIZE]>,

    // ==================== Virtual Memory Statistics ====================
    /// Total virtual memory page count
    pub(super) total_vm: AtomicU64,

    /// Locked memory page count
    pub(super) locked_vm: AtomicU64,

    /// Pinned memory page count
    pub(super) pinned_vm: AtomicU64,

    /// Data segment page count
    pub(super) data_vm: AtomicU64,

    /// Executable segment page count
    pub(super) exec_vm: AtomicU64,

    /// Stack page count
    pub(super) stack_vm: AtomicU64,

    /// Resident set size in pages (review 4.14: OOM badness input).
    /// Incremented when a page is installed into this address space by a
    /// fault, decremented per zapped PTE in unmap_pages. COW-fork shared
    /// pages are counted only in the mm that faults them in after the
    /// break (approximation — no per-PTE accounting).
    pub(super) rss: AtomicU64,

    // ==================== mmap Region Management ====================
    /// mmap region base address
    pub(super) mmap_base: AtomicUsize,

    /// mmap region legacy base address
    pub(super) mmap_legacy_base: AtomicUsize,

    /// Highest virtual memory end address
    pub(super) highest_vm_end: AtomicUsize,

    // ==================== Reference Counting ====================
    /// User count: number of threads sharing this mm
    pub(super) mm_users: AtomicI32,

    /// Reference count: lifetime reference of mm_struct
    pub(super) mm_count: AtomicI32,

    // ==================== Other Fields ====================
    /// Flags
    pub(super) flags: AtomicU64,

    /// Owning task (optional)
    pub(super) owner_pid: AtomicI32,

    /// Inode key of the exec'd image this mm holds a text denial on
    /// (the mm->exe_file stand-in: reference deny_write_access at exec,
    /// allow at the last mm's death). None until this mm's first exec
    /// commit; fork's copy_mm duplicates it with one more hold. Written
    /// once by the owning task at exec commit (or by fork before the
    /// child is published), read at Drop (single, last-Arc) — the lock
    /// only provides interior mutability through the Arc, never
    /// contends.
    pub(super) exe_text_key: Spinlock<Option<(u64, u64)>>,

    /// CPUs that may hold cached user translations of this mm (Linux
    /// mm_cpumask shape, OH boot lane 2026-11-12) — the exact predicate
    /// a TLB shootdown needs.
    ///
    /// Set in context_switch/exec BEFORE the incoming CR3 write. On
    /// x86_64 with PCID OFF (every plain CR3 write fully flushes the
    /// TLB), the bit is CLEARED on switching away (lazy mask): the mask
    /// tracks "CPUs currently running this mm", so a single-threaded
    /// process's shootdown sends ZERO IPIs. With PCID on (NOFLUSH
    /// tagged writes keep entries alive across switches) the mask is
    /// grow-only — over-IPIs, never under-IPIs.
    ///
    /// Correctness: a CPU's stale entries can only be created by an
    /// incoming CR3 write, which is preceded by its bit-set and — MOV
    /// to CR3 being serializing — the bit-set is globally performed
    /// before any cached entry exists; a sender that has already
    /// cleared the PTEs and then reads the mask with Acquire sees every
    /// CPU that can hold old entries (single-variable coherence). The
    /// clear runs with IRQs off before the outgoing flush-on-write, so
    /// no translation window straddles the clear.
    pub(super) cpu_mask: AtomicU64,
}

/// MmStruct flags
pub struct MmFlags;

/// Capacity of the saved auxv, in u64 words (Linux AT_VECTOR_SIZE = 22
/// (type, value) pairs = 44 words).
pub const AT_VECTOR_SIZE: usize = 44;

/// Address space type alias (backward compatibility)
///
/// Recommend using MmStruct directly
pub type AddressSpace = MmStruct;

// ---------------------------------------------------------------------------
// the early-boot memory map: the MemBlock family
// ---------------------------------------------------------------------------

/// Maximum number of memory regions
/// Need enough slots for all reserved regions plus individual page allocations
/// during early boot (device mappings, linear mapping page tables, vmemmap)
pub(super) const MAX_MEMBLOCK_REGIONS: usize = 128;

/// Memory region flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemBlockFlags(pub(super) u32);

/// A memory region descriptor
#[derive(Debug, Clone, Copy)]
pub struct MemBlockRegion {
    /// Physical start address
    pub base: usize,
    /// Region size in bytes
    pub size: usize,
    /// Region flags
    pub flags: MemBlockFlags,
    /// NUMA node ID (0 for UMA systems)
    pub nid: u32,
}

/// A collection of memory regions
#[derive(Debug)]
pub struct MemBlockType {
    /// Array of regions
    pub(super) regions: [MemBlockRegion; MAX_MEMBLOCK_REGIONS],
    /// Number of valid regions
    pub(super) cnt: usize,
    /// Total size of all regions
    pub(super) total_size: usize,
}

/// Global memblock state
pub struct MemBlock {
    /// Available memory regions
    pub(super) memory: MemBlockType,
    /// Reserved memory regions
    pub(super) reserved: MemBlockType,
    /// Whether memblock is initialized
    pub(super) initialized: AtomicBool,
    /// Bottom of available memory (after kernel + reserved)
    pub(super) bottom: usize,
    /// Top of available memory
    pub(super) top: usize,
    /// Current allocation pointer
    pub(super) current: usize,
}

// ---------------------------------------------------------------------------
// the memory zone: Zone and its free areas, the zone snapshot and the GFP flags
// ---------------------------------------------------------------------------

/// Zone types
///
/// On RISC-V, we typically only need ZONE_NORMAL since there are no
/// DMA constraints like on x86. However, we define all zones for
/// completeness and future compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum ZoneType {
    /// Zone for DMA-constrained devices (0-16MB on x86)
    /// Not typically needed on RISC-V, but included for compatibility
    ZoneDma = 0,

    /// Zone for 32-bit DMA devices (0-4GB on 64-bit systems)
    /// May be needed for some RISC-V SoCs with 32-bit DMA limitations
    ZoneDma32 = 1,

    /// Normal zone for all general-purpose memory
    /// This is the primary zone used for most allocations on RISC-V
    ZoneNormal = 2,

    /// Zone for movable pages (can be migrated/compacted)
    /// Used for memory hotplug and compaction
    ZoneMovable = 3,

    /// Number of zone types
    ZoneCount = 4,
}

/// Maximum order for buddy allocator (2^MAX_ORDER pages = 4MB with 4KB pages)
pub const MAX_ORDER: usize = 10;

/// Free area for one order level
pub struct FreeArea {
    /// Head of free list (stores PFN of first free block)
    pub(super) free_list: AtomicUsize,
    /// Number of free blocks at this order
    pub(super) nr_free: AtomicUsize,
}

/// Watermark indices
pub const WMARK_MIN: usize = 0;
pub const WMARK_LOW: usize = 1;
pub const WMARK_HIGH: usize = 2;
pub const NR_WMARK: usize = 3;

/// Memory zone
///
/// Represents a contiguous range of physical memory pages with
/// similar characteristics. Each zone has its own buddy allocator.
pub struct Zone {
    /// Zone type
    pub(super) zone_type: ZoneType,

    /// Zone ID (index in node's zone array)
    pub(super) zone_id: usize,

    /// Node ID (NUMA node this zone belongs to)
    pub(super) node_id: usize,

    /// Start page frame number (PFN)
    pub(super) zone_start_pfn: AtomicUsize,

    /// End page frame number (exclusive)
    pub(super) zone_end_pfn: AtomicUsize,

    /// Total number of pages in the zone
    pub(super) spanned_pages: AtomicUsize,

    /// Number of present pages (excluding holes)
    pub(super) present_pages: AtomicUsize,

    /// Number of managed pages (available for allocation)
    pub(super) managed_pages: AtomicUsize,

    /// Number of currently free pages
    pub(super) free_pages: AtomicUsize,

    /// Free areas for each order (buddy allocator)
    pub(super) free_area: [FreeArea; MAX_ORDER + 1],

    /// Zone lock for buddy operations
    pub(super) lock: Spinlock<()>,

    /// Zone initialized flag
    pub(super) initialized: AtomicBool,

    /// Watermark levels: [WMARK_MIN, WMARK_LOW, WMARK_HIGH]
    /// Set once during init by setup_per_zone_wmarks().
    pub(super) _watermark: [AtomicUsize; NR_WMARK],
}

/// Zone statistics
#[derive(Debug, Clone, Copy)]
pub struct ZoneStats {
    pub zone_type: ZoneType,
    pub zone_id: usize,
    pub node_id: usize,
    pub start_pfn: usize,
    pub end_pfn: usize,
    pub spanned_pages: usize,
    pub present_pages: usize,
    pub managed_pages: usize,
    pub free_pages: usize,
    pub free_blocks: [usize; MAX_ORDER + 1],
}

/// GFP (Get Free Pages) flags
///
/// These flags control allocation behavior and zone selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfpFlags(pub u32);

// ---------------------------------------------------------------------------
// the NUMA node: PglistData and the node snapshot
// ---------------------------------------------------------------------------

/// Maximum number of zones per node
pub const MAX_NR_ZONES: usize = ZoneType::ZoneCount as usize;

/// LRU list type indices (following mm/vmscan.c enum lru_list).
pub const LRU_INACTIVE_ANON: usize = 0;
pub const LRU_ACTIVE_ANON: usize = 1;
pub const LRU_INACTIVE_FILE: usize = 2;
pub const LRU_ACTIVE_FILE: usize = 3;
pub const LRU_UNEVICTABLE: usize = 4;
/// Number of LRU lists.
pub const NR_LRU_LISTS: usize = 5;

/// Page list data structure (represents a NUMA node)
///
/// On UMA systems (like our current RISC-V QEMU), there is only one node
/// containing all zones. On NUMA systems, each node has its own memory
/// and possibly its own zones.
pub struct PglistData {
    /// Node ID
    pub(super) node_id: usize,

    /// Zones in this node
    pub(super) zones: [Option<Zone>; MAX_NR_ZONES],

    /// Number of zones in this node
    pub(super) nr_zones: AtomicUsize,

    /// Node start PFN
    pub(super) node_start_pfn: AtomicUsize,

    /// Node spanned pages (total pages including holes)
    pub(super) node_spanned_pages: AtomicUsize,

    /// Node present pages (excluding holes)
    pub(super) node_present_pages: AtomicUsize,

    /// Total reserved pages
    pub(super) total_reserved_pages: AtomicUsize,

    /// Initialized flag
    pub(super) initialized: AtomicBool,

    // ---- LRU list infrastructure ----

    /// LRU list heads: PFN of the first page (most-recently used end).
    /// 0 means the list is empty.  Protected by `lru_lock`.
    // Published for the mm policy crate (M1b): all-atomic PFN
    // links under lru_lock — the link/unlink loops are crate-side lru
    // policy, safe to operate under forbid.
    pub lru_heads: [AtomicUsize; NR_LRU_LISTS],

    /// LRU list tails: PFN of the last page (least-recently used end,
    /// where kswapd scans from).  0 means the list is empty.
    pub lru_tails: [AtomicUsize; NR_LRU_LISTS],

    /// Number of pages on each LRU list.
    // Published for the mm policy crate (M1b): all-atomic interior
    // mutability — safe to operate under forbid; the LRU size CAS/fetch
    // loops live in the crate-side lru/vmscan policy.
    pub lru_sizes: [AtomicUsize; NR_LRU_LISTS],

    /// Spinlock protecting all LRU list mutations.
    // Published for the mm policy crate (M1b): the LRU list lock —
    // lock-and-link discipline is crate-side lru policy (INV under the
    // lru_lock ordering rule, docs/architecture/lock-ordering.md).
    pub lru_lock: Spinlock<()>,
}

/// Node statistics
#[derive(Debug, Clone)]
pub struct NodeStats {
    pub node_id: usize,
    pub start_pfn: usize,
    pub spanned_pages: usize,
    pub present_pages: usize,
    pub total_free_pages: usize,
    pub zones: Vec<ZoneStats>,
}

// ---------------------------------------------------------------------------
// the anonymous-VMA object and the rmap snapshot
// ---------------------------------------------------------------------------

/// Anonymous VMA
///
/// Represents a group of VMAs that share anonymous pages.
/// When a page is shared (e.g., after fork), all processes
/// mapping that page are linked through anon_vma.
pub struct AnonVma {
    /// Reference count
    pub(super) refcount: AtomicUsize,

    /// Root anon_vma (for hierarchical anon_vmas)
    pub(super) root: AtomicPtr<AnonVma>,

    /// List of child anon_vmas
    pub(super) children: RwSpinlock<Vec<Arc<AnonVma>>>,

    /// Associated VMA
    pub(super) vma: AtomicUsize,
}

/// Anonymous VMA chain entry
pub type AnonVmaChain = AnonVma;

/// Reverse mapping statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct RmapStats {
    /// Number of anon_vmas allocated
    pub anon_vma_count: usize,
    /// Number of pages with reverse mappings
    pub mapped_pages: usize,
    /// Number of pages currently being migrated
    pub migrating_pages: usize,
}

// ---------------------------------------------------------------------------
// the vmemmap snapshot
// ---------------------------------------------------------------------------

/// vmemmap statistics
#[derive(Debug, Clone, Copy)]
pub struct VmemmapStats {
    pub initialized: bool,
    pub vmemmap_base: usize,
    pub start_pfn: usize,
    pub nr_pages: usize,
    pub vmemmap_pages: usize,
}

// ---------------------------------------------------------------------------
// one pool ring slot
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub(super) ptr: usize,
    pub(super) size: usize,
    pub(super) align: usize,
    pub(super) deadline: u64,
}

// ---------------------------------------------------------------------------
// the small-object allocator's page header, class and magazine
// ---------------------------------------------------------------------------

/// In-page header (16 bytes) at the start of every slab page.
#[repr(C)]
pub(super) struct PageHdr {
    pub(super) magic: u16,
    pub(super) cls: u8,
    pub(super) state: u8,
    pub(super) free_head: u16, // first free object index, 0xFFFF = none
    pub(super) free_count: u16,
    pub(super) next_page: u32, // page INDEX of the next active page, 0 = end
    pub(super) _pad: u32,
}

pub(super) struct SmallCache {
    pub(super) lock: Spinlock<()>,
    /// Page INDEX of the first ACTIVE page (0 = none). Under lock.
    pub(super) head: AtomicUsize,
    /// Forensics.
    pub(super) pages_created: AtomicUsize,
    pub(super) pages_reclaimed: AtomicUsize,
    pub(super) obj_allocs: AtomicUsize,
    pub(super) obj_frees: AtomicUsize,
    /// Class-lock acquisitions (slow paths only — one per magazine
    /// refill/flush batch, pre-magazine one per operation). Printed by
    /// the census so a boot's lock-traffic win is directly readable
    /// against obj_allocs+obj_frees on the same line.
    pub(super) lock_holds: AtomicUsize,
    /// Page-list walks that hit the corruption cap (cycle defense,
    /// lb260/lb261 OH allocator-freeze) — nonzero means the wild-write
    /// producer struck this class's list.
    pub(super) walk_cycles: AtomicUsize,
    /// rux:allocwatch (retired debug=allocwatch) section meter: max slow-path section duration
    /// (TSC cycles) and count of sections >10 ms (25M cycles @2.5GHz).
    pub(super) slow_max_cyc: AtomicU64,
    pub(super) slow_over_10ms: AtomicU64,
}

/// Depth of one per-CPU magazine row: the `buf` length of [`Magazine`].
pub(super) const MAG_DEPTH: usize = 16;

#[derive(Clone, Copy)]
pub(super) struct Magazine {
    pub(super) buf: [usize; MAG_DEPTH],
    pub(super) len: usize,
}

// ---------------------------------------------------------------------------
// the two 4 KiB pages
// ---------------------------------------------------------------------------

/// The shared vDSO data page (4 KiB aligned), refreshed by
/// `vdso_data_tick()`.
#[repr(align(4096))]
pub(super) struct VdsoDataPage(pub(super) [u8; 4096]);

/// The vDSO ELF page (4 KiB aligned), built once by `vdso_init()`.
#[repr(align(4096))]
pub(super) struct VdsoCodePage(pub(super) [u8; 4096]);

// ---------------------------------------------------------------------------
// one attribution-ring event
// ---------------------------------------------------------------------------

/// One event: plain atomics (the Task::mm_ops shape) — a concurrent
/// reader may see a partially-updated event under contention, which is
/// acceptable for forensics (LOCK still serializes writers).
pub(super) struct UnEvent {
    /// KIND_* (0 = never written).
    pub(super) kind: AtomicU64,
    /// Acting pid.
    pub(super) pid: AtomicU64,
    /// Kind-specific: start VA (1), root ppn (2), target pfn (3/4).
    pub(super) a: AtomicU64,
    /// End VA (1), count (2/3), new_bits (4).
    pub(super) b: AtomicU64,
    /// Leaves cleared / PTEs replaced.
    pub(super) cnt: AtomicU64,
    /// jiffies at record time.
    pub(super) j: AtomicU64,
}

// ---------------------------------------------------------------------------
// the pin handle
// ---------------------------------------------------------------------------

/// Pin of the user pages backing one byte range `[va, va+len)`.
///
/// `pages[i]` covers the i-th page of the range (page 0 starts at
/// `va & !(PAGE_SIZE-1)`); `None` marks a page that could not be pinned
/// — bytes of the range in such pages MUST be served through the
/// caller's staged fallback, never DMA'd.
pub struct PinnedUserPages {
    /// User VA of the first byte of the range.
    pub(super) va: usize,
    /// Byte length of the range (>= 1; may end mid-page).
    pub(super) len: usize,
    /// Per covered page: physical frame base (page-aligned), or None
    /// for an unpinnable page.
    pub(super) pages: Vec<Option<u64>>,
    /// Number of pages actually pinned (for accounting/asserts).
    pub(super) pinned: usize,
}

// ---------------------------------------------------------------------------
// the per-CPU pageset and its storage row (INV-PCP-1..3)
// ---------------------------------------------------------------------------

/// Frames moved to/from the zone per refill/drain round trip.
pub const PCP_BATCH: usize = 16;

/// Row high-water mark: at this many cached frames a drain returns
/// PCP_BATCH frames to the buddy.
pub(super) const PCP_HIGH: usize = 2 * PCP_BATCH;

#[repr(C, align(64))]
pub(super) struct PcpPageset {
    /// Cached order-0 PFNs. Slots [0..count) are live.
    pub(super) list: [usize; PCP_HIGH],
    pub(super) count: usize,
}

/// Storage row. `UnsafeCell` + manual `Sync`: cross-CPU access never
/// forms a reference (counts are peeked through raw pointers only);
/// mutable access is confined to the owning CPU inside its irq-off
/// window (INV-PCP-3), which serializes nested entry on the same CPU.
#[repr(C, align(64))]
pub(super) struct PcpRow(pub(super) UnsafeCell<PcpPageset>);
