//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The drivers subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct` and `enum` of the subsystem lives
//! here, together with the constants those definitions name (array
//! lengths and what they derive from; a const takes its contiguous
//! siblings so a const block is never split); every `impl` block lives
//! in `hotpath.rs` beside the functions that use it — an impl is code, a
//! definition is data layout. The exceptions are definitions local to
//! code: a type declared inside a function or inside a private inline
//! module of `hotpath.rs`, and the whole of the `timer/` and `intc/`
//! directories, which stay one inline module of `hotpath.rs` each (their
//! per-architecture child modules are chosen by `#[cfg]` on the module
//! declaration), so nothing inside them moves.
//!
//! This file holds no `unsafe`. A field that was private to its old
//! topic file is `pub(super)` here because the code that touches it
//! lives in the sibling module `hotpath`: the drivers directory is the
//! smallest visibility boundary Rust offers for that split.
//!
//! One definition was renamed because two files declared the same name:
//! input/virtio_input.rs's unused `UsedElem` is `VirtioInputUsedElem`
//! here, because virtio/queue.rs's `UsedElem` (the one every caller
//! uses) keeps the name.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this file must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::alloc::Layout;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::vec_deque::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64};

use crate::fs::types::File;
use crate::net::types::SkBuff;
use crate::sync::types::{Spinlock, SpinlockIrqGuard};

// ---------------------------------------------------------------------------
// the block layer: BlockDeviceOps, Geo, DmaSeg, GenDisk, Request,
// ReqCmd, BlockDeviceManager, and the DiskStat counters
// ---------------------------------------------------------------------------

/// Per-disk C-style operations table (the drivers-family ABI, round-5:
/// the pointers are SAFE — registration is the audited edge; a driver
/// that registers an op asserts its implementation upholds the op's
/// documented contract, and the block layer calls it as plain Rust).
#[repr(C)]
pub struct BlockDeviceOps {
    /// Open block device
    pub open: Option<fn() -> i32>,
    /// Release block device
    pub release: Option<fn() -> i32>,
    /// Get geometry info
    pub getgeo: Option<fn(&mut Geo) -> i32>,
    /// Kick quietly-submitted chains on this disk (rung4 prep move 2):
    /// transport-level doorbell for drivers that publish chains without
    /// notifying (the PCI virtio-blk quiet-submit path). The block
    /// layer's drain/recovery paths call [`kick_all_pending_disks`] —
    /// walking the registry — instead of naming any driver. None = the
    /// transport notifies inline (no-op kick).
    pub kick: Option<fn(&GenDisk)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Geo {
    /// Number of heads
    pub heads: u8,
    /// Number of sectors
    pub sectors: u8,
    /// Number of cylinders
    pub cylinders: u16,
    /// Start position
    pub start: u32,
}

/// One scatter-gather segment of a fused block request: a memory range
/// (page-aligned or not — the driver maps it to phys) covering
/// `len` whole 512-byte sectors of the request's CONTIGUOUS device range.
/// The blk-merge.c bio-segment shape: physically-discontiguous memory,
/// device-contiguous sectors.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DmaSeg {
    /// Buffer (DMA target for reads, DMA source for writes)
    pub ptr: *mut u8,
    /// Length in bytes (multiple of the sector size)
    pub len: usize,
}

pub struct GenDisk {
    /// Device name
    pub name: &'static str,
    /// Major device number
    pub major: u32,
    /// First minor device number
    pub first_minor: u32,
    /// Number of minor device numbers
    pub minors: u32,
    /// Capacity (in 512-byte sectors)
    pub capacity: AtomicU64,
    /// Block size
    pub block_size: u32,
    /// Block device operations
    pub ops: Option<&'static BlockDeviceOps>,
    /// Private data (driver-owned; resolve with [`GenDisk::private`])
    pub private_data: Option<*mut u8>,
    /// Request handler function
    pub request_fn: Option<fn(&mut Request)>,
    /// Async read function (device-specific async submit without blocking).
    /// Registration contract (INV-BLKDEV-2, the driver-registration edge):
    /// the implementation receives the registered `&GenDisk` and must keep
    /// `buf` a valid DMA target until the completion fires.
    pub async_read_fn: Option<fn(&GenDisk, u64, &mut [u8], &crate::fs::types::IoCompletion) -> i32>,
    /// Async write function (device-specific async submit without blocking).
    /// The DATA buffer is the DMA SOURCE: the registered implementation
    /// asserts the caller keeps it valid and unmodified until the
    /// completion fires (the write-batch contract, INV-BLKDEV-2).
    pub async_write_fn: Option<fn(&GenDisk, u64, &[u8], &crate::fs::types::IoCompletion) -> i32>,
    /// Async scatter-gather read (one fused request over N segments whose
    /// combined length covers a contiguous sector range starting at
    /// `sector`). None on devices without SG support — callers fall back
    /// to per-segment `async_read_fn` submissions.
    pub async_read_sg_fn: Option<fn(&GenDisk, u64, &[DmaSeg], &crate::fs::types::IoCompletion) -> i32>,
    /// Async scatter-gather write — the fused form of `async_write_fn`;
    /// every segment is a DMA source under the same stability contract.
    pub async_write_sg_fn: Option<fn(&GenDisk, u64, &[DmaSeg], &crate::fs::types::IoCompletion) -> i32>,
}

pub struct Request {
    /// Command type
    pub cmd_type: ReqCmd,
    /// Starting sector
    pub sector: u64,
    /// Data buffer
    pub buffer: Vec<u8>,
    /// Block device (typed handle; see INV-BLKDEV-1 on [`get_disk`])
    pub device: &'static GenDisk,
    /// Final device status (R8-M2): set by the driver before the request
    /// returns. submit_request returns 0 whenever a request_fn is
    /// registered, so a device-side error used to vanish and the caller
    /// copied a ZERO buffer as success (cached as BH_Uptodate!).
    /// (round-5: the never-registered `end_io` callback and the
    /// never-set `completion` token were deleted — dead ABI surface.)
    pub error: core::sync::atomic::AtomicI32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReqCmd {
    /// Read
    Read,
    /// Write
    Write,
    /// Flush
    Flush,
}

pub(super) struct BlockDeviceManager {
    /// Block device list
    pub(super) disks: Spinlock<Vec<Option<Box<GenDisk>>>>,
    /// Device number allocator
    pub(super) major_next: AtomicU32,
}

/// One device's I/O counters.
pub struct DiskStat {
    /// Major device number.
    pub major: u32,
    /// First minor device number.
    pub minor: u32,
    /// Device name (as in /proc/partitions).
    pub name: String,
    /// Reads completed.
    pub reads: u64,
    /// Sectors read.
    pub sectors_read: u64,
    /// Milliseconds spent reading.
    pub ms_reading: u64,
    /// Writes completed.
    pub writes: u64,
    /// Sectors written.
    pub sectors_written: u64,
    /// Milliseconds spent writing.
    pub ms_writing: u64,
    /// I/Os in progress right now.
    pub io_in_progress: u64,
    /// Milliseconds spent doing I/O.
    pub ms_doing_io: u64,
}

/// The live counters of one device.
pub(super) struct Entry {
    pub(super) major: u32,
    pub(super) minor: u32,
    pub(super) name: String,
    pub(super) reads: AtomicU64,
    pub(super) sectors_read: AtomicU64,
    pub(super) ms_reading: AtomicU64,
    pub(super) writes: AtomicU64,
    pub(super) sectors_written: AtomicU64,
    pub(super) ms_writing: AtomicU64,
    pub(super) io_in_progress: AtomicU64,
    pub(super) ms_doing_io: AtomicU64,
}

// ---------------------------------------------------------------------------
// the PCI BAR type and the config-space record
// ---------------------------------------------------------------------------

/// PCI BAR type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BARType {
    MemoryMapped,
    IOMapped,
    None,
}

/// PCI Base Address Register (BAR)
#[derive(Debug, Clone, Copy)]
pub struct PCIBAR {
    pub base_addr: u64,
    pub size: u64,
    pub bar_type: BARType,
    pub prefetchable: bool,
    pub is_64bit: bool,
}

/// PCI configuration space access structure
#[derive(Debug, Clone, Copy)]
pub struct PCIConfig {
    pub base_addr: u64,
}

// ---------------------------------------------------------------------------
// VirtIO: the MMIO register layout, block device and I/O bookkeeping,
// the split ring (Desc, AvailRing, UsedRing, VirtQueue, Vring) and the
// PCI capability family
// ---------------------------------------------------------------------------

/// VirtIO device register layout (compliant with VirtIO 1.0 specification)
#[repr(C)]
pub struct VirtIOBlkRegs {
    /// Magic number (0x00)
    pub magic_value: u32,
    /// Version (0x04)
    pub version: u32,
    /// Device ID (0x08)
    pub device_id: u32,
    /// Vendor ID (0x0C)
    pub vendor: u32,
    /// Device features (0x10)
    pub device_features: u32,
    /// _reserved (0x14)
    pub(super) _reserved1: u32,
    /// Driver-selected features (0x20)
    pub driver_features: u32,
    /// _reserved (0x24)
    pub(super) _reserved2: u32,
    /// Queue select (0x30)
    pub queue_sel: u32,
    /// Queue max count (0x34)
    pub queue_num_max: u32,
    /// Queue count (0x38)
    pub queue_num: u32,
    /// _reserved (0x3C)
    pub(super) _reserved3: u32,
    /// _reserved (0x40)
    pub(super) _reserved4: u32,
    /// Queue ready (0x44) - Modern VirtIO Queue Enable
    pub queue_ready: u32,
    /// _reserved (0x48)
    pub(super) _reserved5: u32,
    /// _reserved (0x4C)
    pub(super) _reserved6: u32,
    /// Queue notify (0x50)
    pub queue_notify: u32,
    /// _reserved (0x54-0x5C)
    pub(super) _reserved7: [u32; 3],
    /// Interrupt status (0x60)
    pub interrupt_status: u32,
    /// Interrupt acknowledge (0x64)
    pub interrupt_ack: u32,
    /// _reserved (0x68-0x6C)
    pub(super) _reserved8: [u32; 2],
    /// Driver status (0x70)
    pub status: u32,
    /// _reserved (0x74+)
    pub(super) _reserved9: [u32; 4],
}

/// VirtIO block device
pub struct VirtIOBlkDevice {
    /// MMIO base address
    pub(super) base_addr: u64,
    /// Block device
    pub disk: GenDisk,
    /// Capacity (sectors)
    pub(super) capacity: u64,
    /// Block size
    pub(super) block_size: u32,
    /// Initialization status
    pub(super) initialized: Spinlock<bool>,
    /// VirtQueue (for I/O operations)
    pub(super) virtqueue: Spinlock<Option<VirtQueue>>,
    /// Queue size
    pub(super) queue_size: u16,
}

pub(super) struct IoBufFreelist {
    /// Parked-buffer chain head, 0 = empty. Stored as usize (address);
    /// the intrusive next pointer lives in the first 8 bytes of each
    /// parked buffer.
    pub(super) head: usize,
    pub(super) count: usize,
}

/// RAII guard for a per-slot BLK lock acquire: records holder forensics
/// on acquire and clears it on release. Behaviorally identical to
/// `PCI_BLK_LOCKS[slot].lock_irqsave()` (irqsave + preempt disable).
pub(crate) struct PciBlkGuard {
    pub(super) slot: usize,
    pub(super) inner: SpinlockIrqGuard<'static, ()>,
}

/// Pending async I/O request for MMIO VirtIO.
pub(super) struct PendingIo {
    /// Completion token to signal when done. NULL = TOMBSTONE: the entry
    /// exists only to keep the chain's descriptor window reserved until
    /// the completion walker consumes the chain's used-ring entry. Used
    /// for (a) synchronous chains (their waiter polls its own response
    /// byte and owns the io_buf) and (b) timed-out async chains whose
    /// waiter has unwound (the io_buf is deliberately leaked — the device
    /// may still DMA its status byte — and the completion must never be
    /// fired into memory its owner stopped owning).
    pub(super) completion: *mut crate::fs::types::IoCompletion,
    /// Pointer to response buffer (allocated during submit, freed on
    /// completion). NULL in tombstones (nothing to free or read).
    pub(super) resp_ptr: *mut u8,
    /// Layout of response buffer for deallocation.
    pub(super) resp_layout: alloc::alloc::Layout,
    /// Pointer to request header buffer (freed on completion). NULL in
    /// tombstones.
    pub(super) header_ptr: *mut u8,
    /// Layout of request header buffer for deallocation.
    pub(super) header_layout: alloc::alloc::Layout,
    /// Head descriptor id of this request's chain (PCI only): the key the
    /// completion walker matches against the used ring's UsedElem.id.
    /// A virtio-blk chain occupies the three consecutive descriptors
    /// [head_desc, head_desc+2]; the publish-time window check (see
    /// pci_pending_slot_reservable) guarantees this id is UNIQUE among live
    /// table entries, so a used-ring entry can never be misattributed.
    pub(super) head_desc: u32,
    /// Submission ordinal of this chain (PCI only): used-ring entry i maps
    /// to slot i % MAX_PENDING_IO_PCI, and the walker's positional fast
    /// path expects THIS entry there. Also the instrumentation key: a fired
    /// entry whose ordinal != its used-ring index proves the device
    /// completed out of submission order (VIRTIO_PCI_REORDER_EVENTS).
    pub(super) ordinal: u16,
    /// Tombstone bookkeeping: true when the chain's waiter has TIMED OUT
    /// (sync deadline or async abandon) and already ran note_timed_out_chain
    /// on the queue. When the walker finally consumes this tombstone's
    /// used-ring entry it calls resolve_leaked_chain(), pairing that
    /// increment — leaked_chains then tracks only chains whose completion
    /// has genuinely not landed yet, instead of accumulating forever.
    pub(super) timed_out: bool,
    /// DMA data-buffer pointer of this chain (async chains only): the
    /// device-truth token the completion walker stamps into
    /// `IoCompletion::complete_with_token`. `bread_wait` compares it
    /// against its own buffer — a mismatch means the walker delivered
    /// ANOTHER chain's completion and this buffer's DMA may not have
    /// landed (the zero-serve guard; the no-journal ext4 root-lookup
    /// family). 0 on tombstones (nothing fires through them).
    pub(super) data_ptr: usize,
    /// Direction of the chain (async submits only; false on every
    /// tombstone — trace_dev is 0 there so nothing reports): selects the
    /// rwbs column of the walker's block:block_rq_complete record.
    pub(super) is_write: bool,
    /// block:block_rq_complete geometry (dev as major<<20|minor, first
    /// sector, sector count), stored at async submit so the completion
    /// walker can record the event without any lookup. Zero on
    /// tombstones and every non-async chain (nothing to report).
    pub(super) trace_dev: u32,
    pub(super) trace_sector: u64,
    pub(super) trace_nr_sector: u32,
    /// Descriptors this chain occupies: [head_desc, head_desc+n_descs)
    /// mod queue_size. 3 for the classic header/data/resp chain, N+2 for
    /// an SG chain with N data segments. Drives the window-overlap check
    /// (pci_pending_slot_reservable) and the descs_held budget release in
    /// the completion walker.
    pub(super) n_descs: u16,
}

/// RAII marker armed around every virtio driver-lock critical section
/// (debug builds only). IoCompletion::complete() and the wait-queue wake
/// sites assert !virtio_lock_held() so any reintroduced nesting panics at
/// the violation point instead of deadlocking the machine.
#[cfg(debug_assertions)]
pub(crate) struct VirtioLockNest;

/// Compile-time no-op stand-in for release builds.
#[cfg(not(debug_assertions))]
pub(crate) struct VirtioLockNest;

/// VirtIO descriptor (16-byte aligned)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Desc {
    /// Address (64-bit)
    pub addr: u64,
    /// Length (32-bit)
    pub len: u32,
    /// Flags (16-bit)
    pub flags: u16,
    /// Next (16-bit)
    pub next: u16,
}

/// Available Ring (2-byte aligned)
#[repr(C)]
pub struct AvailRing {
    /// Flags
    pub flags: u16,
    /// Driver writes next available descriptor index (volatile read/write)
    pub idx: u16,
    // Descriptor index array starts here
    // Array followed by used_event_idx
}

/// Used Ring element (4-byte aligned)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsedElem {
    /// Descriptor index
    pub id: u32,
    /// Bytes written
    pub len: u32,
}

/// Used Ring (4-byte aligned)
#[repr(C)]
pub struct UsedRing {
    /// Flags
    pub flags: u16,
    /// Device writes next available descriptor index (volatile read/write)
    pub idx: u16,
    // Element array starts here
    // Array followed by avail_event_idx
}

/// Width of the queue-notify register write for this transport.
///
/// R34: virtio-mmio rejects every register access whose size != 4 (QEMU:
/// "wrong size access to register!" — the write is dropped), so MMIO
/// queues MUST notify with a 32-bit write. virtio-pci's notify cap accepts
/// sub-4-byte writes (Linux uses iowrite16 there), and the PCI block
/// driver has always used 16-bit notifies, so it keeps W16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyWidth {
    /// 16-bit notify write (virtio-pci notify capability)
    W16,
    /// 32-bit notify write (virtio-mmio QueueNotify register)
    W32,
}

/// VirtIO virtual queue
///
/// Uses Modern VirtIO (v1.0+) layout
///
/// Unsafe-slim round 3: the ring memory is a `safety::dma::DmaBlock`
/// (INV-DMA-1..3) carved once into the descriptor table, available
/// ring, and used ring; all ring/index access goes through the
/// block's bounds-checked volatile paths or the disjoint typed slots,
/// so the per-access unsafe blocks this file used to carry are gone.
/// The only remaining unsafe here is `notify()`'s arch fences and the
/// register write (inherently asm/MMIO), plus the Send/Sync impls.
pub struct VirtQueue {
    /// Queue size
    pub queue_size: u16,
    /// Queue index (used for notifying device)
    pub(super) queue_index: u16,
    /// Queue notification address
    pub(super) queue_notify: u64,
    /// Notify register access width for this transport
    pub(super) notify_width: NotifyWidth,
    /// Interrupt status address (VIRTIO_MMIO_INTERRUPT_STATUS - Read Only)
    pub(super) interrupt_status: u64,
    /// Interrupt acknowledge address (VIRTIO_MMIO_INTERRUPT_ACK - Write Only)
    pub(super) interrupt_ack: u64,
    /// The vring allocation (None after free_vring): one DMA block
    /// (INV-DMA-1) carved into desc/avail/used (INV-DMA-2).
    pub(super) vring: Option<Vring>,
    /// Next descriptor index to allocate
    pub(super) next_desc: AtomicU16,
    /// Chains whose wait timed out and whose descriptors are LEAKED (the
    /// request may still be in flight on the device). Leaked chains must
    /// not count against the in-flight guard forever, and their slots must
    /// never be handed out again — see note_timed_out_chain().
    pub(super) leaked_chains: AtomicU16,
    /// Kernel-side ordinal of the next avail-ring submission. submit()
    /// derives the ring slot and the published avail.idx from this shadow
    /// instead of reading the shared ring back — a corrupted avail.idx
    /// (stray kernel write into the heap-backed vring) previously made the
    /// device walk garbage heads and wedge permanently.
    pub(super) avail_shadow: AtomicU16,
    /// Descriptors currently held by LIVE published chains (the SG-chain
    /// admission budget). The legacy in-flight guard in alloc_desc_chain
    /// assumes 3 descriptors per chain; an SG chain of N data segments
    /// holds N+2, so chains-only accounting under-estimates real ring
    /// pressure exactly when fusing is busiest. Held is incremented at
    /// chain PUBLISH time (async pending store / sync tombstone publish,
    /// both under the BLK lock) and released by the completion walker when
    /// it consumes the chain's used-ring entry — the same lifecycle the
    /// pending-table window reservation follows. Burned allocations
    /// (guard rejected / publish refused before submit_quiet) never touch
    /// this counter. The MMIO legacy queue leaves it at 0 (its chains are
    /// always 3-desc; its size-8 in-flight guard already caps it at 2).
    pub(super) descs_held: AtomicU16,
}

/// The carved vring (INV-DMA-1/2): the block owns the allocation; the
/// slots are disjoint zero-drop views into it (block declared first
/// only for documentation — slot drops are no-ops).
pub(super) struct Vring {
    pub(super) block: crate::safety::dma::DmaBlock,
    /// Descriptor table [queue_size] (driver-owned; device reads).
    pub(super) desc: crate::safety::dma::DmaSlot<[Desc]>,
    /// Available ring header (driver-written).
    pub(super) avail: crate::safety::dma::DmaSlot<AvailRing>,
    /// Used ring header (device-written — read volatile only).
    pub(super) used: crate::safety::dma::DmaSlot<UsedRing>,
    /// Byte offsets of the carved sections (volatile-at offsets and
    /// register programming derive from these).
    pub(super) off_avail: usize,
    pub(super) off_used: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkReqHeader {
    /// Request type (0=read, 1=write, 2=flush)
    pub type_: u32,
    /// Reserved
    pub reserved: u32,
    /// Sector number
    pub sector: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkResp {
    /// Status (0=OK, 1=IOERR, 2=UNSUPPORTED)
    pub status: u8,
}

/// VirtIO PCI Capability types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtIOCapType {
    CommonCfg = 1,     // VIRTIO_PCI_CAP_COMMON_CFG
    NotifyCfg = 2,     // VIRTIO_PCI_CAP_NOTIFY_CFG
    IsrCfg = 3,        // VIRTIO_PCI_CAP_ISR_CFG
    DeviceCfg = 4,       // VIRTIO_PCI_CAP_DEVICE_CFG
    PciCfg = 5,        // VIRTIO_PCI_CAP_PCI_CFG
}

/// VirtIO PCI Capability structure
#[repr(C)]
#[derive(Debug)]
pub(super) struct VirtioPCICap {
    pub(super) cap_vndr: u8,   // Generic PCI field: PCI_CAP_ID_VNDR
    pub(super) cap_next: u8,    // Generic PCI field: next ptr
    pub(super) cap_len: u8,     // Generic PCI field: capability length
    pub(super) cfg_type: u8,    // Identifies the structure (VirtIOCapType)
    pub(super) bar: u8,         // Where to find it
    pub(super) id: u8,          // Multiple capabilities of same type
    pub(super) padding: [u8; 2], // Pad to full dword
    pub(super) offset: u32,     // Offset within bar (little-endian)
    pub(super) length: u32,     // Length of structure in bytes (little-endian)
}

/// VirtIO PCI Notify Capability structure (extended)
#[repr(C)]
#[derive(Debug)]
pub(super) struct VirtioPCINotifyCap {
    pub(super) cap: VirtioPCICap,
    pub(super) notify_off_multiplier: u32,  // Queue notification offset multiplier
}

/// VirtIO PCI device
pub struct VirtIOPCI {
    /// PCI configuration space
    pub pci_config: PCIConfig,
    /// PCI slot number (used for IRQ calculation)
    pub pci_slot: u8,
    /// Common CFG BAR base address
    pub common_cfg_bar: u64,
    /// Common CFG BAR offset
    pub common_cfg_offset: u32,
    /// Device CFG BAR base address
    pub device_cfg_bar: u64,
    /// Device CFG BAR offset
    pub device_cfg_offset: u32,
    /// Notify CFG BAR base address
    pub notify_cfg_bar: u64,
    /// Notify CFG BAR offset
    pub notify_cfg_offset: u32,
    /// Notify offset multiplier
    pub notify_off_multiplier: u32,
    /// ISR CFG BAR base address (critical for interrupt status reading)
    pub isr_cfg_bar: u64,
    /// ISR CFG BAR offset
    pub isr_cfg_offset: u32,
    /// Device base address
    pub base_addr: u64,
    /// virtio-blk disk slot this function drives (multi-disk OH Phase 1b:
    /// the I/O engine — queue, BLK lock, wait queue, pending table — is
    /// selected through this number). 0 until the probe assigns it.
    pub blk_slot: usize,
    /// DIO-CONC: kernel-half VA of the MSI-X table (capability id 0x11),
    /// 0 when the function has no MSI-X cap or its BAR could not be
    /// mapped. Set at construction; `enable_msix_vector0` programs entry
    /// 0 and switches completions onto the MSI vector.
    pub msix_table_va: u64,
    /// DIO-CONC: config-space offset of the MSI-X capability (0 = none).
    pub msix_cap_off: u8,
}

// ---------------------------------------------------------------------------
// the network device record and its operations, and the VirtIO-net
// config/header/device family
// ---------------------------------------------------------------------------

/// Maximum device name length
pub const IFNAMSIZ: usize = 16;

/// Maximum hardware address length
pub const MAX_ADDR_LEN: usize = 32;

/// ARP hardware types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrdType {
    /// Loopback device
    ARPHRD_LOOPBACK = 772,
    /// Ethernet
    ARPHRD_ETHER = 1,
    /// None
    ARPHRD_VOID = 0xFFFF,
}

/// Network device operation interface
#[repr(C)]
pub struct NetDeviceOps {
    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub xmit: fn(skb: SkBuff) -> i32,

    /// Device initialization (optional)
    pub init: Option<fn() -> i32>,

    /// Device teardown (optional)
    pub uninit: Option<fn() -> i32>,

    /// Get statistics (optional)
    pub get_stats: Option<fn() -> DeviceStats>,
}

/// Network device statistics
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceStats {
    /// Received packets
    pub rx_packets: u64,
    /// Transmitted packets
    pub tx_packets: u64,
    /// Received bytes
    pub rx_bytes: u64,
    /// Transmitted bytes
    pub tx_bytes: u64,
    /// Receive errors
    pub rx_errors: u64,
    /// Transmit errors
    pub tx_errors: u64,
    /// Receive dropped
    pub rx_dropped: u64,
    /// Transmit dropped
    pub tx_dropped: u64,
    /// Multicast received packets
    pub multicast: u64,
}

/// Network device
///
/// # Notes
/// - All network devices must implement this structure
/// - Uses function pointers for polymorphic calls
#[repr(C)]
pub struct NetDevice {
    /// Device name (e.g., "lo", "eth0")
    pub name: [u8; IFNAMSIZ],
    /// Device index
    pub ifindex: u32,
    /// MTU (maximum transmission unit)
    pub mtu: u32,
    /// Hardware type (ARPHRD_ETHER, ARPHRD_LOOPBACK, etc.)
    pub type_: ArpHrdType,
    /// Hardware address (MAC address)
    pub addr: [u8; MAX_ADDR_LEN],
    /// Hardware address length
    pub addr_len: u8,
    /// Device operation interface
    pub netdev_ops: &'static NetDeviceOps,
    /// Private data
    pub priv_: *mut u8,
    /// Statistics
    pub stats: DeviceStats,
    /// Device status
    pub flags: u32,
    /// Receive queue length
    pub rx_queue_len: u32,
}

/// VirtIO network device configuration
///
/// Corresponds to VirtIO network device configuration space
#[repr(C)]
pub struct VirtIONetConfig {
    /// MAC address
    pub mac: [u8; 6],
    /// Device status
    pub status: u16,
    /// Maximum VIRTIO packet size
    pub mtu: u16,
}

/// VirtIO network packet header
///
/// Corresponds to VirtIO network device packet header format
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIONetHdr {
    /// Flags
    pub flags: u8,
    /// GSO type
    pub gso_type: u8,
    /// Header length
    pub hdr_len: u16,
    /// GSO size
    pub gso_size: u16,
    /// Checksum start position
    pub csum_start: u16,
    /// Checksum offset
    pub csum_offset: u16,
    /// Buffer count
    pub num_buffers: u16,
}

/// VirtIO network device
pub struct VirtIONetDevice {
    /// MMIO base address
    pub(super) base_addr: u64,
    /// MAC address
    pub(super) mac: [u8; 6],
    /// MTU
    pub(super) mtu: u16,
    /// Initialization status
    pub(super) initialized: Spinlock<bool>,
    /// Transmit queue (TX Queue - Queue 0)
    pub(super) tx_queue: Spinlock<Option<VirtQueue>>,
    /// Receive queue (RX Queue - Queue 1)
    pub(super) rx_queue: Spinlock<Option<VirtQueue>>,
    /// Queue size
    pub(super) queue_size: u16,
    /// Statistics
    pub(super) stats: Spinlock<DeviceStats>,
    /// RX buffer address list
    pub(super) rx_buffers: Spinlock<alloc::vec::Vec<u64>>,
    /// Last processed RX used index
    pub(super) rx_last_used: Spinlock<u16>,
}

/// Global VirtIO network device
/// Exclusivity witness for the eth0 slots (INV-SLOT-3): publication and
/// the register-time fill run in single-threaded boot; post-init
/// mutation of NetDevice (stats, xmit bookkeeping) is serialized by the
/// NetRx softirq / syscall discipline the poll and ioctl paths already
/// document — the same serialization the raw static mut relied on.
pub(super) struct Eth0Phase;

// ---------------------------------------------------------------------------
// the framebuffer info/record and the VirtIO-GPU device and command
// headers
// ---------------------------------------------------------------------------

/// Framebuffer information
#[derive(Clone, Copy)]
pub struct FrameBufferInfo {
    /// Framebuffer physical address
    pub addr: u64,
    /// Framebuffer size (bytes)
    pub size: u32,
    /// Width (pixels)
    pub width: u32,
    /// Height (pixels)
    pub height: u32,
    /// Bytes per row
    pub stride: u32,
    /// Format (xRGB = 1)
    pub format: u32,
}

/// Framebuffer structure
pub struct FrameBuffer {
    /// Framebuffer information
    pub(super) info: FrameBufferInfo,
    /// The mapped framebuffer window (round-5: the frame aperture as a
    /// bounds-checked MmioRegion — INV-MMIO-1/2/3 cover every pixel
    /// access; the old raw ptr + offset math is gone).
    pub(super) fb: crate::safety::mmio::MmioRegion,
}

/// VirtIO-GPU device
pub struct VirtioGpuDevice {
    /// VirtIO PCI device
    pub(super) pci: VirtIOPCI,
    /// Control queue
    pub(super) ctrl_queue: Option<VirtQueue>,
    /// Serializes control-queue descriptor/ring mutation.
    ///
    /// send_command hardcodes descriptors 0/1 and bumps the avail index
    /// in place, so two concurrent submissions would overwrite each
    /// other's descriptors mid-flight. Today the only externally reachable
    /// path (flush_framebuffer) happens to serialize through the
    /// GPU_DEVICE spinlock, but that invariant lives in another module —
    /// this lock makes it local and future-proof (review RACE:
    /// "virtio-gpu desc 无锁").
    pub(super) cmd_lock: crate::sync::types::Spinlock<()>,
    /// Framebuffer information
    pub(super) fb_info: Option<FrameBufferInfo>,
    /// Framebuffer pointer
    pub(super) fb_ptr: *mut u8,
    /// Framebuffer layout
    pub(super) fb_layout: Option<Layout>,
    /// Resource ID
    pub(super) resource_id: u32,
    /// Display rectangle
    pub(super) display_rect: Rect,
}

/// VirtIO-GPU command header (24 bytes)
#[repr(C)]
pub(super) struct GpuCtrlHeader {
    pub(super) hdr_type: u32,
    pub(super) flags: u32,
    pub(super) fence_id: u64,
    pub(super) ctx_id: u32,
    pub(super) padding: u32,
}

/// Rectangle structure (16 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Rect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// Single display output configuration (24 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DisplayPmode {
    pub(super) rect: Rect,
    pub(super) enabled: u32,
    pub(super) flags: u32,
}

/// GET_DISPLAY_INFO response
/// VirtIO 1.0 specification: header + 16 pmode entries (no num_scanouts)
#[repr(C)]
pub(super) struct RespDisplayInfo {
    pub(super) header: GpuCtrlHeader,
    pub(super) pmodes: [DisplayPmode; 16],
}

/// RESOURCE_CREATE_2D command (32 bytes)
#[repr(C)]
pub(super) struct CmdResourceCreate2d {
    pub(super) header: GpuCtrlHeader,
    pub(super) resource_id: u32,
    pub(super) format: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// SET_SCANOUT command (48 bytes)
/// VirtIO 1.2 specification: header(24) + rect(16) + scanout_id(4) + resource_id(4)
#[repr(C)]
pub(super) struct CmdSetScanout {
    pub(super) header: GpuCtrlHeader,
    pub(super) rect: Rect,
    pub(super) scanout_id: u32,
    pub(super) resource_id: u32,
}

/// Memory entry (16 bytes)
#[repr(C)]
pub(super) struct MemEntry {
    pub(super) addr: u64,
    pub(super) length: u32,
    pub(super) padding: u32,
}

/// RESOURCE_ATTACH_BACKING command (48 bytes)
#[repr(C)]
pub(super) struct CmdResourceAttachBacking {
    pub(super) header: GpuCtrlHeader,
    pub(super) resource_id: u32,
    pub(super) nr_entries: u32,
    pub(super) entry: MemEntry,
}

/// RESOURCE_FLUSH command (48 bytes)
#[repr(C)]
pub(super) struct CmdResourceFlush {
    pub(super) header: GpuCtrlHeader,
    pub(super) rect: Rect,           // rect BEFORE resource_id!
    pub(super) resource_id: u32,
    pub(super) padding: u32,
}

/// TRANSFER_TO_HOST_2D command (56 bytes)
/// VirtIO 1.2 specification: header(24) + rect(16) + offset(8) + resource_id(4) + padding(4)
#[repr(C)]
pub(super) struct CmdTransferToHost2d {
    pub(super) header: GpuCtrlHeader,
    pub(super) rect: Rect,
    pub(super) offset: u64,
    pub(super) resource_id: u32,
    pub(super) padding: u32,
}

/// Generic response (24 bytes)
#[repr(C)]
pub(super) struct RespNoData {
    pub(super) header: GpuCtrlHeader,
}

// ---------------------------------------------------------------------------
// the input family: evdev device and uapi records, the raw InputEvent,
// the PS/2 state, the VirtIO-input device
// ---------------------------------------------------------------------------

/// Input device ID (input_id)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputId {
    /// Bus type
    pub bustype: u16,
    /// Vendor ID
    pub vendor: u16,
    /// Product ID
    pub product: u16,
    /// Version
    pub version: u16,
}

/// input_absinfo (uapi layout): current value + the five virtio_absinfo
/// fields.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputAbsinfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

/// KEY_CNT = 768 bits = 96 bytes; uniform storage for every event type's
/// code bitmap (SYN/REL/ABS use far less).
pub(super) const BITS_LEN: usize = 96;
/// Number of event types we carry code bitmaps for: EV_SYN..EV_ABS.
pub(super) const BIT_TYPES: usize = 4;

/// evdev device structure
pub struct EvdevDevice {
    /// Device name
    pub name: [u8; 32],
    /// The file ids of the open file descriptions for this device.
    /// epoll notification targets a FILE (File.file_id — a per-open
    /// generation counter, NOT the device number), so the device has to
    /// remember which files are open on it: an epoll-based reader that
    /// opened this node is only re-armed when the event notification
    /// carries ITS file id.
    pub open_file_ids: Spinlock<[u64; 4]>,
    /// Number of valid entries in `open_file_ids`.
    pub open_file_count: core::sync::atomic::AtomicU32,
    /// Events pushed since boot (rux:inputtrace (retired debug=inputtrace) counter).
    pub events_pushed: core::sync::atomic::AtomicU32,
    /// Device ID
    pub id: InputId,
    /// Whether it is a pointer device
    pub is_pointer: bool,
    /// Code bitmaps for EV_SYN(0)/EV_KEY(1)/EV_REL(2)/EV_ABS(3), read from
    /// the virtio device config (VIRTIO_INPUT_CFG_EV_BITS) at init.
    pub(super) ev_bits: [[u8; BITS_LEN]; BIT_TYPES],
    /// ABS axis info (min/max/fuzz/flat/res from VIRTIO_INPUT_CFG_ABS_INFO)
    /// — reported through EVIOCGABS. Slots: 0=ABS_X, 1=ABS_Y,
    /// 2=ABS_MT_POSITION_X(0x35), 3=ABS_MT_POSITION_Y(0x36). A device that
    /// ADVERTISES an MT axis must also answer a real range for it —
    /// libinput rejects whole devices whose MT axes report minimum==
    /// maximum (the silent device-drop face: probed, never read).
    pub(super) absinfo: [InputAbsinfo; 4],
    /// EVIOCGRAB state (device-level; we have a single client in practice).
    pub(super) grabbed: AtomicBool,
    /// LED state bitmap (bit N = LED_N), maintained from EV_LED writes the
    /// way Linux's evdev_write → input_inject_event does; readable back via
    /// EVIOCGLED(len).
    pub(super) led_state: AtomicU32,
    /// Event queue
    pub event_queue: Spinlock<VecDeque<InputEvent>>,
    /// Poll/select waiters (P3: event-driven poll). Woken in push_event so
    /// a waiter blocked in poll() on this device sleeps until a real input
    /// event arrives instead of waking at tick rate to re-check.
    pub poll_waitq: crate::process::types::WaitQueueHead,
}

pub struct HuntLine {
    pub(super) buf: [u8; 192],
    pub(super) n: usize,
}

// obs-hygiene: the INRD/INOP echo lines were the remaining per-byte
// writers of the input hunt family — stage them per CPU and flush whole
// lines through the same emit-lock-coordinated write the HuntLine
// bursts take, so a kmsg-flood record can no longer splice an INRD line
// mid-flight (the dhardware "missing lines" class), and two CPUs'
// echo lines cannot byte-mix (per-CPU stage — the taskdump DUMP_STAGES
// lesson: a shared stage locked per byte still mixes concurrent
// writers inside one flushed line).
pub(super) struct IttStage {
    pub(super) buf: [u8; 128],
    pub(super) n: usize,
}

/// Raw input event (evdev compatible, 24 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct InputEvent {
    /// Timestamp (seconds)
    pub tv_sec: u64,
    /// Timestamp (microseconds)
    pub tv_usec: u64,
    /// Event type (EV_KEY, EV_REL, EV_ABS)
    pub type_: u16,
    /// Event code (keycode/axis)
    pub code: u16,
    /// Event value (press/release/coordinate value)
    pub value: i32,
}

/// Input event type (for internal use)
#[derive(Debug, Clone, Copy)]
pub enum InputEventKind {
    /// Keyboard event
    Key { code: u16, pressed: bool },
    /// Relative motion event
    RelativeMotion { dx: i32, dy: i32 },
    /// Absolute position event
    AbsolutePosition { x: i32, y: i32 },
    /// Mouse wheel
    Wheel { delta: i32 },
}

/// PS/2 keyboard state
pub struct PS2Keyboard {
    pub(super) shift_pressed: bool,
    pub(super) ctrl_pressed: bool,
    pub(super) alt_pressed: bool,
}

/// PS/2 mouse state
pub struct PS2Mouse {
    pub(super) packet_index: u8,
    pub(super) packet: [u8; 3],
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) left_pressed: bool,
    pub(super) right_pressed: bool,
    pub(super) middle_pressed: bool,
}

/// VirtIO Input configuration registers
#[repr(C)]
pub(super) struct VirtioInputConfig {
    /// Configuration select register
    pub(super) select: u8,
    /// Sub-select register
    pub(super) subsel: u8,
    /// Data size
    pub(super) size: u8,
    /// Reserved
    pub(super) reserved: [u8; 5],
    /// Configuration data (union)
    pub(super) payload: [u8; 128],
}

/// VirtIO Input event (8 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioInputEvent {
    /// Event type
    pub type_: u16,
    /// Event code
    pub code: u16,
    /// Event value
    pub value: i32,
}

/// VirtIO Input device
pub struct VirtioInputDevice {
    /// VirtIO PCI device
    pub(super) pci: VirtIOPCI,
    /// Event queue
    pub(super) event_queue: Option<VirtQueue>,
    /// Event buffer
    pub(super) event_buffer: *mut VirtioInputEvent,
    /// Event buffer layout
    pub(super) event_buffer_layout: Option<Layout>,
    /// Event buffer physical address
    pub(super) event_buffer_phys: u64,
    /// Device name
    pub(super) name: [u8; 32],
    /// Whether it is a pointer device (mouse/touchscreen)
    pub(super) is_pointer: bool,
    /// Last processed used index
    pub(super) last_used: u16,
}

/// Used ring element
#[repr(C)]
pub(super) struct VirtioInputUsedElem {
    pub(super) id: u32,
    pub(super) len: u32,
}

// ---------------------------------------------------------------------------
// the loop device record, the loop_info64 wire-size constant run and
// the LoopStatus view
// ---------------------------------------------------------------------------

pub(super) struct LoopDev {
    /// Backing file (LOOP_SET_FD pins an Arc reference).
    pub(super) file: Spinlock<Option<Arc<File>>>,
    /// Data start offset inside the backing file (bytes).
    pub(super) offset: AtomicU64,
    /// Max bytes exposed (0 = whole file from offset).
    pub(super) sizelimit: AtomicU64,
    /// lo_flags mirror (LO_FLAGS_* — display/detach policy only).
    pub(super) flags: AtomicU64,
    /// Backing path for LOOP_GET_STATUS(64) lo_file_name/lo_name.
    pub(super) name: Spinlock<[u8; 64]>,
    /// Serializes the set_pos/read/write dance on the backing file.
    /// SleepMutex (falloc-wedge lane, 2026-11-16; INV-LOCK-8 rule 3: a
    /// lock whose holder can block must not be a spinlock): the holder
    /// runs a whole backing-file read/write — journal commits, block
    /// I/O waits, and under memory pressure even direct reclaim — which
    /// sleeps for milliseconds at a time. As a Spinlock that was a
    /// preempt-disabled section across all of it: every loop request was
    /// an unpreemptible monopolist window (the falloc-wedge boots
    /// measured pid 352 at preempt=2 across hundreds of ticks), RCU
    /// quiescent rounds stalled on the CPU, and fdreap's grace waits
    /// deferred (the fdreap RCU stall face). Waiters now park on the
    /// mutex's queue; the section is preemptible.
    pub(super) io_lock: crate::sync::types::SleepMutex,
    /// Re-entrancy guard (pcachewedge lane, 2026-11-15): the pid whose
    /// request currently owns `io_lock` (0 = none). The backing-file
    /// write below runs SYNCHRONOUSLY under the lock; under memory
    /// pressure its allocations enter direct reclaim, whose writeback
    /// flushes dirty pages of filesystems ON THIS SAME LOOP — the bio
    /// re-enters loop_request_fn and re-acquires io_lock it already
    /// holds (the recursive request would park forever on the now-
    /// sleeping mutex it already holds).
    /// The guard makes the recursive request FAIL instead: the failed
    /// write keeps its pages dirty (flush_done's write_ok=false
    /// discipline) for a later, non-recursive flush — and reclaim-side
    /// writeback now degrades to clean-only while ANY loop claim is held
    /// by the current task (see shrink_reclaim), so the reference
    /// discipline holds: no synchronous writeback under a held io lock.
    pub(super) io_owner_pid: AtomicU32,
    /// The GenDisk handed out to the block layer (stable address: boxed;
    /// written once at init — the round-5 D-7 conversion makes every
    /// LoopDev field interior-mutable, so LOOPS is a plain `static`).
    pub(super) disk: Spinlock<Option<&'static GenDisk>>,
}

/// `struct loop_info64` wire size (linux/loop.h): 6*u64 + 4*u32 +
/// 64 + 64 + 32 + 2*u64 = 232 bytes. The old value (168) truncated
/// lo_crypt_name/lo_encrypt_key/lo_init — consumers (util-linux losetup,
/// LTP loop tests) read the tail fields.
pub(super) const LOOP_INFO64_SIZE: usize = 232;
/// `struct loop_info` (legacy) wire size on LP64: int +
/// __kernel_old_dev_t (u32 on riscv64 — its UAPI defines old dev_t as
/// unsigned int, verified by offsetof probes against the musl headers
/// our userspace builds with) + u64 + u32 + 4*int + 64 + 32 + 2*u64 + 4,
/// padded to 160.
pub(super) const LOOP_INFO_SIZE: usize = 160;
/// Legacy struct loop_info field offsets (LP64 + 32-bit old_dev_t).
pub(super) const LI_OFF_NUMBER: usize = 0;
pub(super) const LI_OFF_DEVICE: usize = 4;
pub(super) const LI_OFF_INODE: usize = 8;
pub(super) const LI_OFF_RDEVICE: usize = 16;
pub(super) const LI_OFF_OFFSET: usize = 20;
pub(super) const LI_OFF_ENCRYPT_TYPE: usize = 24;
pub(super) const LI_OFF_KEY_SIZE: usize = 28;
pub(super) const LI_OFF_FLAGS: usize = 32;
pub(super) const LI_OFF_NAME: usize = 36;
pub(super) const LI_OFF_INIT: usize = 132;
/// LO_NAME_SIZE.
pub(super) const LO_NAME_SIZE: usize = 64;

/// Filled view of a loop's status, shared by the legacy/64 serializers.
pub(super) struct LoopStatus {
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) rdevice: u64,
    pub(super) offset: u64,
    pub(super) sizelimit: u64,
    pub(super) number: u32,
    pub(super) encrypt_type: u32,
    pub(super) encrypt_key_size: u32,
    pub(super) flags: u32,
    pub(super) name: [u8; LO_NAME_SIZE],
    pub(super) init: [u64; 2],
}

// ---------------------------------------------------------------------------
// the ashmem area and its UAPI records
// ---------------------------------------------------------------------------

/// ASHMEM_NAME_LEN (UAPI).
pub const ASHMEM_NAME_LEN: usize = 256;

/// struct ashmem_pin { u32 offset; u32 len; } (8 bytes on every ABI).
/// All-POD wire fields — Copy for the typed uaccess capsule.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct AshmemPin {
    pub(super) offset: u32,
    pub(super) len: u32,
}

/// Physical page pool backing one ashmem area (SysV ShmPages discipline).
pub(super) struct AshmemPages {
    pub(super) pages: alloc::vec::Vec<usize>,
}

/// One unpinned page range [pgstart, pgend] (inclusive page indices).
#[derive(Clone, Copy)]
pub(super) struct UnpinRange {
    pub(super) pgstart: usize,
    pub(super) pgend: usize,
    pub(super) purged: u32,
}

/// Per-open-file-description ashmem area (Linux struct ashmem_area).
pub struct AshmemArea {
    pub(super) inner: Spinlock<AreaInner>,
}

pub(super) struct AreaInner {
    /// Advisory name (NUL-terminated, at most ASHMEM_NAME_LEN bytes).
    pub(super) name: [u8; ASHMEM_NAME_LEN],
    /// True once a custom name was set via ASHMEM_SET_NAME.
    pub(super) name_set: bool,
    /// Size in bytes, frozen once the pool exists.
    pub(super) size: usize,
    /// Allowed PROT_* bits, starts full, can only narrow.
    pub(super) prot_mask: u32,
    /// Backing pages, created at first mmap.
    pub(super) pages: Option<AshmemPages>,
    /// Sorted, non-overlapping unpinned ranges.
    pub(super) unpinned: alloc::vec::Vec<UnpinRange>,
}

// ---------------------------------------------------------------------------
// the device-init levels and the probe record
// ---------------------------------------------------------------------------

/// Initcall levels, drained in this order (the reference's ladder).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InitLevel {
    /// Pure MMIO device scans that must precede PCI (virtio-mmio).
    Early,
    /// PCI device probes.
    Device,
    /// Network + late peripherals (input, gpu, drm, watchdog).
    Late,
}

/// One registered device probe. `probe` returns the number of devices
/// it bound (0 = negative probe: no such device on this machine — a
/// normal outcome, reported but never fatal).
pub struct DeviceInit {
    /// Stable name for the boot report (e.g. "virtio-blk-mmio").
    pub name: &'static str,
    pub level: InitLevel,
    /// fn() -> bound-device count. Must be re-entrancy-safe to CALL once;
    /// the table is drained exactly once per boot per level.
    pub probe: fn() -> usize,
}

// ---------------------------------------------------------------------------
// the DRM/GEM family: property, mode, dumb buffer, fb and state
// ---------------------------------------------------------------------------

/// One (name, flags, enums, value, values[]) description of a property.
#[derive(Clone, Copy)]
pub(super) struct PropDesc {
    pub(super) prop_id: u32,
    pub(super) name: &'static str,
    pub(super) flags: u32,
    /// (enum-name, enum-value) pairs for ENUM/BITMASK props.
    pub(super) enums: &'static [(&'static str, u64)],
    /// Current value of the property on its owning object.
    pub(super) value: u64,
    /// For RANGE props: [min, max]; for ENUM props: the enum values
    /// (count == enums.len()).
    pub(super) values: &'static [u64],
}

/// The single display mode exposed on the connector. Struct layout is
/// uapi struct drm_mode_modeinfo (68 bytes: the horizontal/vertical
/// fields are __u16 — the old "72" here overran libdrm's exactly-sized
/// modeinfo array by 4 bytes, corrupting the caller's heap: the OH
/// composer devhost took a user-mode #GP in ld.so right after its second
/// GETCONNECTOR and died (r9, "RegHotPlugCallback_ failed 29189" =
/// IPC ServiceDied — every later screen/hotplug registration lost its
/// service).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DrmModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub type_: u32,
    pub name: [u8; 32],
}

/// A dumb-buffer backing store. Kernel-heap allocated (physically
/// contiguous — same discipline as the virtio-gpu framebuffer), mapped
/// to userspace by the mmap path and scanned out through its own virtio
/// 2D resource once flipped.
pub struct GemObject {
    pub id: u32,
    /// Kernel virtual address of the backing (linear map).
    pub kva: *mut u8,
    /// Physical address (what virtio ATTACH_BACKING wants).
    pub phys: u64,
    pub size: usize,
    pub(super) layout: Layout,
    /// virtio-gpu 2D resource bound to this object (0 = none). Set on
    /// first scanout.
    pub(super) virtio_res: AtomicU32,

}

pub(super) struct FbObject {
    pub(super) _gem: Arc<GemObject>,
    pub(super) width: u32,
    pub(super) height: u32,
    /// Row pitch in bytes (composer GEMs carry gralloc's 64-pixel
    /// alignment: 720-wide content on a 3072-byte pitch).
    pub(super) pitch: u32,
    /// virtio_gpu format id the GEM bytes are laid out as.
    pub(super) virtio_format: u32,
}

pub(super) struct DrmState {
    pub(super) gems: BTreeMap<u32, Arc<GemObject>>,
    pub(super) fbs: BTreeMap<u32, FbObject>,
    pub(super) blobs: BTreeMap<u32, Vec<u8>>,
    /// virtio resource currently bound to scanout 0 (0 = the boot fbdev
    /// resource owned by the fbdev driver).
    pub(super) scanout_res: u32,
}
