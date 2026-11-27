//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO virtual queue
//!
//! Queue implementation fully compliant with VirtIO specification
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU16, Ordering};

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
    queue_index: u16,
    /// Queue notification address
    queue_notify: u64,
    /// Notify register access width for this transport
    notify_width: NotifyWidth,
    /// Interrupt status address (VIRTIO_MMIO_INTERRUPT_STATUS - Read Only)
    interrupt_status: u64,
    /// Interrupt acknowledge address (VIRTIO_MMIO_INTERRUPT_ACK - Write Only)
    interrupt_ack: u64,
    /// The vring allocation (None after free_vring): one DMA block
    /// (INV-DMA-1) carved into desc/avail/used (INV-DMA-2).
    vring: Option<Vring>,
    /// Next descriptor index to allocate
    next_desc: AtomicU16,
    /// Chains whose wait timed out and whose descriptors are LEAKED (the
    /// request may still be in flight on the device). Leaked chains must
    /// not count against the in-flight guard forever, and their slots must
    /// never be handed out again — see note_timed_out_chain().
    leaked_chains: AtomicU16,
    /// Kernel-side ordinal of the next avail-ring submission. submit()
    /// derives the ring slot and the published avail.idx from this shadow
    /// instead of reading the shared ring back — a corrupted avail.idx
    /// (stray kernel write into the heap-backed vring) previously made the
    /// device walk garbage heads and wedge permanently.
    avail_shadow: AtomicU16,
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
    descs_held: AtomicU16,
}

/// The carved vring (INV-DMA-1/2): the block owns the allocation; the
/// slots are disjoint zero-drop views into it (block declared first
/// only for documentation — slot drops are no-ops).
struct Vring {
    block: crate::safety::dma::DmaBlock,
    /// Descriptor table [queue_size] (driver-owned; device reads).
    desc: crate::safety::dma::DmaSlot<[Desc]>,
    /// Available ring header (driver-written).
    avail: crate::safety::dma::DmaSlot<AvailRing>,
    /// Used ring header (device-written — read volatile only).
    used: crate::safety::dma::DmaSlot<UsedRing>,
    /// Byte offsets of the carved sections (volatile-at offsets and
    /// register programming derive from these).
    off_avail: usize,
    off_used: usize,
}


// ---- Safe used-ring / response readers (safe-call discipline,
// INV-SAFECALL-1..3): the ONE audited volatile edge per shape; every
// poll loop and forensic printer in the virtio stack calls these. ----

/// Read the used ring's `idx` (device-written) volatily.
///
/// INV: `used_ring` originates from `VirtQueue::used_ring_ptr()` on a
/// queue whose vring is still allocated (the callers hold the queue
/// reference); offset 2 is the idx field of the v1.0 used ring.
pub fn used_ring_idx(used_ring: *const UsedRing) -> u16 {
    if used_ring.is_null() {
        return 0;
    }
    // SAFETY: the invariant above — a live vring's used ring; the u16
    // at +2 is within the allocation; volatile because the device
    // writes it (INV-DMA-3).
    crate::drivers::edges::volatile_read_u16((used_ring as usize + 2) as *const u16)
}

/// Read a used-ring entry's descriptor id (device-written) volatily.
///
/// INV: as `used_ring_idx`; `slot` must be < queue_size (callers wrap
/// with `% queue_size`, which the device protocol also guarantees).
pub fn used_ring_elem_id(used_ring: *const UsedRing, slot: usize) -> u32 {
    if used_ring.is_null() {
        return u32::MAX;
    }
    // SAFETY: live used ring; the entry at +4+slot*8 is within the
    // allocation for slot < queue_size; volatile device-written memory.
    crate::drivers::edges::volatile_read_u32((used_ring as usize + 4 + slot * 8) as *const u32)
}

/// True once the device has written THIS request's response byte.
///
/// INV: `resp_status` points at this request's response status byte
/// (device writes it exactly once on completion; initialized 0xFF at
/// submit) or is null (treated as not-done).
pub fn resp_done(resp_status: *const u8) -> bool {
    resp_status_byte(resp_status) != 0xFF
}

/// Volatile read of a request's response status byte (null reads as
/// the 0xFF "pending" sentinel). Same INV as [`resp_done`].
pub fn resp_status_byte(resp_status: *const u8) -> u8 {
    if resp_status.is_null() {
        return 0xFF;
    }
    // SAFETY: caller-owned response byte of a live request; volatile
    // so each poll re-reads DMA memory (INV-DMA-3).
    crate::drivers::edges::volatile_read_u8(resp_status)
}

/// r-strand instrument (lat_fs conviction round, 2026-11-19): the sync
/// wait loop's phase counts, printed by the periodic dump. A stranded
/// waiter shows SLEWED (schedule returned many times but RESP stayed
/// pending = wake reached us yet our byte never landed) vs NEVER
/// (schedule never returned = the wake never reached the task; the
/// strand is scheduler-side).
pub static VW_WAIT_ENTER: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_SCHED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_RESP: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_TIMEOUT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);


impl VirtQueue {
    /// Create new VirtQueue (using contiguous memory layout)
    ///
    /// # Parameters
    /// - `queue_size`: Queue size (must be power of 2)
    /// - `queue_index`: Queue index (written when notifying device)
    /// - `queue_notify`: Queue notification register address
    /// - `interrupt_status`: Interrupt status register address
    /// - `interrupt_ack`: Interrupt acknowledge register address
    pub fn new(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64) -> Option<Self> {
        Self::with_notify_width(queue_size, queue_index, queue_notify, interrupt_status, interrupt_ack, NotifyWidth::W16)
    }

    /// Create new VirtQueue with an explicit notify access width.
    pub fn with_notify_width(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64, notify_width: NotifyWidth) -> Option<Self> {
        let desc_size = queue_size as usize * 16;
        let avail_size = 2 + 2 + queue_size as usize * 2 + 2;
        let used_size = 2 + 2 + queue_size as usize * 8 + 2;

        // VirtIO 1.0 specification: descriptor table, available ring, and used ring must be page-aligned (at least 4096 bytes)
        const PAGE_SIZE: usize = 4096;

        let desc_size_aligned = (desc_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let avail_size_aligned = (avail_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let used_size_aligned = (used_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let total_size = desc_size_aligned + avail_size_aligned + used_size_aligned;

        // INV-DMA-1/2: one page-aligned tracked allocation; the carve
        // below checks alignment, bounds, and disjointness so the old
        // hand offset arithmetic is gone. Each v1.0 section starts on
        // its own page boundary; the ring arrays after each header are
        // carved too, so every device-shared byte belongs to a carved
        // range.
        let mut block = crate::safety::dma::DmaBlock::new(total_size, PAGE_SIZE)?;
        let (mut desc, off_desc) =
            block.carve_array_aligned::<Desc>(queue_size as usize, PAGE_SIZE)?;
        if off_desc != 0 {
            crate::println!("virtio: ERROR: vring desc carve not page-aligned!");
            return None;
        }
        let avail_off_expected = off_desc + desc_size_aligned;
        let (mut avail, off_avail) = block.carve_aligned::<AvailRing>(PAGE_SIZE)?;
        if off_avail != avail_off_expected {
            crate::println!("virtio: ERROR: vring avail carve offset wrong!");
            return None;
        }
        // avail ring entries + used_event (driver-written).
        let (_avail_ring, _) =
            block.carve_array::<u16>(queue_size as usize + 1)?;
        let used_off_expected = off_avail + avail_size_aligned;
        let (mut used, off_used) = block.carve_aligned::<UsedRing>(PAGE_SIZE)?;
        if off_used != used_off_expected {
            crate::println!("virtio: ERROR: vring used carve offset wrong!");
            return None;
        }
        // used ring entries + avail_event (device-written).
        let (_used_ring, _) =
            block.carve_array::<UsedElem>(queue_size as usize)?;
        let (_avail_event, _) = block.carve_array::<u16>(1)?;
        let expected_total = used_off_expected + used_size_aligned;
        if block.frontier_offset() > expected_total {
            crate::println!("virtio: ERROR: vring carve exceeded section math!");
            return None;
        }

        // INV-DMA-2: driver-owned init through the disjoint slots
        // (the struct below adopts them after init).
        for i in 0..queue_size as usize {
            desc[i] = Desc { addr: 0, len: 0, flags: 0, next: 0 };
        }
        avail.flags = 0;
        avail.idx = 0;
        used.flags = 0;
        used.idx = 0;

        Some(Self {
            queue_size,
            queue_index,
            queue_notify,
            notify_width,
            interrupt_status,
            interrupt_ack,
            vring: Some(Vring {
                // MPK phase 1: tag the AVAIL section's pages pkey=1
                // (the guard is a no-op returning 0 pages when
                // mpkguard is off). The section is page-isolated by
                // construction; the tagged leaves are in the shared
                // kernel-half tables (visible in every address space).
                // This is the ONLY tagging call — the window discipline
                // lives in DmaBlock's volatile accessors.
                block: {
                    #[allow(unused_mut)]
                    let mut b = block;
                    let _pages = b.set_pkey_range(
                        off_avail,
                        off_avail + avail_size_aligned,
                    );
                    b
                },
                desc,
                avail,
                used,
                off_avail,
                off_used,
            }),
            next_desc: AtomicU16::new(0),
            leaked_chains: AtomicU16::new(0),
            avail_shadow: AtomicU16::new(0),
            descs_held: AtomicU16::new(0),
        })
    }

    /// Free the vring allocation backing this queue.
    ///
    /// VirtQueue has no Drop impl (the long-lived configured queues are
    /// intentionally never freed), so transient per-call queues — the
    /// legacy read_block/write_block paths — MUST call this on every exit
    /// path or leak a page-granular vring per I/O (review BUG: "write_block
    /// desc 失败清理" — the allocation leak fired on the desc-failure early
    /// returns too).
    ///
    /// Now safe (INV-DMA-1): dropping the DmaBlock deallocates with the
    /// stored layout; the slots are zero-drop views, so drop order cannot
    /// use-after-free.
    ///
    /// Precondition (unchanged, still the caller's contract): the queue
    /// must not be registered with a device (no in-flight DMA).
    pub fn free_vring(&mut self) {
        // MPK phase 1: untag BEFORE the block drops — a freed tagged
        // page re-enters the allocator still carrying pkey=1 and every
        // later user faults (the transient per-call queues' path).
        if let Some(vr) = self.vring.as_mut() {
            vr.block.clear_pkey_range();
        }
        self.vring = None;
    }

    /// Get current available index
    pub fn get_avail(&self) -> u16 {
        match self.vring.as_ref() {
            // INV-DMA-3: device-visible ring index, bounds-checked
            // volatile read at the carve offset.
            Some(vr) => vr.block.read_volatile_at::<u16>(vr.off_avail + 2),
            None => 0,
        }
    }

    /// Get current used index
    pub fn get_used(&self) -> u16 {
        match self.vring.as_ref() {
            // INV-DMA-3: device-written used index.
            Some(vr) => vr.block.read_volatile_at::<u16>(vr.off_used + 2),
            None => 0,
        }
    }

    /// Get element from used ring
    ///
    /// # Parameters
    /// - `idx`: Index in used ring
    ///
    /// # Returns
    /// UsedElem containing descriptor ID and length
    pub fn get_used_elem(&self, idx: u16) -> Option<UsedElem> {
        let vr = self.vring.as_ref()?;
        // Used ring structure: flags (2) + idx (2) + ring (queue_size * 8)
        let slot = (idx % self.queue_size) as usize;
        Some(vr.block.read_volatile_at::<UsedElem>(vr.off_used + 4 + slot * 8))
    }

    /// Get last processed used index (for tracking)
    pub fn get_last_used(&self) -> u16 {
        // This should be maintained by driver, simplified implementation here
        self.get_used()
    }

    /// Notify device of new request
    pub fn notify(&self) {
        // Disable external interrupts during MMIO operations.
        // NOTE: SEIE is bit 9 of sie — beyond csrci's 5-bit immediate. The
        // old "csrci sie, 9" wrote mask 0b01001 (bits 0 and 3, both WPRI)
        // and masked nothing; use a register-based csrc with the real mask.
        #[cfg(feature = "riscv64")]
        let sie_backup: u64;
        #[cfg(feature = "riscv64")]
        // Reading sie and clearing SEIE (bit 9) crosses the audited
        // INV-DRV-CSR edge.
        let sie_backup: u64 = crate::drivers::edges::riscv_mask_seie();

        // RISC-V MMIO fence: fence w, o
        // This ensures all previous writes (to descriptor table, available ring)
        // are visible before the MMIO write to the notify register.
        // MMIO write fence: RISCV_FENCE(w, o)
        #[cfg(feature = "riscv64")]
        crate::drivers::edges::fence_w_o();

        // queue_notify is a valid MMIO address for the VirtIO notify
        // register; the volatile write crosses the INV-DRV-MMIO edge.
        match self.notify_width {
            NotifyWidth::W16 => {
                // virtio-pci notify capability (matches Linux iowrite16)
                crate::drivers::edges::mmio_write16(
                    self.queue_notify as *mut u16, self.queue_index);
            }
            NotifyWidth::W32 => {
                // virtio-mmio QueueNotify: QEMU drops any access != 4 bytes
                crate::drivers::edges::mmio_write32(
                    self.queue_notify as *mut u32, self.queue_index as u32);
            }
        }

        // RISC-V MMIO fence after write: fence iorw, iorw
        // This is the WRITE-side device fence (Linux __io_aw(): "fence
        // iorw, iorw"): the MMIO store must reach the device before any
        // subsequent memory or MMIO access by this hart. The old
        // "fence i, ir" is the READ-side ordering fence (__io_ar) used in
        // the wrong place — it ordered device INPUT against instruction
        // fetch and did not order the notify store at all (review ARCH:
        // fence i,ir 用反, weakly-ordered platform risk).
        #[cfg(feature = "riscv64")]
        crate::drivers::edges::fence_iorw();

        // Restore interrupts
        #[cfg(feature = "riscv64")]
        // Restoring sie crosses the audited INV-DRV-CSR edge.
        crate::drivers::edges::riscv_sie_restore(sie_backup);
    }

    /// Wait for device to complete request
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        // Timeout value from config (in loop iterations, approximately microseconds)
        self.wait_for_completion_max(prev_used, crate::config::VIRTIO_QUEUE_TIMEOUT_US)
    }

    /// Wait for device to complete request with an explicit iteration budget.
    ///
    /// `max_iters` is a spin-loop iteration budget (~µs each under TCG).
    /// Callers that hold a lock_irqsave across this wait (e.g. the net TX
    /// path) MUST pass a small budget: every iteration runs with external
    /// interrupts disabled, so the budget directly bounds the worst-case
    /// IRQ-blackout window (review TIMING: xmit used 10M+50M iters — tens
    /// of seconds of disabled interrupts under TCG).
    pub fn wait_for_completion_max(&self, prev_used: u16, max_iters: u64) -> u16 {
        if self.vring.is_none() {
            return prev_used;
        }

        let mut timeout = max_iters;

        loop {
            // Use memory barrier to ensure read ordering
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

            let used_idx = self.get_used();

            if used_idx != prev_used {
                return used_idx;
            }

            core::hint::spin_loop();

            timeout -= 1;
            if timeout == 0 {
                return used_idx;
            }
        }
    }

    /// Get raw pointer to used ring (for interrupt-driven wait outside
    /// queue lock). INV: valid while the queue's vring is allocated;
    /// read it through [`used_ring_idx`]/[`used_ring_elem_id`].
    pub fn used_ring_ptr(&self) -> *const UsedRing {
        match self.vring.as_ref() {
            Some(vr) => vr.used.as_ptr(),
            None => core::ptr::null(),
        }
    }

    /// Kernel-side submission ordinal snapshot (VW forensic
    /// instrumentation): equals avail.idx under a healthy submit path.
    pub fn avail_shadow_snapshot(&self) -> u16 {
        self.avail_shadow.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Direct-console one-line snapshot of a stalling poller's world (the
    /// VW-POLLSTALL family): the poller's own response byte, the device's
    /// used-ring index, the caller's prev_used ordinal, the walker's
    /// position, the submission shadow, and the un-kicked chain count.
    /// Direct console (putchar_no_lock) because pr_err is silently dropped
    /// on the OH boots (lb70/lb71 lesson).
    ///
    /// Class split, one line:
    /// - used > prev_used, resp still 0xFF: the device completed entries
    ///   past our ordinal but never wrote OUR byte — completion attribution
    ///   or window-reuse corruption;
    /// - used == prev_used, shadow > used: chains are published but the
    ///   device never consumed them — lost kick (or a wedged ring);
    /// - used > prev_used with resp written is unreachable here (the loop
    ///   exits on the byte).
    #[allow(clippy::too_many_arguments)]
    fn vw_pollstall_print(
        blk_slot: usize,
        resp_status: *const u8,
        used_ring: *const UsedRing,
        prev_used: u16,
        tag: &[u8],
    ) {
        use crate::console::putchar_no_lock as putchar;
        fn pdec(mut v: u64) {
            use crate::console::putchar_no_lock as putchar;
            let mut buf = [0u8; 20];
            let mut n = 0usize;
            if v == 0 {
                putchar(b'0');
                return;
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            let mut i = n;
            while i > 0 {
                i -= 1;
                putchar(buf[i]);
            }
        }
        for &c in b"\nVW-" {
            putchar(c);
        }
        for &c in tag {
            putchar(c);
        }
        for &c in b" slot=" {
            putchar(c);
        }
        pdec(blk_slot as u64);
        let rv = resp_status_byte(resp_status);
        for &c in b" resp=0x" {
            putchar(c);
        }
        pdec(rv as u64);
        let used_idx = used_ring_idx(used_ring);
        for &c in b" used=" {
            putchar(c);
        }
        pdec(used_idx as u64);
        for &c in b" prev_used=" {
            putchar(c);
        }
        pdec(prev_used as u64);
        let (walker_last, unkicked) =
            crate::drivers::virtio::pci_blk_lag_snapshot(blk_slot);
        for &c in b" walker=" {
            putchar(c);
        }
        pdec(walker_last as u64);
        let shadow = crate::drivers::virtio::get_pci_device_queue_at(blk_slot)
            .map(|q| q.avail_shadow_snapshot())
            .unwrap_or(0);
        for &c in b" shadow=" {
            putchar(c);
        }
        pdec(shadow as u64);
        for &c in b" unkicked=" {
            putchar(c);
        }
        pdec(unkicked as u64);
        for &c in b" pc=0x" {
            putchar(c);
        }
        pdec(crate::interrupt::preempt::preempt_count() as u32 as u64);
        putchar(b'\n');
    }

    /// Wait for device to complete request using interrupt-driven sleep.
    ///
    /// Instead of busy-wait polling, the current task sleeps on a wait queue
    /// and is woken by the VirtIO interrupt handler when the device completes
    /// the request. The BKL is released during sleep and re-acquired on wakeup.
    ///
    /// Completion is tracked through THIS request's response byte
    /// (`resp_status`, initialized to 0xFF at submit and written by the
    /// device on completion). Waiting on the shared used-ring index alone is
    /// wrong with concurrent submitters: ANY other request's completion
    /// advances it, releasing this waiter while its own response is still
    /// 0xFF — the caller then reads a garbage status and frees the response
    /// buffer while the device still DMAs into it.
    ///
    /// Callers that hold preempt-disable state (spinlocks — e.g. the VFS
    /// mutation lock over an O_CREAT that reaches here through the journal
    /// write path) never sleep: switching such a task out parks it with the
    /// lock held, and every later acquirer spins with preemption disabled,
    /// so the sleeping holder can never be rescheduled — the jchurn VFS
    /// wedge. They poll instead; the device writes the response via DMA
    /// regardless of CPU IRQ state.
    ///
    /// Pointer contracts (INV-SAFECALL-2 boundary; reads route through
    /// [`used_ring_idx`]/[`resp_done`]):
    /// - `used_ring` must point to a valid VirtIO used ring
    /// - `wait_queue` must be the correct wait queue for this device's interrupt
    /// - `resp_status` must point to this request's response status byte and
    ///   stay valid until this returns
    pub fn wait_for_used_interruptible(
        used_ring: *const UsedRing,
        wait_queue: &crate::process::wait::WaitQueueHead,
        prev_used: u16,
        resp_status: *const u8,
        blk_slot: usize,
    ) -> u16 {
        if used_ring.is_null() {
            return prev_used;
        }

        // Detect early boot phase: no current task or PID 0 (idle/boot thread).
        // During early boot (e.g., ext4 mount before scheduler/IRQ init),
        // interrupts are not enabled and there is no scheduler, so we must
        // use synchronous polling with a large timeout (matching the MMIO
        // path's budget). The normal path relies on interrupt-driven wakeup
        // with a 10s wall-clock deadline.
        let is_early_boot = match crate::sched::current() {
            Some(task) if task.pid() != 0 => false,
            _ => true,
        };

        // Early boot and preempt-disabled callers both poll.
        let poll_only =
            is_early_boot || crate::interrupt::preempt::preempt_count() != 0;

        // POLL-PATH DEADLINE + PERIODIC RECOVERY (the lb66 installs stall):
        // a preempt-guarded poller cannot sleep, but it CAN burn its whole
        // iteration budget (and the caller's retry stack, 441s observed)
        // when the completion never lands. The sleep path escapes via its
        // 10s deadline + kick-and-walk leg; the poll path had NEITHER —
        // poll_only skipped the deadline entirely and its only "recovery"
        // was the caller's post-timeout retry, which re-poll the same dead
        // ring. Now the poll path gets the same discipline, wall-clock
        // based (read_time: TSC-derived, IRQ-independent — a deadline on
        // jiffies would never fire under an irqsave guard):
        //   - every 10 ms: re-kick the device (covers a lost/quiet kick —
        //     an un-kicked chain's completion NEVER arrives no matter how
        //     long we spin) and run the completion walker in this task's
        //     context (covers irq_exit softirq deferral: with a preempt
        //     guard held, irq_exit defers Block softirqs to ksoftirqd,
        //     and ksoftirqd cannot run under our guard on this CPU — the
        //     walker is the last-line consumer when no interrupt dispatch
        //     will ever come; process-context calls take the blocking
        //     acquire, exactly like the drain/rescue paths);
        //   - at the 10 s deadline: same terminal behavior as the sleep
        //     path — return the prev_used sentinel so the caller runs its
        //     bounded late-drain, flags the tombstone, and fails loudly.
        // Early-boot polling keeps the pure spin (no scheduler, no IRQ
        // machinery up yet; its budget stays the 5M-lap cap).
        const POLL_RECOVER_TICKS: u64 =
            10 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;
        const POLL_DEADLINE_TICKS: u64 = 10 * crate::config::TIMER_CLOCK_FREQ_HZ;
        let poll_recover = poll_only && !is_early_boot;
        let poll_t0 = if poll_recover {
            Some(crate::drivers::timer::read_time())
        } else {
            None
        };
        let mut poll_next_recover = poll_t0.map(|t0| t0 + POLL_RECOVER_TICKS);

        // Early boot / lock holders: large timeout for reliable synchronous
        // polling. Under TCG a completion lands within ~10-20ms (device BH
        // runs on the timer cadence); the budget must comfortably exceed
        // that or the poll times out just before completion and the CALLER's
        // late-drain (which runs with the caller's locks held) burns seconds.
        // Normal path: bounded by the 10s wall-clock deadline below, NOT by
        // an iteration count — every Block softirq wakes the whole sync
        // queue, so a 5000-wake cap could expire in well under a second of
        // false wakes and convert healthy I/O into timeout+EIO storms (the
        // AC-2 "leaked multi-block read" failure family). The preempt-guard
        // poll is bounded by POLL_DEADLINE_TICKS above.
        let max_iterations: u64 = if poll_only {
            5_000_000
        } else {
            1 << 40
        };

        // 10s wall-clock deadline for the sleeping path (the
        // IoCompletion::wait discipline): a wakeup timer armed AT the
        // deadline guarantees schedule() returns even if no completion
        // ever wakes us, so a lost interrupt or a wedged device cannot
        // strand a synchronous waiter forever (one lost completion used
        // to strand it permanently — no retry, no timeout, no way out).
        let mut deadline = 0u64;
        let mut deadline_timer = 0u64; // 0 = not armed
        let mut poll_fallback = false; // timer budget exhausted: 1-jiffy re-arms
        // SLEEP-PATH RECOVER LEG (oh-speed, the bistable 10s boot cliff):
        // the waiter-side rescue below only runs when a WAKE arrives; a
        // fully lost interrupt (device completed, no wake ever comes)
        // left the sleeper stranded until the 10s deadline — measured on
        // the OH boots as one 10.00s EXT4-BIGLOCK hold (holder blocked in
        // a single sync read while EVERY other ext4 op queued behind the
        // global lock; samgr InitProfile spent 10037ms locked out; the
        // face hits ~1/8-1/14 boots and turns a 4.5s boot into 14.5s).
        // The poll path got a 10ms re-kick+walk leg (lb66); the sleep
        // path now gets the same discipline at a coarser cadence: a
        // periodic recover WAKE (timer) so the kick/walk/rescue below
        // runs every SLEEP_RECOVER_MS instead of only at the 10s edge.
        // The timer is armed per sleep and deleted on wake — a healthy
        // completion (interrupt wake in µs/ms) never observes it.
        const SLEEP_RECOVER_MS: u64 = 250;
        let mut recover_at = u64::MAX; // gate: only the !poll_only path arms it
        let mut wait_started = 0u64;
        if !poll_only {
            deadline = crate::drivers::timer::get_jiffies()
                .saturating_add(crate::drivers::timer::msecs_to_jiffies(10_000));
            let pid = crate::sched::get_current_pid();
            deadline_timer = crate::timer::add_timer_wakeup(deadline, pid);
            if deadline_timer == 0 {
                poll_fallback = true;
            }
            let now = crate::drivers::timer::get_jiffies();
            wait_started = now;
            recover_at = now
                .saturating_add(crate::drivers::timer::msecs_to_jiffies(SLEEP_RECOVER_MS));
        }

        let result = 'wait_done: {
            for _iteration in 0..max_iterations {
                // Early boot / lock holder: pure spin-poll (no scheduler).
                if poll_only {
                    // SAFETY: resp_done reads the caller-owned response byte.
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }

                    // Wall-clock recovery + deadline (see the constants
                    // block above): every 4096 laps read the TSC clock —
                    // a few ns of overhead per µs of spin — and at each
                    // 10ms boundary re-kick + walk + re-check; at the 10s
                    // deadline give up with the sentinel, exactly like
                    // the sleeping path's sync-deadline leg.
                    if let Some(next_recover) = poll_next_recover.as_mut() {
                        if _iteration & 0xFFF == 0 {
                            let now = crate::drivers::timer::read_time();
                            if now >= *next_recover {
                                let t0 = poll_t0.unwrap_or(now);
                                if now.saturating_sub(t0) >= POLL_DEADLINE_TICKS {
                                    // Terminal: direct-console verdict line
                                    // (pr_err is dropped on the OH boots —
                                    // lb70/lb71 lesson), then the sentinel.
                                    // SAFETY: same reads as the loop above.
                                    Self::vw_pollstall_print(
                                        blk_slot,
                                        resp_status,
                                        used_ring,
                                        prev_used,
                                        b"POLLSTALL-DL",
                                    );
                                    break 'wait_done prev_used;
                                }
                                // Recovery pass: lost/quiet kick, or a
                                // completion the deferred softirq never
                                // consumed. Cheap when caught up.
                                crate::drivers::virtio::pci_blk_kick(blk_slot);
                                crate::drivers::virtio::pci_process_async_completions_slot(
                                    blk_slot,
                                );
                                if resp_done(resp_status) {
                                    break 'wait_done used_ring_idx(used_ring);
                                }
                                *next_recover = now + POLL_RECOVER_TICKS;
                            }
                        }
                    }

                    // lb66 installs hang instrument (2M laps ≈ 40% of the
                    // poll budget with no completion): one direct-console
                    // snapshot per call — the poller's own view of the
                    // rings splits the engine (used advanced past this
                    // request = delivery loss; chain still at_device =
                    // submission/kick loss; resp byte stuck at 0xFF with
                    // the ring drained = descriptor-window reuse).
                    if _iteration == 2_000_000 {
                        Self::vw_pollstall_print(
                            blk_slot,
                            resp_status,
                            used_ring,
                            prev_used,
                            b"POLLSTALL",
                        );
                    }
                    core::hint::spin_loop();
                    continue;
                }

                VW_WAIT_ENTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                // Normal path: get current task and sleep on wait queue.
                // The idle task has SchedPolicy::Idle which enqueue_task() ignores,
                // so sleeping would be a permanent deadlock.
                let current = match crate::sched::current() {
                    Some(task) if task.pid() != 0 => task,
                    _ => {
                        core::hint::spin_loop();
                        continue;
                    }
                };

                // Fast check: our own request may have completed already.
                // SAFETY: resp_done reads the caller-owned response byte.
                if resp_done(resp_status) {
                    break 'wait_done used_ring_idx(used_ring);
                }

                // SLEEP-PATH RECOVER PASS (see the SLEEP_RECOVER_MS block
                // above): the completion has NOT landed and the recover
                // gate is due — re-kick the device (a lost/quiet kick's
                // completion never arrives no matter how long we sleep)
                // and run the completion walker in this task's context
                // (covers a used ring the deferred softirq never consumed
                // AND the lagged-completion freeze the waiter-side rescue
                // below handles per-wake — here it runs even with no wake
                // at all, because the recover TIMER woke us). Mirrors the
                // poll path's 10ms leg at this path's coarser cadence.
                // The forensic print fires only for waits already >=1s
                // outstanding (a genuinely anomalous IO under this rig's
                // µs-ms completion class) so a host-load hiccup cannot
                // spam the serial.
                if crate::drivers::timer::get_jiffies() >= recover_at {
                    let now = crate::drivers::timer::get_jiffies();
                    if now >= wait_started
                        + crate::drivers::timer::msecs_to_jiffies(1_000)
                    {
                        crate::drivers::virtio::vw_report("sync-recover");
                    }
                    crate::drivers::virtio::pci_blk_kick(blk_slot);
                    crate::drivers::virtio::pci_process_async_completions_slot(blk_slot);
                    // SAFETY: resp_done reads the caller-owned response byte.
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }
                    recover_at = now.saturating_add(
                        crate::drivers::timer::msecs_to_jiffies(SLEEP_RECOVER_MS),
                    );
                }

                // Register on the wait queue with the task state set to
                // INTERRUPTIBLE under the queue lock (prepare_to_wait), then
                // re-check the response and truly sleep in schedule().
                //
                // The old code only ran wait_queue.add() — the task state was
                // never set to INTERRUPTIBLE, so schedule() found it still
                // RUNNING and returned IMMEDIATELY: the "wait" was a 5000-lap
                // spin that could expire before a TCG completion (10-20ms)
                // landed. The caller then treated its still-healthy request as
                // timed out, leaked the 64B block, and the retry's fresh
                // descriptor chain overwrote the still-in-flight one — the
                // device silently dropped the corrupted chain and every later
                // waiter on those slots starved (observed: execve of a
                // buffer-cold binary, e.g. toybox cp, hanging forever in this
                // loop with 55s+ of CPU burned).
                //
                // prepare_to_wait closes the lost-wakeup race the old 256-lap
                // spin patched over: the state change and queue insertion are
                // atomic w.r.t. the IRQ handler's wake_up_all().
                wait_queue.prepare_to_wait(current, false, true);

                // Re-check AFTER registering: a completion whose interrupt
                // fired between the fast check above and prepare_to_wait is
                // caught here instead of sleeping on a queue nobody will wake.
                if resp_done(resp_status) {
                    VW_WAIT_RESP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    wait_queue.finish_wait(current);
                    // R36-B2 compensation (IoCompletion::wait discipline): a
                    // racing fire may have enqueued us between prepare and
                    // finish — take ourselves back off the GRQ or nr_running
                    // stays inflated until our next context switch.
                    crate::sched::dequeue_task(&*current);
                    break 'wait_done used_ring_idx(used_ring);
                }

                // Deadline check AFTER prepare_to_wait: we are about to sleep,
                // so this is the last point the deadline can abort without a
                // wake. The deadline timer (or the 1-jiffy fallback re-arm)
                // makes sure schedule() comes back at or after it.
                if (deadline_timer != 0 || poll_fallback)
                    && crate::drivers::timer::get_jiffies() >= deadline
                {
                    wait_queue.finish_wait(current);
                    // Same R36-B2 compensation as above.
                    crate::sched::dequeue_task(&*current);
                    // Final chance: a lost KICK (quiet batch submit nobody
                    // drained) recovers here — kick and walk once, then
                    // re-check before reporting the timeout sentinel.
                    crate::drivers::virtio::vw_report("sync-deadline");
                    crate::drivers::virtio::pci_blk_kick(blk_slot);
                    crate::drivers::virtio::pci_process_async_completions_slot(blk_slot);
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }
                    // prev_used sentinel: caller runs its late-drain and
                    // retry (R21-N2 — never free a buffer the device may
                    // still DMA into).
                    VW_WAIT_TIMEOUT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
break 'wait_done prev_used;
                }

                if poll_fallback {
                    // Re-arm a 1-jiffy wake so the deadline check above runs
                    // every jiffy even without a completion wake.
                    let pid = crate::sched::get_current_pid();
                    let dl = crate::drivers::timer::get_jiffies().saturating_add(1);
                    let id = crate::timer::add_timer_wakeup(dl, pid);
                    if id != 0 {
                        // R54: schedule() now restores the caller's SIE state;
                        // wait-path callers re-arm explicitly (semaphore.rs
                        // discipline) so ticks/IPIs reach this CPU across the
                        // wait loop.
                        crate::arch::cpu::restore_irq(true);
                        crate::sched::schedule();
                        VW_WAIT_SCHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        crate::timer::del_timer(id);
                    } else {
                        // Timer table STILL exhausted: never schedule()
                        // without a wake source — bounded spin with IRQs
                        // enabled (the completion walker can still run) and
                        // re-check.
                        wait_queue.finish_wait(current);
                        crate::arch::cpu::restore_irq(true);
                        for _ in 0..100_000 {
                            core::hint::spin_loop();
                        }
                        continue;
                    }
                } else {
                    // Sleep until woken by interrupt
                    // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
                    //
                    // SLEEP-PATH RECOVER WAKE (see the SLEEP_RECOVER_MS
                    // block): with ONLY the wait-queue as a wake source, a
                    // fully lost interrupt meant schedule() never returned
                    // until the 10s deadline timer. Arm a short one-shot
                    // wakeup so the recover pass above runs every
                    // SLEEP_RECOVER_MS even with no interrupt at all; the
                    // timer is deleted on wake (a healthy completion in
                    // µs/ms never observes it). Timer-table exhaustion
                    // degrades to the old behavior (wait-queue + 10s
                    // deadline) — never worse than before.
                    let rid = {
                        let pid = crate::sched::get_current_pid();
                        let at = crate::drivers::timer::get_jiffies().saturating_add(
                            crate::drivers::timer::msecs_to_jiffies(SLEEP_RECOVER_MS),
                        );
                        crate::timer::add_timer_wakeup(at, pid)
                    };
                    crate::arch::cpu::restore_irq(true);
                    crate::sched::schedule();
                    VW_WAIT_SCHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if rid != 0 {
                        crate::timer::del_timer(rid);
                    }
                }

                // Remove from wait queue and restore RUNNING state, then loop
                // back to re-check the response.
                wait_queue.finish_wait(current);

                // Waiter-side rescue (the ftest01 lost-wakeup fix): a wake
                // whose response is still pending can mean the used ring is
                // ahead of the completion walker AND no interrupt is coming
                // (the device is idle; the walker exited with lag and the
                // softirq pending bits are clear — freeze-dumped live with
                // PENDING_LAST one behind used.idx and six waiters asleep).
                // Drain the lag ourselves before re-sleeping: this is cheap
                // when caught up (two loads) and converts a lost wake into
                // at most one extra loop. No-op on MMIO-only boots.
                crate::drivers::virtio::pci_rescue_lagged_completions(blk_slot);
            }

            // Budget exhausted without seeing OUR response (only reachable
            // on the poll_only spin budgets): the prev_used sentinel.
            // Callers treat `new_used == prev_expected` as "request
            // possibly still in flight" and run their bounded late-drain
            // before erroring (R21-N2: never free a buffer the device may
            // still DMA into — a true timeout leaks the 64B block instead).
            // Returning the current used-ring index here would let
            // concurrent completions of OTHER requests mask our timeout
            // and skip that protection.
            prev_used
        };

        // Deadline-timer bookkeeping on EVERY exit path: an armed timer
        // left behind would fire a stray wakeup into whatever the task is
        // doing ten seconds later.
        if deadline_timer != 0 {
            crate::timer::del_timer(deadline_timer);
        }
        result
    }

    /// Wait for a specific descriptor to appear in the used ring.
    ///
    /// Unlike `wait_for_used_interruptible` which returns when ANY new entry
    /// appears, this function scans the used ring for the entry matching
    /// `expected_desc_id` and only returns when that specific request has
    /// been completed by the device. This is necessary when multiple requests
    /// may be in flight concurrently on the same VirtQueue.
    ///
    /// # Safety
    /// - `used_ring` must point to a valid VirtIO used ring
    /// - `wait_queue` must be the correct wait queue for this device's interrupt
    pub fn wait_for_desc_completion(
        used_ring: *const UsedRing,
        wait_queue: &crate::process::wait::WaitQueueHead,
        start_idx: u16,
        expected_desc_id: u32,
        queue_size: u16,
    ) -> bool {
        if used_ring.is_null() || queue_size == 0 {
            return false;
        }

        // Detect early boot phase (same logic as wait_for_used_interruptible)
        let is_early_boot = match crate::sched::current() {
            Some(task) if task.pid() != 0 => false,
            _ => true,
        };

        // Preempt-disabled callers must not sleep (same wedge discipline as
        // wait_for_used_interruptible): poll the used ring instead.
        let poll_only =
            is_early_boot || crate::interrupt::preempt::preempt_count() != 0;

        let max_iterations = if poll_only { 1_000_000 } else { 5000 };

        for _iteration in 0..max_iterations {
            // Read the current used ring index
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let used_idx = used_ring_idx(used_ring);

            // Scan all new entries since start_idx looking for our descriptor
            let mut scan_idx = start_idx;
            while scan_idx != used_idx {
                let ring_slot = scan_idx as usize % queue_size as usize;
                let entry_id = used_ring_elem_id(used_ring, ring_slot);
                if entry_id == expected_desc_id {
                    return true;  // Our descriptor was completed
                }
                scan_idx = scan_idx.wrapping_add(1);
            }

            // Early boot / lock holder: pure spin-poll
            if poll_only {
                core::hint::spin_loop();
                continue;
            }

            // Normal path: check scheduler availability
            let current = match crate::sched::current() {
                Some(task) if task.pid() != 0 => task,
                _ => {
                    core::hint::spin_loop();
                    continue;
                }
            };

            // Register with the task state set to INTERRUPTIBLE under the
            // queue lock — same discipline as wait_for_used_interruptible
            // (a bare add() never slept: schedule() saw a RUNNING task and
            // returned immediately, turning the wait into a timed spin).
            wait_queue.prepare_to_wait(current, false, true);

            // Fast re-check after registering (lost-wakeup protection):
            // a completion that landed between the scan above and the
            // prepare_to_wait is caught here without sleeping.
            let used_idx2 = used_ring_idx(used_ring);
            let mut si = start_idx;
            while si != used_idx2 {
                let ring_slot = si as usize % queue_size as usize;
                let eid = used_ring_elem_id(used_ring, ring_slot);
                if eid == expected_desc_id {
                    wait_queue.finish_wait(current);
                    // R36-B2 compensation (same as wait_for_used_interruptible
                    // and the wait_event! macro): a racing wake between
                    // prepare_to_wait and this re-check may have enqueued us
                    // while we never slept — take ourselves back off the GRQ
                    // or nr_running stays inflated until our next context
                    // switch. dequeue_task is a no-op when nothing enqueued us.
                    crate::sched::dequeue_task(&*current);
                    return true;
                }
                si = si.wrapping_add(1);
            }

            // Sleep until woken by interrupt
            // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            wait_queue.finish_wait(current);
        }

        false  // Timeout
    }

    /// Add descriptor chain to queue WITHOUT notifying the device.
    ///
    /// For batch submission: publish N chains quietly, then `notify()` once —
    /// every notify is an MMIO trap (device emulation under TCG), so a
    /// 128-block read-ahead window pays one kick instead of 128. Callers MUST
    /// notify before waiting for completions (or rely on the safety kick in
    /// the submit path when too many chains go unked).
    pub fn submit_quiet(&mut self, head_idx: u16) {
        // The slot ordinal comes from the kernel-side shadow, never from a
        // read-back of the shared ring: the vring lives in heap memory, and
        // a stray kernel write that smashed avail.idx made every later
        // submit publish a bogus index — the device then walked garbage
        /// ring slots, read a header-less chain, and stopped completing
        /// anything (permanent I/O wedge).
        let idx = self.avail_shadow.fetch_add(1, Ordering::AcqRel) as usize;
        let ring_idx = idx % self.queue_size as usize;
        let Some(vr) = self.vring.as_ref() else { return };
        // INV-DMA-2/3: bounds-checked volatile writes into the carved
        // avail section (the device reads this memory concurrently).

        // Memory barrier before writing to available ring
        core::sync::atomic::fence(Ordering::Release);

        // Write descriptor head index to available ring
        vr.block
            .write_volatile_at(vr.off_avail + 4 + ring_idx * 2, head_idx);

        // Memory barrier to ensure ring write completes before index update
        core::sync::atomic::fence(Ordering::Release);

        // Update available index (this signals to device that new
        // request is ready). Derived from the shadow — this also
        // REPAIRS a corrupted ring index on every submit.
        let new_idx = (idx as u16).wrapping_add(1);
        vr.block.write_volatile_at(vr.off_avail + 2, new_idx);
    }

    /// Add descriptor chain to queue and notify device
    pub fn submit(&mut self, head_idx: u16) {
        self.submit_quiet(head_idx);

        // Full memory barrier before notify
        core::sync::atomic::fence(Ordering::SeqCst);

        // Notify device
        Self::notify(self);
    }

    /// Get descriptor
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        let vr = self.vring.as_ref()?;
        // INV-DMA-2: bounds-checked disjoint slot read.
        vr.desc.get(idx as usize).copied()
    }

    /// Allocate new descriptor (reclaims from used ring when possible).
    /// Assumes the chain being built consumes 3 descriptors (the virtio-blk
    /// header/data/resp layout) — see `alloc_desc_chain`.
    pub fn alloc_desc(&mut self) -> Option<u16> {
        self.alloc_desc_chain(3)
    }

    /// Allocate the head descriptor of a chain that will consume
    /// `chain_len` descriptors in total.
    ///
    /// R24 (R14 MED "RX 因 blk 的链限流只剩 2 缓冲"): the in-flight limiter
    /// was hard-wired to 3 descriptors per chain. virtio-net RX buffers are
    /// single-descriptor chains, so the RX queue was capped at 2 posted
    /// buffers (2*3+3 > 8) even though the queue holds 8. Passing the real
    /// chain length restores the full ring for 1-desc (RX) and 2-desc
    /// (net TX) users; blk keeps the 3-desc accounting via alloc_desc().
    pub fn alloc_desc_chain(&mut self, _chain_len: u16) -> Option<u16> {
        self.alloc_desc_run_checked(1)
    }

    /// Common admission + allocation core. Reserves `n` CONSECUTIVE
    /// descriptor ids (one free-running fetch_add — callers build chains
    /// that occupy exactly [base, base+n) mod queue_size) after both
    /// admission guards pass:
    ///
    /// 1. the legacy chains-in-flight guard (unchanged semantics: caps the
    ///    MMIO size-8 queue at 2 chains, keeps the PCI queue's coarse
    ///    chain-count bound);
    /// 2. the descriptors-held budget (`descs_held + n <= queue_size`) —
    ///    accurate for SG chains whose length the legacy guard cannot see.
    ///
    /// The held budget counts only PUBLISHED chains, so a reservation here
    /// that later fails to publish must not (and does not) touch it.
    fn alloc_desc_run_checked(&mut self, n: u16) -> Option<u16> {
        let used_idx = self.get_used();
        let avail_idx = self.get_avail();

        // Descriptor allocation is a free-running counter taken modulo
        // queue_size. Do NOT clamp it against the used-ring index: next_desc
        // counts DESCRIPTORS (2-3 per request) while used_idx counts
        // COMPLETED CHAINS — different domains. After next_desc's first
        // u16 wrap (~21k requests) the old clamp
        // `if next_desc < used_idx { next_desc = used_idx }` started to
        // fire and re-based the allocator on the completion watermark,
        // handing out the slots of still-in-flight chains; the overwritten
        // chain head then produced a header-less descriptor chain that the
        // device silently drops — used.idx stalls forever and every later
        // waiter starves (the journal-churn VFS wedge family).
        //
        // Slot safety comes from the in-flight guard below plus the fact
        // that live chains always occupy CONSECUTIVE descriptor ranges: at
        // most two chains may be in flight, so their ranges are adjacent
        // and disjoint (queue_size 8, chains of 2-3 descriptors).
        //
        // Check if all descriptors are in flight (avail - used >= queue_size)
        // Note: indices wrap at u16::MAX, not queue_size.
        // Discount chains that timed out and leaked: they will never
        // advance the used ring (or may do so arbitrarily late), but
        // counting them here forever would starve allocation after two
        // timeouts — the permanent-I/O-wedge cascade. SATURATING, never
        // wrapping: leaked only ever counts UNRESOLVED timed-out chains
        // (see note_timed_out_chain / resolve_leaked_chain), but a stale
        // overcount must clamp in_flight to zero (queue very idle), not
        // wrap it to ~65535 — the old wrapping_sub turned one accounting
        // drift into a PERMANENT "all descriptors in flight" refusal and
        // every submission failed with -5 forever after.
        let leaked = self.leaked_chains.load(Ordering::Acquire);
        let in_flight = avail_idx.wrapping_sub(used_idx).saturating_sub(leaked);
        // Bound by the WORST-CASE chain length (3: blk header/data/resp),
        // not the caller's: a 2-descriptor flush admitted under the old
        // per-caller guard could become the 4th concurrent chain and wrap
        // onto the oldest live chain's slots (R9-5/R10-3 family).
        if in_flight.saturating_mul(3).saturating_add(3) > self.queue_size {
            return None;
        }
        // SG-aware budget (see `descs_held`): the legacy guard above
        // assumes 3 descriptors per chain and under-counts whenever long
        // chains dominate the ring. This is the accurate gate.
        if n > 0 {
            let held = self.descs_held.load(Ordering::Acquire);
            if held.saturating_add(n) > self.queue_size {
                return None;
            }
        }

        let idx = self.next_desc.fetch_add(n, Ordering::AcqRel) % self.queue_size;
        Some(idx)
    }

    /// Allocate `n` consecutive descriptors for one multi-segment chain
    /// (header + n data descriptors + response is n+2 total; callers pass
    /// the TOTAL). Returns the first (head) descriptor index; the chain
    /// occupies [head, head+n) mod queue_size. Under the BLK lock, so the
    /// free-running counter cannot interleave with another chain build.
    pub fn alloc_desc_run(&mut self, n: u16) -> Option<u16> {
        if n == 0 || n > self.queue_size {
            return None;
        }
        self.alloc_desc_run_checked(n)
    }

    /// Mark `n` descriptors as held by a chain being PUBLISHED (pending
    /// store or sync tombstone). Called under the BLK lock after
    /// submit_quiet; pairs with the completion walker's release_descs.
    pub fn hold_descs(&self, n: u16) {
        self.descs_held.fetch_add(n, Ordering::AcqRel);
    }

    /// Release `n` descriptors whose chain's used-ring entry the walker
    /// consumed. Saturating: a stale overcount must clamp to zero, never
    /// wrap (the leaked_chains lesson applied symmetrically).
    pub fn release_descs(&self, n: u16) {
        let _ = self
            .descs_held
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                if v >= n {
                    Some(v - n)
                } else {
                    Some(0)
                }
            });
    }

    /// Snapshot of the held-descriptor count (diagnostics).
    pub fn descs_held_snapshot(&self) -> u16 {
        self.descs_held.load(Ordering::Acquire)
    }

    /// Reclaim descriptors that the device has finished processing.
    ///
    /// Called after I/O completion. Descriptor slots are recycled purely by
    /// the allocator's modulo cycle under the in-flight guard (see
    /// alloc_desc_chain for why next_desc must never be re-based on the
    /// used-ring index).
    pub fn reclaim_descs(&mut self) {
    }

    /// Record that a chain's wait TIMED OUT and its slots are leaked.
    ///
    /// The leaked count discounts the chain from the in-flight admission
    /// guard until its (possibly very late) completion lands; the PCI
    /// completion walker pairs every increment with resolve_leaked_chain
    /// when it consumes a TIMED-OUT tombstone's used-ring entry, so the
    /// counter tracks genuinely unresolved chains instead of accumulating
    /// forever.
    ///
    /// Descriptor-slot safety no longer needs the old next_desc window
    /// skip: timed-out PCI chains keep a tombstone in the pending table,
    /// and pci_pending_slot_reservable refuses to publish any chain whose
    /// descriptor window overlaps a live tombstone's. (The skip also raced
    /// concurrent chain builds: it advanced next_desc from process
    /// context while another CPU held the BLK lock mid-build, breaking
    /// the three-descriptor consecutiveness the window math relies on.)
    pub fn note_timed_out_chain(&self) {
        let n = self.leaked_chains.fetch_add(1, Ordering::AcqRel) + 1;
        // Cap warning: enough leaked chains to pin a meaningful slice of
        // the descriptor ring (queue_size/3 windows) means the device or
        // the completion path is losing I/Os — rate-limited, but loud.
        let cap = (self.queue_size / 3) as u16;
        if n >= cap && (n == cap || (n - cap) % 16 == 0) {
            crate::pr_warn!(
                "virtio-blk: {} chains leaked (timed out, unresolved) — \
                 completion loss suspected",
                n
            );
        }
    }

    /// Pair a note_timed_out_chain increment: a timed-out chain's
    /// completion finally landed (the walker consumed its tombstone's
    /// used-ring entry), so stop discounting it from the admission guard.
    /// Saturating: never decrements past zero.
    pub fn resolve_leaked_chain(&self) {
        let _ = self
            .leaked_chains
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                if v > 0 {
                    Some(v - 1)
                } else {
                    None
                }
            });
    }

    /// Reset descriptor allocator
    ///
    /// Note: This is UNSAFE under concurrent I/O and should only be used
    /// during single-threaded initialization.
    pub fn reset_desc_allocator(&mut self) {
        self.next_desc.store(0, Ordering::Release);
    }

    /// Set descriptor content
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if let Some(vr) = self.vring.as_mut() {
            // INV-DMA-2: bounds-checked disjoint slot write (device
            // reads; the release fence orders the publish).
            if let Some(d) = vr.desc.get_mut(idx as usize) {
                *d = Desc { addr, len, flags, next };
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Get descriptor table address (0 when freed)
    pub fn get_desc_addr(&self) -> u64 {
        self.vring
            .as_ref()
            .map(|vr| vr.desc.as_ptr() as *mut Desc as u64)
            .unwrap_or(0)
    }

    /// Get Available Ring address (0 when freed)
    pub fn get_avail_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.avail.as_ptr() as u64).unwrap_or(0)
    }

    /// Get Used Ring address (0 when freed)
    pub fn get_used_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.used.as_ptr() as u64).unwrap_or(0)
    }

    /// Get vring base address (0 when freed)
    pub fn get_vring_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.block.addr() as u64).unwrap_or(0)
    }

    /// Get queue notification address
    pub fn get_notify_addr(&self) -> u64 {
        self.queue_notify
    }

    // ---- Typed ring access for drivers that program the vring
    // directly (GPU/input control queues). INV-DMA-2: bounds-checked
    // disjoint-slot views; device-visible ring-entry writes are
    // volatile. ----

    /// Bounds-checked mutable view of descriptor `idx` through a
    /// shared queue handle.
    ///
    /// The vring's descriptor table is driver-owned memory that the
    /// device only READS; submission is serialized by each driver's
    /// device lock (the same exclusion the pre-conversion raw-pointer
    /// code relied on). This is the ONE audited reborrow edge for that
    /// pattern (INV-DMA-2: bounds-checked disjoint slot).
    pub(crate) fn desc_mut(&self, idx: u16) -> Option<&mut Desc> {
        let vr = self.vring.as_ref()?;
        let base = vr.desc.as_ptr() as *mut Desc;
        if (idx as usize) < vr.desc.len() {
            // idx bounds-checked above; the slot's range is disjoint
            // from every other slot (INV-DMA-2); caller's device lock
            // provides the exclusion (the audited raw_mut edge).
            Some(crate::drivers::edges::raw_mut(crate::drivers::edges::ptr_add(base, idx as usize)))
        } else {
            None
        }
    }

    /// Mutable view of the available-ring header (driver-owned; same
    /// caller-lock discipline as [`desc_mut`]).
    pub(crate) fn avail_header_mut(&self) -> Option<&mut AvailRing> {
        let vr = self.vring.as_ref()?;
        let base = vr.avail.as_ptr();
        // Disjoint slot (INV-DMA-2); the avail header is driver-written;
        // caller's device lock serializes (the audited raw_mut edge).
        Some(crate::drivers::edges::raw_mut(base))
    }

    /// Volatile write of avail ring slot `ring_idx` (device reads).
    pub(crate) fn write_avail_ring_slot(&self, ring_idx: usize, head: u16) {
        if let Some(vr) = self.vring.as_ref() {
            vr.block
                .write_volatile_at(vr.off_avail + 4 + ring_idx * 2, head);
        }
    }

    /// Volatile write of the avail header idx (publish).
    pub(crate) fn write_avail_idx(&self, new_idx: u16) {
        if let Some(vr) = self.vring.as_ref() {
            vr.block.write_volatile_at(vr.off_avail + 2, new_idx);
        }
    }

    /// Volatile read of the avail header idx (= [`get_avail`]).
    pub(crate) fn avail_idx(&self) -> u16 {
        self.get_avail()
    }
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

impl core::fmt::Display for VirtIOBlkResp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.status {
            0 => write!(f, "OK"),
            1 => write!(f, "IOERR"),
            2 => write!(f, "UNSUPPORTED"),
            _ => write!(f, "UNKNOWN({})", self.status),
        }
    }
}

pub mod req_type {
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

pub mod status {
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}
