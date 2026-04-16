//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IRQ descriptor and action management
//!
//! Core data structures: IrqReturn, IrqAction, IrqData, IrqDesc.
//! Registration API: request_irq, free_irq.
//! Dispatch: handle_irq_event, handle_fasteoi_irq.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;
use alloc::boxed::Box;

use crate::config::PLIC_MAX_INTERRUPTS;
use crate::config::MAX_CPUS;

/// Maximum number of IRQs
const NR_IRQS: usize = PLIC_MAX_INTERRUPTS;

// ==================== Return value ====================

/// Return value from an interrupt handler.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqReturn {
    /// Interrupt was not from this device
    None = 0,
    /// Interrupt was handled
    Handled = 1,
    /// Handler requests wake of a thread (reserved for Phase 4)
    WakeThread = 2,
}

// ==================== IRQ flags ====================

/// Allow sharing this IRQ line with other devices
pub const IRQF_SHARED: u32 = 0x00000001;

// ==================== IrqData ====================

/// Per-interrupt hardware state.
/// Embedded inside IrqDesc.
#[repr(C)]
pub struct IrqData {
    /// Virtual IRQ number
    pub irq: u32,
    /// Hardware IRQ number
    pub hwirq: u32,
    /// Pointer to the irq_chip
    pub chip: Option<&'static crate::interrupt::irqchip::IrqChip>,
    /// Opaque chip-private data
    pub chip_data: usize,
}

impl IrqData {
    pub const fn new(irq: u32) -> Self {
        Self {
            irq,
            hwirq: irq,
            chip: None,
            chip_data: 0,
        }
    }
}

// ==================== IrqAction ====================

/// Interrupt action descriptor.
/// One per handler registered via request_irq.
/// Actions form a singly-linked list for shared interrupts.
pub struct IrqAction {
    /// The interrupt handler function
    pub handler: fn(u32, usize) -> IrqReturn,
    /// Device-specific cookie
    pub dev_id: usize,
    /// Human-readable name (shown in /proc/interrupts)
    pub name: &'static str,
    /// Flags (IRQF_SHARED, etc.)
    pub flags: u32,
    /// Next action in the chain (for shared interrupts)
    pub next: Option<Box<IrqAction>>,
}

// ==================== IrqDesc ====================

/// Interrupt descriptor. One per virtual IRQ number.
/// Stored in a static array indexed by IRQ number.
pub struct IrqDesc {
    /// Hardware/data state for this interrupt
    pub irq_data: Spinlock<IrqData>,
    /// Head of the action chain (linked list of handlers)
    pub action: Spinlock<Option<Box<IrqAction>>>,
    /// Depth of disable nesting (0 = enabled, >0 = disabled)
    pub depth: AtomicU32,
    /// Per-CPU interrupt counts for /proc/interrupts
    pub per_cpu_count: [AtomicU64; MAX_CPUS],
}

impl IrqDesc {
    /// Create a default IrqDesc. IRQ number fixed up during init().
    pub const fn new() -> Self {
        Self {
            irq_data: Spinlock::new(IrqData::new(0)),
            action: Spinlock::new(None),
            depth: AtomicU32::new(0),
            per_cpu_count: [const { AtomicU64::new(0) }; MAX_CPUS],
        }
    }
}

// ==================== Static array ====================

/// Global interrupt descriptor table. Indexed by virtual IRQ number.
static IRQ_DESC_ARRAY: [IrqDesc; NR_IRQS] = {
    const INIT: IrqDesc = IrqDesc::new();
    [INIT; NR_IRQS]
};

/// Initialize the irq_desc array.
/// Called once during boot, before any driver registration.
pub fn init() {
    for i in 0..NR_IRQS {
        let mut data = IRQ_DESC_ARRAY[i].irq_data.lock();
        data.irq = i as u32;
        data.hwirq = i as u32;
    }
}

// ==================== Lookup ====================

/// Get a reference to the irq_desc for a given virq.
pub fn irq_to_desc(irq: u32) -> Option<&'static IrqDesc> {
    if (irq as usize) < NR_IRQS {
        Some(&IRQ_DESC_ARRAY[irq as usize])
    } else {
        None
    }
}

// ==================== Statistics ====================

/// Increment per-CPU counter for an IRQ
#[inline]
pub fn irq_inc_count(irq: u32, cpu: usize) {
    if (irq as usize) < NR_IRQS && cpu < MAX_CPUS {
        IRQ_DESC_ARRAY[irq as usize].per_cpu_count[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Get per-CPU count for an IRQ
#[inline]
pub fn irq_get_count(irq: u32, cpu: usize) -> u64 {
    if (irq as usize) < NR_IRQS && cpu < MAX_CPUS {
        IRQ_DESC_ARRAY[irq as usize].per_cpu_count[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Get the handler name for an IRQ.
///
/// Acquires action lock to safely read the name field.
pub fn irq_get_name(irq: u32) -> Option<&'static str> {
    if (irq as usize) >= NR_IRQS {
        return None;
    }
    let action_guard = IRQ_DESC_ARRAY[irq as usize].action.lock();
    action_guard.as_ref().map(|a| a.name)
}

// ==================== Registration ====================

/// Register an interrupt handler.
///
/// # Arguments
/// - `irq`: Virtual IRQ number
/// - `handler`: Function called when the interrupt fires
/// - `flags`: IRQF_* flags
/// - `name`: Device name for /proc/interrupts
/// - `dev_id`: Device cookie for shared interrupt demux and free_irq
pub fn request_irq(
    irq: u32,
    handler: fn(u32, usize) -> IrqReturn,
    flags: u32,
    name: &'static str,
    dev_id: usize,
) -> Result<(), &'static str> {
    if (irq as usize) >= NR_IRQS {
        return Err("IRQ number out of range");
    }

    let desc = &IRQ_DESC_ARRAY[irq as usize];
    // Use lock_irqsave: action lock is also acquired in handle_irq_event
    // which runs in IRQ context on any CPU.
    let mut action_guard = desc.action.lock_irqsave();

    let new_action = Box::new(IrqAction {
        handler,
        dev_id,
        name,
        flags,
        next: None,
    });

    match action_guard.as_mut() {
        None => {
            // First handler for this IRQ
            *action_guard = Some(new_action);
        }
        Some(existing) => {
            // Shared IRQ: check compatibility
            if existing.flags & IRQF_SHARED == 0 || flags & IRQF_SHARED == 0 {
                return Err("IRQ already in use and not shared");
            }
            // Walk to end of chain
            let mut tail = existing;
            loop {
                if tail.dev_id == dev_id {
                    return Err("dev_id already registered on this IRQ");
                }
                let has_next = tail.next.is_some();
                if !has_next {
                    break;
                }
                tail = tail.next.as_mut().unwrap();
            }
            tail.next = Some(new_action);
        }
    }

    drop(action_guard);

    // Unmask the IRQ in hardware (irq_data lock is also taken in
    // handle_fasteoi_irq during interrupt dispatch).
    let irq_data = desc.irq_data.lock_irqsave();
    if let Some(ref chip) = irq_data.chip {
        if let Some(unmask) = chip.irq_unmask {
            unmask(&irq_data);
        }
    }

    Ok(())
}

/// Unregister an interrupt handler.
///
/// 1. Mask the IRQ in hardware to prevent new interrupts
/// 2. Acquire action lock and remove handler
/// 3. Increment depth (disable nesting) to prevent re-enable
/// 4. If no handlers remain, keep IRQ disabled; otherwise unmask
///
/// # Arguments
/// - `irq`: Virtual IRQ number
/// - `dev_id`: Must match the dev_id passed to request_irq
pub fn free_irq(irq: u32, dev_id: usize) -> Result<(), &'static str> {
    if (irq as usize) >= NR_IRQS {
        return Err("IRQ number out of range");
    }

    let desc = &IRQ_DESC_ARRAY[irq as usize];

    // Step 1: Mask IRQ in hardware and increment disable depth
    {
        let irq_data = desc.irq_data.lock_irqsave();
        if let Some(ref chip) = irq_data.chip {
            if let Some(mask) = chip.irq_mask {
                mask(&irq_data);
            }
        }
    }
    desc.depth.fetch_add(1, Ordering::Relaxed);

    // Step 2: Synchronize — spin until action lock is free (no in-flight handler).
    // handle_irq_event acquires action lock with lock_irqsave, so once we
    // acquire it here, any in-progress handler for this IRQ has finished.
    let mut action_guard = desc.action.lock_irqsave();

    let head = match action_guard.as_mut() {
        None => {
            // No handler — undo depth and return error
            desc.depth.fetch_sub(1, Ordering::Relaxed);
            return Err("No handler registered for this IRQ");
        }
        Some(h) => h,
    };

    // Case 1: head matches
    if head.dev_id == dev_id {
        let old = action_guard.take().unwrap();
        let was_last = old.next.is_none();
        *action_guard = old.next;
        drop(action_guard);

        // If handlers remain, unmask and restore depth
        if !was_last {
            desc.depth.fetch_sub(1, Ordering::Relaxed);
            let irq_data = desc.irq_data.lock_irqsave();
            if let Some(ref chip) = irq_data.chip {
                if let Some(unmask) = chip.irq_unmask {
                    unmask(&irq_data);
                }
            }
        }
        return Ok(());
    }

    // Case 2: search in chain
    let mut found = false;
    {
        let mut current = &*head;
        while let Some(ref next) = current.next {
            if next.dev_id == dev_id {
                found = true;
                break;
            }
            current = next;
        }
    }

    if !found {
        desc.depth.fetch_sub(1, Ordering::Relaxed);
        // Unmask since we didn't actually remove anything
        let irq_data = desc.irq_data.lock_irqsave();
        if let Some(ref chip) = irq_data.chip {
            if let Some(unmask) = chip.irq_unmask {
                unmask(&irq_data);
            }
        }
        return Err("dev_id not found in action chain");
    }

    // Remove the matching entry
    let mut current = head;
    loop {
        let dev_id_next = current.next.as_ref().map(|n| n.dev_id);
        match dev_id_next {
            Some(id) if id == dev_id => {
                current.next = current.next.take().unwrap().next;
                break;
            }
            Some(_) => {
                current = current.next.as_mut().unwrap();
            }
            None => {
                desc.depth.fetch_sub(1, Ordering::Relaxed);
                return Err("dev_id not found in action chain");
            }
        }
    }

    drop(action_guard);

    // Handlers remain — unmask and restore depth
    desc.depth.fetch_sub(1, Ordering::Relaxed);
    let irq_data = desc.irq_data.lock_irqsave();
    if let Some(ref chip) = irq_data.chip {
        if let Some(unmask) = chip.irq_unmask {
            unmask(&irq_data);
        }
    }
    Ok(())
}

// ==================== Dispatch ====================

/// Iterate the action chain and call each handler.
///
/// Acquires the action spinlock to prevent concurrent modification by
/// `free_irq`. Uses lock_irqsave because this runs in IRQ context.
fn handle_irq_event(desc: &IrqDesc, irq: u32) -> IrqReturn {
    let action_guard = desc.action.lock_irqsave();
    let mut retval = IrqReturn::None;
    let mut action = action_guard.as_ref();
    while let Some(act) = action {
        let ret = (act.handler)(irq, act.dev_id);
        if ret == IrqReturn::Handled {
            retval = IrqReturn::Handled;
        }
        action = act.next.as_ref();
    }
    retval
}

/// Flow handler for fast EOI (suitable for PLIC level-triggered).
/// Called from generic_handle_domain_irq after hwirq→virq lookup.
pub fn handle_fasteoi_irq(irq: u32) {
    let desc = match irq_to_desc(irq) {
        Some(d) => d,
        None => return,
    };

    // Skip if IRQ is disabled (depth > 0)
    if desc.depth.load(Ordering::Relaxed) > 0 {
        // Still must EOI to prevent PLIC from starving
        let irq_data = desc.irq_data.lock_irqsave();
        if let Some(ref chip) = irq_data.chip {
            if let Some(eoi) = chip.irq_eoi {
                eoi(&irq_data);
            }
        }
        return;
    }

    let cpu = crate::arch::cpu_id() as usize;

    // Increment statistics
    irq_inc_count(irq, cpu);

    // Dispatch to action chain
    handle_irq_event(desc, irq);

    // EOI: signal end of interrupt to hardware
    let irq_data = desc.irq_data.lock_irqsave();
    if let Some(ref chip) = irq_data.chip {
        if let Some(eoi) = chip.irq_eoi {
            eoi(&irq_data);
        }
    }
}

// ==================== NMI ====================

/// Maximum number of NMI handler slots
const NR_NMI: usize = 4;

/// NMI handler function type — no arguments, no return value.
/// NMI handlers must be lock-free and non-blocking.
type NmiHandler = fn();

/// NMI handler slots (write-once at init, read-only during dispatch).
/// Protected by NMI_LOCK during registration; no lock needed during dispatch.
static mut NMI_HANDLERS: [Option<NmiHandler>; NR_NMI] = [None; NR_NMI];
static mut NMI_COUNT: usize = 0;

/// Register an NMI handler.
///
/// Returns the slot index on success, or an error string on failure.
/// Handlers must be registered before NMIs can fire (i.e., during boot init).
pub fn request_nmi(_name: &'static str, handler: NmiHandler) -> Result<usize, &'static str> {
    // BKL serializes registration — no extra lock needed
    // SAFETY: BKL serializes all request_nmi calls; NMI_COUNT is only mutated here.
    if unsafe { NMI_COUNT } >= NR_NMI {
        return Err("No free NMI handler slots");
    }
    let idx = unsafe { NMI_COUNT };
    // SAFETY: BKL serializes registration; idx is bounds-checked above;
    // NMI_HANDLERS slot is unused and will be written before any NMI can fire.
    unsafe {
        NMI_HANDLERS[idx] = Some(handler);
        NMI_COUNT += 1;
    }
    Ok(idx)
}

/// Unregister an NMI handler by slot index.
pub fn free_nmi(index: usize) -> Result<(), &'static str> {
    if index >= NR_NMI {
        return Err("NMI index out of range");
    }
    // SAFETY: index is bounds-checked above; BKL serializes free_nmi with request_nmi.
    unsafe {
        NMI_HANDLERS[index] = None;
    }
    Ok(())
}

/// NMI flow handler — lock-free dispatch.
///
/// Called from architecture-specific NMI entry code.
/// No EOI, no statistics, no softirq invocation on exit.
pub fn handle_fasteoi_nmi() {
    super::preempt::irqentry_nmi_enter();

    for i in 0..NR_NMI {
        // SAFETY: NMI_HANDLERS is only written during boot (request_nmi, serialized by BKL);
        // read during NMI dispatch sees fully initialized Option<NmiHandler>.
        if let Some(handler) = unsafe { NMI_HANDLERS[i] } {
            handler();
        }
    }

    super::preempt::irqentry_nmi_exit();
}

/// Trigger an NMI backtrace on the given CPU mask.
///
/// On QEMU virt (no Smrnmi extension), this is a stub that only
/// dumps the current CPU's stack. On real hardware, it would send
/// an NMI IPI to the target CPUs.
pub fn arch_trigger_cpumask_backtrace(_cpus: u64) {
    // QEMU virt has no NMI support — stub only.
    // On hardware with Smrnmi, send NMI IPI to target CPUs.
    crate::pr_debug!("NMI backtrace: stub (no Smrnmi on QEMU virt)");
}
