//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 块设备驱动
//!
//! 参考: drivers/block/virtio_blk.c, Documentation/virtio/

use spin::Mutex;

use crate::drivers::blkdev::{GenDisk, Request, BlockDeviceOps};

pub mod queue;
pub mod probe;
pub mod offset;
pub mod virtio_pci;

/// VirtIO 设备寄存器布局（符合 VirtIO 1.0 规范）
///
#[repr(C)]
pub struct VirtIOBlkRegs {
    /// 魔数 (0x00)
    pub magic_value: u32,
    /// 版本 (0x04)
    pub version: u32,
    /// 设备 ID (0x08)
    pub device_id: u32,
    /// 厂商 ID (0x0C)
    pub vendor: u32,
    /// 设备特征 (0x10)
    pub device_features: u32,
    /// _reserved (0x14)
    _reserved1: u32,
    /// 驱动选择的特征 (0x20)
    pub driver_features: u32,
    /// _reserved (0x24)
    _reserved2: u32,
    /// 队列选择 (0x30)
    pub queue_sel: u32,
    /// 队列最大数量 (0x34)
    pub queue_num_max: u32,
    /// 队列数量 (0x38)
    pub queue_num: u32,
    /// _reserved (0x3C)
    _reserved3: u32,
    /// _reserved (0x40)
    _reserved4: u32,
    /// 队列就绪 (0x44) - Modern VirtIO Queue Enable
    pub queue_ready: u32,
    /// _reserved (0x48)
    _reserved5: u32,
    /// _reserved (0x4C)
    _reserved6: u32,
    /// 队列通知 (0x50)
    pub queue_notify: u32,
    /// _reserved (0x54-0x5C)
    _reserved7: [u32; 3],
    /// 中断状态 (0x60)
    pub interrupt_status: u32,
    /// 中断应答 (0x64)
    pub interrupt_ack: u32,
    /// _reserved (0x68-0x6C)
    _reserved8: [u32; 2],
    /// 驱动状态 (0x70)
    pub status: u32,
    /// _reserved (0x74+)
    _reserved9: [u32; 4],
}

/// VirtIO 块设备
pub struct VirtIOBlkDevice {
    /// MMIO 基地址
    base_addr: u64,
    /// 块设备
    pub disk: GenDisk,
    /// 容量（扇区数）
    capacity: u64,
    /// 块大小
    block_size: u32,
    /// 初始化状态
    initialized: Mutex<bool>,
    /// VirtQueue（用于 I/O 操作）
    virtqueue: Mutex<Option<queue::VirtQueue>>,
    /// 队列大小
    queue_size: u16,
    /// IRQ 号
    irq: u32,
}

unsafe impl Send for VirtIOBlkDevice {}
unsafe impl Sync for VirtIOBlkDevice {}

impl VirtIOBlkDevice {
    /// 创建新的 VirtIO 块设备
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            disk: GenDisk::new("virtblk", 0, 1, 512, None as Option<&BlockDeviceOps>),
            capacity: 0,
            block_size: 512,
            initialized: Mutex::new(false),
            virtqueue: Mutex::new(None),
            queue_size: 0,
            irq: 1,  // 默认 IRQ 1（第一个 VirtIO 设备）
        }
    }

    /// 初始化设备
    pub fn init(&mut self) -> Result<(), &'static str> {
        // VirtIO MMIO 寄存器偏移量
        const MAGIC_VALUE_OFFSET: u64 = 0x000;
        const VERSION_OFFSET: u64 = 0x004;
        const DEVICE_ID_OFFSET: u64 = 0x008;
        const STATUS_OFFSET: u64 = 0x070;
        const GUEST_PAGE_SIZE_OFFSET: u64 = 0x028;
        const DEVICE_FEATURES_OFFSET: u64 = 0x010;
        const DRIVER_FEATURES_OFFSET: u64 = 0x020;
        const QUEUE_SEL_OFFSET: u64 = 0x030;
        const QUEUE_NUM_MAX_OFFSET: u64 = 0x034;
        const QUEUE_NUM_OFFSET: u64 = 0x038;

        // 辅助宏：打印寄存器读写
        macro_rules! read_reg {
            ($offset:expr, $name:expr) => {
                {
                    let ptr = (self.base_addr + $offset) as *const u32;
                    core::ptr::read_volatile(ptr)
                }
            };
        }

        macro_rules! write_reg {
            ($offset:expr, $name:expr, $val:expr) => {
                {
                    let ptr = (self.base_addr + $offset) as *mut u32;
                    core::ptr::write_volatile(ptr, $val);
                }
            };
        }

        unsafe {
            // 1. 验证魔数
            let magic = read_reg!(MAGIC_VALUE_OFFSET, "MAGIC_VALUE");
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // 2. 验证版本（只支持 Modern VirtIO 1.0+）
            let version = read_reg!(VERSION_OFFSET, "VERSION");
            if version != 2 {
                return Err("Unsupported VirtIO version: only Modern VirtIO 1.0+ (version 2) is supported, Legacy VirtIO is not supported");
            }

            // 3. 验证设备 ID
            let device_id = read_reg!(DEVICE_ID_OFFSET, "DEVICE_ID");
            if device_id != 2 {
                return Err("Not a VirtIO block device");
            }

            // 4. 状态机：重置设备
            write_reg!(STATUS_OFFSET, "STATUS", 0x00);

            // 5. 状态机：ACKNOWLEDGE (0x01)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01);
            let status = read_reg!(STATUS_OFFSET, "STATUS");

            // 6. 状态机：DRIVER (0x02)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            let status = read_reg!(STATUS_OFFSET, "STATUS");

            // 检查是否需要重置
            if status & 0x40 != 0 {
                write_reg!(STATUS_OFFSET, "STATUS", 0x00);
                write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            }

            // 7. 读取设备特性
            let _device_features = read_reg!(DEVICE_FEATURES_OFFSET, "DEVICE_FEATURES");

            // 9. 特性协商（Modern VirtIO）
            // 写入 DRIVER_FEATURES 寄存器
            // 设置 FEATURES_OK 位（表示特性协商完成）
            write_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES", 0);

            // 9.5. 设置 FEATURES_OK 位
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x08);

            // ========== VirtQueue 设置 ==========

            // 10. 选择队列 0
            write_reg!(QUEUE_SEL_OFFSET, "QUEUE_SEL", 0);

            // 11. 读取最大队列大小
            let max_queue_size = read_reg!(QUEUE_NUM_MAX_OFFSET, "QUEUE_NUM_MAX");

            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // 12. 设置队列数量
            write_reg!(QUEUE_NUM_OFFSET, "QUEUE_NUM", self.queue_size as u32);

            // 13. 创建 VirtQueue（分配 vring 内存）
            let virtqueue = match queue::VirtQueue::new(
                self.queue_size,
                0,  // queue_index: 块设备只使用队列 0
                self.base_addr + 0x50,  // queue_notify
                self.base_addr + 0x60,  // interrupt_status
                self.base_addr + 0x64,  // interrupt_ack
            ) {
                Some(vq) => vq,
                None => return Err("Failed to allocate VirtQueue"),
            };

            let desc_addr = virtqueue.get_desc_addr();
            let avail_addr = virtqueue.get_avail_addr();
            let used_addr = virtqueue.get_used_addr();
            // 14. Modern VirtIO: 设置队列地址（64位，分高低位）
            // Modern VirtIO 使用三个独立的地址寄存器对来设置队列
            use crate::drivers::virtio::offset;
            const QUEUE_DESC_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DESC_LO as u64;
            const QUEUE_DESC_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DESC_HI as u64;
            const QUEUE_DRIVER_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DRIVER_LO as u64;
            const QUEUE_DRIVER_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DRIVER_HI as u64;
            const QUEUE_DEVICE_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DEVICE_LO as u64;
            const QUEUE_DEVICE_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DEVICE_HI as u64;
            const QUEUE_READY_OFFSET: u64 = offset::COMMON_CFG_QUEUE_ENABLE as u64;

            // 转换虚拟地址为物理地址
            #[cfg(feature = "riscv64")]
            let desc_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(desc_addr)
            ).0;
            #[cfg(not(feature = "riscv64"))]
            let desc_phys_addr = desc_addr;
            #[cfg(feature = "riscv64")]
            let avail_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(avail_addr)
            ).0;
            #[cfg(not(feature = "riscv64"))]
            let avail_phys_addr = avail_addr;
            #[cfg(feature = "riscv64")]
            let used_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(used_addr)
            ).0;
            #[cfg(not(feature = "riscv64"))]
            let used_phys_addr = used_addr;

            // 写入描述符表地址（低32位）
            write_reg!(QUEUE_DESC_LO_OFFSET, "QUEUE_DESC_LO", (desc_phys_addr & 0xFFFFFFFF) as u32);
            // 写入描述符表地址（高32位）
            write_reg!(QUEUE_DESC_HI_OFFSET, "QUEUE_DESC_HI", (desc_phys_addr >> 32) as u32);

            // 写入可用环地址（低32位）
            write_reg!(QUEUE_DRIVER_LO_OFFSET, "QUEUE_DRIVER_LO", (avail_phys_addr & 0xFFFFFFFF) as u32);
            // 写入可用环地址（高32位）
            write_reg!(QUEUE_DRIVER_HI_OFFSET, "QUEUE_DRIVER_HI", (avail_phys_addr >> 32) as u32);

            // 写入已用环地址（低32位）
            write_reg!(QUEUE_DEVICE_LO_OFFSET, "QUEUE_DEVICE_LO", (used_phys_addr & 0xFFFFFFFF) as u32);
            // 写入已用环地址（高32位）
            write_reg!(QUEUE_DEVICE_HI_OFFSET, "QUEUE_DEVICE_HI", (used_phys_addr >> 32) as u32);

            // 设置队列就绪位
            write_reg!(QUEUE_READY_OFFSET, "QUEUE_READY", 1);

            // 15. 读取设备容量
            const VIRTIO_BLK_CONFIG_CAPACITY: u64 = 0x100;
            let cap_ptr = (self.base_addr + VIRTIO_BLK_CONFIG_CAPACITY) as *const u64;
            self.capacity = *cap_ptr;

            // 16. 更新块设备信息
            self.disk.set_capacity(self.capacity as u32);
            self.disk.set_request_fn(Self::handle_request);
            *self.virtqueue.lock() = Some(virtqueue);

            // 17. 状态机：DRIVER_OK (0x04)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x08 | 0x04);

            // 内存屏障
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

            // 标记为已初始化
            *self.initialized.lock() = true;

            Ok(())
        }
    }

    /// 获取容量
    pub fn get_capacity(&self) -> u64 {
        self.capacity
    }

    /// 处理 I/O 请求
    unsafe extern "C" fn handle_request(req: &mut Request) {
        // 从 private_data 获取 VirtIOBlkDevice 指针
        let gd = &*req.device;
        let device_ptr = match gd.private_data {
            Some(ptr) => ptr as *const VirtIOBlkDevice,
            None => {
                if let Some(end_io) = req.end_io {
                    end_io(req, -5);  // EIO
                }
                return;
            }
        };

        let device = &*device_ptr;

        // 根据命令类型执行相应的操作
        let result = match req.cmd_type {
            crate::drivers::blkdev::ReqCmd::Read => {
                // 读取块
                device.read_block(req.sector, &mut req.buffer)
            }
            crate::drivers::blkdev::ReqCmd::Write => {
                // 写入块
                device.write_block(req.sector, &req.buffer)
            }
            crate::drivers::blkdev::ReqCmd::Flush => {
                // 刷新操作（暂时返回成功）
                Ok(())
            }
        };

        // 调用完成回调
        match result {
            Ok(()) => {
                if let Some(end_io) = req.end_io {
                    end_io(req, 0);  // Success
                }
            }
            Err(err) => {
                crate::println!("virtio-blk: I/O error: {}", err);
                if let Some(end_io) = req.end_io {
                    end_io(req, err);
                }
            }
        }
    }

    /// 读取块
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), i32> {
        if !*self.initialized.lock() {
            return Err(-5);  // EIO
        }

        // 获取 VirtQueue
        let mut queue_guard = self.virtqueue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return Err(-5),
        };

        use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

        // 构造 VirtIO 块请求头
        let req_header = VirtIOBlkReqHeader {
            type_: queue::req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        };

        // 分配请求头缓冲区（需要持久化直到请求完成）
        let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
        let header_ptr: *mut VirtIOBlkReqHeader;
        unsafe {
            header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
        }
        if header_ptr.is_null() {
            return Err(-12);  // ENOMEM
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
            return Err(-12);  // ENOMEM
        }
        unsafe {
            (*resp_ptr).status = 0xFF;  // 初始化为无效状态
        }

        // VirtIO 描述符标志
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // 将虚拟地址转换为物理地址（VirtIO 设备需要物理地址进行 DMA）
        #[cfg(feature = "riscv64")]
        let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64)
        ).0;
        #[cfg(feature = "riscv64")]
        let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
        ).0;
        #[cfg(feature = "riscv64")]
        let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64)
        ).0;

        // 如果不是 RISC-V，使用原始地址（仅用于其他架构）
        #[cfg(not(feature = "riscv64"))]
        let header_phys_addr = header_ptr as u64;
        #[cfg(not(feature = "riscv64"))]
        let data_phys_addr = buf.as_ptr() as u64;
        #[cfg(not(feature = "riscv64"))]
        let resp_phys_addr = resp_ptr as u64;

        // 分配三个描述符
        let header_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err(-5),
        };
        let data_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err(-5),
        };
        let resp_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err(-5),
        };

        // 设置请求头描述符（只读，设备读取）- 使用物理地址
        queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // 设置数据缓冲区描述符（只写，设备写入）- 使用物理地址
        // 对于读请求，数据缓冲区必须是设备可写的
        queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,  // WRITE + NEXT
            resp_desc_idx,
        );

        // 设置响应描述符（只写，设备写入）- 使用物理地址
        queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            0,  // 最后一个描述符
            0,
        );

        // 提交到可用环
        queue.submit(header_desc_idx);

        // 通知设备
        queue.notify();

        // 等待设备完成请求
        let prev_used = queue.get_used();
        let used = queue.wait_for_completion(prev_used);

        // 检查中断状态并清除
        const INTERRUPT_STATUS_OFFSET: u64 = 0x60;
        unsafe {
            let irq_ptr = (self.base_addr + INTERRUPT_STATUS_OFFSET) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_ptr);
            if irq_status != 0 {
                const INTERRUPT_ACK_OFFSET: u64 = 0x64;
                let ack_ptr = (self.base_addr + INTERRUPT_ACK_OFFSET) as *mut u32;
                core::ptr::write_volatile(ack_ptr, irq_status);
            }
        }

        // 检查响应状态
        unsafe {
            let status = (*resp_ptr).status;
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);

            if status == queue::status::VIRTIO_BLK_S_OK {
                Ok(())
            } else {
                Err(-5)  // EIO
            }
        }
    }

    /// 写入块
    pub fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), i32> {
        if !*self.initialized.lock() {
            return Err(-5);  // EIO
        }

        // 获取 VirtQueue
        let mut queue_guard = self.virtqueue.lock();
        let queue = queue_guard.as_mut().ok_or(-5)?;

        use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

        // 构造 VirtIO 块请求头
        let req_header = VirtIOBlkReqHeader {
            type_: queue::req_type::VIRTIO_BLK_T_OUT,
            reserved: 0,
            sector,
        };

        // 分配请求头缓冲区（需要持久化直到请求完成）
        let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
        let header_ptr: *mut VirtIOBlkReqHeader;
        unsafe {
            header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
        }
        if header_ptr.is_null() {
            return Err(-12);  // ENOMEM
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
            return Err(-12);  // ENOMEM
        }
        unsafe {
            (*resp_ptr).status = 0xFF;  // 初始化为无效状态
        }

        // VirtIO 描述符标志
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // 分配三个描述符
        let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

        // 设置请求头描述符（只读，设备读取）
        queue.set_desc(
            header_desc_idx,
            header_ptr as u64,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // 设置数据缓冲区描述符（只读，设备读取）
        queue.set_desc(
            data_desc_idx,
            buf.as_ptr() as u64,
            buf.len() as u32,
            VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // 设置响应描述符（只写，设备写入）
        queue.set_desc(
            resp_desc_idx,
            resp_ptr as u64,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        // 提交到可用环
        queue.submit(header_desc_idx);

        // 通知设备
        queue.notify();

        // 等待完成
        let prev_used = queue.get_used();
        let _used = queue.wait_for_completion(prev_used);

        // 检查响应状态
        unsafe {
            let status = (*resp_ptr).status;
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);

            if status == queue::status::VIRTIO_BLK_S_OK {
                Ok(())
            } else if status == queue::status::VIRTIO_BLK_S_IOERR {
                Err(-5)  // EIO
            } else {
                Err(-5)  // EIO
            }
        }
    }
}

/// VirtIO 块设备操作
static VIRTIO_BLK_OPS: BlockDeviceOps = BlockDeviceOps {
    open: None,
    release: None,
    getgeo: None,
};

/// 全局 VirtIO 块设备（MMIO）
static mut VIRTIO_BLK: Option<VirtIOBlkDevice> = None;

/// 全局 VirtIO PCI 块设备（使用裸指针存储）
static mut VIRTIO_PCI_BLK: Option<crate::drivers::virtio::virtio_pci::VirtIOPCI> = None;

/// 全局 VirtIO PCI 块设备 VirtQueue（已配置的队列）
static mut VIRTIO_PCI_BLK_QUEUE: Option<queue::VirtQueue> = None;

/// 全局 VirtIO PCI 块设备期望的 used.idx（用于跟踪 I/O 完成状态）
/// 每次提交请求时递增，用于检测设备是否完成了请求
static mut VIRTIO_PCI_EXPECTED_USED_IDX: u16 = 0;

/// PCI 设备就绪标志（使用原子类型确保多核可见性）
static VIRTIO_PCI_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// 初始化 VirtIO 块设备
///
/// # 参数
/// - `base_addr`: MMIO 基地址（QEMU virt 平台通常为 0x10001000）
pub fn init(base_addr: u64) -> Result<(), &'static str> {
    unsafe {
        let mut device = VirtIOBlkDevice::new(base_addr);

        device.init()?;

        // 存储设备到静态变量
        VIRTIO_BLK = Some(device);

        // 现在设备已经在静态存储中，更新 private_data 指针
        if let Some(ref mut dev) = VIRTIO_BLK {
            let device_ptr = dev as *const VirtIOBlkDevice as *mut u8;
            dev.disk.private_data = Some(device_ptr);
        }

        Ok(())
    }
}

/// 注册 PCI VirtIO 设备
///
/// # 参数
/// - `device`: PCI VirtIO 设备
pub fn register_pci_device(device: crate::drivers::virtio::virtio_pci::VirtIOPCI) {
    unsafe {
        VIRTIO_PCI_BLK = Some(device);
        // 确保设备写入对所有 CPU 可见
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // 设置就绪标志（必须在写入设备后设置）
        VIRTIO_PCI_READY.store(true, core::sync::atomic::Ordering::SeqCst);
    }
}

/// 获取 VirtIO 块设备
///
/// 优先返回 PCI VirtIO 设备，如果没有则返回 MMIO 设备
pub fn get_device() -> Option<&'static VirtIOBlkDevice> {
    unsafe {
        // 如果有 PCI 设备，通过它进行 I/O
        // 注意：目前 PCI 设备使用独立的 I/O 接口，这里返回 MMIO 设备作为后备
        VIRTIO_BLK.as_ref()
    }
}

/// 获取 PCI VirtIO 设备
pub fn get_pci_device() -> Option<&'static crate::drivers::virtio::virtio_pci::VirtIOPCI> {
    // 检查设备是否就绪
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    unsafe {
        VIRTIO_PCI_BLK.as_ref()
    }
}

/// 设置 PCI VirtIO 块设备的 VirtQueue
///
/// # 参数
/// - `queue`: 已配置的 VirtQueue
pub fn set_pci_device_queue(queue: queue::VirtQueue) {
    unsafe {
        // 存储引用而不是移动队列
        VIRTIO_PCI_BLK_QUEUE = Some(queue);
        // 初始化期望的 used.idx 为 0（新队列从 0 开始）
        VIRTIO_PCI_EXPECTED_USED_IDX = 0;
    }
}

/// 获取 PCI VirtIO 块设备的 VirtQueue（可变引用）
pub fn get_pci_device_queue_mut() -> Option<&'static mut queue::VirtQueue> {
    // 检查设备是否就绪
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    unsafe {
        VIRTIO_PCI_BLK_QUEUE.as_mut()
    }
}

/// 获取 PCI VirtIO 块设备的 VirtQueue（只读引用）
pub fn get_pci_device_queue() -> Option<&'static queue::VirtQueue> {
    // 检查设备是否就绪
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    unsafe {
        VIRTIO_PCI_BLK_QUEUE.as_ref()
    }
}

/// 获取期望的 used.idx（用于等待 I/O 完成）
pub fn get_expected_used_idx() -> u16 {
    unsafe { VIRTIO_PCI_EXPECTED_USED_IDX }
}

/// 递增期望的 used.idx（在提交请求后调用）
pub fn increment_expected_used_idx() {
    unsafe {
        VIRTIO_PCI_EXPECTED_USED_IDX = VIRTIO_PCI_EXPECTED_USED_IDX.wrapping_add(1);
    }
}

/// 注册 PCI VirtIO 设备的 GenDisk
///
/// 创建一个 GenDisk 包装器，使 ext4 驱动可以通过标准块设备接口访问 PCI VirtIO 设备
pub fn register_pci_gen_disk() {
    use alloc::boxed::Box;

    unsafe {
        // 检查 PCI 设备是否存在
        if VIRTIO_PCI_BLK.is_none() {
            crate::println!("virtio: No PCI device to register as GenDisk");
            return;
        }

        // 创建 GenDisk
        let mut disk = Box::new(GenDisk::new(
            "pci-virtblk",
            8,  // major number (arbitrary, but unique)
            1,  // minors
            512, // block size
            None as Option<&BlockDeviceOps>,
        ));

        // 读取设备容量
        if let Some(pci_dev) = VIRTIO_PCI_BLK.as_ref() {
            let device_cfg_addr = pci_dev.common_cfg_bar + 0x2000;
            let capacity_ptr = device_cfg_addr as *const u64;
            let capacity_sectors = core::ptr::read_volatile(capacity_ptr);
            disk.set_capacity(capacity_sectors as u32);
        }

        // 设置请求处理函数
        disk.set_request_fn(pci_virtio_handle_request);

        // 注册到块设备管理器
        let _ = crate::drivers::blkdev::register_disk(disk);
    }
}

/// PCI VirtIO 块设备请求处理函数
///
/// 此函数由块设备层调用，用于处理读写请求
unsafe extern "C" fn pci_virtio_handle_request(req: &mut Request) {
    use crate::drivers::blkdev::ReqCmd;

    // 检查设备是否就绪（使用 SeqCst 确保最强的内存可见性）
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::SeqCst) {
        crate::println!("virtio: ERROR - PCI device not ready");
        if let Some(end_io) = req.end_io {
            end_io(req, -6);  // ENXIO
        }
        return;
    }

    // 获取 PCI 设备
    let pci_dev = match VIRTIO_PCI_BLK.as_ref() {
        Some(dev) => dev,
        None => {
            crate::println!("virtio: ERROR - No PCI device for request");
            if let Some(end_io) = req.end_io {
                end_io(req, -6);  // ENXIO
            }
            return;
        }
    };

    // 根据命令类型执行操作
    let result = match req.cmd_type {
        ReqCmd::Read => {
            // 读取块
            pci_virtio_read_block(pci_dev, req.sector, &mut req.buffer)
        }
        ReqCmd::Write => {
            // 写入块（暂不支持）
            Err(-5)  // EIO
        }
        ReqCmd::Flush => {
            // 刷新操作（暂返回成功）
            Ok(())
        }
    };

    // 调用完成回调
    match result {
        Ok(()) => {
            if let Some(end_io) = req.end_io {
                end_io(req, 0);
            }
        }
        Err(err) => {
            if let Some(end_io) = req.end_io {
                end_io(req, err);
            }
        }
    }
}

/// 使用 PCI VirtIO 设备读取块
fn pci_virtio_read_block(
    pci_dev: &crate::drivers::virtio::virtio_pci::VirtIOPCI,
    sector: u64,
    buf: &mut [u8],
) -> Result<(), i32> {
    use virtio_pci::read_block_using_configured_queue;

    match read_block_using_configured_queue(pci_dev, sector, buf) {
        Ok(_) => Ok(()),
        Err(_) => Err(-5),  // EIO
    }
}

/// 获取 PCI VirtIO GenDisk
///
/// 从块设备管理器获取 PCI VirtIO 设备的 GenDisk
pub fn get_pci_gen_disk() -> Option<&'static GenDisk> {
    // PCI VirtIO 设备使用 major number 8
    crate::drivers::blkdev::get_disk(8).map(|ptr| unsafe { &*ptr })
}

/// PCI VirtIO-Blk 中断处理器（Modern VirtIO 1.0+）
///
/// 处理 PCI VirtIO 设备的中断
///
/// # 参数
/// - `irq`: 中断号（用于在 PLIC 上完成中断）
///
/// # 说明
/// PCI VirtIO 使用传统的 INTx 中断，通过 PCI INTx 引脚传递
/// 中断在 PLIC 层面处理，不需要读取设备特定的中断状态寄存器
pub fn interrupt_handler_pci(irq: usize) {
    crate::println!("virtio-blk: interrupt_handler_pci called (IRQ {})!", irq);
    unsafe {
        if let Some(_pci_device) = VIRTIO_PCI_BLK.as_ref() {
            crate::println!("virtio-blk: Handling PCI VirtIO interrupt (IRQ {})", irq);

            // 检查队列的 used ring 是否有更新（调试）
            if let Some(queue_guard) = VIRTIO_PCI_BLK_QUEUE.as_ref() {
                let used_idx = queue_guard.get_used();
                crate::println!("virtio-blk: used.idx = {}", used_idx);
            }

            // 在 PLIC 上完成中断（Critical: 必须完成才能接收下一个中断）
            #[cfg(feature = "riscv64")]
            {
                let hart_id = crate::arch::riscv64::smp::cpu_id();
                crate::drivers::intc::plic::complete(hart_id as usize, irq);
                crate::println!("virtio-blk: Interrupt completed at PLIC (hart={}, irq={})", hart_id, irq);
            }

            crate::println!("virtio-blk: PCI VirtIO interrupt handled");
            return;
        } else {
            crate::println!("virtio-blk: ERROR: No PCI VirtIO device found!");
        }
    }
}

/// VirtIO-Blk 中断处理器（Legacy MMIO VirtIO）
///
/// 处理 Legacy MMIO VirtIO-Blk 设备的中断
pub fn interrupt_handler() {
    crate::println!("virtio-blk: interrupt_handler called (MMIO)!");
    unsafe {
        // MMIO VirtIO 设备（Legacy VirtIO）
        if let Some(device) = VIRTIO_BLK.as_ref() {
            // 读取中断状态 (INTERRUPT_STATUS at 0x60)
            let irq_status_ptr = (device.base_addr + 0x60) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_status_ptr);

            crate::println!("virtio-blk: MMIO IRQ status = 0x{:x}", irq_status);

            if irq_status != 0 {
                crate::println!("virtio-blk: MMIO Interrupt! status=0x{:x}", irq_status);

                // 清除中断（INTERRUPT_ACK at 0x64）
                let irq_ack_ptr = (device.base_addr + 0x64) as *mut u32;
                core::ptr::write_volatile(irq_ack_ptr, irq_status);

                // 获取队列并打印状态
                if let Some(queue_guard) = device.virtqueue.try_lock() {
                    if let Some(queue) = queue_guard.as_ref() {
                        let used_idx = queue.get_used();
                        crate::println!("virtio-blk: used_idx now = {}", used_idx);
                    }
                }
            }
        } else {
            crate::println!("virtio-blk: ERROR: No VirtIO block device found!");
        }
    }
}

/// 使能 VirtIO-Blk 设备中断
///
/// # 参数
/// - `base_addr`: VirtIO 设备的 MMIO 基地址
///
/// # 说明
/// 根据 MMIO 基地址计算对应的 IRQ 号并使能
pub fn enable_device_interrupt(base_addr: u64) {
    // QEMU RISC-V virt 平台:
    // - VirtIO 设备从 0x10001000 开始
    // - 每个设备占用 0x1000 字节
    // - IRQ 从 1 开始，每个设备对应一个 IRQ
    const VIRTIO_MMIO_BASE: u64 = 0x10001000;
    const VIRTIO_MMIO_SIZE: u64 = 0x1000;

    let slot = ((base_addr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_SIZE) as u32;
    let irq = (slot + 1) as usize;  // IRQ 1-8 对应 slot 0-7

    crate::println!("virtio-blk: Enabling IRQ {} for device at 0x{:x} (slot {})", irq, base_addr, slot);

    // 使能 IRQ（在当前 boot hart 上）
    #[cfg(feature = "riscv64")]
    {
        let boot_hart = crate::arch::riscv64::smp::cpu_id();
        crate::drivers::intc::plic::enable_interrupt(boot_hart, irq);

        // 也更新设备中的 IRQ 号
        unsafe {
            if let Some(ref mut dev) = VIRTIO_BLK {
                dev.irq = irq as u32;
            }
        }
    }
}
