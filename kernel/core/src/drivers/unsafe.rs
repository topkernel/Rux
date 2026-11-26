//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The drivers' audited unsafe edges (framework zone).
//!
//! Pass 3 (seed-core §2c.5) collects every inline unsafe region of
//! drivers/hotpath.rs — including the inline timer/intc modules — into
//! this file, one audited contract per edge. hotpath.rs keeps the
//! policy/flow code and calls these edges; every wrapper carries the
//! contract its census row cited (or an explicit GAP note where no
//! harness exists yet). Arch-specific edges keep the `#[cfg(feature)]`
//! attribute their hotpath caller sites sit behind.
//!
//! Contract registry (mirrors docs/development/unsafe-verification.md):
//! - INV-MMIO-1         — device/register windows are identity-mapped or
//!   MSR-backed before first use; the volatile access shape is fixed.
//! - F5                 — arch timer/CSR/SBI programming floor (per-arch
//!   edges, cfg-gated exactly like their hotpath callers).
//! - INV-BLKDEV-1/2/3   — gendisk registry stability, io_buf pool layout,
//!   driver-private slot discipline.
//! - INV-SAFECALL-1     — the R17-C io_buf carve + pending completion
//!   protocol edges.
//! - F7                 — the PCI pending-table access discipline (the
//!   caller holds the disk's BLK lock; no second acquisition).
//! - INV-DMA-2/3        — vring slot disjointness / device-completed
//!   memory reads.
//! - INV-KPTR-3, INV-SLOT-1, INV-SREF-1, INV-CHARDEV-PRIV-1, INV-KTASK-1,
//!   INV-FS-FILEPRIV-1, F9 — per-area contracts cited at each edge.

use crate::fs::types::{BufferHead, DevNo};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ptr::read_volatile;
use super::types::{
    AvailRing, BlockDeviceManager, Desc, FrameBuffer, GemObject, GenDisk,
    NetDevice, NotifyWidth, PendingIo, UsedRing, VirtIONetDevice,
    VirtIOBlkDevice, VirtIOBlkReqHeader, VirtIOBlkResp, VirtioGpuDevice,
    VirtioInputDevice, VirtioInputEvent, VirtQueue,
};
use super::hotpath::{IO_BUF_LAYOUT, MAX_PENDING_IO_PCI, PCI_BLK_PENDING};
use super::hotpath::{INPUT_BH_TASKLET, input_bh_run};
#[cfg(feature = "x86_64")]
use super::hotpath::intc::apic::{
    APIC_BASE_ENABLE, APIC_BASE_X2APIC, KVM_HC_SEND_IPI, KVM_HC_VMMCALL,
    KVM_PV_IPI, MSR_IA32_APIC_BASE, X2APIC, X2APIC_ICR, X2APIC_ID, X2APIC_SVR,
};
#[cfg(feature = "x86_64")]
use super::hotpath::timer::x86_64::{PIT_CH0_DATA, PIT_CH0_MODE3_BIN, PIT_MODE_CMD};
#[cfg(feature = "x86_64")]
use crate::arch::cpu::{io_wait, outb, rdmsr, wrmsr};
#[cfg(feature = "aarch64")]
use super::hotpath::intc::gicv3::{find_my_rd_base, redist_configure_local};
#[cfg(feature = "x86_64")]
use core::sync::atomic::Ordering;
#[cfg(any(feature = "aarch64", feature = "riscv64"))]
use core::arch::asm;

// ==========================================================================
// pass 3 (seed-core §2c.5): drivers's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ drm: GEM-object edges (F9) ------------

/// Reclaim the dma-buf file's Box<Arc<GemObject>> at close.
///
/// # Contract (F9)
/// F9: the Box<Arc<GemObject>> the export stored is the sole
/// owning reference; close retires it (the Arc may keep the
/// object alive for other holders).
#[inline(always)]
pub(crate) fn gem_arc_box_take(p: *mut Arc<GemObject>) -> alloc::boxed::Box<Arc<GemObject>> {
unsafe { alloc::boxed::Box::from_raw(p as *mut Arc<GemObject>) }
}

/// Clone the Arc behind a dma-buf file's private slot.
///
/// # Contract (F9)
/// F9: the slot holds Box<Arc<GemObject>> for the file's
/// lifetime; the clone hands out a new owning reference.
#[inline(always)]
pub(crate) fn gem_arc_clone(p: *const Arc<GemObject>) -> Arc<GemObject> {
unsafe { (*(p as *const Arc<GemObject>)).clone() }
}

/// Byte view of a u32 slice (GEM dump formatting).
///
/// # Contract (F9)
/// F9: [u32] and its byte image have identical size and
/// alignment; the bytes alias the same allocation.
#[inline(always)]
pub(crate) fn u32_slice_as_u8<'a>(v: &[u32]) -> &'a [u8] {
unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) }
}

/// Diagnostic volatile scan of an imported GEM buffer.
///
/// # Contract (F9)
/// F9: `gem` is a live imported object (its pages are pinned
/// for the machine's lifetime); the reads are volatile and
/// the results land in the caller's counters/samples.
#[inline(always)]
pub(crate) fn drm_scan_screen(gem: &GemObject, nz: &mut usize, clear: &mut usize, samples: &mut [u32; 3]) {
            unsafe {
                let base = gem.kva as *const u32;
                let words = gem.size / 4;
                for i in 0..words {
                    let v = base.add(i).read_volatile();
                    if v != 0 {
                        *nz += 1;
                        if v == 0xff00_0000 {
                            *clear += 1;
                        }
                    }
                }
                let row_words = 983040 / 1280; // pitch/4 for these buffers
                let s = |off: usize| -> u32 {
                    if off < words {
                        base.add(off).read_volatile()
                    } else {
                        0
                    }
                };
                samples[0] = s(row_words * 640 + 360);
                samples[1] = s(row_words * 640 + 200);
                samples[2] = s(row_words * 300 + 360);
            }
}

/// Diagnostic volatile scan of a presented framebuffer.
///
/// # Contract (GAP)
/// Ledger GAP: `gem` is the live backing object of the
/// presented fb; the reads are volatile diagnostics that
/// feed the caller's counters/samples.
#[inline(always)]
pub(crate) fn drm_scan_present(gem: &GemObject, nz: &mut usize, paint: &mut usize, samples: &mut [u32; 4], pitch: u32, width: u32, height: u32) {
        unsafe {
            let base = gem.kva as *const u32;
            let words = gem.size / 4;
            for i in 0..words {
                let v = base.add(i).read_volatile();
                if v != 0 {
                    *nz += 1;
                }
            }
            let row_words = (pitch / 4) as usize;
            let vis_words = width as usize;
            for row in 0..height as usize {
                let rowp = base.add(row * row_words);
                for col in (0..vis_words).step_by(2) {
                    let v = rowp.add(col).read_volatile();
                    if v != 0 && v != 0xff00_0000 {
                        *paint += 1;
                    }
                }
            }
            let mid_row = (height as usize / 2) * row_words;
            let last_row = (height as usize - 1).saturating_mul(row_words);
            let s = |off: usize| -> u32 {
                if off < words {
                    base.add(off).read_volatile()
                } else {
                    0
                }
            };
            samples[0] = s(720 / 4);
            samples[1] = s(row_words + 720 / 4);
            samples[2] = s(mid_row + (width as usize / 2));
            samples[3] = s(last_row + 720 / 4);
        }
}


// ------------ auto-trait proofs (Send / Sync) ------------

/// INV-BLKDEV-1 (ledger row GAP): a registered GenDisk is boxed once into the manager's Vec and never removed — the Box outlives every shared view handed out of the registry.
unsafe impl Send for GenDisk {}
/// INV-BLKDEV-1 (ledger row GAP): GenDisk state is manager-owned and its per-disk fields are only written at init before registration.
unsafe impl Sync for GenDisk {}
/// INV-BLKDEV-1 (ledger row GAP): the manager's registry is Spinlock-guarded; no thread holds a &mut view outside the lock.
unsafe impl Send for BlockDeviceManager {}
/// INV-BLKDEV-1 (ledger row GAP): every field access goes through the manager's Spinlock; shared &self views are read-only.
unsafe impl Sync for BlockDeviceManager {}
/// Ledger GAP: VirtIOBlkDevice is only accessed from kernel context; internal Spinlocks serialize all mutable access to shared fields.
unsafe impl Send for VirtIOBlkDevice {}
// SAFETY: All shared state is protected by Spinlocks (irqsafe where needed),
// ensuring no data races across threads/CPUs.
/// Ledger GAP: all shared state is protected by Spinlocks (irqsafe where needed), so shared &self views see a consistent state.
unsafe impl Sync for VirtIOBlkDevice {}
// SAFETY: PendingIo is stored in a Spinlock-protected table and only accessed
// from IRQ/softirq context; raw pointers within are not shared across threads.
/// F7 family (ledger row GAP): PendingIo lives in a Spinlock- protected table and is only touched from IRQ/softirq context; its raw pointers are not shared across threads.
unsafe impl Send for PendingIo {}
/// Ledger GAP: a VirtQueue is owned by one device instance; every shared view sits behind the device's own lock discipline.
unsafe impl Send for VirtQueue {}
/// Ledger GAP: the queue's ring memory is only written by the driver under the device lock or by the device through DMA (INV-DMA-2/3).
unsafe impl Sync for VirtQueue {}
/// Ledger GAP: the net::space section banner above this impl stayed in hotpath.rs; the type has no cross-thread &mut sharing.
unsafe impl Send for NetDevice {}
// SAFETY (round-5): NetDevice's cross-thread state (stats, flags) is
// mutated under the NetRx softirq / syscall discipline each op
// documents; the priv_ raw slot is driver-owned and set before
// registration. Sync was implicit in the old `static mut` usage.
/// Round-5: NetDevice's cross-thread state (stats, flags) is mutated under the NetRx softirq / syscall discipline each op documents; the priv_ raw slot is driver-owned and set before registration.
unsafe impl Sync for NetDevice {}
/// Ledger GAP: all shared state is protected by Spinlocks (irqsave where needed); the device is confined to the net driver.
unsafe impl Send for VirtIONetDevice {}
/// Ledger GAP: a FrameBuffer owns its fb_ptr allocation outright (allocated in init_framebuffer, freed in Drop).
unsafe impl Send for FrameBuffer {}
/// Ledger GAP: FrameBuffer is only reached through the GPU_DEVICE slot's lock; no concurrent &mut view exists.
unsafe impl Sync for FrameBuffer {}
/// Ledger GAP: the device's shared state sits behind Spinlocks; the raw ctrl-queue pointers are only touched under cmd_lock.
unsafe impl Send for VirtioGpuDevice {}
/// Ledger GAP: all command-path state is lock-serialized; shared &self views see a consistent device.
unsafe impl Sync for VirtioGpuDevice {}
/// Ledger GAP: the device's shared state sits behind Spinlocks.
unsafe impl Send for VirtioInputDevice {}
/// Ledger GAP: all event-buffer access is confined to the device's own queue/reader discipline.
unsafe impl Sync for VirtioInputDevice {}
// SAFETY: the object's mutable state is a single AtomicU32; kva/phys are
// stable for the object's lifetime.
/// The object's mutable state is a single AtomicU32; kva/phys are stable for the object's lifetime (ledger GAP).
unsafe impl Send for GemObject {}
/// Ledger GAP: shared views only read the stable kva/phys/size and the AtomicU32 resource id.
unsafe impl Sync for GemObject {}

// ------------ ashmem: pool-page edges (INV-FS-FILEPRIV-1) ------------

/// Put one ashmem pool page; report whether this was the last ref.
///
/// # Contract (GAP)
/// Ledger GAP: the descriptor exists for this in-RAM page
/// (the area's page list owns it); true means the put dropped
/// the last reference and the caller frees the underlying
/// pages.
#[inline(always)]
pub(crate) fn page_put_is_last(page: *mut crate::mm::types::Page) -> bool {
unsafe { (*page).put_page() == 0 }
}

/// Copy one chunk out of a pool page's linear map.
///
/// # Contract (INV-FS-FILEPRIV-1)
/// INV-FS-FILEPRIV-1: src is the kernel linear map of a live
/// pool page; [in_pg, in_pg+chunk) and [0, chunk) are within
/// the page and the caller's buffer (chunk = min of both).
#[inline(always)]
pub(crate) fn ashmem_read_copy_chunk(src: *const u8, in_pg: usize, buf_ptr: *mut u8, done: usize, chunk: usize) {
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(in_pg), buf_ptr.add(done), chunk);
        }
}

/// Flush all TLBs after ashmem re-mapped pool pages.
///
/// # Contract (INV-FS-FILEPRIV-1)
/// INV-FS-FILEPRIV-1: the pool pages' mappings changed under
/// existing VMAs; a full flush is the conservative correct
/// shape for this driver.
#[inline(always)]
pub(crate) fn ashmem_flush_tlb_all() {
    unsafe { crate::arch::mm::asid::flush_tlb_all(); }
}


// ------------ loop: loop-device file edges (INV-BLKDEV-2 / INV-CHARDEV-PRIV-1) ------------

/// Copy one block range out of a bread()'d buffer head and
/// release it.
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: bh came from bread (the caller holds the
/// read reference until the brelse inside this edge); b_data
/// is block_size bytes so [in_off, in_off+want) is in bounds.
#[inline(always)]
pub(crate) fn loop_bh_read_into(bh: *mut BufferHead, buf: &mut [u8], done: usize, want: usize, in_off: usize) {
        unsafe {
            let data = &(*bh).b_data;
            buf[done..done + want].copy_from_slice(&data[in_off..in_off + want]);
            crate::fs::hotpath::brelse(bh);
        }
}

/// Copy one block range into a bread()'d buffer head, mark it
/// dirty, sync and release. False = the sync failed.
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: bh came from bread; b_data is block_size
/// bytes so [in_off, in_off+want) is in bounds. False means
/// sync_dirty_buffer failed: the caller stops the copy loop
/// (the original inline `break`).
#[inline(always)]
pub(crate) fn loop_bh_write_from(bh: *mut BufferHead, buf: &[u8], done: usize, want: usize, in_off: usize) -> bool {
        unsafe {
            {
                let data = &mut (*bh).b_data;
                data[in_off..in_off + want].copy_from_slice(&buf[done..done + want]);
            }
            (*bh).set_state_bit(crate::fs::types::BufferState::BH_Dirty);
            let r = crate::fs::hotpath::sync_dirty_buffer(bh);
            crate::fs::hotpath::brelse(bh);
            if r.is_err() {
                return false;
            }
        }
        true
}


// ------------ input: virtio-input and evdev edges (INV-SREF-1 / INV-CHARDEV-PRIV-1 / INV-KTASK-1) ------------

/// Arm the input bottom-half tasklet with its handler.
///
/// # Contract (GAP)
/// Ledger GAP: touched from the probe (single-threaded init
/// context) and from tasklet_schedule in hard-IRQ context,
/// which is the API's designed use; a static tasklet struct
/// lives forever.
#[inline(always)]
pub(crate) fn input_bh_tasklet_init() {
    unsafe { INPUT_BH_TASKLET.init(input_bh_run) };
}

/// Read one device-written input event from the DMA buffer.
///
/// # Contract (GAP)
/// Ledger GAP: the event buffer is a live registered DMA
/// buffer; desc_idx is the used-ring id, in-bounds for the
/// queue_size-sized allocation.
#[inline(always)]
pub(crate) fn input_event_read(event_buffer: *mut VirtioInputEvent, desc_idx: usize) -> VirtioInputEvent {
unsafe { read_volatile(event_buffer.add(desc_idx)) }
}

/// Free the input device's event DMA buffer.
///
/// # Contract (GAP)
/// Ledger GAP: the buffer was allocated with exactly
/// `layout` in init_virtio (alloc_zeroed); the device is
/// quiesced before the Drop runs.
#[inline(always)]
pub(crate) fn input_buffer_dealloc(p: *mut u8, layout: alloc::alloc::Layout) {
                unsafe {
                    alloc::alloc::dealloc(p, layout);
                }
}

/// Read a parked waiter's pid through the raw task pointer.
///
/// # Contract (INV-KTASK-1)
/// INV-KTASK-1: `task` is the current task parking here (the
/// dispatcher identity-checked the File against EVDEV_OPS);
/// the task is alive for the whole wait/unwait handshake.
#[inline(always)]
pub(crate) fn park_task_pid(task: *mut crate::process::types::Task) -> crate::process::types::Pid {
unsafe { (*task).pid() }
}

/// Read the DevNo a chardev file's private slot stores.
///
/// # Contract (INV-SREF-1)
/// INV-SREF-1: devfs_open stored a Box<DevNo> (file-lifetime);
/// the slot is valid while the file is open. The one read
/// edge shared by evdev and loop files.
#[inline(always)]
pub(crate) fn devno_of_private(p: *const u8) -> DevNo {
unsafe { *(p as *const DevNo) }
}

/// Reclaim the file's private Box<DevNo> at close.
///
/// # Contract (INV-CHARDEV-PRIV-1)
/// INV-CHARDEV-PRIV-1: the Box<DevNo> devfs_open stored has
/// its sole owning reference in the slot; close retires it.
#[inline(always)]
pub(crate) fn devno_box_take(p: *mut DevNo) -> alloc::boxed::Box<DevNo> {
unsafe { Box::from_raw(p as *mut DevNo) }
}


// ------------ gpu: framebuffer and control-queue edges (INV-SLOT-1 / INV-MMIO-1) ------------

/// Flush the GPU device through its raw static-slot pointer.
///
/// # Contract (INV-SLOT-1)
/// INV-SLOT-1: GPU_DEVICE is a static set once by
/// set_gpu_device() during probe and never cleared; the
/// pointer stays valid for the machine's lifetime.
#[inline(always)]
pub(crate) fn gpu_flush(device: *const VirtioGpuDevice) {
    unsafe { (*device).flush() };
}

/// Shared view of the registered GPU device.
///
/// # Contract (INV-SLOT-1)
/// INV-SLOT-1: GPU_DEVICE is set once during boot and never
/// cleared; the reference cannot outlive the machine.
#[inline(always)]
pub(crate) fn gpu_device_ref<'a>(device: *const VirtioGpuDevice) -> &'a VirtioGpuDevice {
unsafe { &*device }
}

/// Zeroed allocation of a DMA buffer (null-checked by caller).
///
/// # Contract (INV-MMIO-1)
/// INV-MMIO-1 family: the layout has non-zero size (fb_size =
/// stride*height with width/height checked != 0); the null
/// check follows. Shared RX/event/dumb-buffer edge.
#[inline(always)]
pub(crate) fn dma_zeroed_alloc(layout: alloc::alloc::Layout) -> *mut u8 {
unsafe { alloc::alloc::alloc_zeroed(layout) }
}

/// Allocate the control-queue command/response buffer.
///
/// # Contract (GAP)
/// Ledger GAP: the layout is non-zero-sized (both parts are
/// >= 24 bytes); the null check follows immediately.
#[inline(always)]
pub(crate) fn gpu_ctrl_alloc(layout: alloc::alloc::Layout) -> *mut u8 {
unsafe { alloc::alloc::alloc(layout) }
}

/// Response half of the control-queue io buffer.
///
/// # Contract (GAP)
/// Ledger GAP: io_buf is cmd_size + resp_size bytes, so
/// +cmd_size is within the allocation.
#[inline(always)]
pub(crate) fn gpu_ctrl_resp_half(io_buf: *mut u8, cmd_size: usize) -> *mut u8 {
unsafe { io_buf.add(cmd_size) }
}

/// Copy the command into the control-queue buffer.
///
/// # Contract (GAP)
/// Ledger GAP: `cmd` references at least cmd_size initialized
/// bytes; io_buf has room for them (layout above).
#[inline(always)]
pub(crate) fn gpu_ctrl_cmd_copy<CMD>(cmd: &CMD, cmd_size: usize, io_buf: *mut u8) {
        unsafe {
            core::ptr::copy_nonoverlapping(cmd as *const CMD as *const u8, io_buf, cmd_size);
        }
}

/// riscv64 fence before reading back the used ring.
///
/// # Contract (GAP)
/// Ledger GAP: bare cache/memory fence instruction, no
/// operands — orders the avail publish against the poll.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn gpu_ctrl_fence_riscv64() {
            unsafe {
                core::arch::asm!("fence");
            }
}

/// Copy the device-written response back to the caller.
///
/// # Contract (GAP)
/// Ledger GAP: the device wrote resp_size bytes at resp_ptr
/// (the completion fired — the used-ring read above passed).
#[inline(always)]
pub(crate) fn gpu_ctrl_resp_copy<RESP>(resp_ptr: *mut u8, resp: &mut RESP, resp_size: usize) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        resp_ptr,
                        resp as *mut RESP as *mut u8,
                        resp_size,
                    );
                }
}

/// Deallocation with the exact allocation layout.
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3: `p` was allocated with exactly `layout` (the
/// framebuffer in init_framebuffer; the shared edge for the
/// input event buffer, the GEM backing store and the RX
/// rollback path).
#[inline(always)]
pub(crate) fn layout_dealloc(p: *mut u8, layout: alloc::alloc::Layout) {
                unsafe {
                    alloc::alloc::dealloc(p, layout);
                }
}


// ------------ virtio-net: RX buffer edges (INV-KPTR-3) ------------

/// Byte view of the device-completed RX buffer.
///
/// # Contract (INV-KPTR-3)
/// INV-KPTR-3: `buf_virt` is the linear-mapping address of a
/// device-completed RX buffer owned by this device (taken
/// from its own descriptor table); `total_len` is the
/// device-reported length, checked > header size by the
/// caller.
#[inline(always)]
pub(crate) fn rx_hdr_slice<'a>(buf_virt: u64, total_len: usize) -> &'a [u8] {
        unsafe {
            core::slice::from_raw_parts(buf_virt as *const u8, total_len)
        }
}

/// Free an RX buffer with its exact allocation layout.
///
/// # Contract (INV-KPTR-3)
/// INV-KPTR-3: the buffer was allocated with the identical
/// layout in refill_rx_buffers (64-aligned, buf_size bytes).
#[inline(always)]
pub(crate) fn rx_buffer_dealloc(addr: u64, buf_size: usize) {
        unsafe {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(buf_size, 64) {
                alloc::alloc::dealloc(addr as *mut u8, layout);
            } else {
                crate::pr_err!("virtio_net: invalid RX dealloc layout buf_size={}", buf_size);
            }
        }
}

/// Allocate one RX buffer (null-checked by the caller).
///
/// # Contract (INV-KPTR-3)
/// INV-KPTR-3: the layout is validated (from_size_align Ok)
/// by the caller before this allocation; the null check
/// follows immediately.
#[inline(always)]
pub(crate) fn rx_buffer_alloc(layout: alloc::alloc::Layout) -> *mut u8 {
unsafe { alloc::alloc::alloc(layout) as *mut u8 }
}


// ------------ virtio::queue: vring access and notify edges (INV-DMA-2 / INV-DMA-3) ------------

/// Read the used-ring idx field (offset 2, u16).
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3: a live vring's used ring (the null check is the
/// caller's); the u16 at +2 is the device-written idx.
#[inline(always)]
pub(crate) fn vring_used_idx(used_ring: *const UsedRing) -> u16 {
    unsafe { core::ptr::read_volatile((used_ring as usize + 2) as *const u16) }
}

/// Read one used-ring entry's id field.
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3: live used ring; the entry at +4+slot*8 is
/// within the ring (queue_size checked by the caller).
#[inline(always)]
pub(crate) fn vring_used_elem_id(used_ring: *const UsedRing, slot: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile((used_ring as usize + 4 + slot * 8) as *const u32)
    }
}

/// Read the device-written response byte of a live request.
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3: caller-owned response byte of a live request;
/// volatile read (the device may write it any time up to the
/// completion).
#[inline(always)]
pub(crate) fn resp_status_read(resp_status: *const u8) -> u8 {
unsafe { core::ptr::read_volatile(resp_status) }
}

/// Read `sie` and clear SEIE (bit 9) around the notify store.
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3 (F-VIO-NOTIFY family): register-based CSR
/// read/modify around the MMIO kick so the device IRQ cannot
/// nest into the notify window on riscv64.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn virtio_notify_sie_save() -> u64 {
        let sie_backup: u64;
        unsafe {
            core::arch::asm!(
                "csrr {sie}, sie",
                "li {mask}, {seie}",
                "csrc sie, {mask}",
                sie = out(reg) sie_backup,
                mask = out(reg) _,
                seie = const 1 << 9,
            );
        }
        sie_backup
}

/// Store fence (w, o) before the notify MMIO write.
///
/// # Contract (INV-DMA-3)
/// Orders the descriptor/avail writes before the device
/// kick on riscv64.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn virtio_notify_fence_wo() {
        unsafe {
            core::arch::asm!("fence w, o");
        }
}

/// The queue-notify MMIO store (the device kick itself).
///
/// # Contract (INV-DMA-3)
/// INV-DMA-3 (IRREDUCIBLE F-VIO-NOTIFY): `queue_notify` is a
/// valid MMIO address for the VirtIO notify register; the
/// width matches the capability negotiation (16-byte PCI
/// notify / 32-byte virtio-mmio — QEMU drops access != 4B).
#[inline(always)]
pub(crate) fn virtio_notify_mmio(notify_width: NotifyWidth, queue_notify: u64, queue_index: u16) {
        unsafe {
            match notify_width {
                NotifyWidth::W16 => {
                    // virtio-pci notify capability (matches Linux iowrite16)
                    let queue_notify = queue_notify as *mut u16;
                    core::ptr::write_volatile(queue_notify, queue_index);
                }
                NotifyWidth::W32 => {
                    // virtio-mmio QueueNotify: QEMU drops any access != 4 bytes
                    let queue_notify = queue_notify as *mut u32;
                    core::ptr::write_volatile(queue_notify, queue_index as u32);
                }
            }
        }
}

/// Full fence (iorw, iorw) after the notify MMIO write.
///
/// # Contract (INV-DMA-3)
/// Orders the kick against everything around it on riscv64.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn virtio_notify_fence_iorw() {
        unsafe {
            core::arch::asm!("fence iorw, iorw");
        }
}

/// Restore the `sie` CSR saved around the notify window.
///
/// # Contract (INV-DMA-3)
/// Writing the sie CSR to restore the previous interrupt-
/// enable state is valid on S-mode; `sie_backup` is the value
/// virtio_notify_sie_save returned.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn virtio_notify_sie_restore(sie_backup: u64) {
        unsafe {
            core::arch::asm!(
                "csrw sie, {sie}",
                sie = in(reg) sie_backup,
            );
        }
}

/// Mutable view of one descriptor-table slot.
///
/// # Contract (INV-DMA-2)
/// INV-DMA-2: the idx bounds check is the caller's (the
/// slot's range is disjoint from every other slot); the
/// caller's device lock provides the exclusion.
#[inline(always)]
pub(crate) fn vq_desc_mut<'a>(base: *mut Desc, idx: u16) -> &'a mut Desc {
unsafe { &mut *base.add(idx as usize) }
}

/// Mutable view of the available-ring header.
///
/// # Contract (INV-DMA-2)
/// INV-DMA-2: disjoint slot, the avail header is driver-
/// written; the caller's device lock serializes.
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn vq_avail_mut<'a>(base: *mut AvailRing) -> &'a mut AvailRing {
unsafe { &mut *base }
}


// ------------ virtio-blk: io_buf pool, pending table and completion edges (INV-BLKDEV-2 / INV-SAFECALL-1 / F7) ------------

/// Read a parked io_buf's intrusive next pointer (first 8 bytes).
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: parked buffers were allocated with
/// IO_BUF_LAYOUT; the first 8 bytes hold the intrusive next
/// pointer (the consumer re-initializes the header before
/// use).
#[inline(always)]
pub(crate) fn io_buf_park_pop_head(p: *mut u8) -> usize {
unsafe { *(p as *mut usize) }
}

/// Heap allocation of one 64-byte io_buf (pool underflow path).
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: IO_BUF_LAYOUT is a non-zero-size valid
/// layout (proven at compile time by the const).
#[inline(always)]
pub(crate) fn io_buf_heap_alloc() -> *mut u8 {
unsafe { alloc::alloc::alloc(IO_BUF_LAYOUT) }
}

/// Park the intrusive next pointer in a pooled io_buf's head.
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: `p` is a live 64-byte, 16-aligned block;
/// parking the next pointer in its first 8 bytes is safe
/// because every consumer re-initializes the header before
/// use.
#[inline(always)]
pub(crate) fn io_buf_park_store_head(p: *mut u8, head: usize) {
unsafe { *(p as *mut usize) = head }
}

/// Heap deallocation of one pooled-layout io_buf (pool overflow).
///
/// # Contract (INV-BLKDEV-2)
/// INV-BLKDEV-2: `p` was allocated with IO_BUF_LAYOUT (64/16).
#[inline(always)]
pub(crate) fn io_buf_heap_dealloc(p: *mut u8) {
unsafe { alloc::alloc::dealloc(p, IO_BUF_LAYOUT) }
}

/// Split one pooled 64-byte io_buf into its typed halves and
/// initialize them (the R17-C combined block).
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: `io_buf` is non-null (callers check), 64
/// bytes, 16-aligned; +48 leaves 16 bytes for the response —
/// both in bounds. THE one audited carve edge for the pool
/// shape (ledger row GAP; the contract is the doc above).
#[inline(always)]
pub(crate) fn io_buf_split_init(io_buf: *mut u8, type_: u32, sector: u64) -> (*mut VirtIOBlkReqHeader, *mut VirtIOBlkResp) {
    unsafe {
        let header_ptr = io_buf as *mut VirtIOBlkReqHeader;
        let resp_ptr = io_buf.add(48) as *mut VirtIOBlkResp;
        *header_ptr = VirtIOBlkReqHeader {
            type_,
            reserved: 0,
            sector,
        };
        (*resp_ptr).status = 0xFF;
        (header_ptr, resp_ptr)
    }
}

/// Heap deallocation of a NON-pool-shaped pending entry block.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: dealloc with the exact layout the
/// submitter allocated with (non-pool shape; the caller
/// already routed pool-shaped blocks to io_buf_free).
#[inline(always)]
pub(crate) fn io_buf_nonpool_dealloc(ptr: *mut u8, layout: alloc::alloc::Layout) {
        unsafe { alloc::alloc::dealloc(ptr, layout) };
}

/// Mark a pending's completion as under delivery.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: the count protocol keeps the IoCompletion
/// (and its waiter's stack) alive; callers hold the
/// pending-table lock that serializes the handshake.
#[inline(always)]
pub(crate) fn completion_begin_delivery(completion: *mut crate::fs::types::IoCompletion) {
    unsafe { (*completion).begin_delivery(); }
}

/// Complete a pending's completion through the raw pointer.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: see the doc — count protocol + retire
/// order (complete_with_token, then end_delivery).
#[inline(always)]
pub(crate) fn completion_deliver(completion: *mut crate::fs::types::IoCompletion, status: i32, token: usize) {
    unsafe {
        (*completion).complete_with_token(status, token);
        (*completion).end_delivery();
    }
}

/// Wait out any walker delivery of THIS completion still in flight.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: the entry was taken from a table before the
/// scan, so the tombstone pass could not retire it; the
/// delivering count gates a late complete() into a recycled
/// waiter stack (the ftest01 wandering-corruption fix).
#[inline(always)]
pub(crate) fn completion_wait_deliveries(comp: *mut crate::fs::types::IoCompletion) -> bool {
unsafe { (*comp).wait_deliveries() }
}

/// Capture the caller's return address (virtio acquire site).
///
/// # Contract (INV-SAFECALL-1)
/// Pure register read (`mv ra`), no memory access or side
/// effects (holder forensics, same technique as spinlock.rs).
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn pciblk_site_ra_riscv64() -> usize {
    let ra: usize;
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    ra
}

/// Capture the caller's return address (virtio acquire site).
///
/// # Contract (INV-SAFECALL-1)
/// Frame pointers are forced on, so [rbp+8] is this inlined
/// helper's caller's return address.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn pciblk_site_ra_x86_64() -> usize {
    let ra: usize;
    unsafe {
        core::arch::asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    ra
}

/// Lock-free console byte for holder forensics.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1: putchar_no_lock is a byte-wise polling
/// UART write usable from any context, including with
/// interrupts disabled (the holder may be wedged holding
/// printk's lock).
#[inline(always)]
pub(crate) fn wedge_console_put(b: u8) {
unsafe { crate::console::putchar_no_lock(b) }
}

/// Shared view of a disk's pending-table slots.
///
/// # Contract (F7)
/// F7 (SUBMIT-2X): the caller holds that disk's BLK lock,
/// which serializes every pending-table access — the table's
/// own lock is deliberately not taken inside the window.
#[inline(always)]
pub(crate) fn pci_pending_table_ref(disk: usize) -> &'static [Option<PendingIo>; MAX_PENDING_IO_PCI] {
unsafe { PCI_BLK_PENDING[disk].get_ref() }
}

/// Exclusive view of a disk's pending-table slots.
///
/// # Contract (F7)
/// F7 (SUBMIT-2X): the caller holds that disk's BLK lock
/// (submit publish, walker collect, sync tombstones and
/// abandon all run under it) — direct access, no second
/// lock acquisition inside the window.
#[inline(always)]
pub(crate) fn pci_pending_table_mut(disk: usize) -> &'static mut [Option<PendingIo>; MAX_PENDING_IO_PCI] {
unsafe { PCI_BLK_PENDING[disk].get_mut_unchecked() }
}

/// Walker-side begin_delivery through a fired entry's raw pointer.
///
/// # Contract (INV-SAFECALL-1)
/// INV-SAFECALL-1 (ledger row GAP): the ABANDON-vs-DELIVERY
/// protocol — mark the waiter's completion as under delivery
/// BEFORE the table lock drops, so abandon_pending_completion
/// can never fire into a waiter's kernel stack after its
/// wait() returned.
#[inline(always)]
pub(crate) fn pending_begin_delivery_walk(completion: *mut crate::fs::types::IoCompletion) {
                            unsafe {
                                (*completion).begin_delivery();
                            }
}


// ------------ blkdev: gendisk registry edges (INV-BLKDEV-1 / INV-BLKDEV-3) ------------

/// Resolve the disk's private-data slot as a shared `&T`.
///
/// # Contract (INV-BLKDEV-3)
/// INV-BLKDEV-3: `private_data`, set at device init before
/// registration, points to a `T` that lives at least as long
/// as the registered disk (machine lifetime) and is not
/// mutated through this shared view while registered.
#[inline(always)]
pub(crate) fn gen_disk_private_ref<'a, T>(p: *mut u8) -> &'a T {
unsafe { &*(p as *const T) }
}

/// Extend a boxed registered disk's shared view to 'static.
///
/// # Contract (INV-BLKDEV-1)
/// INV-BLKDEV-1: a registered disk is boxed ONCE into the
/// manager's Vec and never removed (no unregister path); a
/// Box's heap allocation is address-stable across Vec growth.
#[inline(always)]
pub(crate) fn gen_disk_ref<'a>(p: &GenDisk) -> &'a GenDisk {
unsafe { &*(p as *const GenDisk) }
}


// ------------ intc: interrupt-controller register edges (INV-MMIO-1 / F5) ------------

/// Read ICC_IAR1_EL1 (the GICv3 acknowledge register).
///
/// # Contract (INV-MMIO-1)
/// The read is the acknowledge side effect itself: ICC_IAR1_EL1
/// returns the pending INTID and drops its priority; QEMU fixes
/// SRE=1 so the system-register form is always available.
#[cfg(feature = "aarch64")]
#[inline(always)]
pub(crate) fn gicv3_iar1_read() -> u64 {
let v: u64;
    unsafe { asm!("mrs {}, icc_iar1_el1", out(reg) v, options(nomem, nostack)) }
        v
}

/// Write ICC_EOIR1_EL1 (end of interrupt).
///
/// # Contract (INV-MMIO-1)
/// The write deactivates the previously acknowledged INTID
/// (EOImode stays 0 = single-write EOI).
#[cfg(feature = "aarch64")]
#[inline(always)]
pub(crate) fn gicv3_eoir1_write(intid: u32) {
    unsafe {
        asm!("msr icc_eoir1_el1, {}",
             in(reg) intid as u64, options(nomem, nostack))
    };
}

/// Write ICC_SGI1R_EL1 (raise an SGI on the encoded targets).
///
/// # Contract (INV-MMIO-1)
/// `sgi` carries the GICv3 SGI field layout the caller built
/// (INTID [27:24], TargetList [15:0]); the write raises the
/// SGI on the target cores.
#[cfg(feature = "aarch64")]
#[inline(always)]
pub(crate) fn gicv3_sgi1r_write(sgi: u64) {
    unsafe {
        asm!("dsb st",
             "msr icc_sgi1r_el1, {}",
             in(reg) sgi, options(nomem, nostack))
    };
}

/// The GICv3 CPU-interface enable sequence (SRE, PMR, group-1).
///
/// # Contract (INV-MMIO-1)
/// IRREDUCIBLE F-GIC-A1: ICC_* system-register programming
/// (reference kernel drivers/irqchip/irq-gic-v3.c gic_cpu_init);
/// register-file access with no safe shape. Runs per CPU from
/// the boot path; idempotent.
#[cfg(feature = "aarch64")]
#[inline(always)]
pub(crate) fn gicv3_cpu_interface_enable() {
    unsafe {
        // 1. System-register access (QEMU fixes SRE=1; set defensively).
        let sre: u64;
        asm!("mrs {}, icc_sre_el1", out(reg) sre, options(nomem, nostack));
        if sre & 1 == 0 {
            asm!("msr icc_sre_el1, {}", in(reg) 1u64, options(nomem, nostack));
            asm!("isb", options(nomem, nostack));
        }

        // 2. Per-CPU redistributor: locate + configure SGI/PPI bank.
        if let Some(rd) = find_my_rd_base() {
            redist_configure_local(rd);
        }

        // 3. Unmask priorities and forward Group 1 to EL1.
        asm!("msr icc_pmr_el1, {}", in(reg) 0xFFu64, options(nomem, nostack));
        asm!("msr icc_igrpen1_el1, {}", in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

/// x2APIC MSR read of one LAPIC register.
///
/// # Contract (F5)
/// READY gates first use; the MSR index is one of the LAPIC
/// register constants.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_reg_read_msr(msr_x2apic: u32) -> u32 {
unsafe { rdmsr(msr_x2apic) as u32 }
}

/// xAPIC MMIO read of one LAPIC register.
///
/// # Contract (F5)
/// `va` is the kernel-half device-alias address of the LAPIC
/// window (Linux fixmap discipline: shared into every mm,
/// never a low identity VA), mapped at setup_device_mappings.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_reg_read_mmio(va: usize) -> u32 {
unsafe { core::ptr::read_volatile(va as *const u32) }
}

/// x2APIC MSR write of one LAPIC register.
///
/// # Contract (F5)
/// READY gates first use; the MSR index is one of the LAPIC
/// register constants.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_reg_write_msr(msr_x2apic: u32, value: u32) {
unsafe { wrmsr(msr_x2apic, value as u64) }
}

/// xAPIC MMIO write of one LAPIC register.
///
/// # Contract (F5)
/// `va` is the kernel-half device-alias address of the LAPIC
/// window (see apic_reg_read_mmio).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_reg_write_mmio(va: usize, value: u32) {
unsafe { core::ptr::write_volatile(va as *mut u32, value) }
}

/// INV:F5 (KVM pv-IPI hypercall, IRREDUCIBLE
#[cfg(feature = "x86_64")]
/// One FIXED unicast through the PV hypercall. Returns false (caller
/// falls back to the ICR write) when not negotiated or the hypervisor
/// refuses. The register convention is the KVM pv hypercall ABI:
/// rax=nr, rbx=a1, rcx=a2, rdx=a3, rsi=a4, return rax (0 on success,
/// negative -KVM_E* on refusal).
pub(crate) fn kvm_pv_send_ipi(dest: u32, vector: u8) -> bool {
    if !KVM_PV_IPI.load(Ordering::Acquire) {
        return false;
    }
    // IRREDUCIBLE F-ROW (F-HVCALL-X1): the KVM hypercall ABI
    // (vmmcall/vmcall with the RBX-shuttle push/mov/pop bracket) —
    // reference kernel arch/x86/kernel/kvm.c __send_ipi_mask's hc()
    // twin; a register protocol with no memory operands has no safe
    // shape. The bitmap is a single-bit cluster with min=dest
    // (bitmap_low=1, bitmap_high=0).
    unsafe fn hc(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, vmmcall: bool) -> i64 {
        let ret: i64;
        // The KVM ABI passes arg1 in RBX, which LLVM reserves — shuttle
        // it through any free GPR with an explicit push/mov/pop bracket
        // (stack-neutral; that is why nostack is not claimed).
        if vmmcall {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {b}",
                "vmmcall",
                "pop rbx",
                b = in(reg) a1,
                in("rax") nr, in("rcx") a2, in("rdx") a3, in("rsi") a4,
                lateout("rax") ret,
            );
        } else {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {b}",
                "vmcall",
                "pop rbx",
                b = in(reg) a1,
                in("rax") nr, in("rcx") a2, in("rdx") a3, in("rsi") a4,
                lateout("rax") ret,
            );
        }
        ret
    }
    let vmmcall = KVM_HC_VMMCALL.load(Ordering::Acquire);
    // SAFETY: register-only protocol, see hc.
    let ret = unsafe { hc(KVM_HC_SEND_IPI, 1, 0, dest as u64, vector as u64, vmmcall) };
    ret == 0
}
/// Raw x2APIC ICR write.
///
/// # Contract (F5)
/// In x2APIC the delivery-status bit is reserved/RAZ: the
/// write is posted and back-to-back MSR writes serialize
/// sufficiently (the caller falls back to the MMIO polling
/// shape on xAPIC).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_x2apic_icr_write(icr: u64) {
unsafe { wrmsr(X2APIC_ICR, icr) }
}

/// Read this helper's caller's return address off the frame.
///
/// # Contract (F5)
/// Pure register-relative read ([rbp+8], frame pointers forced
/// on), no memory side effects.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn tick_arm_ra_read() -> u64 {
        let ra: u64;
        unsafe {
            core::arch::asm!(
                "mov {}, qword ptr [rbp + 8]",
                out(reg) ra,
                options(nomem, nostack, preserves_flags)
            );
        }
        ra
}

/// Map [start, end) of the physical range into the linear map.
///
/// # Contract (INV-MMIO-1)
/// Kernel-only linear-map VAs for firmware-table regions the
/// e820 walk skipped; the regions are reserved (never handed
/// to the page allocator). Re-mapping an already-mapped PTE
/// rewrites the same value (idempotent).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn map_firmware_range_linear(page_offset: u64, start: usize, end: usize, flags: u64) {
    unsafe {
        crate::arch::mm::mmu_init::map_kernel_region(
            page_offset + start as u64,
            start as u64,
            (end - start) as u64,
            flags,
        );
    }
}

/// Read the local APIC id without the READY guard.
///
/// # Contract (INV-MMIO-1)
/// x2APIC was enabled by the base MSR programming before this
/// runs (MADT parsing while the APIC is globally enabled but
/// the driver is not yet marked ready).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_read_id_raw() -> u32 {
unsafe { rdmsr(X2APIC_ID) as u32 }
}

/// Program IA32_APIC_BASE: global enable, EXTD per `has_x2`.
///
/// # Contract (INV-MMIO-1)
/// IA32_APIC_BASE programming of THIS cpu; the caller derived
/// `has_x2` from CPUID leaf 1 ECX bit 21 (the RT-2X probe
/// story lives on the hotpath fn).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_base_enable_extd(has_x2: bool) {
    unsafe {
        let base = rdmsr(MSR_IA32_APIC_BASE);
        let mut new = base | APIC_BASE_ENABLE;
        if has_x2 {
            new |= APIC_BASE_X2APIC;
        }
        wrmsr(MSR_IA32_APIC_BASE, new);
    }
}

/// Read the x2APIC SVR (spurious-vector register).
///
/// # Contract (INV-MMIO-1)
/// Gated on the EXTD write above; a functioning APIC reads
/// nonzero (reset 0xFF), an unbacked MSR range reads 0.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_svr_read() -> u64 {
unsafe { rdmsr(X2APIC_SVR) }
}

/// Fall back to MMIO xAPIC: clear EXTD, keep global enable.
///
/// # Contract (INV-MMIO-1)
/// IA32_APIC_BASE programming of THIS cpu (the SVR probe
/// above found a dead MSR interface).
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_base_clear_extd() {
            unsafe {
                let base = rdmsr(MSR_IA32_APIC_BASE);
                wrmsr(MSR_IA32_APIC_BASE, base & !APIC_BASE_X2APIC | APIC_BASE_ENABLE);
            }
}

/// Enable the local APIC on an AP in the mode the BSP chose.
///
/// # Contract (INV-MMIO-1)
/// IA32_APIC_BASE programming of THIS cpu; X2APIC holds the
/// mode the BSP published before secondaries came up.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn apic_init_secondary_base() {
    unsafe {
        let base = rdmsr(MSR_IA32_APIC_BASE)
            | APIC_BASE_ENABLE
            | if X2APIC.load(Ordering::Acquire) { APIC_BASE_X2APIC } else { 0 };
        wrmsr(MSR_IA32_APIC_BASE, base);
    }
}

/// Read one 32-bit goldfish-RTC device register.
///
/// # Contract (INV-MMIO-1)
/// The page at GOLDFISH_RTC_BASE is identity-mapped with
/// device flags by setup_device_mappings; only reachable
/// after that (INV-MMIO-1).
#[inline(always)]
pub(crate) fn goldfish_rtc_reg_read(va: u64, offset: u64) -> u32 {
unsafe { core::ptr::read_volatile((va + offset) as *const u32) }
}


// ------------ timer: arch timer programming edges (F5) ------------

/// Read the riscv64 time CSR (rdtime).
///
/// # Contract (F5)
/// rdtime is a read-only CSR available in S-mode; no other
/// side effect (INV:F5 timer floor).
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn timer_riscv64_read_time() -> u64 {
    unsafe {
        let time: u64;
        core::arch::asm!(
            "rdtime {}",
            out(reg) time,
            options(nostack, readonly)
        );
        time
    }
}

/// Arm the riscv64 timer through the SBI TIMER extension.
///
/// # Contract (F5)
/// sbi_rt::set_timer is an SBI ecall with no special
/// preconditions; `value` is an absolute deadline (u64).
/// Per-hart: applies to the calling hart.
#[cfg(feature = "riscv64")]
#[inline(always)]
pub(crate) fn timer_riscv64_sbi_set_timer(value: u64) {
    unsafe {
        sbi_rt::set_timer(value);
    }
}

/// Write the stimecmp CSR (sstc extension).
///
/// # Contract (F5)
/// A single CSR write of the absolute deadline; more direct
/// than SBI and avoids its round trip.
#[cfg(feature = "riscv64")]
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn timer_riscv64_stimecmp_set(deadline: u64) {
    unsafe {
        asm!(
            "csrw stimecmp, {0}",
            in(reg) deadline,
            options(nomem, nostack)
        );
    }
}

/// Program PIT channel 0 for square-wave ticks (IRQ0 fallback).
///
/// # Contract (F5)
/// Ports 0x40/0x43 are the fixed 8254 channel-0 data and
/// mode/command registers; this is the standard programming
/// sequence. Called when the LAPIC tick could not arm.
#[cfg(feature = "x86_64")]
#[inline(always)]
pub(crate) fn pit_ch0_square_wave_init(divisor: u16) {
    unsafe {
        outb(PIT_MODE_CMD, PIT_CH0_MODE3_BIN);
        io_wait();
        outb(PIT_CH0_DATA, (divisor & 0xff) as u8);
        io_wait();
        outb(PIT_CH0_DATA, (divisor >> 8) as u8);
    }
}

/// Arm the aarch64 EL1 virtual timer to an absolute deadline.
///
/// # Contract (F5)
/// Writing cntv_cval_el0/cntv_ctl_el0 arms the per-cpu timer;
/// the only side effect is the future PPI.
#[cfg(feature = "aarch64")]
#[inline(always)]
pub(crate) fn timer_aarch64_arm(deadline: u64) {
    unsafe {
        core::arch::asm!(
            "msr cntv_cval_el0, {d}",
            "msr cntv_ctl_el0, {e}",
            d = in(reg) deadline,
            e = in(reg) 1u64, // enable, unmasked
            options(nomem, nostack)
        );
    }
}

