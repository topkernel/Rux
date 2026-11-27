//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO Input device driver
//!
//! Implements VirtIO Input PCI device initialization and event reading
#![forbid(unsafe_code)]

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

        // A virtio-input device without a Device CFG capability is unusable
        // (every one of its registers lives there) — and it would make the
        // config reads below touch address 0.
        if device.pci.device_cfg_bar == 0 {
            return None;
        }

        device.init_virtio()?;
        device.read_device_info();

        Some(device)
    }

    /// Initialize VirtIO device
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;

        // INV-MMIO-1..3: the common-config register window for the
        // whole init sequence below.
        let common = crate::safety::mmio::MmioRegion::new(common_cfg as usize, 0x100);

        // Step 1: Reset device
        common.write_u8(offset::DEVICE_STATUS as usize, 0);
        fence(Ordering::SeqCst);

        // Steps 2-3: Set ACKNOWLEDGE | DRIVER
        common.write_u8(
            offset::DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER) as u8,
        );
        fence(Ordering::SeqCst);

        // Steps 4-6: Feature negotiation (no special features needed)
        common.write_u8(
            offset::DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8,
        );
        fence(Ordering::SeqCst);

        // Step 7: Verify FEATURES_OK
        let status_val = common.read_u8(offset::DEVICE_STATUS as usize);
        if (status_val & status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // Step 8: Initialize event queue
        common.write_u16(offset::COMMON_CFG_QUEUE_SELECT as usize, EVENT_QUEUE);
        fence(Ordering::SeqCst);

        let queue_size = common.read_u16(offset::COMMON_CFG_QUEUE_SIZE as usize);

        if queue_size == 0 {
            return None;
        }

        // Allocate event buffer
        let buffer_layout = Layout::from_size_align(
            queue_size as usize * core::mem::size_of::<VirtioInputEvent>(),
            4096,
        ).ok()?;

        let event_buffer = crate::drivers::edges::raw_alloc_zeroed(buffer_layout);
        if event_buffer.is_null() {
            return None;
        }

        self.event_buffer = event_buffer as *mut VirtioInputEvent;
        self.event_buffer_layout = Some(buffer_layout);

        // Get physical address
        self.event_buffer_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(event_buffer as u64)
        ).0;

        // Create VirtQueue. The notify address comes from the spec formula
        // via get_notify_addr (reads queue_notify_off from common cfg; the
        // old inline `queue_index * multiplier * 2` scaled the multiplier
        // — already in bytes — a second time, review BUG "notify ×2").
        let isr_base = self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64;

        let queue = VirtQueue::new(
            queue_size,
            EVENT_QUEUE,
            self.pci.get_notify_addr(EVENT_QUEUE),
            isr_base,
            isr_base + 4,
        )?;

        // Get queue physical addresses
        let desc_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_desc_addr())
        ).0;
        let avail_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_avail_addr())
        ).0;
        let used_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_used_addr())
        ).0;

        // Set queue addresses (INV-MMIO-1..3: the common-config window).
        let common = crate::safety::mmio::MmioRegion::new(common_cfg as usize, 0x100);
        common.write_u32(offset::COMMON_CFG_QUEUE_DESC_LO as usize, desc_phys as u32);
        common.write_u32(offset::COMMON_CFG_QUEUE_DESC_HI as usize, (desc_phys >> 32) as u32);
        common.write_u32(offset::COMMON_CFG_QUEUE_DRIVER_LO as usize, avail_phys as u32);
        common.write_u32(offset::COMMON_CFG_QUEUE_DRIVER_HI as usize, (avail_phys >> 32) as u32);
        common.write_u32(offset::COMMON_CFG_QUEUE_DEVICE_LO as usize, used_phys as u32);
        common.write_u32(offset::COMMON_CFG_QUEUE_DEVICE_HI as usize, (used_phys >> 32) as u32);
        fence(Ordering::SeqCst);

        // Enable queue
        common.write_u16(offset::COMMON_CFG_QUEUE_ENABLE as usize, 1);
        fence(Ordering::SeqCst);

        self.event_queue = Some(queue);

        // Step 9: Set DRIVER_OK
        common.write_u8(
            offset::DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8,
        );
        fence(Ordering::SeqCst);

        // Submit initial buffers to receive events
        self.submit_event_buffers();

        Some(())
    }

    /// Read device info
    fn read_device_info(&mut self) {
        // The virtio-input config registers (select/subsel/size/payload)
        // live in the DEVICE CFG capability BAR, NOT in common cfg. The
        // old code read/wrote config through common_cfg_bar — those writes
        // landed on device_feature_select and the "name" reads returned
        // common-cfg garbage, so is_pointer() misclassified every device
        // as a keyboard (review BUG: virtio-input 从错误 BAR 读 config).
        let config_base = self.pci.device_cfg_bar;

        // INV-MMIO-1..3: the device-config register window (select/
        // subsel/size + 32-byte payload at +8).
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            // Select ID_NAME configuration
            cfg.write_u8(0, VIRTIO_INPUT_CFG_ID_NAME);
            cfg.write_u8(1, 0);
            fence(Ordering::SeqCst);

            // Read name
            for i in 0..31usize {
                let c = cfg.read_u8(8 + i);
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
        // Device CFG BAR — see read_device_info.
        let config_base = self.pci.device_cfg_bar;

        // INV-MMIO-1..3: device-config window (see read_device_info).
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            // Check EV_ABS event (absolute coordinates)
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, EV_ABS as u8);
            fence(Ordering::SeqCst);

            // Check ABS_X and ABS_Y bits
            let has_abs_x = (cfg.read_u8(8) & 0x01) != 0;

            if has_abs_x {
                return true;
            }

            // Check EV_REL event (relative coordinates)
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, EV_REL as u8);
            fence(Ordering::SeqCst);

            // Check REL_X and REL_Y bits
            let has_rel_x = (cfg.read_u8(8) & 0x01) != 0;

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

        // INV-DMA-2: typed bounds-checked ring views.
        {
            // Submit all buffers
            for i in 0..queue_size {
                let event_ptr = self.event_buffer_phys + (i * core::mem::size_of::<VirtioInputEvent>()) as u64;

                let Some(desc) = queue.desc_mut(i as u16) else { continue };
                desc.addr = event_ptr;
                desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
                desc.flags = 0x02; // VIRTQ_DESC_F_WRITE
                desc.next = 0;

                // Add to available ring
                let idx = queue.avail_idx() as usize;
                queue.write_avail_ring_slot(idx % queue_size, i as u16);
                fence(Ordering::SeqCst);
                queue.write_avail_idx((idx as u16).wrapping_add(1));
            }

            fence(Ordering::SeqCst);
            queue.notify();
        }
    }

    /// Read input event
    pub fn read_event(&mut self) -> Option<InputEvent> {
        let queue = self.event_queue.as_ref()?;

        {
            let used_idx = queue.get_used() as usize;
            let last_used = self.last_used as usize;

            if crate::trace::EVENT_INPUTTRACE.armed() {
                static DBG_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                let n = DBG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 8 || used_idx != 0 {
                    let nm: alloc::vec::Vec<u8> = self.name_bytes_vec();
                    crate::pr_warn!("IN-RDBG used_idx={} last_used={} qs={} ptr={} name={}", used_idx, last_used, queue.queue_size, self.is_pointer, alloc::string::String::from_utf8_lossy(&nm));
                }
            }
            if used_idx == last_used {
                return None;
            }

            // Get used descriptor (volatile read: device writes via DMA)
            let used_elem = queue
                .get_used_elem((last_used % queue.queue_size as usize) as u16)
                .unwrap_or(crate::drivers::virtio::queue::UsedElem { id: 0, len: 0 });

            let desc_idx = used_elem.id as usize;
            let _len = used_elem.len;

            // Read event
            // SAFETY: the event buffer is a live registered DMA buffer
            // of queue_size events (INV at init); volatile because the
            // device writes it.
            let event = crate::drivers::edges::volatile_read_t(crate::drivers::edges::ptr_add(self.event_buffer, desc_idx));

            // Resubmit buffer
            let Some(desc) = queue.desc_mut(desc_idx as u16) else {
                return None;
            };
            desc.addr = self.event_buffer_phys + (desc_idx * core::mem::size_of::<VirtioInputEvent>()) as u64;
            desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
            desc.flags = 0x02;
            desc.next = 0;

            let aidx = queue.avail_idx() as usize;
            queue.write_avail_ring_slot(aidx % queue.queue_size as usize, desc_idx as u16);
            fence(Ordering::SeqCst);
            queue.write_avail_idx((aidx as u16).wrapping_add(1));
            self.last_used = self.last_used.wrapping_add(1);

            fence(Ordering::SeqCst);
            queue.notify();

            // Convert to standard InputEvent
            Some(InputEvent::new(event.type_, event.code, event.value))
        }
    }

    /// Check if there are events
    pub fn name_bytes_vec(&self) -> alloc::vec::Vec<u8> {
        self.name
            .iter()
            .take_while(|&&c| c != 0)
            .copied()
            .collect()
    }

    pub fn has_event(&self) -> bool {
        // INV-DMA-3: device-written used index via the typed volatile
        // getter.
        match &self.event_queue {
            Some(queue) => queue.get_used() as usize != self.last_used as usize,
            None => false,
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

    /// ISR capability address of this function (the virtio device-side
    /// EOI register). Read lock-free by the hard-IRQ top half.
    pub fn isr_cfg_bar(&self) -> u64 {
        self.pci.isr_cfg_bar
    }

    /// riscv64 gpex INTx line for this function — the same swizzle the
    /// virtio-blk PCI registration uses: IRQ = 32 + ((pin-1 + slot) % 4).
    #[cfg(feature = "riscv64")]
    pub fn intx_irq_line(&self) -> u32 {
        let int_pin = self.pci.pci_config.read_config_byte(0x3D) as u32;
        32 + ((int_pin.saturating_sub(1) + self.pci.pci_slot as u32) % 4)
    }

    /// x86_64: register this function on its PIC INTx line with the
    /// shared virtio dispatcher (which reads the ISR and calls `service`
    /// only when this function raised the line).
    #[cfg(feature = "x86_64")]
    pub fn enable_pci_intx_irq(&self, service: fn()) -> Option<u8> {
        self.pci.enable_pci_intx_irq(service)
    }

    /// Read a code bitmap from the device config (VIRTIO_INPUT_CFG_EV_BITS).
    ///
    /// `ev_type` selects the event type (EV_SYN/EV_KEY/EV_REL/EV_ABS/...);
    /// the device answers with the bitmap of codes it can emit for that
    /// type. This is the authoritative capability source — exactly what a
    /// Linux virtio-input guest reports back through EVIOCGBIT(ev, len).
    ///
    /// Returns the number of valid bytes written to `out` (0 = the device
    /// does not support this event type).
    pub fn read_ev_bitmap(&mut self, ev_type: u8, out: &mut [u8]) -> usize {
        let config_base = self.pci.device_cfg_bar;
        if config_base == 0 {
            return 0;
        }
        // INV-MMIO-1..3: device-config window.
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, ev_type);
            fence(Ordering::SeqCst);

            // u.size = bitmap size in bytes (spec: 5.10.6.2)
            let size = cfg.read_u8(2) as usize;
            if size == 0 {
                return 0;
            }
            let n = size.min(out.len()).min(128);
            for i in 0..n {
                out[i] = cfg.read_u8(8 + i);
            }
            n
        }
    }

    /// Read absolute-axis info from the device config
    /// (VIRTIO_INPUT_CFG_ABS_INFO): { min, max, fuzz, flat, res } as LE u32s
    /// (virtio_absinfo layout). Mirrors what Linux reports via
    /// EVIOCGABS(axis) — xf86-input-evdev reads ABS min/max to scale the
    /// tablet's absolute coordinates to the screen.
    pub fn read_absinfo(&mut self, axis: u8) -> Option<[u32; 5]> {
        let config_base = self.pci.device_cfg_bar;
        if config_base == 0 {
            return None;
        }
        // INV-MMIO-1..3: device-config window.
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            cfg.write_u8(0, VIRTIO_INPUT_CFG_ABS_INFO);
            cfg.write_u8(1, axis);
            fence(Ordering::SeqCst);

            let size = cfg.read_u8(2) as usize;
            if size < 20 {
                return None;
            }
            let mut vals = [0u32; 5];
            for (i, v) in vals.iter_mut().enumerate() {
                let mut b = [0u8; 4];
                for (j, bj) in b.iter_mut().enumerate() {
                    *bj = cfg.read_u8(8 + i * 4 + j);
                }
                *v = u32::from_le_bytes(b);
            }
            Some(vals)
        }
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
                {
                    crate::drivers::edges::raw_dealloc(self.event_buffer as *mut u8, layout);
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

    // Shared ECAM walker: 0x8000 stride per slot + all 8 functions (the
    // old 0x1000 stride hid every slot >= 4 — review BUG).
    for ecam_addr in pci::find_ecam_devices(VIRTIO_INPUT_PCI_VENDOR, &[VIRTIO_INPUT_PCI_DEVICE]) {
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
    // Shared ECAM walker: 0x8000 stride per slot + all 8 functions (the
    // old 0x1000 stride hid every slot >= 4 — review BUG).
    for ecam_addr in pci::find_ecam_devices(VIRTIO_INPUT_PCI_VENDOR, &[VIRTIO_INPUT_PCI_DEVICE]) {
        if let Ok(virtio_pci) = VirtIOPCI::new(ecam_addr) {
            return VirtioInputDevice::new(virtio_pci);
        }
    }

    None
}
