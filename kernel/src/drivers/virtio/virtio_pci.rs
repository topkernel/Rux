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
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;

/// VirtIO PCI Capability 类型
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtIOCapType {
    CommonCfg = 1,     // VIRTIO_PCI_CAP_COMMON_CFG
    NotifyCfg = 2,     // VIRTIO_PCI_CAP_NOTIFY_CFG
    IsrCfg = 3,        // VIRTIO_PCI_CAP_ISR_CFG
    DeviceCfg = 4,       // VIRTIO_PCI_CAP_DEVICE_CFG
    PciCfg = 5,        // VIRTIO_PCI_CAP_PCI_CFG
}

/// VirtIO PCI Capability 结构
#[repr(C)]
#[derive(Debug)]
struct VirtioPCICap {
    cap_vndr: u8,   // Generic PCI field: PCI_CAP_ID_VNDR
    cap_next: u8,    // Generic PCI field: next ptr
    cap_len: u8,     // Generic PCI field: capability length
    cfg_type: u8,    // Identifies the structure (VirtIOCapType)
    bar: u8,         // Where to find it
    id: u8,          // Multiple capabilities of same type
    padding: [u8; 2], // Pad to full dword
    offset: u32,     // Offset within bar (little-endian)
    length: u32,     // Length of structure in bytes (little-endian)
}

/// VirtIO PCI Notify Capability 结构（扩展）
#[repr(C)]
#[derive(Debug)]
struct VirtioPCINotifyCap {
    cap: VirtioPCICap,
    notify_off_multiplier: u32,  // Queue notification offset multiplier
}

/// PCI Capability 链表指针
const PCI_CAPABILITY_LIST: u8 = 0x34;
const PCI_CAP_ID_VNDR: u8 = 0x09;  // Vendor-specific capability

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
    /// PCI 槽位号（用于计算 IRQ）
    pub pci_slot: u8,
    /// Common CFG BAR 基地址
    pub common_cfg_bar: u64,
    /// Common CFG BAR 内偏移
    pub common_cfg_offset: u32,
    /// Device CFG BAR 基地址
    pub device_cfg_bar: u64,
    /// Device CFG BAR 内偏移
    pub device_cfg_offset: u32,
    /// Notify CFG BAR 基地址
    pub notify_cfg_bar: u64,
    /// Notify CFG BAR 内偏移
    pub notify_cfg_offset: u32,
    /// Notify offset multiplier
    pub notify_off_multiplier: u32,
    /// ISR CFG BAR 基地址（关键！用于中断状态读取）
    pub isr_cfg_bar: u64,
    /// ISR CFG BAR 内偏移
    pub isr_cfg_offset: u32,
    /// 设备基地址
    pub base_addr: u64,
}

impl VirtIOPCI {
    /// 查找 VirtIO PCI capability
    ///
    /// # 参数
    /// - `cap_type`: 要查找的 capability 类型
    ///
    /// # 返回
    /// 返回 capability 的偏移位置，如果未找到返回 0
    fn find_virtio_capability(&self, cap_type: VirtIOCapType) -> Option<u8> {
        unsafe {
            // 从 capabilities list 指针开始
            let mut cap_ptr = self.pci_config.read_config_byte(PCI_CAPABILITY_LIST);
            let mut iterations = 0;
            const MAX_ITERATIONS: u8 = 48;  // 最多检查 48 个 capability

            while cap_ptr != 0 && iterations < MAX_ITERATIONS {
                // 读取 capability ID
                let cap_id = self.pci_config.read_config_byte(cap_ptr);

                if cap_id == PCI_CAP_ID_VNDR {
                    // 这是 vendor-specific capability，检查类型
                    let cfg_type = self.pci_config.read_config_byte(cap_ptr + 3);

                    if cfg_type == cap_type as u8 {
                        return Some(cap_ptr);
                    }
                }

                // 移动到下一个 capability
                let next_ptr = self.pci_config.read_config_byte(cap_ptr + 1);
                if next_ptr == cap_ptr {
                    // 检测到循环，退出
                    crate::println!("virtio-pci: WARNING - capability loop detected at {}", cap_ptr);
                    break;
                }
                cap_ptr = next_ptr;
                iterations += 1;
            }

            if iterations >= MAX_ITERATIONS {
                crate::println!("virtio-pci: WARNING - too many capability iterations");
            }
        }

        None
    }

    /// 读取 VirtIO PCI capability 信息
    ///
    /// # 参数
    /// - `cap_offset`: capability 在 PCI 配置空间的偏移
    ///
    /// # 返回
    /// (bar_index, bar_offset, length)
    fn read_virtio_cap(&self, cap_offset: u8) -> Option<(u8, u32, u32)> {
        unsafe {
            // 读取 capability 字段
            let bar = self.pci_config.read_config_byte(cap_offset + 4);

            // 读取 offset 和 length (little-endian)
            let offset_lo = self.pci_config.read_config_byte(cap_offset + 8) as u32;
            let offset_hi = self.pci_config.read_config_byte(cap_offset + 9) as u32;
            let offset = offset_lo | (offset_hi << 8);

            let len_lo = self.pci_config.read_config_byte(cap_offset + 12) as u32;
            let len_hi = self.pci_config.read_config_byte(cap_offset + 13) as u32;
            let length = len_lo | (len_hi << 8);

            if bar >= 6 {
                // 保留的 BAR 值
                return None;
            }

            Some((bar, offset, length))
        }
    }
    /// 创建新的 VirtIO PCI 设备
    ///
    /// # 参数
    /// - `pci_base`: PCI 配置空间基地址（ECAM）
    pub fn new(pci_base: u64) -> Result<Self, &'static str> {
        let pci_config = PCIConfig::new(pci_base);

        // 计算 PCI 槽位号（用于 IRQ 计算）
        let pci_slot = ((pci_base - crate::drivers::pci::RISCV_PCIE_ECAM_BASE) / crate::drivers::pci::PCIE_ECAM_SIZE) as u8;

        // 验证厂商 ID 和设备 ID
        let vendor_id = pci_config.vendor_id();
        let device_id = pci_config.device_id();

        if vendor_id != vendor::RED_HAT {
            return Err("Not a VirtIO device (wrong vendor)");
        }

        match device_id {
            virtio_device::VIRTIO_BLK_MODERN => {
                // VirtIO 块设备（Modern VirtIO 1.0+）
            }
            virtio_device::VIRTIO_NET => {
                // VirtIO 网络设备
            }
            virtio_device::VIRTIO_BLK => {
                return Err("Legacy VirtIO device not supported");
            }
            _ => {
                if device_id != 0 {
                    return Err("Unknown VirtIO device");
                }
            }
        }

        // 使能总线主控和内存空间访问
        pci_config.enable_bus_master();

        // 创建临时实例以使用 capability 扫描方法
        let temp_device = Self {
            pci_config,
            pci_slot,  // 在这里添加 pci_slot
            common_cfg_bar: 0,
            common_cfg_offset: 0,
            device_cfg_bar: 0,
            device_cfg_offset: 0,
            notify_cfg_bar: 0,
            notify_cfg_offset: 0,
            notify_off_multiplier: 0,
            isr_cfg_bar: 0,
            isr_cfg_offset: 0,
            base_addr: 0,
        };

        // ========== 扫描 VirtIO PCI capabilities ==========
        // 1. 查找 Common CFG capability
        let (common_bar, common_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::CommonCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Common CFG capability"),
                }
            }
            None => return Err("Common CFG capability not found (not a Modern VirtIO device)"),
        };

        // 2. 查找 Notify CFG capability
        let (notify_bar, notify_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Notify CFG capability"),
                }
            }
            None => return Err("Notify CFG capability not found"),
        };

        // 2.5. 查找 ISR CFG capability (必需！用于中断状态)
        let (isr_bar, isr_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::IsrCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read ISR CFG capability"),
                }
            }
            None => return Err("ISR CFG capability not found"),
        };

        // 3. 查找 Device CFG capability (可选)
        let (device_bar, device_offset, _) = temp_device.find_virtio_capability(VirtIOCapType::DeviceCfg)
            .and_then(|cap_offset| temp_device.read_virtio_cap(cap_offset))
            .unwrap_or((0xFF, 0, 0));  // 0xFF 表示不存在

        // ========== PCI BAR 地址分配 ==========
        // VirtIO PCI 设备需要内核分配 BAR 地址
        // 使用固定的 MMIO 区域：0x40000000 - 0x50000000 (256MB)
        const PCI_MMIO_BASE: u64 = 0x40000000;
        let mut mmio_offset = 0u64;

        // 收集需要分配的 BAR 索引（去重）
        let mut bars_to_assign = alloc::vec::Vec::new();
        bars_to_assign.push(common_bar);
        if notify_bar != common_bar {
            bars_to_assign.push(notify_bar);
        }
        if isr_bar != common_bar && isr_bar != notify_bar {
            bars_to_assign.push(isr_bar);
        }
        if device_bar != 0xFF && device_bar != common_bar && device_bar != notify_bar && device_bar != isr_bar {
            bars_to_assign.push(device_bar);
        }

        // 存储分配后的 BAR 信息
        let mut assigned_bars = alloc::collections::btree_map::BTreeMap::new();

        // 为每个 BAR 分配地址
        for &bar_idx in &bars_to_assign {
            // 探测 BAR 大小
            let bar_size = pci_config.probe_bar_size(bar_idx);

            // 计算对齐后的地址
            let aligned_addr = if mmio_offset % bar_size != 0 {
                ((mmio_offset / bar_size) + 1) * bar_size
            } else {
                mmio_offset
            };

            let bar_addr = PCI_MMIO_BASE + aligned_addr;

            // 写入 BAR 地址并存储返回的 PCIBAR 对象
            match pci_config.assign_bar(bar_idx, bar_addr) {
                Ok(bar_obj) => {
                    mmio_offset = aligned_addr + bar_size;
                    assigned_bars.insert(bar_idx, bar_obj);
                }
                Err(e) => {
                    crate::println!("virtio-pci: ERROR - Failed to assign BAR{}: {}", bar_idx, e);
                    return Err("Failed to assign PCI BAR");
                }
            }
        }

        // ========== 使用分配的 BAR 信息 ==========
        let common_bar_obj = assigned_bars.get(&common_bar)
            .ok_or("Common CFG BAR not assigned")?;
        if common_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Common CFG BAR is not memory mapped");
        }
        let common_cfg_bar = common_bar_obj.base_addr;

        let notify_bar_obj = assigned_bars.get(&notify_bar)
            .ok_or("Notify CFG BAR not assigned")?;
        if notify_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Notify CFG BAR is not memory mapped");
        }
        let notify_cfg_bar = notify_bar_obj.base_addr;

        let device_cfg_bar = if device_bar != 0xFF {
            match assigned_bars.get(&device_bar) {
                Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => bar_obj.base_addr,
                _ => 0,
            }
        } else {
            0
        };

        // 提取 ISR CFG BAR (关键！用于中断状态读取)
        let isr_cfg_bar = match assigned_bars.get(&isr_bar) {
            Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => bar_obj.base_addr,
            _ => return Err("ISR CFG BAR not assigned or not memory mapped"),
        };

        // ========== 读取 notify_off_multiplier ==========
        // 从 Notify CFG capability 的偏移 16 (notify_off_multiplier 字段)
        // notify_off_multiplier 是 Notify CFG capability 结构的一部分，位于 PCI 配置空间
        let notify_off_multiplier = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                // notify_off_multiplier 位于 capability 结构的偏移 16
                pci_config.read_config_dword(cap_offset + 16)
            }
            None => 0,
        };

        Ok(Self {
            pci_config,
            pci_slot,
            common_cfg_bar: common_cfg_bar + common_offset as u64,
            common_cfg_offset: common_offset,
            device_cfg_bar: device_cfg_bar + device_offset as u64,
            device_cfg_offset: device_offset,
            // 关键修复：notify_cfg_bar 应该是纯 BAR 基地址，不加 offset
            // get_notify_addr 会在使用时加上 queue_index * multiplier
            notify_cfg_bar: notify_cfg_bar,
            notify_cfg_offset: notify_offset,
            notify_off_multiplier,
            isr_cfg_bar: isr_cfg_bar + isr_offset as u64,
            isr_cfg_offset: isr_offset,
            base_addr: common_cfg_bar + common_offset as u64,  // 使用 Common CFG 作为主要访问地址
        })
    }

    /// 重置设备
    pub fn reset_device(&self) {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *mut u32;
            core::ptr::write_volatile(status_ptr, 0);
        }
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
    ///
    /// VirtIO 1.0 PCI 规范:
    /// - 0x00: device_feature_select (写只）- 选择特性位集
    /// - 0x04: device_feature (读只）- 实际特性位
    pub fn read_device_features(&self) -> u32 {
        unsafe {
            // 首先写入 0 到 device_feature_select 选择特性位 0-31
            let select_ptr = (self.common_cfg_bar + 0x00) as *mut u32;
            core::ptr::write_volatile(select_ptr, 0u32);

            // 然后从 device_feature 读取实际特性位
            let features_ptr = (self.common_cfg_bar + 0x04) as *const u32;
            core::ptr::read_volatile(features_ptr)
        }
    }

    /// 写入驱动特性
    ///
    /// VirtIO 1.0 PCI 规范:
    /// - 0x08: driver_feature_select (写只）- 选择特性位集
    /// - 0x0C: driver_feature (写只）- 实际特性位
    pub fn write_driver_features(&self, features: u32) {
        unsafe {
            // 首先写入 0 到 driver_feature_select 选择特性位 0-31
            let select_ptr = (self.common_cfg_bar + 0x08) as *mut u32;
            core::ptr::write_volatile(select_ptr, 0u32);

            // 然后写入到 driver_feature
            let features_ptr = (self.common_cfg_bar + 0x0C) as *mut u32;
            core::ptr::write_volatile(features_ptr, features);
        }
    }

    /// 设置队列
    pub fn setup_queue(&self, queue_index: u16, virt_queue: &queue::VirtQueue) -> Result<(), &'static str> {
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

            // 使用设备支持的最大队列大小
            let _queue_size = queue_max_size;
        }

        // 获取描述符表、可用环、已用环的物理地址
        let desc_addr = virt_queue.get_desc_addr();
        let avail_addr = virt_queue.get_avail_addr();
        let used_addr = virt_queue.get_used_addr();

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

        Ok(())
    }

    /// 获取通知地址
    pub fn get_notify_addr(&self, queue_index: u16) -> u64 {
        // 关键修复：根据 VirtIO 1.0 规范 4.1.4.4，
        // 通知地址 = notify_offset + 2 * (queue_index * notify_off_multiplier)
        // 即：对 notify_off_multiplier 乘以 2（因为它是以 16 位为单位，转换为字节需要乘 2）
        let queue_offset = (queue_index as u64 * self.notify_off_multiplier as u64) * 2;
        self.notify_cfg_bar + self.notify_cfg_offset as u64 + queue_offset
    }

    /// 通知设备
    pub fn notify(&self, queue_index: u16) {
        let notify_addr = self.get_notify_addr(queue_index);
        unsafe {
            let notify_ptr = notify_addr as *mut u16;
            // VirtIO 1.0 规范：写入队列索引（16位）到通知寄存器
            core::ptr::write_volatile(notify_ptr, queue_index);
        }
    }

    /// 使能设备中断
    ///
    /// RISC-V QEMU virt 平台的 PCIe IRQ 路由（来自 QEMU 源码 hw/riscv/virt.c）：
    /// PCIE_IRQ 基址 = 32，共 4 个 IRQ (32-35)
    /// 公式: IRQ = 32 + ((INT_PIN + PCI_slot) % 4)
    /// 参考: create_pcie_irq_map() 中的 irq_nr = PCIE_IRQ + ((pin + PCI_SLOT(devfn)) % PCI_NUM_PINS)
    pub fn enable_device_interrupt(&self) {
        // 读取 INT_PIN 来确定 IRQ 偏移
        let int_pin = self.pci_config.read_config_byte(0x3D);

        // PCIe IRQ 计算公式（QEMU RISC-V virt 平台）
        // 注意：INT_PIN 从 1 开始（INTA=1, INTB=2, INTC=3, INTD=4）
        let irq = 32 + ((int_pin as u32 + self.pci_slot as u32) % 4);

        // 使能 IRQ（在当前 boot hart 上）
        #[cfg(feature = "riscv64")]
        {
            let boot_hart = crate::arch::riscv64::smp::cpu_id();
            crate::drivers::intc::plic::enable_interrupt(boot_hart, irq as usize);
        }
    }

    /// 设置队列 MSI-X 向量
    ///
    /// VirtIO 1.0 规范要求在 queue_enable 之前设置 MSI-X 向量
    /// 这告诉设备使用哪个 MSI-X 向量来发送队列完成中断
    ///
    /// # 参数
    /// - `queue_index`: 队列索引（0 为第一个队列）
    /// - `vector`: MSI-X 向量号（0 表示不使用 MSI-X，使用传统 INTx）
    pub fn set_queue_vector(&self, queue_index: u16, vector: u16) {
        // VirtIO Common CFG 偏移 0x1C: queue_msix_vector
        unsafe {
            let vector_ptr = (self.common_cfg_bar + 0x1C) as *mut u16;
            core::ptr::write_volatile(vector_ptr, vector);
        }
        let _ = queue_index; // 避免未使用警告
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
            0,  // queue_index
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

/// 使用已配置的 VirtQueue 读取块设备
///
/// # 参数
/// - `pci_dev`: VirtIO PCI 设备
/// - `sector`: 起始扇区号
/// - `buf`: 数据缓冲区
///
/// # 返回
/// 成功返回读取的字节数，失败返回错误码
pub fn read_block_using_configured_queue(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &mut [u8]
) -> Result<usize, &'static str> {
    use crate::drivers::virtio::queue::{VirtIOBlkReqHeader, VirtIOBlkResp, req_type};

    // 获取已配置的 VirtQueue（可变引用）
    let virt_queue = match crate::drivers::virtio::get_pci_device_queue_mut() {
        Some(q) => q,
        None => return Err("No configured VirtQueue found"),
    };

    // 重置描述符分配器，以便重用描述符
    virt_queue.reset_desc_allocator();

    // 分配三个描述符
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
        crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64)
    ).0;
    #[cfg(feature = "riscv64")]
    let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64)
    ).0;

    // 对于 PCI VirtIO，我们需要确保缓冲区在物理内存中可访问
    #[cfg(feature = "riscv64")]
    let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
    ).0;
    #[cfg(not(feature = "riscv64"))]
    let data_phys_addr = buf.as_ptr() as u64;

    // 设置请求头描述符
    virt_queue.set_desc(
        header_desc_idx,
        header_phys_addr,
        core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
        VIRTQ_DESC_F_NEXT,
        data_desc_idx,
    );

    // 设置数据缓冲区描述符（设备写入）
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

    // 获取当前的期望值（提交前的 used.idx 期望值）
    let prev_expected = crate::drivers::virtio::get_expected_used_idx();

    // 提交到可用环
    virt_queue.submit(header_desc_idx);

    // 递增期望的 used.idx（跟踪我们期望的完成计数）
    crate::drivers::virtio::increment_expected_used_idx();

    // 通知设备（使用 PCI 设备的 notify 方法）
    pci_dev.notify(0);

    // 等待完成 - 等待 used.idx 达到期望值
    let new_used = virt_queue.wait_for_completion(prev_expected);

    if new_used == prev_expected {
        // 请求失败，设备没有更新 used ring
        unsafe {
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
        }
        return Err("VirtIO request timeout");
    }

    // 读取响应状态
    let status = unsafe { *resp_ptr };

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
