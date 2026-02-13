//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO PCI 传输层
//!
//! 实现 VirtIO 设备的 PCI 传输（Modern VirtIO 1.0+）
//! 参考: Linux kernel drivers/virtio/virtio_pci_modern.c

use crate::drivers::pci::{PCIConfig, vendor, virtio_device, BARType};
use crate::drivers::virtio::queue;
use crate::drivers::virtio::offset;

/// VirtIO 设备状态位
pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}

/// VirtIO PCI 设备
pub struct VirtIOPCI {
    /// PCI 配置空间
    pub pci_config: PCIConfig,
    /// Common CFG BAR 基地址
    pub common_cfg_bar: u64,
    /// Device CFG BAR 基地址
    pub device_cfg_bar: u64,
    /// Notify CFG BAR 基地址
    pub notify_cfg_bar: u64,
    /// Notify offset multiplier
    pub notify_off_multiplier: u32,
    /// 设备基地址
    pub base_addr: u64,
}

impl VirtIOPCI {
    /// 创建新的 VirtIO PCI 设备
    ///
    /// # 参数
    /// - `pci_base`: PCI 配置空间基地址（ECAM）
    pub fn new(pci_base: u64) -> Result<Self, &'static str> {
        crate::println!("virtio-pci: Initializing VirtIO PCI device at 0x{:x}", pci_base);

        let pci_config = PCIConfig::new(pci_base);

        // 验证厂商 ID 和设备 ID
        let vendor_id = pci_config.vendor_id();
        let device_id = pci_config.device_id();

        if vendor_id != vendor::RED_HAT {
            return Err("Not a VirtIO device (wrong vendor)");
        }

        match device_id {
            virtio_device::VIRTIO_BLK | virtio_device::VIRTIO_BLK_MODERN => {
                // VirtIO 块设备（Legacy 或 Modern）
            }
            virtio_device::VIRTIO_NET => {
                // VirtIO 网络设备
            }
            _ => {
                if device_id != 0 {
                    return Err("Unknown VirtIO device");
                }
            }
        }

        crate::println!("virtio-pci: Vendor=0x{:04x}, Device=0x{:04x}", vendor_id, device_id);

        // 使能总线主控和内存空间访问
        pci_config.enable_bus_master();

        // 读取 BAR0 (Common CFG)
        let bar0 = pci_config.read_bar(0);
        if bar0.bar_type != BARType::MemoryMapped {
            return Err("BAR0 is not memory mapped");
        }

        let common_cfg_bar = bar0.base_addr;
        crate::println!("virtio-pci: Common CFG BAR = 0x{:x}", common_cfg_bar);

        // 读取 BAR1 (Device CFG, 可选)
        let bar1 = pci_config.read_bar(1);
        let device_cfg_bar = if bar1.bar_type == BARType::MemoryMapped {
            bar1.base_addr
        } else {
            0
        };

        // 读取 BAR2 (Notify CFG)
        let bar2 = pci_config.read_bar(2);
        let notify_cfg_bar = if bar2.bar_type == BARType::MemoryMapped {
            bar2.base_addr
        } else {
            return Err("BAR2 is not memory mapped (Notify CFG)");
        };

        crate::println!("virtio-pci: Device CFG BAR = 0x{:x}", device_cfg_bar);
        crate::println!("virtio-pci: Notify CFG BAR = 0x{:x}", notify_cfg_bar);

        // TODO: 读取 notify_off_multiplier from capabilities
        let notify_off_multiplier = 0;

        Ok(Self {
            pci_config,
            common_cfg_bar,
            device_cfg_bar,
            notify_cfg_bar,
            notify_off_multiplier,
            base_addr: common_cfg_bar,  // 使用 Common CFG 作为主要访问地址
        })
    }

    /// 重置设备
    pub fn reset_device(&self) {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *mut u32;
            core::ptr::write_volatile(status_ptr, 0);
        }
        crate::println!("virtio-pci: Device reset");
    }

    /// 设置设备状态
    pub fn set_status(&self, status: u32) {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *mut u32;
            core::ptr::write_volatile(status_ptr, status);
        }
    }

    /// 读取设备状态
    pub fn get_status(&self) -> u32 {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *const u32;
            core::ptr::read_volatile(status_ptr)
        }
    }

    /// 读取设备特性
    pub fn read_device_features(&self) -> u32 {
        unsafe {
            let features_ptr = (self.common_cfg_bar + 0x00) as *const u32;
            core::ptr::read_volatile(features_ptr)
        }
    }

    /// 写入驱动特性
    pub fn write_driver_features(&self, features: u32) {
        unsafe {
            let features_ptr = (self.common_cfg_bar + 0x04) as *mut u32;
            core::ptr::write_volatile(features_ptr, features);
        }
    }

    /// 设置队列
    pub fn setup_queue(&self, queue_index: u16, virt_queue: &queue::VirtQueue) -> Result<(), &'static str> {
        crate::println!("virtio-pci: Setting up queue {}", queue_index);

        // 选择队列
        unsafe {
            let queue_select_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16;
            core::ptr::write_volatile(queue_select_ptr, queue_index);
        }

        // 获取队列大小
        unsafe {
            let queue_size_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16;
            let queue_max_size = core::ptr::read_volatile(queue_size_ptr);

            if queue_max_size == 0 {
                return Err("Queue not available");
            }

            crate::println!("virtio-pci: Queue max size = {}", queue_max_size);
        }

        // 获取描述符表、可用环、已用环的物理地址
        let desc_addr = virt_queue.get_desc_addr();
        let avail_addr = virt_queue.get_avail_addr();
        let used_addr = virt_queue.get_used_addr();

        crate::println!("virtio-pci: VirtQueue addresses:");
        crate::println!("  desc:  0x{:x}", desc_addr);
        crate::println!("  avail: 0x{:x}", avail_addr);
        crate::println!("  used:  0x{:x}", used_addr);

        // 转换为物理地址
        #[cfg(feature = "riscv64")]
        let desc_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(desc_addr)
        ).0;
        #[cfg(feature = "riscv64")]
        let avail_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(avail_addr)
        ).0;
        #[cfg(feature = "riscv64")]
        let used_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(used_addr)
        ).0;

        #[cfg(not(feature = "riscv64"))]
        let desc_phys = desc_addr;
        #[cfg(not(feature = "riscv64"))]
        let avail_phys = avail_addr;
        #[cfg(not(feature = "riscv64"))]
        let used_phys = used_addr;

        crate::println!("virtio-pci: Physical addresses:");
        crate::println!("  desc_phys:  0x{:x}", desc_phys);
        crate::println!("  avail_phys: 0x{:x}", avail_phys);
        crate::println!("  used_phys:  0x{:x}", used_phys);

        // 写入描述符表地址 (64-bit)
        unsafe {
            let desc_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32;
            let desc_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32;
            core::ptr::write_volatile(desc_lo_ptr, (desc_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(desc_hi_ptr, (desc_phys >> 32) as u32);
        }

        // 写入可用环地址 (64-bit)
        unsafe {
            let driver_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32;
            let driver_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32;
            core::ptr::write_volatile(driver_lo_ptr, (avail_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(driver_hi_ptr, (avail_phys >> 32) as u32);
        }

        // 写入已用环地址 (64-bit)
        unsafe {
            let device_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32;
            let device_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32;
            core::ptr::write_volatile(device_lo_ptr, (used_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(device_hi_ptr, (used_phys >> 32) as u32);
        }

        // 使能队列
        unsafe {
            let queue_enable_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16;
            core::ptr::write_volatile(queue_enable_ptr, 1);
        }

        crate::println!("virtio-pci: Queue {} configured successfully", queue_index);

        Ok(())
    }

    /// 获取通知地址
    pub fn get_notify_addr(&self, queue_index: u16) -> u64 {
        self.notify_cfg_bar + (queue_index as u64 * self.notify_off_multiplier as u64)
    }

    /// 通知设备
    pub fn notify(&self, queue_index: u16) {
        let notify_addr = self.get_notify_addr(queue_index);
        unsafe {
            let notify_ptr = notify_addr as *mut u16;
            core::ptr::write_volatile(notify_ptr, queue_index);
        }
    }

    /// 从块设备读取数据
    ///
    /// # 参数
    /// - `sector`: 起始扇区号
    /// - `buf`: 数据缓冲区
    ///
    /// # 返回
    /// 成功返回读取的字节数，失败返回错误码
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<usize, &'static str> {
        use crate::drivers::virtio::queue::{VirtIOBlkReqHeader, VirtIOBlkResp, req_type};
        use crate::arch::riscv64::mm::VirtAddr;

        // 分配三个描述符
        let virt_queue_opt: Option<queue::VirtQueue> = queue::VirtQueue::new(8u16,
            self.notify_cfg_bar + offset::QUEUE_NOTIFY as u64,
            self.common_cfg_bar + offset::INTERRUPT_STATUS as u64,
            self.common_cfg_bar + offset::INTERRUPT_ACK as u64);
        let mut virt_queue = match virt_queue_opt {
            None => return Err("Failed to create VirtQueue"),
            Some(q) => q,
        };

        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc header descriptor"),
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc data descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc response descriptor"),
        };

        crate::println!("virtio-pci-blk: Allocated descriptors: header={}, data={}, resp={}",
            header_desc_idx, data_desc_idx, resp_desc_idx);

        // 构造 VirtIO 块请求头
        let req_header = VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        };

        // 分配请求头缓冲区
        let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
        let header_ptr: *mut VirtIOBlkReqHeader;
        unsafe {
            header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
        }
        if header_ptr.is_null() {
            return Err("Failed to allocate header");
        }
        unsafe {
            *header_ptr = req_header;
        }

        // 分配响应缓冲区
        let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
        let resp_ptr: *mut VirtIOBlkResp;
        unsafe {
            resp_ptr = alloc::alloc::alloc(resp_layout) as *mut VirtIOBlkResp;
        }
        if resp_ptr.is_null() {
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            }
            return Err("Failed to allocate response");
        }
        unsafe {
            (*resp_ptr).status = 0xFF;  // 初始化为无效状态
        }

        // VirtIO 描述符标志
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // 将虚拟地址转换为物理地址
        #[cfg(feature = "riscv64")]
        let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(header_ptr as u64)
        ).0;
        #[cfg(feature = "riscv64")]
        let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(resp_ptr as u64)
        ).0;

        // 设置请求头描述符
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // 设置数据缓冲区描述符（设备写入）
        // 对于 PCI VirtIO，我们需要确保缓冲区在物理内存中可访问
        #[cfg(feature = "riscv64")]
        let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(buf.as_ptr() as u64)
        ).0;
        #[cfg(not(feature = "riscv64"))]
        let data_phys_addr = buf.as_ptr() as u64;

        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // 设置响应描述符
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        crate::println!("virtio-pci-blk: Descriptor configuration:");
        crate::println!("  header: addr=0x{:x}, len={}", header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>());
        crate::println!("  data: addr=0x{:x}, len={}", data_phys_addr, buf.len());
        crate::println!("  resp: addr=0x{:x}, len={}", resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>());

        // 提交到可用环
        virt_queue.submit(header_desc_idx);

        // 通知设备
        virt_queue.notify();

        // 等待完成
        let prev_used = virt_queue.get_used();
        let new_used = virt_queue.wait_for_completion(prev_used);

        if new_used == prev_used {
            // 请求失败，设备没有更新 used ring
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
            }
            return Err("VirtIO request timeout");
        }

        // 读取响应状态
        let status = unsafe { *resp_ptr };
        crate::println!("virtio-pci-blk: Response status = {}", status);

        // 清理缓冲区
        unsafe {
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
        }

        match status.status {
            crate::drivers::virtio::queue::status::VIRTIO_BLK_S_OK => Ok(buf.len()),
            _ => Err("VirtIO block I/O error"),
        }
    }
}
