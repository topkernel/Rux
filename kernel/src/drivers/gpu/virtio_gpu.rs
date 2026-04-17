//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO-GPU device driver
//!
//! Implements VirtIO-GPU PCI device initialization and framebuffer management

use crate::println;
use crate::drivers::pci::{self, virtio_device};
use crate::drivers::virtio::virtio_pci::{VirtIOPCI, status};
use crate::drivers::virtio::queue::VirtQueue;
use crate::drivers::virtio::offset;
use super::framebuffer::{FrameBuffer, FrameBufferInfo};
use super::virtio_cmd::cmd;
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

/// VirtIO-GPU vendor ID (Red Hat)
const VIRTIO_GPU_PCI_VENDOR: u16 = 0x1AF4;

/// VirtIO queue indices
const CTRL_QUEUE: u16 = 0;   // Control queue
const CURSOR_QUEUE: u16 = 1; // Cursor queue

/// VirtIO-GPU device
pub struct VirtioGpuDevice {
    /// VirtIO PCI device
    pci: VirtIOPCI,
    /// Control queue
    ctrl_queue: Option<VirtQueue>,
    /// Framebuffer information
    fb_info: Option<FrameBufferInfo>,
    /// Framebuffer pointer
    fb_ptr: *mut u8,
    /// Framebuffer layout
    fb_layout: Option<Layout>,
    /// Resource ID
    resource_id: u32,
    /// Display rectangle
    display_rect: Rect,
}

/// VirtIO-GPU command header (24 bytes)
#[repr(C)]
struct GpuCtrlHeader {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// Rectangle structure (16 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Single display output configuration (24 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DisplayPmode {
    rect: Rect,
    enabled: u32,
    flags: u32,
}

/// GET_DISPLAY_INFO response
/// VirtIO 1.0 specification: header + 16 pmode entries (no num_scanouts)
#[repr(C)]
struct RespDisplayInfo {
    header: GpuCtrlHeader,
    pmodes: [DisplayPmode; 16],
}

/// RESOURCE_CREATE_2D command (32 bytes)
#[repr(C)]
struct CmdResourceCreate2d {
    header: GpuCtrlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// SET_SCANOUT command (48 bytes)
/// VirtIO 1.2 specification: header(24) + rect(16) + scanout_id(4) + resource_id(4)
#[repr(C)]
struct CmdSetScanout {
    header: GpuCtrlHeader,
    rect: Rect,
    scanout_id: u32,
    resource_id: u32,
}

/// Memory entry (16 bytes)
#[repr(C)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// RESOURCE_ATTACH_BACKING command (48 bytes)
#[repr(C)]
struct CmdResourceAttachBacking {
    header: GpuCtrlHeader,
    resource_id: u32,
    nr_entries: u32,
    entry: MemEntry,
}

/// RESOURCE_FLUSH command (48 bytes)
#[repr(C)]
struct CmdResourceFlush {
    header: GpuCtrlHeader,
    rect: Rect,           // rect BEFORE resource_id!
    resource_id: u32,
    padding: u32,
}

/// TRANSFER_TO_HOST_2D command (56 bytes)
/// VirtIO 1.2 specification: header(24) + rect(16) + offset(8) + resource_id(4) + padding(4)
#[repr(C)]
struct CmdTransferToHost2d {
    header: GpuCtrlHeader,
    rect: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

/// Generic response (24 bytes)
#[repr(C)]
struct RespNoData {
    header: GpuCtrlHeader,
}

unsafe impl Send for VirtioGpuDevice {}
unsafe impl Sync for VirtioGpuDevice {}

impl VirtioGpuDevice {
    /// Create a new VirtIO-GPU device
    pub fn new(pci: VirtIOPCI) -> Option<Self> {
        let mut device = Self {
            pci,
            ctrl_queue: None,
            fb_info: None,
            fb_ptr: core::ptr::null_mut(),
            fb_layout: None,
            resource_id: 1,
            display_rect: Rect::default(),
        };

        // Initialize VirtIO device
        device.init_virtio()?;

        Some(device)
    }

    /// Initialize VirtIO device
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;
        let notify_base = self.pci.notify_cfg_bar + self.pci.notify_cfg_offset as u64;
        let isr_base = self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64;

        // Step 1: Reset device
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, 0);
        }
        fence(Ordering::SeqCst);

        // Step 2: Set ACKNOWLEDGE
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, status::ACKNOWLEDGE as u8);
        }
        fence(Ordering::SeqCst);

        // Step 3: Set DRIVER
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER) as u8);
        }
        fence(Ordering::SeqCst);

        // Step 4: Read device features
        let device_features_low = unsafe {
            write_volatile((common_cfg + offset::DEVICE_FEATURE_SELECT as u64) as *mut u32, 0);
            fence(Ordering::SeqCst);
            read_volatile((common_cfg + offset::DEVICE_FEATURES as u64) as *const u32)
        };
        let device_features_high = unsafe {
            write_volatile((common_cfg + offset::DEVICE_FEATURE_SELECT as u64) as *mut u32, 1);
            fence(Ordering::SeqCst);
            read_volatile((common_cfg + offset::DEVICE_FEATURES as u64) as *const u32)
        };
        let _ = (device_features_low, device_features_high); // suppress unused warning

        // Step 5: Write driver features
        // VIRTIO_F_VERSION_1 (bit 32) must be negotiated, but it's in the second feature word
        // First word (feature_select = 0): no special features needed
        // Second word (feature_select = 1): VIRTIO_F_VERSION_1 = bit 0

        // Write first feature word (no GPU special features needed)
        unsafe {
            write_volatile((common_cfg + offset::DRIVER_FEATURE_SELECT as u64) as *mut u32, 0);
            fence(Ordering::SeqCst);
            write_volatile((common_cfg + offset::DRIVER_FEATURES as u64) as *mut u32, 0);
        }
        fence(Ordering::SeqCst);

        // Write second feature word (VIRTIO_F_VERSION_1 = bit 0 of word 1)
        unsafe {
            write_volatile((common_cfg + offset::DRIVER_FEATURE_SELECT as u64) as *mut u32, 1);
            fence(Ordering::SeqCst);
            write_volatile((common_cfg + offset::DRIVER_FEATURES as u64) as *mut u32, 1); // bit 0 = VIRTIO_F_VERSION_1
        }
        fence(Ordering::SeqCst);

        // Step 6: Set FEATURES_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8);
        }
        fence(Ordering::SeqCst);

        // Step 7: Verify FEATURES_OK
        let status_val = unsafe { read_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *const u8) };
        if (status_val & status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // Step 8: Initialize control queue
        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16, CTRL_QUEUE);
        }
        fence(Ordering::SeqCst);

        let queue_size = unsafe { read_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16) };

        if queue_size == 0 {
            return None;
        }

        // According to VirtIO 1.0 specification, notification address offset needs to be multiplied by 2
        // because notify_off_multiplier is in 16-bit units
        let notify_offset = (CTRL_QUEUE as u64) * (self.pci.notify_off_multiplier as u64) * 2;

        let queue = VirtQueue::new(
            queue_size,
            CTRL_QUEUE,
            notify_base + notify_offset,
            isr_base,
            isr_base + 4,
        )?;

        // Convert virtual addresses to physical addresses
        #[cfg(feature = "riscv64")]
        let desc_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.desc as u64 })
        ).0;
        #[cfg(feature = "riscv64")]
        let avail_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.avail as u64 })
        ).0;
        #[cfg(feature = "riscv64")]
        let used_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.used as u64 })
        ).0;

        #[cfg(not(feature = "riscv64"))]
        let desc_phys = unsafe { queue.desc as u64 };
        #[cfg(not(feature = "riscv64"))]
        let avail_phys = unsafe { queue.avail as u64 };
        #[cfg(not(feature = "riscv64"))]
        let used_phys = unsafe { queue.used as u64 };

        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32, desc_phys as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32, (desc_phys >> 32) as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32, avail_phys as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32, (avail_phys >> 32) as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32, used_phys as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32, (used_phys >> 32) as u32);
        }
        fence(Ordering::SeqCst);

        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16, 1);
        }
        fence(Ordering::SeqCst);

        self.ctrl_queue = Some(queue);

        // Step 9: Set DRIVER_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8);
        }
        fence(Ordering::SeqCst);

        Some(())
    }

    /// Initialize framebuffer and send GPU commands
    pub fn init_framebuffer(&mut self) -> Option<&FrameBufferInfo> {
        // Step 1: Get display info
        let display_info = self.get_display_info()?;

        // Use first pmode (scanout 0)
        let pmode0 = &display_info.pmodes[0];

        // Even if enabled is 0, try to use this display configuration
        // Some QEMU versions may return enabled=0 but still support scanout
        self.display_rect = pmode0.rect;

        let width = pmode0.rect.width;
        let height = pmode0.rect.height;

        if width == 0 || height == 0 {
            return None;
        }

        let stride = width * 4;
        let fb_size = (stride * height) as usize;

        // Step 2: Allocate framebuffer
        let layout = Layout::from_size_align(fb_size, 4096).ok()?;
        let fb_ptr = unsafe { alloc_zeroed(layout) };

        if fb_ptr.is_null() {
            return None;
        }

        self.fb_ptr = fb_ptr;
        self.fb_layout = Some(layout);

        // Step 3: Create 2D resource
        self.create_resource_2d(width, height)?;

        // Step 4: Attach backing storage (use physical address)
        #[cfg(feature = "riscv64")]
        let fb_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(fb_ptr as u64)
        ).0;
        #[cfg(not(feature = "riscv64"))]
        let fb_phys = fb_ptr as u64;

        self.attach_backing(fb_phys, fb_size as u32)?;

        // Define full rectangle
        let full_rect = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };

        // Step 5: Set scanout first (important: must be before transferring data)
        self.set_scanout(0, self.resource_id, &full_rect)?;

        // Step 6: Transfer framebuffer to device
        let _ = self.transfer_to_host_2d(self.resource_id, 0, &full_rect);

        // Step 7: Flush resource to display
        let _ = self.resource_flush(self.resource_id, &full_rect);

        // Save framebuffer information
        self.fb_info = Some(FrameBufferInfo {
            addr: fb_ptr as u64,
            size: fb_size as u32,
            width,
            height,
            stride,
            format: 1,
        });

        self.fb_info.as_ref()
    }

    /// Get display info
    fn get_display_info(&self) -> Option<RespDisplayInfo> {
        let cmd = GpuCtrlHeader {
            hdr_type: cmd::GET_DISPLAY_INFO,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        let mut resp = RespDisplayInfo {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            pmodes: [DisplayPmode::default(); 16],
        };

        self.send_command(&cmd, core::mem::size_of::<GpuCtrlHeader>(),
                         &mut resp, core::mem::size_of::<RespDisplayInfo>())?;

        if resp.header.hdr_type != cmd::RESP_OK_DISPLAY_INFO {
            return None;
        }

        Some(resp)
    }

    /// Create 2D resource
    fn create_resource_2d(&self, width: u32, height: u32) -> Option<()> {
        let cmd = CmdResourceCreate2d {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: self.resource_id,
            format: 3, // R8G8B8A8_UNORM (try different format)
            width,
            height,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceCreate2d>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Attach backing storage
    fn attach_backing(&self, addr: u64, size: u32) -> Option<()> {
        let cmd = CmdResourceAttachBacking {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: self.resource_id,
            nr_entries: 1,
            entry: MemEntry {
                addr,
                length: size,
                padding: 0,
            },
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceAttachBacking>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Set scanout
    fn set_scanout(&self, scanout_id: u32, resource_id: u32, rect: &Rect) -> Option<()> {
        let cmd = CmdSetScanout {
            header: GpuCtrlHeader {
                hdr_type: cmd::SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,
            scanout_id,
            resource_id,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdSetScanout>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Transfer data to host
    fn transfer_to_host_2d(&self, resource_id: u32, offset: u64, rect: &Rect) -> Option<()> {
        let cmd = CmdTransferToHost2d {
            header: GpuCtrlHeader {
                hdr_type: cmd::TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,
            offset,
            resource_id,
            padding: 0,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdTransferToHost2d>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Flush resource to display
    fn resource_flush(&self, resource_id: u32, rect: &Rect) -> Option<()> {
        let cmd = CmdResourceFlush {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,          // rect comes BEFORE resource_id!
            resource_id,
            padding: 0,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceFlush>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            // RESOURCE_FLUSH may return error, but doesn't affect display
            // Some implementations only need TRANSFER_TO_HOST_2D
        }

        Some(())
    }

    /// Send command to VirtIO-GPU
    fn send_command<CMD, RESP>(&self,
                               cmd: &CMD,
                               cmd_size: usize,
                               resp: &mut RESP,
                               resp_size: usize) -> Option<()> {
        let queue = self.ctrl_queue.as_ref()?;

        // Convert virtual addresses to physical addresses
        #[cfg(feature = "riscv64")]
        let cmd_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(cmd as *const CMD as u64)
        ).0;
        #[cfg(feature = "riscv64")]
        let resp_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(resp as *mut RESP as u64)
        ).0;

        #[cfg(not(feature = "riscv64"))]
        let cmd_phys = cmd as *const CMD as u64;
        #[cfg(not(feature = "riscv64"))]
        let resp_phys = resp as *mut RESP as u64;

        // Use first descriptor to send command, second descriptor to receive response
        // LIMITATION: Hardcodes descriptors 0 and 1, so only one GPU command can
        // be in-flight at a time. Proper fix would use the descriptor allocator
        // (queue.alloc_desc()) for concurrent command support.
        unsafe {
            // Set command descriptor
            let desc0 = &mut *queue.desc.add(0);
            desc0.addr = cmd_phys;
            desc0.len = cmd_size as u32;
            desc0.flags = 0x01; // VIRTQ_DESC_F_NEXT
            desc0.next = 1;

            // Set response descriptor
            let desc1 = &mut *queue.desc.add(1);
            desc1.addr = resp_phys;
            desc1.len = resp_size as u32;
            desc1.flags = 0x02; // VIRTQ_DESC_F_WRITE
            desc1.next = 0;

            // Update available ring
            // AvailRing structure: flags (u16) + idx (u16) + ring[] (u16 array)
            // ring array starts at offset 4
            let avail = &mut *queue.avail;
            let idx = avail.idx as usize;
            let ring_idx = idx % queue.queue_size as usize;

            // ring array immediately follows AvailRing struct
            let ring_ptr = (queue.avail as *mut u8).add(4) as *mut u16;
            write_volatile(ring_ptr.add(ring_idx), 0); // Descriptor index 0

            // Ensure all writes are visible to device (memory barrier)
            fence(Ordering::SeqCst);

            // RISC-V needs to flush CPU cache to memory
            #[cfg(feature = "riscv64")]
            core::arch::asm!("fence");

            write_volatile(core::ptr::addr_of_mut!(avail.idx), avail.idx.wrapping_add(1));
            fence(Ordering::SeqCst);

            // Notify device
            queue.notify();
            fence(Ordering::SeqCst);

            // Wait for response (simple polling)
            for _ in 0..100000 {
                fence(Ordering::SeqCst);
                let used = &*queue.used;
                if used.idx as usize >= idx + 1 {
                    return Some(());
                }
            }

            None
        }
    }

    /// Flush display
    pub fn flush(&self) {
        let rect = self.display_rect;

        // 1. Transfer framebuffer data to device
        let _ = self.transfer_to_host_2d(self.resource_id, 0, &rect);

        // 2. Flush resource to display
        let _ = self.resource_flush(self.resource_id, &rect);
    }

    /// Get framebuffer
    pub fn get_framebuffer(&self) -> Option<FrameBuffer> {
        let info = self.fb_info.as_ref()?;
        unsafe {
            Some(FrameBuffer::new(info.addr, FrameBufferInfo {
                addr: info.addr,
                size: info.size,
                width: info.width,
                height: info.height,
                stride: info.stride,
                format: info.format,
            }))
        }
    }
}

impl Drop for VirtioGpuDevice {
    fn drop(&mut self) {
        if !self.fb_ptr.is_null() {
            if let Some(layout) = self.fb_layout {
                unsafe {
                    dealloc(self.fb_ptr, layout);
                }
            }
        }
    }
}

/// Probe VirtIO-GPU device
pub fn probe_virtio_gpu() -> Option<VirtioGpuDevice> {
    for device in 0..32u8 {
        let ecam_addr = pci::RISCV_PCIE_ECAM_BASE + ((device as u64) * pci::PCIE_ECAM_SIZE);

        let vendor_id = unsafe { read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { read_volatile((ecam_addr as *const u16).add(1)) };

        if vendor_id == VIRTIO_GPU_PCI_VENDOR && device_id == virtio_device::VIRTIO_GPU {
            let virtio_pci = VirtIOPCI::new(ecam_addr).ok()?;
            return VirtioGpuDevice::new(virtio_pci);
        }
    }

    None
}
