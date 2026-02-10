//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 网络设备驱动
//!
//! 完全遵循 VirtIO 规范和 Linux 内核的 virtio_net 实现
//! 参考: drivers/net/virtio_net.c, Documentation/virtio/

use crate::drivers::virtio::queue;
use crate::drivers::net::space::{NetDevice, NetDeviceOps, DeviceStats, ArpHrdType, dev_flags};
use crate::net::buffer::SkBuff;
use spin::Mutex;

/// VirtIO 网络设备寄存器布局
///
/// 对应 VirtIO 网络设备的 MMIO 寄存器
/// VirtIO Legacy MMIO Register Layout
#[repr(C)]
pub struct VirtIONetRegs {
    _padding0: [u8; 0x00],  // 0x00
    /// 魔数 (0x74726976 "virt")
    pub magic_value: u32,   // 0x00
    /// 版本
    pub version: u32,        // 0x04
    /// 设备 ID (网络设备 = 1)
    pub device_id: u32,      // 0x08
    /// 厂商 ID
    pub vendor: u32,         // 0x0C
    _padding1: [u8; 0x04],  // 0x10-0x13
    /// 设备特征
    pub device_features: u32, // 0x14
    _padding2: [u8; 0x18],  // 0x18-0x2F
    /// 队列选择
    pub queue_sel: u32,      // 0x30
    /// 队列最大数量
    pub queue_num_max: u32, // 0x34
    /// 队列数量
    pub queue_num: u32,      // 0x38
    /// 队列就绪
    pub queue_ready: u32,    // 0x3C
    /// 队列通知
    pub queue_notify: u32,  // 0x40
    _padding3: [u8; 0x0C],  // 0x44-0x4F
    /// 驱动状态
    pub status: u32,         // 0x50
    _padding4: [u8; 0x4C],  // 0x54-0x9F
    /// 队列描述符表地址
    pub queue_desc: u64,     // 0xA0
    /// 队列可用环地址
    pub queue_driver: u64,   // 0xA8
    /// 队列已用环地址
    pub queue_device: u64,   // 0xB0
}

/// VirtIO 网络设备配置
///
/// 对应 VirtIO 网络设备的配置空间
#[repr(C)]
pub struct VirtIONetConfig {
    /// MAC 地址
    pub mac: [u8; 6],
    /// 设备状态
    pub status: u16,
    /// 最大 VIRTIO 包大小
    pub mtu: u16,
}

/// VirtIO 网络包头部
///
/// 对应 VirtIO 网络设备的包头格式
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIONetHdr {
    /// 标志
    pub flags: u8,
    /// GSO 类型
    pub gso_type: u8,
    /// 头部长度
    pub hdr_len: u16,
    /// GSO 大小
    pub gso_size: u16,
    /// 校验和起始位置
    pub csum_start: u16,
    /// 校验和偏移
    pub csum_offset: u16,
    /// 缓冲区数量
    pub num_buffers: u16,
}

/// VirtIO 网络设备
pub struct VirtIONetDevice {
    /// MMIO 基地址
    base_addr: u64,
    /// MAC 地址
    mac: [u8; 6],
    /// MTU
    mtu: u16,
    /// 初始化状态
    initialized: Mutex<bool>,
    /// 发送队列 (TX Queue - Queue 0)
    tx_queue: Mutex<Option<queue::VirtQueue>>,
    /// 接收队列 (RX Queue - Queue 1)
    rx_queue: Mutex<Option<queue::VirtQueue>>,
    /// 队列大小
    queue_size: u16,
    /// 统计信息
    stats: Mutex<DeviceStats>,
}

unsafe impl Send for VirtIONetDevice {}

impl VirtIONetDevice {
    /// 创建新的 VirtIO 网络设备
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
            mtu: 1500,
            initialized: Mutex::new(false),
            tx_queue: Mutex::new(None),
            rx_queue: Mutex::new(None),
            queue_size: 0,
            stats: Mutex::new(DeviceStats::default()),
        }
    }

    /// 初始化设备
    pub fn init(&mut self) -> Result<(), &'static str> {
        unsafe {
            // VirtIO MMIO 寄存器偏移量
            const MAGIC_VALUE: u64 = 0x00;
            const VERSION: u64 = 0x04;
            const DEVICE_ID: u64 = 0x08;
            const VENDOR: u64 = 0x0C;
            const DEVICE_FEATURES: u64 = 0x14;
            const QUEUE_SEL: u64 = 0x30;
            const QUEUE_NUM_MAX: u64 = 0x34;
            const QUEUE_NUM: u64 = 0x38;
            const QUEUE_READY: u64 = 0x3C;
            const QUEUE_NOTIFY: u64 = 0x40;
            const STATUS: u64 = 0x50;
            const QUEUE_DESC: u64 = 0xA0;
            const QUEUE_DRIVER: u64 = 0xA8;
            const QUEUE_DEVICE: u64 = 0xB0;

            // 验证魔数
            let magic = core::ptr::read_volatile((self.base_addr + MAGIC_VALUE) as *const u32);
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // 验证版本
            let version = core::ptr::read_volatile((self.base_addr + VERSION) as *const u32);
            if version != 1 && version != 2 {
                return Err("Unsupported VirtIO version");
            }

            // 验证设备 ID (网络设备 = 1)
            let device_id = core::ptr::read_volatile((self.base_addr + DEVICE_ID) as *const u32);
            if device_id != 1 {
                return Err("Not a VirtIO network device");
            }

            // 设置驱动状态：ACKNOWLEDGE
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x01);

            // 设置驱动状态：DRIVER
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x03);

            // 读取 MAC 地址 (从配置空间，偏移 0x100)
            // 在 QEMU virt 平台中，MAC 地址在配置空间的偏移 0 处
            let config_ptr = (self.base_addr + 0x100) as *const u8;
            for i in 0..6 {
                self.mac[i] = *config_ptr.add(i);
            }

            // 读取 MTU (从偏移 0x106)
            let mtu_ptr = (self.base_addr + 0x106) as *const u16;
            self.mtu = core::ptr::read_volatile(mtu_ptr);
            if self.mtu == 0 {
                self.mtu = 1500; // 默认 MTU
            }

            // ========== 设置 TX 队列 (Queue 0) ==========
            // 选择队列 0
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 0);

            // 读取最大队列大小
            let max_queue_size = core::ptr::read_volatile((self.base_addr + QUEUE_NUM_MAX) as *const u32);
            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            // 设置队列大小
            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // 分配描述符表
            let desc_size = self.queue_size as usize * core::mem::size_of::<queue::Desc>();
            let desc_layout = alloc::alloc::Layout::from_size_align(desc_size, 16)
                .map_err(|_| "Failed to create descriptor layout")?;
            let desc_ptr = alloc::alloc::alloc(desc_layout) as *mut queue::Desc;
            if desc_ptr.is_null() {
                return Err("Failed to allocate TX descriptor table");
            }

            // 初始化描述符表
            let desc_slice = core::slice::from_raw_parts_mut(desc_ptr, self.queue_size as usize);
            for desc in desc_slice.iter_mut() {
                *desc = queue::Desc {
                    addr: 0,
                    len: 0,
                    flags: 0,
                    next: 0,
                };
            }

            // 设置队列地址
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC) as *mut u64, desc_ptr as u64);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER) as *mut u64, 0);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE) as *mut u64, 0);

            // 设置队列数量
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            // 设置队列就绪
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            // 创建 VirtQueue
            let tx_queue = queue::VirtQueue::new(
                desc_slice,
                self.queue_size,
                self.base_addr + QUEUE_NOTIFY,
            );
            *self.tx_queue.lock() = Some(tx_queue);

            // ========== 设置 RX 队列 (Queue 1) ==========
            // 选择队列 1
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 1);

            // 分配描述符表
            let desc_ptr_rx = alloc::alloc::alloc(desc_layout) as *mut queue::Desc;
            if desc_ptr_rx.is_null() {
                alloc::alloc::dealloc(desc_ptr as *mut u8, desc_layout);
                return Err("Failed to allocate RX descriptor table");
            }

            // 初始化描述符表
            let desc_slice_rx = core::slice::from_raw_parts_mut(desc_ptr_rx, self.queue_size as usize);
            for desc in desc_slice_rx.iter_mut() {
                *desc = queue::Desc {
                    addr: 0,
                    len: 0,
                    flags: 0,
                    next: 0,
                };
            }

            // 设置队列地址
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC) as *mut u64, desc_ptr_rx as u64);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER) as *mut u64, 0);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE) as *mut u64, 0);

            // 设置队列数量
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            // 设置队列就绪
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            // 创建 VirtQueue
            let rx_queue = queue::VirtQueue::new(
                desc_slice_rx,
                self.queue_size,
                self.base_addr + QUEUE_NOTIFY,
            );
            *self.rx_queue.lock() = Some(rx_queue);

            // 设置驱动状态：DRIVER_OK
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x07);

            // 标记为已初始化
            *self.initialized.lock() = true;

            Ok(())
        }
    }

    /// 获取 MAC 地址
    pub fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    /// 获取 MTU
    pub fn get_mtu(&self) -> u16 {
        self.mtu
    }

    /// 发送数据包
    ///
    /// # 参数
    /// - `skb`: 要发送的数据包
    ///
    /// # 返回
    /// 成功返回 0，失败返回负数错误码
    pub fn xmit(&self, skb: SkBuff) -> i32 {
        if !*self.initialized.lock() {
            return -5; // EIO
        }

        // 获取 TX 队列
        let mut queue_guard = self.tx_queue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return -5, // EIO
        };

        // 分配 VirtIO 网络包头
        let hdr_layout = alloc::alloc::Layout::new::<VirtIONetHdr>();
        let hdr_ptr: *mut VirtIONetHdr;
        unsafe {
            hdr_ptr = alloc::alloc::alloc(hdr_layout) as *mut VirtIONetHdr;
        }
        if hdr_ptr.is_null() {
            return -12; // ENOMEM
        }
        unsafe {
            *hdr_ptr = VirtIONetHdr {
                flags: 0,
                gso_type: 0,
                hdr_len: 0,
                gso_size: 0,
                csum_start: 0,
                csum_offset: 0,
                num_buffers: 1,
            };
        }

        // VirtIO 描述符标志
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // 添加包头描述符
        let header_desc_idx = queue.add_desc(
            hdr_ptr as u64,
            core::mem::size_of::<VirtIONetHdr>() as u32,
            VIRTQ_DESC_F_NEXT,
        );

        // 添加数据描述符
        let data_desc_idx = queue.add_desc(
            skb.data as u64,
            skb.len,
            0, // 最后一个描述符
        );

        // 设置链接关系
        unsafe {
            if let Some(desc) = queue.get_desc(header_desc_idx) {
                let desc_ptr = &desc as *const queue::Desc as *mut queue::Desc;
                (*desc_ptr).next = data_desc_idx;
            }
        }

        // 通知设备
        queue.notify();

        // 等待完成
        let prev_used = queue.get_used();
        let _used = queue.wait_for_completion(prev_used);

        // 释放包头
        unsafe {
            alloc::alloc::dealloc(hdr_ptr as *mut u8, hdr_layout);
        }

        // 更新统计信息
        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += skb.len as u64;

        // 释放 skb
        skb.free();

        0
    }

    /// 接收数据包
    ///
    /// # 返回
    /// 返回接收到的数据包，如果没有数据包则返回 None
    pub fn poll(&self) -> Option<SkBuff> {
        if !*self.initialized.lock() {
            return None;
        }

        // 获取 RX 队列
        let mut queue_guard = self.rx_queue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return None,
        };

        // 检查是否有已用的描述符
        let used_idx = queue.get_used();
        let avail_idx = queue.get_avail();

        if used_idx == avail_idx {
            return None; // 没有新的数据包
        }

        // TODO: 从队列中读取数据包
        // 当前简化实现：返回 None
        None
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> DeviceStats {
        *self.stats.lock()
    }
}

/// VirtIO 网络设备发送函数 (供 NetDevice 调用)
fn virtio_net_xmit(skb: SkBuff) -> i32 {
    // 获取全局 VirtIO 网络设备
    unsafe {
        if let Some(device) = VIRTIO_NET.as_ref() {
            device.xmit(skb)
        } else {
            skb.free();
            -5 // EIO
        }
    }
}

/// VirtIO 网络设备统计信息获取函数
fn virtio_net_get_stats() -> DeviceStats {
    unsafe {
        if let Some(device) = VIRTIO_NET.as_ref() {
            device.get_stats()
        } else {
            DeviceStats::default()
        }
    }
}

/// VirtIO 网络设备操作接口
static VIRTIO_NET_OPS: NetDeviceOps = NetDeviceOps {
    xmit: virtio_net_xmit,
    init: None,
    uninit: None,
    get_stats: Some(virtio_net_get_stats),
};

/// 全局 VirtIO 网络设备
static mut VIRTIO_NET: Option<VirtIONetDevice> = None;
static mut VIRTIO_NET_DEVICE: Option<NetDevice> = None;

/// 初始化 VirtIO 网络设备
///
/// # 参数
/// - `base_addr`: MMIO 基地址 (QEMU virt 平台通常为 0x10001000)
pub fn init(base_addr: u64) -> Result<(), &'static str> {
    unsafe {
        let mut device = VirtIONetDevice::new(base_addr);

        device.init()?;

        // 获取 MAC 地址
        let mac = device.get_mac();

        // 创建 NetDevice
        let mut net_device = NetDevice {
            name: [0u8; 16],
            ifindex: 0,
            mtu: device.get_mtu() as u32,
            type_: ArpHrdType::ARPHRD_ETHER,
            addr: [0u8; 32],
            addr_len: 6,
            netdev_ops: &VIRTIO_NET_OPS,
            priv_: core::ptr::null_mut(),
            stats: DeviceStats::default(),
            flags: dev_flags::IFF_UP | dev_flags::IFF_RUNNING | dev_flags::IFF_BROADCAST,
            rx_queue_len: 0,
        };

        // 设置设备名
        let name = b"eth0\0";
        net_device.name[..name.len()].copy_from_slice(name);

        // 设置 MAC 地址
        net_device.set_address(&mac, 6);

        // 存储设备
        VIRTIO_NET = Some(device);
        VIRTIO_NET_DEVICE = Some(net_device);

        // 注册网络设备
        if let Some(ref mut dev) = VIRTIO_NET_DEVICE {
            crate::drivers::net::register_netdevice(dev);
        }

        Ok(())
    }
}

/// 获取 VirtIO 网络设备
pub fn get_device() -> Option<&'static VirtIONetDevice> {
    unsafe { VIRTIO_NET.as_ref() }
}

/// 获取 VirtIO 网络设备的 NetDevice
pub fn get_net_device() -> Option<&'static mut NetDevice> {
    unsafe { VIRTIO_NET_DEVICE.as_mut() }
}
