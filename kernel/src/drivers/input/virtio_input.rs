//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO Input device driver
//!
//! Implements VirtIO Input PCI device initialization and event reading

use crate::println;
use crate::drivers::pci;
use crate::drivers::virtio::virtio_pci::VirtIOPCI;
use crate::drivers::virtio::queue::VirtQueue;
use crate::drivers::virtio::offset;
use crate::drivers::virtio::offset::status;
use super::event::*;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

// ============================================================================
// VirtIO Input PCI device IDs
// ============================================================================

/// VirtIO Input device Vendor ID (Red Hat)
const VIRTIO_INPUT_PCI_VENDOR: u16 = 0x1AF4;

/// VirtIO Input device Device ID (0x1040 + 18 = 0x1052)
const VIRTIO_INPUT_PCI_DEVICE: u16 = 0x1052;

// ============================================================================
// VirtIO Input queue indices
// ============================================================================

/// Event queue (device -> driver)
const EVENT_QUEUE: u16 = 0;
/// Status queue (driver -> device)
const STATUS_QUEUE: u16 = 1;

// ============================================================================
// VirtIO Input configuration structures
// ============================================================================

/// VirtIO Input configuration registers
#[repr(C)]
struct VirtioInputConfig {
    /// Configuration select register
    select: u8,
    /// Sub-select register
    subsel: u8,
    /// Data size
    size: u8,
    /// Reserved
    reserved: [u8; 5],
    /// Configuration data (union)
    payload: [u8; 128],
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

// ============================================================================
// Configuration selectors
// ============================================================================

/// Unused configuration
const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
/// ID name string
const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
/// ID serial number string
const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
/// ID device IDs
const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
/// Property bitmap
const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
/// Event bitmap
const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
/// Absolute axis info
const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;

// ============================================================================
// VirtIO Input device
// ============================================================================

/// VirtIO Input device
pub struct VirtioInputDevice {
    /// VirtIO PCI device
    pci: VirtIOPCI,
    /// Event queue
    event_queue: Option<VirtQueue>,
    /// Event buffer
    event_buffer: *mut VirtioInputEvent,
    /// Event buffer layout
    event_buffer_layout: Option<Layout>,
    /// Event buffer physical address
    event_buffer_phys: u64,
    /// Device name
    name: [u8; 32],
    /// Whether it is a pointer device (mouse/touchscreen)
    is_pointer: bool,
    /// Last processed used index
    last_used: u16,
}

unsafe impl Send for VirtioInputDevice {}
unsafe impl Sync for VirtioInputDevice {}

impl VirtioInputDevice {
    /// Create new VirtIO Input device
    pub fn new(pci: VirtIOPCI) -> Option<Self> {
        let mut device = Self {
            pci,
            event_queue: None,
            event_buffer: core::ptr::null_mut(),
            event_buffer_layout: None,
            event_buffer_phys: 0,
            name: [0; 32],
            is_pointer: false,
            last_used: 0,
        };

        device.init_virtio()?;
        device.read_device_info();

        Some(device)
    }

    /// Initialize VirtIO device
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;

        // Step 1: Reset device
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, 0);
        }
        fence(Ordering::SeqCst);

        // Steps 2-3: Set ACKNOWLEDGE | DRIVER
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // Steps 4-6: Feature negotiation (no special features needed)
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // Step 7: Verify FEATURES_OK
        let status_val = unsafe {
            read_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *const u8)
        };
        if (status_val & status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // Step 8: Initialize event queue
        unsafe {
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16,
                EVENT_QUEUE,
            );
        }
        fence(Ordering::SeqCst);

        let queue_size = unsafe {
            read_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16)
        };

        if queue_size == 0 {
            return None;
        }

        // Allocate event buffer
        let buffer_layout = Layout::from_size_align(
            queue_size as usize * core::mem::size_of::<VirtioInputEvent>(),
            4096,
        ).ok()?;

        let event_buffer = unsafe { alloc_zeroed(buffer_layout) };
        if event_buffer.is_null() {
            return None;
        }

        self.event_buffer = event_buffer as *mut VirtioInputEvent;
        self.event_buffer_layout = Some(buffer_layout);

        // Get physical address
        self.event_buffer_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(event_buffer as u64)
        ).0;

        // Create VirtQueue
        let notify_base = self.pci.notify_cfg_bar + self.pci.notify_cfg_offset as u64;
        let notify_offset = (EVENT_QUEUE as u64) * (self.pci.notify_off_multiplier as u64) * 2;
        let isr_base = self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64;

        let queue = VirtQueue::new(
            queue_size,
            EVENT_QUEUE,
            notify_base + notify_offset,
            isr_base,
            isr_base + 4,
        )?;

        // Get queue physical addresses
        let desc_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.desc as u64 })
        ).0;
        let avail_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.avail as u64 })
        ).0;
        let used_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.used as u64 })
        ).0;

        // Set queue addresses
        unsafe {
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32,
                desc_phys as u32,
            );
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32,
                (desc_phys >> 32) as u32,
            );
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32,
                avail_phys as u32,
            );
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32,
                (avail_phys >> 32) as u32,
            );
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32,
                used_phys as u32,
            );
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32,
                (used_phys >> 32) as u32,
            );
        }
        fence(Ordering::SeqCst);

        // Enable queue
        unsafe {
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16,
                1,
            );
        }
        fence(Ordering::SeqCst);

        self.event_queue = Some(queue);

        // Step 9: Set DRIVER_OK
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // Submit initial buffers to receive events
        self.submit_event_buffers();

        Some(())
    }

    /// Read device info
    fn read_device_info(&mut self) {
        let config_base = self.pci.common_cfg_bar;

        // Read device name
        unsafe {
            // Select ID_NAME configuration
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_ID_NAME);
            write_volatile((config_base + 1) as *mut u8, 0);
            fence(Ordering::SeqCst);

            // Read name
            let payload = (config_base + 8) as *const u8;
            for i in 0..31 {
                let c = read_volatile(payload.add(i));
                if c == 0 {
                    break;
                }
                self.name[i] = c;
            }
        }

        // Detect if it is a pointer device
        self.is_pointer = self.check_pointer_device();
    }

    /// Check if it is a pointer device
    fn check_pointer_device(&self) -> bool {
        let config_base = self.pci.common_cfg_bar;

        unsafe {
            // Check EV_ABS event (absolute coordinates)
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_EV_BITS);
            write_volatile((config_base + 1) as *mut u8, EV_ABS as u8);
            fence(Ordering::SeqCst);

            let payload = (config_base + 8) as *const u8;
            // Check ABS_X and ABS_Y bits
            let has_abs_x = (read_volatile(payload) & 0x01) != 0;

            if has_abs_x {
                return true;
            }

            // Check EV_REL event (relative coordinates)
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_EV_BITS);
            write_volatile((config_base + 1) as *mut u8, EV_REL as u8);
            fence(Ordering::SeqCst);

            // Check REL_X and REL_Y bits
            let has_rel_x = (read_volatile(payload) & 0x01) != 0;

            has_rel_x
        }
    }

    /// Submit event buffers
    fn submit_event_buffers(&mut self) {
        let queue = match &self.event_queue {
            Some(q) => q,
            None => return,
        };

        let queue_size = queue.queue_size as usize;

        unsafe {
            // Submit all buffers
            for i in 0..queue_size {
                let event_ptr = self.event_buffer_phys + (i * core::mem::size_of::<VirtioInputEvent>()) as u64;

                let desc = &mut *queue.desc.add(i);
                desc.addr = event_ptr;
                desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
                desc.flags = 0x02; // VIRTQ_DESC_F_WRITE
                desc.next = 0;

                // Add to available ring
                let avail = &mut *queue.avail;
                let ring_ptr = (queue.avail as *mut u8).add(4) as *mut u16;
                let idx = avail.idx as usize;
                write_volatile(ring_ptr.add(idx % queue_size), i as u16);
                fence(Ordering::SeqCst);
                avail.idx = avail.idx.wrapping_add(1);
            }

            fence(Ordering::SeqCst);
            queue.notify();
        }
    }

    /// Read input event
    pub fn read_event(&mut self) -> Option<InputEvent> {
        let queue = self.event_queue.as_ref()?;

        unsafe {
            let used = &*queue.used;
            let used_idx = core::ptr::read_volatile(&used.idx) as usize;
            let last_used = self.last_used as usize;

            if used_idx == last_used {
                return None;
            }

            // Get used descriptor (volatile read: device writes via DMA)
            let used_ring = (queue.used as *const u8).add(8) as *const UsedElem;
            let used_elem = read_volatile(used_ring.add(last_used % queue.queue_size as usize));

            let desc_idx = used_elem.id as usize;
            let _len = used_elem.len;

            // Read event
            let event = read_volatile(self.event_buffer.add(desc_idx));

            // Resubmit buffer
            let desc = &mut *queue.desc.add(desc_idx);
            desc.addr = self.event_buffer_phys + (desc_idx * core::mem::size_of::<VirtioInputEvent>()) as u64;
            desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
            desc.flags = 0x02;
            desc.next = 0;

            let avail = &mut *queue.avail;
            let ring_ptr = (queue.avail as *mut u8).add(4) as *mut u16;
            write_volatile(ring_ptr.add(avail.idx as usize % queue.queue_size as usize), desc_idx as u16);
            fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
            self.last_used = self.last_used.wrapping_add(1);

            fence(Ordering::SeqCst);
            queue.notify();

            // Convert to standard InputEvent
            Some(InputEvent::new(event.type_, event.code, event.value))
        }
    }

    /// Check if there are events
    pub fn has_event(&self) -> bool {
        if let Some(queue) = &self.event_queue {
            unsafe {
                let used = &*queue.used;
                used.idx as usize != self.last_used as usize
            }
        } else {
            false
        }
    }

    /// Get device name
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Whether it is a pointer device
    pub fn is_pointer(&self) -> bool {
        self.is_pointer
    }
}

/// Used ring element
#[repr(C)]
struct UsedElem {
    id: u32,
    len: u32,
}

impl Drop for VirtioInputDevice {
    fn drop(&mut self) {
        if let Some(layout) = self.event_buffer_layout {
            if !self.event_buffer.is_null() {
                unsafe {
                    dealloc(self.event_buffer as *mut u8, layout);
                }
            }
        }
    }
}

// ============================================================================
// Device probing
// ============================================================================

/// Probe VirtIO Input devices
pub fn probe_virtio_input_devices() -> Option<(VirtioInputDevice, Option<VirtioInputDevice>)> {
    let mut keyboard: Option<VirtioInputDevice> = None;
    let mut pointer: Option<VirtioInputDevice> = None;

    for device in 0..32u8 {
        let ecam_addr = pci::RISCV_PCIE_ECAM_BASE + ((device as u64) * pci::PCIE_ECAM_SIZE);

        let vendor_id = unsafe { read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { read_volatile((ecam_addr as *const u16).add(1)) };

        if vendor_id == VIRTIO_INPUT_PCI_VENDOR && device_id == VIRTIO_INPUT_PCI_DEVICE {
            if let Ok(virtio_pci) = VirtIOPCI::new(ecam_addr) {
                if let Some(input_dev) = VirtioInputDevice::new(virtio_pci) {
                    if input_dev.is_pointer() {
                        if pointer.is_none() {
                            pointer = Some(input_dev);
                        }
                    } else {
                        if keyboard.is_none() {
                            keyboard = Some(input_dev);
                        }
                    }
                }
            }
        }

        // If both devices found, stop
        if keyboard.is_some() && pointer.is_some() {
            break;
        }
    }

    if keyboard.is_some() || pointer.is_some() {
        // Return (keyboard, pointer)
        Some((keyboard?, pointer))
    } else {
        None
    }
}

/// Probe single VirtIO Input device
pub fn probe_virtio_input() -> Option<VirtioInputDevice> {
    for device in 0..32u8 {
        let ecam_addr = pci::RISCV_PCIE_ECAM_BASE + ((device as u64) * pci::PCIE_ECAM_SIZE);

        let vendor_id = unsafe { read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { read_volatile((ecam_addr as *const u16).add(1)) };

        if vendor_id == VIRTIO_INPUT_PCI_VENDOR && device_id == VIRTIO_INPUT_PCI_DEVICE {
            if let Ok(virtio_pci) = VirtIOPCI::new(ecam_addr) {
                return VirtioInputDevice::new(virtio_pci);
            }
        }
    }

    None
}
