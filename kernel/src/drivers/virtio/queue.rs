//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO virtual queue
//!
//! Queue implementation fully compliant with VirtIO specification

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
    /// Descriptor table pointer (at start of contiguous memory block)
    pub(crate) desc: *mut Desc,
    /// Available Ring pointer
    pub(crate) avail: *mut AvailRing,
    /// Used Ring pointer
    pub(crate) used: *mut UsedRing,
    /// vring address
    vring_addr: u64,
    /// Layout of the combined vring allocation (for free_vring)
    vring_layout: alloc::alloc::Layout,
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
}

unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

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

        let layout = alloc::alloc::Layout::from_size_align(total_size, PAGE_SIZE).ok()?;
        // SAFETY: layout is non-zero, page-aligned; null check follows.
        let mem_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
        if mem_ptr.is_null() {
            return None;
        }

        let addr = mem_ptr as usize;
        if addr & (PAGE_SIZE - 1) != 0 {
            crate::println!("virtio: ERROR: vring not page-aligned!");
            // SAFETY: mem_ptr was allocated above with the same layout but failed alignment; reclaim it.
            unsafe { alloc::alloc::dealloc(mem_ptr, layout) };
            return None;
        }

        let desc = mem_ptr as *mut Desc;
        // SAFETY: pointers are within the allocated region; offsets are computed from
        // page-aligned sizes and do not exceed total_size.
        let avail = unsafe { (mem_ptr as usize + desc_size_aligned) as *mut AvailRing };
        let used = unsafe { (mem_ptr as usize + desc_size_aligned + avail_size_aligned) as *mut UsedRing };

        // SAFETY: avail and used are within the allocated region; initializing ring fields.
        unsafe {
            (*avail).flags = 0;
            (*avail).idx = 0;
            (*used).flags = 0;
            (*used).idx = 0;
        }

        for i in 0..queue_size {
            // SAFETY: i < queue_size, and desc points to queue_size descriptors in the allocation.
            unsafe {
                *desc.add(i as usize) = Desc { addr: 0, len: 0, flags: 0, next: 0 };
            }
        }

        Some(Self {
            queue_size,
            queue_index,
            queue_notify,
            notify_width,
            interrupt_status,
            interrupt_ack,
            desc,
            avail,
            used,
            vring_addr: mem_ptr as u64,
            vring_layout: layout,
            next_desc: AtomicU16::new(0),
            leaked_chains: AtomicU16::new(0),
            avail_shadow: AtomicU16::new(0),
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
    /// # Safety
    /// The queue must not be registered with a device (no in-flight DMA).
    pub unsafe fn free_vring(&mut self) {
        if !self.desc.is_null() {
            // SAFETY: vring_layout is the exact layout used for the single
            // allocation that desc/avail/used all point into.
            unsafe {
                alloc::alloc::dealloc(self.desc as *mut u8, self.vring_layout);
            }
            self.desc = core::ptr::null_mut();
            self.avail = core::ptr::null_mut();
            self.used = core::ptr::null_mut();
        }
    }

    /// Get current available index
    /// Get current available index
    pub fn get_avail(&self) -> u16 {
        // SAFETY: avail is set in new() and points to the VirtIO available ring in our allocation.
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) }
    }

    /// Get current used index
    pub fn get_used(&self) -> u16 {
        // SAFETY: used is set in new() and points to the VirtIO used ring in our allocation.
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.used).idx)) }
    }

    /// Get element from used ring
    ///
    /// # Parameters
    /// - `idx`: Index in used ring
    ///
    /// # Returns
    /// UsedElem containing descriptor ID and length
    pub fn get_used_elem(&self, idx: u16) -> Option<UsedElem> {
        if self.used.is_null() {
            return None;
        }

        // SAFETY: idx is wrapped to queue_size; ring_base is within the used ring allocation.
        unsafe {
            // Used ring structure: flags (2) + idx (2) + ring (queue_size * 8)
            let ring_base = (self.used as usize) + 4;
            let elem_ptr = (ring_base + (idx % self.queue_size) as usize * 8) as *const UsedElem;
            Some(core::ptr::read_volatile(elem_ptr))
        }
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
        // SAFETY: reading sie and clearing SEIE (bit 9) via a register-based
        // csrc is valid in S-mode; the mask register is a scratch local.
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

        // RISC-V MMIO fence: fence w, o
        // This ensures all previous writes (to descriptor table, available ring)
        // are visible before the MMIO write to the notify register.
        // MMIO write fence: RISCV_FENCE(w, o)
        #[cfg(feature = "riscv64")]
        unsafe {
            core::arch::asm!("fence w, o");
        }

        // SAFETY: queue_notify is a valid MMIO address for the VirtIO notify register;
        // volatile write ensures the store reaches the device.
        unsafe {
            match self.notify_width {
                NotifyWidth::W16 => {
                    // virtio-pci notify capability (matches Linux iowrite16)
                    let queue_notify = self.queue_notify as *mut u16;
                    core::ptr::write_volatile(queue_notify, self.queue_index);
                }
                NotifyWidth::W32 => {
                    // virtio-mmio QueueNotify: QEMU drops any access != 4 bytes
                    let queue_notify = self.queue_notify as *mut u32;
                    core::ptr::write_volatile(queue_notify, self.queue_index as u32);
                }
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
        unsafe {
            core::arch::asm!("fence iorw, iorw");
        }

        // Restore interrupts
        #[cfg(feature = "riscv64")]
        // SAFETY: writing sie CSR to restore the previous interrupt-enable state is valid on S-mode.
        unsafe {
            core::arch::asm!(
                "csrw sie, {sie}",
                sie = in(reg) sie_backup,
            );
        }
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
        if self.used.is_null() {
            return prev_used;
        }

        let mut timeout = max_iters;

        loop {
            // Use memory barrier to ensure read ordering
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

            // SAFETY: used points to the VirtIO used ring in our allocation;
            // offset 2 is the idx field (u16), within the used ring structure.
            let used_idx = unsafe {
                let used_idx_ptr = (self.used as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };

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

    /// Get raw pointer to used ring (for interrupt-driven wait outside queue lock)
    pub fn used_ring_ptr(&self) -> *const UsedRing {
        self.used
    }

    /// Kernel-side submission ordinal snapshot (VW forensic
    /// instrumentation): equals avail.idx under a healthy submit path.
    pub fn avail_shadow_snapshot(&self) -> u16 {
        self.avail_shadow.load(core::sync::atomic::Ordering::Acquire)
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
    /// # Safety
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

        /// True once the device has written THIS request's response.
        ///
        /// # Safety
        /// `resp_status` points to the caller's response byte which the
        /// device writes (DMA) exactly once, on completion of THIS request.
        unsafe fn resp_done(resp_status: *const u8) -> bool {
            if resp_status.is_null() {
                return false;
            }
            // SAFETY: see above; volatile so each poll re-reads DMA memory.
            core::ptr::read_volatile(resp_status) != 0xFF
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

        // Early boot / lock holders: large timeout for reliable synchronous
        // polling. Under TCG a completion lands within ~10-20ms (device BH
        // runs on the timer cadence); the budget must comfortably exceed
        // that or the poll times out just before completion and the CALLER's
        // late-drain (which runs with the caller's locks held) burns seconds.
        // Normal path: bounded by the 10s wall-clock deadline below, NOT by
        // an iteration count — every Block softirq wakes the whole sync
        // queue, so a 5000-wake cap could expire in well under a second of
        // false wakes and convert healthy I/O into timeout+EIO storms (the
        // AC-2 "leaked multi-block read" failure family).
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
        if !poll_only {
            deadline = crate::drivers::timer::get_jiffies()
                .saturating_add(crate::drivers::timer::msecs_to_jiffies(10_000));
            let pid = crate::sched::get_current_pid();
            deadline_timer = crate::timer::add_timer_wakeup(deadline, pid);
            if deadline_timer == 0 {
                poll_fallback = true;
            }
        }

        let result = 'wait_done: {
            for _iteration in 0..max_iterations {
                // Early boot / lock holder: pure spin-poll (no scheduler).
                if poll_only {
                    // SAFETY: resp_done reads the caller-owned response byte.
                    if unsafe { resp_done(resp_status) } {
                        // SAFETY: used_ring offset 2 is the idx field.
                        break 'wait_done unsafe {
                            let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                            core::ptr::read_volatile(used_idx_ptr)
                        };
                    }
                    core::hint::spin_loop();
                    continue;
                }

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
                if unsafe { resp_done(resp_status) } {
                    // SAFETY: used_ring offset 2 is the idx field.
                    break 'wait_done unsafe {
                        let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                        core::ptr::read_volatile(used_idx_ptr)
                    };
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
                if unsafe { resp_done(resp_status) } {
                    wait_queue.finish_wait(current);
                    // R36-B2 compensation (IoCompletion::wait discipline): a
                    // racing fire may have enqueued us between prepare and
                    // finish — take ourselves back off the GRQ or nr_running
                    // stays inflated until our next context switch.
                    crate::sched::dequeue_task(&*current);
                    // SAFETY: used_ring offset 2 is the idx field.
                    break 'wait_done unsafe {
                        let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                        core::ptr::read_volatile(used_idx_ptr)
                    };
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
                    if unsafe { resp_done(resp_status) } {
                        // SAFETY: used_ring offset 2 is the idx field.
                        break 'wait_done unsafe {
                            let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                            core::ptr::read_volatile(used_idx_ptr)
                        };
                    }
                    // prev_used sentinel: caller runs its late-drain and
                    // retry (R21-N2 — never free a buffer the device may
                    // still DMA into).
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
                    crate::arch::cpu::restore_irq(true);
                    crate::sched::schedule();
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
            let used_idx = unsafe {
                let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };

            // Scan all new entries since start_idx looking for our descriptor
            let mut scan_idx = start_idx;
            while scan_idx != used_idx {
                let ring_slot = scan_idx as usize % queue_size as usize;
                // SAFETY: UsedRing is [flags:u16, idx:u16, ring:UsedElem[]];
                // offset 4 + ring_slot * 8 is within the allocated ring.
                let entry_id = unsafe {
                    let elem_ptr = (used_ring as usize + 4 + ring_slot * 8) as *const u32;
                    core::ptr::read_volatile(elem_ptr)
                };
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
            let used_idx2 = unsafe {
                let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };
            let mut si = start_idx;
            while si != used_idx2 {
                let ring_slot = si as usize % queue_size as usize;
                let eid = unsafe {
                    let elem_ptr = (used_ring as usize + 4 + ring_slot * 8) as *const u32;
                    core::ptr::read_volatile(elem_ptr)
                };
                if eid == expected_desc_id {
                    wait_queue.finish_wait(current);
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
        // SAFETY: avail points to the available ring in our allocation; the ring
        // pointer at offset 4 is within the allocated region; all volatile
        // writes target fields of the VirtIO vring we own.
        unsafe {
            let avail = &mut *self.avail;

            // Memory barrier before writing to available ring
            core::sync::atomic::fence(Ordering::Release);

            // Write descriptor head index to available ring
            let ring_ptr = (self.avail as usize + 4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(ring_idx), head_idx);

            // Memory barrier to ensure ring write completes before index update
            core::sync::atomic::fence(Ordering::Release);

            // Update available index (this signals to device that new
            // request is ready). Derived from the shadow — this also
            // REPAIRS a corrupted ring index on every submit.
            let new_idx = (idx as u16).wrapping_add(1);
            core::ptr::write_volatile(&mut (*avail).idx as *mut u16, new_idx);
        }
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
        if idx < self.queue_size {
            // SAFETY: idx < queue_size, desc points to queue_size descriptors in the allocation.
            unsafe { Some(*self.desc.add(idx as usize)) }
        } else {
            None
        }
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

        let idx = self.next_desc.fetch_add(1, Ordering::AcqRel) % self.queue_size;
        Some(idx)
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
        if idx < self.queue_size {
            // SAFETY: idx < queue_size, desc points to queue_size descriptors in the allocation.
            unsafe {
                *self.desc.add(idx as usize) = Desc { addr, len, flags, next };
            }
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        }
    }

    /// Get descriptor table address
    pub fn get_desc_addr(&self) -> u64 {
        self.desc as u64
    }

    /// Get Available Ring address
    pub fn get_avail_addr(&self) -> u64 {
        self.avail as u64
    }

    /// Get Used Ring address
    pub fn get_used_addr(&self) -> u64 {
        self.used as u64
    }

    /// Get vring base address
    pub fn get_vring_addr(&self) -> u64 {
        self.vring_addr
    }

    /// Get queue notification address
    pub fn get_notify_addr(&self) -> u64 {
        self.queue_notify
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
