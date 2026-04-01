//! IRQ descriptor and action management
//!
//! Core data structures: IrqReturn, IrqAction, IrqData, IrqDesc.
//! Registration API: request_irq, free_irq.
//! Dispatch: handle_irq_event, handle_fasteoi_irq.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
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

/// Per-interrupt hardware state (Linux: struct irq_data).
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
    pub irq_data: Mutex<IrqData>,
    /// Head of the action chain (linked list of handlers)
    pub action: Mutex<Option<Box<IrqAction>>>,
    /// Depth of disable nesting (0 = enabled, >0 = disabled)
    pub depth: AtomicU32,
    /// Per-CPU interrupt counts for /proc/interrupts
    pub per_cpu_count: [AtomicU64; MAX_CPUS],
}

impl IrqDesc {
    /// Create a default IrqDesc. IRQ number fixed up during init().
    pub const fn new() -> Self {
        Self {
            irq_data: Mutex::new(IrqData::new(0)),
            action: Mutex::new(None),
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

/// Get the handler name for an IRQ
pub fn irq_get_name(irq: u32) -> Option<&'static str> {
    if (irq as usize) >= NR_IRQS {
        return None;
    }
    let action = IRQ_DESC_ARRAY[irq as usize].action.lock();
    action.as_ref().map(|a| a.name)
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
    let mut action_guard = desc.action.lock();

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

    // Unmask the IRQ in hardware
    let irq_data = desc.irq_data.lock();
    if let Some(ref chip) = irq_data.chip {
        if let Some(unmask) = chip.irq_unmask {
            unmask(&irq_data);
        }
    }

    Ok(())
}

/// Unregister an interrupt handler.
///
/// # Arguments
/// - `irq`: Virtual IRQ number
/// - `dev_id`: Must match the dev_id passed to request_irq
pub fn free_irq(irq: u32, dev_id: usize) -> Result<(), &'static str> {
    if (irq as usize) >= NR_IRQS {
        return Err("IRQ number out of range");
    }

    let desc = &IRQ_DESC_ARRAY[irq as usize];
    let mut action_guard = desc.action.lock();

    let head = match action_guard.as_mut() {
        None => return Err("No handler registered for this IRQ"),
        Some(h) => h,
    };

    // Case 1: head matches
    if head.dev_id == dev_id {
        let old = action_guard.take().unwrap();
        *action_guard = old.next;
        return Ok(());
    }

    // Case 2: search in chain
    // Collect matching next pointer identity to avoid borrow conflict
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
        return Err("dev_id not found in action chain");
    }

    // Now remove it - walk again and unlink
    let mut current = head;
    loop {
        let dev_id_next = current.next.as_ref().map(|n| n.dev_id);
        match dev_id_next {
            Some(id) if id == dev_id => {
                current.next = current.next.take().unwrap().next;
                return Ok(());
            }
            Some(_) => {
                current = current.next.as_mut().unwrap();
            }
            None => return Err("dev_id not found in action chain"),
        }
    }
}

// ==================== Dispatch ====================

/// Iterate the action chain and call each handler.
fn handle_irq_event(desc: &IrqDesc, irq: u32) -> IrqReturn {
    let action_guard = desc.action.lock();
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

    let cpu = crate::arch::cpu_id() as usize;

    // Increment statistics
    irq_inc_count(irq, cpu);

    // Dispatch to action chain
    handle_irq_event(desc, irq);

    // EOI: signal end of interrupt to hardware
    let irq_data = desc.irq_data.lock();
    if let Some(ref chip) = irq_data.chip {
        if let Some(eoi) = chip.irq_eoi {
            eoi(&irq_data);
        }
    }
}
