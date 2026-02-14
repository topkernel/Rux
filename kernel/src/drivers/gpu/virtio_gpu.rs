//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO-GPU 设备驱动
//!
//! 实现 VirtIO-GPU PCI 设备的初始化和 framebuffer 管理
//! 参考: VirtIO 1.2 规范

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

/// VirtIO 队列索引
const CTRL_QUEUE: u16 = 0;   // 控制队列
const CURSOR_QUEUE: u16 = 1; // 光标队列

/// VirtIO-GPU 设备
pub struct VirtioGpuDevice {
    /// VirtIO PCI 设备
    pci: VirtIOPCI,
    /// 控制队列
    ctrl_queue: Option<VirtQueue>,
    /// 帧缓冲区信息
    fb_info: Option<FrameBufferInfo>,
    /// 帧缓冲区指针
    fb_ptr: *mut u8,
    /// 帧缓冲区布局
    fb_layout: Option<Layout>,
    /// 资源 ID
    resource_id: u32,
    /// 显示矩形
    display_rect: Rect,
}

/// VirtIO-GPU 命令头 (24 字节)
#[repr(C)]
struct GpuCtrlHeader {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// 矩形结构 (16 字节)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// 单个显示输出配置 (24 字节)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct DisplayPmode {
    rect: Rect,
    enabled: u32,
    flags: u32,
}

/// GET_DISPLAY_INFO 响应
/// VirtIO 1.0 规范: header + 16 个 pmode (无 num_scanouts)
#[repr(C)]
struct RespDisplayInfo {
    header: GpuCtrlHeader,
    pmodes: [DisplayPmode; 16],
}

/// RESOURCE_CREATE_2D 命令 (32 字节)
#[repr(C)]
struct CmdResourceCreate2d {
    header: GpuCtrlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// SET_SCANOUT 命令 (48 字节)
/// VirtIO 1.2 规范: header(24) + rect(16) + scanout_id(4) + resource_id(4)
#[repr(C)]
struct CmdSetScanout {
    header: GpuCtrlHeader,
    rect: Rect,
    scanout_id: u32,
    resource_id: u32,
}

/// 内存条目 (16 字节)
#[repr(C)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// RESOURCE_ATTACH_BACKING 命令 (48 字节)
#[repr(C)]
struct CmdResourceAttachBacking {
    header: GpuCtrlHeader,
    resource_id: u32,
    nr_entries: u32,
    entry: MemEntry,
}

/// RESOURCE_FLUSH 命令 (48 字节)
#[repr(C)]
struct CmdResourceFlush {
    header: GpuCtrlHeader,
    resource_id: u32,
    padding: u32,
    rect: Rect,
}

/// TRANSFER_TO_HOST_2D 命令 (56 字节)
/// VirtIO 1.2 规范: header(24) + rect(16) + offset(8) + resource_id(4) + padding(4)
#[repr(C)]
struct CmdTransferToHost2d {
    header: GpuCtrlHeader,
    rect: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

/// 通用响应 (24 字节)
#[repr(C)]
struct RespNoData {
    header: GpuCtrlHeader,
}

unsafe impl Send for VirtioGpuDevice {}
unsafe impl Sync for VirtioGpuDevice {}

impl VirtioGpuDevice {
    /// 创建新的 VirtIO-GPU 设备
    pub fn new(pci: VirtIOPCI) -> Option<Self> {
        println!("virtio-gpu: Initializing VirtIO-GPU device...");

        let mut device = Self {
            pci,
            ctrl_queue: None,
            fb_info: None,
            fb_ptr: core::ptr::null_mut(),
            fb_layout: None,
            resource_id: 1,
            display_rect: Rect::default(),
        };

        // 初始化 VirtIO 设备
        device.init_virtio()?;

        println!("virtio-gpu: Device initialized successfully");
        Some(device)
    }

    /// 初始化 VirtIO 设备
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;
        let notify_base = self.pci.notify_cfg_bar + self.pci.notify_cfg_offset as u64;
        let isr_base = self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64;

        // 步骤 1: 重置设备
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, 0);
        }
        fence(Ordering::SeqCst);

        // 步骤 2: 设置 ACKNOWLEDGE
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, status::ACKNOWLEDGE as u8);
        }
        fence(Ordering::SeqCst);

        // 步骤 3: 设置 DRIVER
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER) as u8);
        }
        fence(Ordering::SeqCst);

        // 步骤 4: 读取设备特性
        let device_features = unsafe {
            write_volatile((common_cfg + offset::DEVICE_FEATURE_SELECT as u64) as *mut u32, 0);
            fence(Ordering::SeqCst);
            read_volatile((common_cfg + offset::DEVICE_FEATURES as u64) as *const u32)
        };
        println!("virtio-gpu: Device features: {:#x}", device_features);

        // 步骤 5: 写入驱动特性
        let driver_features = 0u32;
        unsafe {
            write_volatile((common_cfg + offset::DRIVER_FEATURE_SELECT as u64) as *mut u32, 0);
            fence(Ordering::SeqCst);
            write_volatile((common_cfg + offset::DRIVER_FEATURES as u64) as *mut u32, driver_features);
        }
        fence(Ordering::SeqCst);
        println!("virtio-gpu: Driver features: {:#x}", driver_features);

        // 步骤 6: 设置 FEATURES_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8);
        }
        fence(Ordering::SeqCst);

        // 步骤 7: 验证 FEATURES_OK
        let status_val = unsafe { read_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *const u8) };
        if (status_val & status::FEATURES_OK as u8) == 0 {
            println!("virtio-gpu: Device rejected FEATURES_OK! status={:#x}", status_val);
            return None;
        }

        // 步骤 8: 初始化控制队列
        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16, CTRL_QUEUE);
        }
        fence(Ordering::SeqCst);

        let queue_size = unsafe { read_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16) };
        println!("virtio-gpu: Control queue size: {}", queue_size);

        if queue_size == 0 {
            println!("virtio-gpu: Invalid queue size!");
            return None;
        }

        let notify_offset = (CTRL_QUEUE as u64) * (self.pci.notify_off_multiplier as u64);

        let queue = VirtQueue::new(
            queue_size,
            CTRL_QUEUE,
            notify_base + notify_offset,
            isr_base,
            isr_base + 4,
        )?;

        let desc_addr = unsafe { queue.desc as u64 };
        let avail_addr = unsafe { queue.avail as u64 };
        let used_addr = unsafe { queue.used as u64 };

        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32, desc_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32, (desc_addr >> 32) as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32, avail_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32, (avail_addr >> 32) as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32, used_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32, (used_addr >> 32) as u32);
        }
        fence(Ordering::SeqCst);

        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16, 1);
        }
        fence(Ordering::SeqCst);

        self.ctrl_queue = Some(queue);

        // 步骤 9: 设置 DRIVER_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8);
        }
        fence(Ordering::SeqCst);

        println!("virtio-gpu: VirtIO initialization complete");
        Some(())
    }

    /// 初始化帧缓冲区并发送 GPU 命令
    pub fn init_framebuffer(&mut self) -> Option<&FrameBufferInfo> {
        println!("virtio-gpu: Initializing display...");

        // 步骤 1: 获取显示信息
        let display_info = self.get_display_info()?;

        // 使用第一个 pmode (scanout 0)
        let pmode0 = &display_info.pmodes[0];
        println!("virtio-gpu: Display[0]: {}x{} enabled={} flags={}",
                 pmode0.rect.width, pmode0.rect.height,
                 pmode0.enabled, pmode0.flags);
        println!("virtio-gpu: Rect: x={} y={}", pmode0.rect.x, pmode0.rect.y);

        // 即使 enabled 为 0，也尝试使用该显示配置
        // 某些 QEMU 版本可能返回 enabled=0 但仍支持扫描输出
        self.display_rect = pmode0.rect;

        let width = pmode0.rect.width;
        let height = pmode0.rect.height;

        if width == 0 || height == 0 {
            println!("virtio-gpu: Invalid display dimensions!");
            return None;
        }

        let stride = width * 4;
        let fb_size = (stride * height) as usize;

        // 步骤 2: 分配帧缓冲区
        let layout = Layout::from_size_align(fb_size, 4096).ok()?;
        let fb_ptr = unsafe { alloc_zeroed(layout) };

        if fb_ptr.is_null() {
            println!("virtio-gpu: Failed to allocate framebuffer!");
            return None;
        }

        println!("virtio-gpu: Framebuffer at {:p}, size {}", fb_ptr, fb_size);

        self.fb_ptr = fb_ptr;
        self.fb_layout = Some(layout);

        // 步骤 3: 创建 2D 资源
        self.create_resource_2d(width, height)?;

        // 步骤 4: 附加后备存储（使用物理地址）
        #[cfg(feature = "riscv64")]
        let fb_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(fb_ptr as u64)
        ).0;
        #[cfg(not(feature = "riscv64"))]
        let fb_phys = fb_ptr as u64;

        self.attach_backing(fb_phys, fb_size as u32)?;

        // 步骤 5: 传输帧缓冲区到设备
        let full_rect = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        self.transfer_to_host_2d(self.resource_id, 0, &full_rect)?;

        // 步骤 6: 设置扫描输出
        self.set_scanout(0, self.resource_id, &full_rect)?;

        // 保存帧缓冲区信息
        self.fb_info = Some(FrameBufferInfo {
            addr: fb_ptr as u64,
            size: fb_size as u32,
            width,
            height,
            stride,
            format: 1,
        });

        println!("virtio-gpu: Display initialized successfully");
        self.fb_info.as_ref()
    }

    /// 获取显示信息
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
            println!("virtio-gpu: GET_DISPLAY_INFO failed: {:#x}", resp.header.hdr_type);
            return None;
        }

        // 统计已启用的扫描输出数量
        let enabled_count = resp.pmodes.iter().filter(|p| p.enabled != 0).count();
        println!("virtio-gpu: {} enabled scanout(s)", enabled_count);
        Some(resp)
    }

    /// 创建 2D 资源
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
            format: 1, // B8G8R8A8_UNORM
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
            println!("virtio-gpu: RESOURCE_CREATE_2D failed: {:#x}", resp.header.hdr_type);
            return None;
        }

        println!("virtio-gpu: Created 2D resource {}", self.resource_id);
        Some(())
    }

    /// 附加后备存储
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
            println!("virtio-gpu: RESOURCE_ATTACH_BACKING failed: {:#x}", resp.header.hdr_type);
            return None;
        }

        println!("virtio-gpu: Attached backing storage");
        Some(())
    }

    /// 设置扫描输出
    fn set_scanout(&self, scanout_id: u32, resource_id: u32, rect: &Rect) -> Option<()> {
        println!("virtio-gpu: SET_SCANOUT scanout_id={} resource_id={} rect=({},{},{},{})",
                 scanout_id, resource_id, rect.x, rect.y, rect.width, rect.height);

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

        println!("virtio-gpu: CmdSetScanout size={} header_size={}",
                 core::mem::size_of::<CmdSetScanout>(),
                 core::mem::size_of::<GpuCtrlHeader>());

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
            println!("virtio-gpu: SET_SCANOUT failed: {:#x}", resp.header.hdr_type);
            return None;
        }

        println!("virtio-gpu: Set scanout configured");
        Some(())
    }

    /// 传输数据到主机
    fn transfer_to_host_2d(&self, resource_id: u32, offset: u64, rect: &Rect) -> Option<()> {
        println!("virtio-gpu: TRANSFER_TO_HOST_2D resource_id={} offset={} rect=({},{},{},{})",
                 resource_id, offset, rect.x, rect.y, rect.width, rect.height);

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
            println!("virtio-gpu: TRANSFER_TO_HOST_2D failed: {:#x}", resp.header.hdr_type);
            return None;
        }

        println!("virtio-gpu: Transfer to host completed");
        Some(())
    }

    /// 发送命令到 VirtIO-GPU
    fn send_command<CMD, RESP>(&self,
                               cmd: &CMD,
                               cmd_size: usize,
                               resp: &mut RESP,
                               resp_size: usize) -> Option<()> {
        let queue = self.ctrl_queue.as_ref()?;

        // 将虚拟地址转换为物理地址
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

        // 使用第一个描述符发送命令，第二个描述符接收响应
        unsafe {
            // 设置命令描述符
            let desc0 = &mut *queue.desc.add(0);
            desc0.addr = cmd_phys;
            desc0.len = cmd_size as u32;
            desc0.flags = 0x01; // VIRTQ_DESC_F_NEXT
            desc0.next = 1;

            // 设置响应描述符
            let desc1 = &mut *queue.desc.add(1);
            desc1.addr = resp_phys;
            desc1.len = resp_size as u32;
            desc1.flags = 0x02; // VIRTQ_DESC_F_WRITE
            desc1.next = 0;

            // 更新可用环
            // AvailRing 结构: flags (u16) + idx (u16) + ring[] (u16 array)
            // ring 数组从偏移 4 开始
            let avail = &mut *queue.avail;
            let idx = avail.idx as usize;
            let ring_idx = idx % queue.queue_size as usize;

            // ring 数组紧跟在 AvailRing 结构体后面
            let ring_ptr = (queue.avail as *mut u8).add(4) as *mut u16;
            write_volatile(ring_ptr.add(ring_idx), 0); // 描述符索引 0
            fence(Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
            fence(Ordering::SeqCst);

            // 通知设备
            queue.notify();
            fence(Ordering::SeqCst);

            // 等待响应 (简单轮询)
            for _ in 0..100000 {
                fence(Ordering::SeqCst);
                let used = &*queue.used;
                if used.idx as usize >= idx + 1 {
                    return Some(());
                }
            }

            println!("virtio-gpu: Command timeout!");
            None
        }
    }

    /// 刷新显示
    pub fn flush(&self) {
        let rect = self.display_rect;

        let cmd = CmdResourceFlush {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: self.resource_id,
            padding: 0,
            rect,
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

        let _ = self.send_command(&cmd, core::mem::size_of::<CmdResourceFlush>(),
                                  &mut resp, core::mem::size_of::<RespNoData>());
    }

    /// 获取帧缓冲区
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

/// 探测 VirtIO-GPU 设备
pub fn probe_virtio_gpu() -> Option<VirtioGpuDevice> {
    println!("virtio-gpu: Scanning for VirtIO-GPU device...");

    for device in 0..32u8 {
        let ecam_addr = pci::RISCV_PCIE_ECAM_BASE + ((device as u64) * pci::PCIE_ECAM_SIZE);

        let vendor_id = unsafe { read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { read_volatile((ecam_addr as *const u16).add(1)) };

        if vendor_id == VIRTIO_GPU_PCI_VENDOR && device_id == virtio_device::VIRTIO_GPU {
            println!("virtio-gpu: Found device at device:{}", device);

            let virtio_pci = VirtIOPCI::new(ecam_addr).ok()?;
            return VirtioGpuDevice::new(virtio_pci);
        }
    }

    println!("virtio-gpu: No VirtIO-GPU device found");
    println!("virtio-gpu: Add '-device virtio-gpu-pci' to QEMU command line");
    None
}
