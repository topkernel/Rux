//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO Input 设备驱动
//!
//! 实现 VirtIO Input PCI 设备的初始化和事件读取
//! 参考: VirtIO 1.2 规范 - Input Device

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
// VirtIO Input PCI 设备 ID
// ============================================================================

/// VirtIO Input 设备 Vendor ID (Red Hat)
const VIRTIO_INPUT_PCI_VENDOR: u16 = 0x1AF4;

/// VirtIO Input 设备 Device ID (0x1040 + 18 = 0x1052)
/// 参考: VirtIO 1.2 规范
const VIRTIO_INPUT_PCI_DEVICE: u16 = 0x1052;

// ============================================================================
// VirtIO Input 队列索引
// ============================================================================

/// 事件队列 (设备 -> 驱动)
const EVENT_QUEUE: u16 = 0;
/// 状态队列 (驱动 -> 设备)
const STATUS_QUEUE: u16 = 1;

// ============================================================================
// VirtIO Input 配置结构
// ============================================================================

/// VirtIO Input 配置寄存器
#[repr(C)]
struct VirtioInputConfig {
    /// 配置选择寄存器
    select: u8,
    /// 子选择寄存器
    subsel: u8,
    /// 数据大小
    size: u8,
    /// 保留
    reserved: [u8; 5],
    /// 配置数据 (union)
    payload: [u8; 128],
}

/// VirtIO Input 事件 (8 字节)
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioInputEvent {
    /// 事件类型
    pub type_: u16,
    /// 事件代码
    pub code: u16,
    /// 事件值
    pub value: i32,
}

// ============================================================================
// 配置选择器
// ============================================================================

/// 未使用的配置
const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
/// ID 名称字符串
const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
/// ID 序列号字符串
const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
/// ID 设备 ID
const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
/// 属性位图
const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
/// 事件位图
const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
/// 绝对轴信息
const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;

// ============================================================================
// VirtIO Input 设备
// ============================================================================

/// VirtIO Input 设备
pub struct VirtioInputDevice {
    /// VirtIO PCI 设备
    pci: VirtIOPCI,
    /// 事件队列
    event_queue: Option<VirtQueue>,
    /// 事件缓冲区
    event_buffer: *mut VirtioInputEvent,
    /// 事件缓冲区布局
    event_buffer_layout: Option<Layout>,
    /// 事件缓冲区物理地址
    event_buffer_phys: u64,
    /// 设备名称
    name: [u8; 32],
    /// 是否为指针设备（鼠标/触摸屏）
    is_pointer: bool,
    /// 上次处理的已用索引
    last_used: u16,
}

unsafe impl Send for VirtioInputDevice {}
unsafe impl Sync for VirtioInputDevice {}

impl VirtioInputDevice {
    /// 创建新的 VirtIO Input 设备
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

    /// 初始化 VirtIO 设备
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;

        // 步骤 1: 重置设备
        unsafe {
            write_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *mut u8, 0);
        }
        fence(Ordering::SeqCst);

        // 步骤 2-3: 设置 ACKNOWLEDGE | DRIVER
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // 步骤 4-6: 特性协商（不需要特殊特性）
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // 步骤 7: 验证 FEATURES_OK
        let status_val = unsafe {
            read_volatile((common_cfg + offset::DEVICE_STATUS as u64) as *const u8)
        };
        if (status_val & status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // 步骤 8: 初始化事件队列
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

        // 分配事件缓冲区
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

        // 获取物理地址
        self.event_buffer_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(event_buffer as u64)
        ).0;

        // 创建 VirtQueue
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

        // 获取队列物理地址
        let desc_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.desc as u64 })
        ).0;
        let avail_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.avail as u64 })
        ).0;
        let used_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(unsafe { queue.used as u64 })
        ).0;

        // 设置队列地址
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

        // 启用队列
        unsafe {
            write_volatile(
                (common_cfg + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16,
                1,
            );
        }
        fence(Ordering::SeqCst);

        self.event_queue = Some(queue);

        // 步骤 9: 设置 DRIVER_OK
        unsafe {
            write_volatile(
                (common_cfg + offset::DEVICE_STATUS as u64) as *mut u8,
                (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8,
            );
        }
        fence(Ordering::SeqCst);

        // 提交初始缓冲区以接收事件
        self.submit_event_buffers();

        Some(())
    }

    /// 读取设备信息
    fn read_device_info(&mut self) {
        let config_base = self.pci.common_cfg_bar;

        // 读取设备名称
        unsafe {
            // 选择 ID_NAME 配置
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_ID_NAME);
            write_volatile((config_base + 1) as *mut u8, 0);
            fence(Ordering::SeqCst);

            // 读取名称
            let payload = (config_base + 8) as *const u8;
            for i in 0..31 {
                let c = read_volatile(payload.add(i));
                if c == 0 {
                    break;
                }
                self.name[i] = c;
            }
        }

        // 检测是否为指针设备
        self.is_pointer = self.check_pointer_device();
    }

    /// 检查是否为指针设备
    fn check_pointer_device(&self) -> bool {
        let config_base = self.pci.common_cfg_bar;

        unsafe {
            // 检查 EV_ABS 事件 (绝对坐标)
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_EV_BITS);
            write_volatile((config_base + 1) as *mut u8, EV_ABS as u8);
            fence(Ordering::SeqCst);

            let payload = (config_base + 8) as *const u8;
            // 检查 ABS_X 和 ABS_Y 位
            let has_abs_x = (read_volatile(payload) & 0x01) != 0;

            if has_abs_x {
                return true;
            }

            // 检查 EV_REL 事件 (相对坐标)
            write_volatile((config_base + 0) as *mut u8, VIRTIO_INPUT_CFG_EV_BITS);
            write_volatile((config_base + 1) as *mut u8, EV_REL as u8);
            fence(Ordering::SeqCst);

            // 检查 REL_X 和 REL_Y 位
            let has_rel_x = (read_volatile(payload) & 0x01) != 0;

            has_rel_x
        }
    }

    /// 提交事件缓冲区
    fn submit_event_buffers(&mut self) {
        let queue = match &self.event_queue {
            Some(q) => q,
            None => return,
        };

        let queue_size = queue.queue_size as usize;

        unsafe {
            // 提交所有缓冲区
            for i in 0..queue_size {
                let event_ptr = self.event_buffer_phys + (i * core::mem::size_of::<VirtioInputEvent>()) as u64;

                let desc = &mut *queue.desc.add(i);
                desc.addr = event_ptr;
                desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
                desc.flags = 0x02; // VIRTQ_DESC_F_WRITE
                desc.next = 0;

                // 添加到可用环
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

    /// 读取输入事件
    pub fn read_event(&mut self) -> Option<InputEvent> {
        let queue = self.event_queue.as_ref()?;

        unsafe {
            let used = &*queue.used;
            let used_idx = used.idx as usize;
            let last_used = self.last_used as usize;

            if used_idx == last_used {
                return None;
            }

            // 获取已使用的描述符
            let used_ring = (queue.used as *const u8).add(8) as *const UsedElem;
            let used_elem = read_volatile(used_ring.add(last_used % queue.queue_size as usize));

            let desc_idx = used_elem.id as usize;
            let _len = used_elem.len;

            // 读取事件
            let event = read_volatile(self.event_buffer.add(desc_idx));

            // 重新提交缓冲区
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

            // 转换为标准 InputEvent
            Some(InputEvent::new(event.type_, event.code, event.value))
        }
    }

    /// 检查是否有事件
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

    /// 获取设备名称
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// 是否为指针设备
    pub fn is_pointer(&self) -> bool {
        self.is_pointer
    }
}

/// Used 环元素
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
// 设备探测
// ============================================================================

/// 探测 VirtIO Input 设备
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

        // 如果两个设备都找到了，就停止
        if keyboard.is_some() && pointer.is_some() {
            break;
        }
    }

    if keyboard.is_some() || pointer.is_some() {
        // 返回 (keyboard, pointer)
        Some((keyboard?, pointer))
    } else {
        None
    }
}

/// 探测单个 VirtIO Input 设备
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
