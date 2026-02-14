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
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use core::ptr::{read_volatile, write_volatile};

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
}

/// VirtIO-GPU 命令头
#[repr(C)]
struct GpuCtrlHeader {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// 矩形结构
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
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
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 步骤 2: 设置 ACKNOWLEDGE
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, status::ACKNOWLEDGE as u8);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 步骤 3: 设置 DRIVER
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER) as u8);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 步骤 4: 读取设备特性
        let device_features = unsafe {
            write_volatile((common_cfg + offset::DEVICE_FEATURE_SELECT as u64) as *mut u32, 0);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            read_volatile((common_cfg + offset::DEVICE_FEATURES as u64) as *const u32)
        };
        println!("virtio-gpu: Device features: {:#x}", device_features);

        // 步骤 5: 写入驱动特性
        // VirtIO-GPU 特性位：
        // bit 0: VIRTIO_GPU_F_VIRGL (3D)
        // bit 1: VIRTIO_GPU_F_EDID
        // bit 2: VIRTIO_GPU_F_RESOURCE_UUID
        // bit 3: VIRTIO_GPU_F_RESOURCE_BLOB
        // 我们只使用基本 2D 功能，不请求任何额外特性
        let driver_features = 0u32; // 只使用基本功能
        unsafe {
            write_volatile((common_cfg + offset::DRIVER_FEATURE_SELECT as u64) as *mut u32, 0);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            write_volatile((common_cfg + offset::DRIVER_FEATURES as u64) as *mut u32, driver_features);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        println!("virtio-gpu: Driver features: {:#x}", driver_features);

        // 步骤 6: 设置 FEATURES_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 步骤 7: 验证 FEATURES_OK
        let status_val = unsafe { read_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *const u8) };
        if (status_val & status::FEATURES_OK as u8) == 0 {
            println!("virtio-gpu: Device rejected FEATURES_OK! status={:#x}", status_val);
            return None;
        }

        // 步骤 8: 初始化控制队列
        // 选择队列 0
        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16, CTRL_QUEUE);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 读取队列大小
        let queue_size = unsafe { read_volatile((common_cfg + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16) };
        println!("virtio-gpu: Control queue size: {}", queue_size);

        if queue_size == 0 {
            println!("virtio-gpu: Invalid queue size!");
            return None;
        }

        // 计算通知偏移
        let notify_offset = (CTRL_QUEUE as u64) * (self.pci.notify_off_multiplier as u64);

        // 创建 VirtQueue
        let queue = VirtQueue::new(
            queue_size,
            CTRL_QUEUE,
            notify_base + notify_offset,
            isr_base,
            isr_base + 4,
        )?;

        // 写入队列地址到 Common CFG
        let desc_addr = unsafe { queue.desc as u64 };
        let avail_addr = unsafe { queue.avail as u64 };
        let used_addr = unsafe { queue.used as u64 };

        unsafe {
            // 描述符表地址
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32, desc_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32, (desc_addr >> 32) as u32);
            // 可用环地址
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32, avail_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32, (avail_addr >> 32) as u32);
            // 已用环地址
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32, used_addr as u32);
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32, (used_addr >> 32) as u32);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // 启用队列 (queue_enable = 1)
        unsafe {
            write_volatile((common_cfg + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16, 1);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        self.ctrl_queue = Some(queue);

        // 步骤 9: 设置 DRIVER_OK
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        println!("virtio-gpu: VirtIO initialization complete");
        Some(())
    }

    /// 初始化帧缓冲区
    pub fn init_framebuffer(&mut self) -> Option<&FrameBufferInfo> {
        println!("virtio-gpu: Initializing framebuffer...");

        // 使用默认分辨率 (QEMU virt 默认 1024x768)
        let width = 1024u32;
        let height = 768u32;
        let stride = width * 4; // 32bpp
        let fb_size = (stride * height) as usize;

        println!("virtio-gpu: Target resolution: {}x{}", width, height);

        // 分配帧缓冲区内存
        let layout = Layout::from_size_align(fb_size, 4096).ok()?;
        let fb_ptr = unsafe { alloc_zeroed(layout) };

        if fb_ptr.is_null() {
            println!("virtio-gpu: Failed to allocate framebuffer!");
            return None;
        }

        println!("virtio-gpu: Framebuffer allocated at {:p}, size {}", fb_ptr, fb_size);

        self.fb_ptr = fb_ptr;
        self.fb_layout = Some(layout);

        // 保存帧缓冲区信息
        self.fb_info = Some(FrameBufferInfo {
            addr: fb_ptr as u64,
            size: fb_size as u32,
            width,
            height,
            stride,
            format: 1, // xRGB
        });

        println!("virtio-gpu: Framebuffer initialized successfully");
        self.fb_info.as_ref()
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

    /// 刷新显示
    pub fn flush(&self) {
        // TODO: 发送 RESOURCE_FLUSH 命令
    }
}

impl Drop for VirtioGpuDevice {
    fn drop(&mut self) {
        // 释放帧缓冲区内存
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

    // 扫描 PCI 总线
    for device in 0..32u8 {
        let ecam_addr = pci::RISCV_PCIE_ECAM_BASE + ((device as u64) * pci::PCIE_ECAM_SIZE);

        // 读取 Vendor ID 和 Device ID
        let vendor_id = unsafe { read_volatile((ecam_addr as *const u16)) };
        let device_id = unsafe { read_volatile((ecam_addr as *const u16).add(1)) };

        if vendor_id == VIRTIO_GPU_PCI_VENDOR && device_id == virtio_device::VIRTIO_GPU {
            println!("virtio-gpu: Found device at device:{}", device);

            // 创建 VirtIO PCI 设备
            let virtio_pci = VirtIOPCI::new(ecam_addr).ok()?;
            return VirtioGpuDevice::new(virtio_pci);
        }
    }

    println!("virtio-gpu: No VirtIO-GPU device found");
    println!("virtio-gpu: Add '-device virtio-gpu-device' to QEMU command line");
    None
}
