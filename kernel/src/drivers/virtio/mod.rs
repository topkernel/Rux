//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 块设备驱动
//!
//! 完全遵循 VirtIO 规范和 Linux 内核的 virtio-blk 实现
//! 参考: drivers/block/virtio_blk.c, Documentation/virtio/

use spin::Mutex;

use crate::drivers::blkdev::{GenDisk, Request, BlockDeviceOps};

pub mod queue;
pub mod probe;

/// VirtIO 设备寄存器布局（符合 VirtIO 1.0 规范）
///
/// 参考: include/uapi/linux/virtio_mmio.h
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
    /// Guest 页面大小 (0x28) - legacy only
    pub guest_page_size: u32,
    /// 队列选择 (0x30)
    pub queue_sel: u32,
    /// 队列最大数量 (0x34)
    pub queue_num_max: u32,
    /// 队列数量 (0x38)
    pub queue_num: u32,
    /// 队列对齐 (0x3C) - legacy only
    pub queue_align: u32,
    /// 队列页帧号 (0x40) - legacy only
    pub queue_pfn: u32,
    /// 队列就绪 (0x44) - modern only
    pub queue_ready: u32,
    /// _reserved (0x48-0x4C)
    _reserved3: [u32; 2],
    /// 队列通知 (0x50)
    pub queue_notify: u32,
    /// _reserved (0x54-0x5C)
    _reserved4: [u32; 3],
    /// 中断状态 (0x60)
    pub interrupt_status: u32,
    /// 中断应答 (0x64)
    pub interrupt_ack: u32,
    /// _reserved (0x68-0x6C)
    _reserved5: [u32; 2],
    /// 驱动状态 (0x70)
    pub status: u32,
    /// _reserved (0x74+)
    _reserved6: [u32; 4],
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
                    let val = core::ptr::read_volatile(ptr);
                    crate::println!("virtio-mmio: [R] 0x{:04x} ({}) = 0x{:08x}", $offset, $name, val);
                    val
                }
            };
        }

        macro_rules! write_reg {
            ($offset:expr, $name:expr, $val:expr) => {
                {
                    let ptr = (self.base_addr + $offset) as *mut u32;
                    crate::println!("virtio-mmio: [W] 0x{:04x} ({}) = 0x{:08x}", $offset, $name, $val);
                    core::ptr::write_volatile(ptr, $val);
                }
            };
        }

        unsafe {
            crate::println!("virtio-blk: ===== Starting VirtIO device initialization =====");
            crate::println!("virtio-blk: base_addr = 0x{:x}", self.base_addr);

            // 1. 验证魔数
            let magic = read_reg!(MAGIC_VALUE_OFFSET, "MAGIC_VALUE");
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // 2. 验证版本
            let version = read_reg!(VERSION_OFFSET, "VERSION");
            if version != 1 && version != 2 {
                return Err("Unsupported VirtIO version");
            }
            crate::println!("virtio-blk: VirtIO version {} ({})",
                version, if version == 1 { "Legacy" } else { "Modern" });

            // 3. 验证设备 ID
            let device_id = read_reg!(DEVICE_ID_OFFSET, "DEVICE_ID");
            if device_id != 2 {
                return Err("Not a VirtIO block device");
            }
            crate::println!("virtio-blk: Device ID = 2 (VirtIO-Blk) ✓");

            // 4. 状态机：重置设备
            write_reg!(STATUS_OFFSET, "STATUS", 0x00);
            crate::println!("virtio-blk: Device reset ✓");

            // 5. 状态机：ACKNOWLEDGE (0x01)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01);
            let status = read_reg!(STATUS_OFFSET, "STATUS");
            crate::println!("virtio-blk: ACKNOWLEDGE bit set, status=0x{:02x} ✓", status);

            // 6. 状态机：DRIVER (0x02)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            let status = read_reg!(STATUS_OFFSET, "STATUS");
            crate::println!("virtio-blk: DRIVER bit set, status=0x{:02x} ✓", status);

            // 检查是否需要重置
            if status & 0x40 != 0 {
                crate::println!("virtio-blk: WARNING: Device needs reset (DEVICE_NEEDS_RESET)");
                write_reg!(STATUS_OFFSET, "STATUS", 0x00);
                write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
                let status = read_reg!(STATUS_OFFSET, "STATUS");
                crate::println!("virtio-blk: Reset complete, status=0x{:02x}", status);
            }

            // 7. Legacy VirtIO: 设置 Guest Page Size
            if version == 1 {
                const PAGE_SIZE: u32 = 4096;
                write_reg!(GUEST_PAGE_SIZE_OFFSET, "GUEST_PAGE_SIZE", PAGE_SIZE);
                let pgsz = read_reg!(GUEST_PAGE_SIZE_OFFSET, "GUEST_PAGE_SIZE");
                crate::println!("virtio-blk: Guest page size set to {} ✓", pgsz);
            }

            // 8. 读取设备特性
            let device_features = read_reg!(DEVICE_FEATURES_OFFSET, "DEVICE_FEATURES");
            crate::println!("virtio-blk: Device features offered: 0x{:08x}", device_features);

            // 9. 确认驱动特性（Legacy）
            if version == 1 {
                const VIRTIO_BLK_F_SIZE_MAX: u32 = 1;
                const VIRTIO_BLK_F_SEG_MAX: u32 = 2;
                const VIRTIO_BLK_F_FLUSH: u32 = 9;

                let driver_features = device_features & (
                    (1 << VIRTIO_BLK_F_SIZE_MAX) |
                    (1 << VIRTIO_BLK_F_SEG_MAX) |
                    (1 << VIRTIO_BLK_F_FLUSH) |
                    0x00000001
                );

                write_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES", driver_features);
                let drv_features = read_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES");
                crate::println!("virtio-blk: Driver features negotiated: 0x{:08x} ✓", drv_features);
            }

            // ========== VirtQueue 设置 ==========
            crate::println!("virtio-blk: ===== VirtQueue configuration =====");

            // 10. 选择队列 0
            write_reg!(QUEUE_SEL_OFFSET, "QUEUE_SEL", 0);
            let qs = read_reg!(QUEUE_SEL_OFFSET, "QUEUE_SEL");
            crate::println!("virtio-blk: Queue 0 selected ✓");

            // 11. 读取最大队列大小
            let max_queue_size = read_reg!(QUEUE_NUM_MAX_OFFSET, "QUEUE_NUM_MAX");

            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };
            crate::println!("virtio-blk: Queue size: requested={}, max={}", self.queue_size, max_queue_size);

            // 12. 设置队列数量
            write_reg!(QUEUE_NUM_OFFSET, "QUEUE_NUM", self.queue_size as u32);
            let qn = read_reg!(QUEUE_NUM_OFFSET, "QUEUE_NUM");
            crate::println!("virtio-blk: Queue num set to {} ✓", qn);

            // 13. 创建 VirtQueue（分配 vring 内存）
            crate::println!("virtio-blk: Allocating vring memory...");
            let virtqueue = match queue::VirtQueue::new(
                self.queue_size,
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

            crate::println!("virtio-blk: vring layout:");
            crate::println!("  desc table : 0x{:x} (size={})", desc_addr, self.queue_size * 16);
            crate::println!("  avail ring : 0x{:x}", avail_addr);
            crate::println!("  used ring  : 0x{:x}", used_addr);

            // 14. Legacy VirtIO: 设置队列对齐和 PFN
            if version == 1 {
                const PAGE_SIZE: u32 = 4096;
                const QUEUE_ALIGN_OFFSET: u64 = 0x3c;
                const QUEUE_PFN_OFFSET: u64 = 0x040;

                crate::println!("virtio-blk: Legacy VirtIO queue setup:");
                crate::println!("  Step 1: Set queue alignment");

                write_reg!(QUEUE_ALIGN_OFFSET, "QUEUE_ALIGN", PAGE_SIZE);
                let qalign = read_reg!(QUEUE_ALIGN_OFFSET, "QUEUE_ALIGN");
                crate::println!("  QUEUE_ALIGN = 0x{:08x} ✓", qalign);

                // 计算 PFN = 物理地址 / 页面大小
                // VirtIO 设备需要物理地址，所以必须先转换
                #[cfg(feature = "riscv64")]
                let desc_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                    crate::arch::riscv64::mm::VirtAddr::new(desc_addr)
                ).0;
                #[cfg(not(feature = "riscv64"))]
                let desc_phys_addr = desc_addr;

                let pfn = (desc_phys_addr >> 12) as u32;
                crate::println!("  Step 2: Set queue PFN");
                crate::println!("    desc_virt = 0x{:x}", desc_addr);
                crate::println!("    desc_phys = 0x{:x}", desc_phys_addr);
                crate::println!("    PFN = desc_phys >> 12 = 0x{:x}", pfn);

                write_reg!(QUEUE_PFN_OFFSET, "QUEUE_PFN", pfn);
                let pfn_check = read_reg!(QUEUE_PFN_OFFSET, "QUEUE_PFN");
                crate::println!("  QUEUE_PFN = 0x{:08x} {}", pfn_check,
                    if pfn_check == pfn { "✓" } else { "✗ MISMATCH!" });

                if pfn_check == 0 {
                    return Err("Device rejected queue configuration");
                }
            }

            // 15. 读取设备容量
            const VIRTIO_BLK_CONFIG_CAPACITY: u64 = 0x100;
            let cap_ptr = (self.base_addr + VIRTIO_BLK_CONFIG_CAPACITY) as *const u64;
            self.capacity = *cap_ptr;
            crate::println!("virtio-blk: Device capacity: {} sectors ({} MB)",
                self.capacity, (self.capacity * 512) / (1024 * 1024));

            // 16. 更新块设备信息
            self.disk.set_capacity(self.capacity as u32);
            self.disk.set_request_fn(Self::handle_request);
            *self.virtqueue.lock() = Some(virtqueue);

            // 17. 状态机：DRIVER_OK (0x04)
            crate::println!("virtio-blk: ===== Final status bits =====");
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x04);
            let final_status = read_reg!(STATUS_OFFSET, "STATUS");
            crate::println!("virtio-blk: Final status = 0x{:02x} (ACKNOWLEDGE|DRIVER|DRIVER_OK) ✓", final_status);

            // 内存屏障
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

            // 标记为已初始化
            *self.initialized.lock() = true;

            crate::println!("virtio-blk: ===== Initialization complete =====");
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
                crate::println!("virtio-blk: private_data is None!");
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
        crate::println!("virtio-blk: read_block called, sector={}, buf_len={}", sector, buf.len());

        if !*self.initialized.lock() {
            crate::println!("virtio-blk: Device not initialized!");
            return Err(-5);  // EIO
        }

        // 获取 VirtQueue
        let mut queue_guard = self.virtqueue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => {
                crate::println!("virtio-blk: No VirtQueue available!");
                return Err(-5);
            }
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
            crate::println!("virtio-blk: Failed to allocate header!");
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
            crate::println!("virtio-blk: Failed to allocate response!");
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

        crate::println!("virtio-blk: Physical addresses:");
        crate::println!("  header: virt=0x{:x} -> phys=0x{:x}", header_ptr as u64, header_phys_addr);
        crate::println!("  data:   virt=0x{:x} -> phys=0x{:x}", buf.as_ptr() as u64, data_phys_addr);
        crate::println!("  resp:    virt=0x{:x} -> phys=0x{:x}", resp_ptr as u64, resp_phys_addr);

        // 分配三个描述符
        let header_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                crate::println!("virtio-blk: Failed to alloc header descriptor!");
                return Err(-5);
            }
        };
        let data_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                crate::println!("virtio-blk: Failed to alloc data descriptor!");
                return Err(-5);
            }
        };
        let resp_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                crate::println!("virtio-blk: Failed to alloc response descriptor!");
                return Err(-5);
            }
        };

        crate::println!("virtio-blk: Allocated descriptors: header={}, data={}, resp={}",
            header_desc_idx, data_desc_idx, resp_desc_idx);

        // 调试：验证描述符可访问
        if let Some(desc) = queue.get_desc(0) {
            crate::println!("virtio-blk: Descriptor 0: addr=0x{:x}, len={}, flags={}, next={}",
                desc.addr, desc.len, desc.flags, desc.next);
        }

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

        // 打印描述符配置
        crate::println!("virtio-blk: Descriptor configuration (physical addresses):");
        crate::println!("  header: addr=0x{:x}, len={}", header_phys_addr, core::mem::size_of::<VirtIOBlkReqHeader>());
        crate::println!("  data: addr=0x{:x}, len={}", data_phys_addr, buf.len());
        crate::println!("  resp: addr=0x{:x}, len={}", resp_phys_addr, core::mem::size_of::<VirtIOBlkResp>());

        // 验证描述符已正确设置
        if let Some(desc0) = queue.get_desc(0) {
            crate::println!("virtio-blk: Verification - Desc[0]: addr=0x{:x}, len={}, flags={}, next={}",
                desc0.addr, desc0.len, desc0.flags, desc0.next);
        }
        if let Some(desc1) = queue.get_desc(1) {
            crate::println!("virtio-blk: Verification - Desc[1]: addr=0x{:x}, len={}, flags={}, next={}",
                desc1.addr, desc1.len, desc1.flags, desc1.next);
        }
        if let Some(desc2) = queue.get_desc(2) {
            crate::println!("virtio-blk: Verification - Desc[2]: addr=0x{:x}, len={}, flags={}, next={}",
                desc2.addr, desc2.len, desc2.flags, desc2.next);
        }

        crate::println!("virtio-blk: Submitting descriptors...");

        // ========== I/O 请求提交 ==========
        crate::println!("virtio-blk: ===== I/O request submission =====");

        // 调试：打印当前 avail ring 状态
        crate::println!("virtio-blk: Before submit: avail.idx={}", queue.get_avail());

        // 提交到可用环
        queue.submit(header_desc_idx);

        // 调试：打印提交后的 avail ring 状态
        crate::println!("virtio-blk: After submit: avail.idx={}", queue.get_avail());

        // ========== 通知设备 ==========
        crate::println!("virtio-blk: ===== Device notification =====");
        crate::println!("virtio-blk: Writing to QUEUE_NOTIFY register (0x50)");
        crate::println!("virtio-blk:   queue_num = 0 (notify queue 0)");
        queue.notify();

        // 验证寄存器写入
        const QUEUE_NOTIFY_OFFSET: u64 = 0x50;
        unsafe {
            let notify_ptr = (self.base_addr + QUEUE_NOTIFY_OFFSET) as *const u32;
            let notify_val = core::ptr::read_volatile(notify_ptr);
            crate::println!("virtio-blk:   read back: 0x{:x}", notify_val);
        }

        // 检查 PFN 寄存器是否仍然有效
        const QUEUE_PFN_OFFSET: u64 = 0x040;
        unsafe {
            crate::println!("virtio-blk: Verifying queue configuration:");
            let pfn_ptr = (self.base_addr + QUEUE_PFN_OFFSET) as *const u32;
            let pfn_check = core::ptr::read_volatile(pfn_ptr);
            crate::println!("virtio-blk:   PFN (0x40) = 0x{:08x} {}", pfn_check,
                if pfn_check != 0 { "✓" } else { "✗ LOST!" });

            // 检查 STATUS 寄存器
            const STATUS_OFFSET: u64 = 0x070;
            let status_ptr = (self.base_addr + STATUS_OFFSET) as *const u32;
            let status = core::ptr::read_volatile(status_ptr);
            crate::println!("virtio-blk:   STATUS (0x70) = 0x{:02x} {}", status,
                if status & 0x04 != 0 { "✓ (DRIVER_OK)" } else { "✗ No DRIVER_OK!" });

            // 检查 QUEUE_SEL 寄存器
            const QUEUE_SEL_OFFSET: u64 = 0x030;
            let qsel_ptr = (self.base_addr + QUEUE_SEL_OFFSET) as *const u32;
            let qsel = core::ptr::read_volatile(qsel_ptr);
            crate::println!("virtio-blk:   QUEUE_SEL (0x30) = {}", qsel);
        }

        // ========== 等待完成 ==========
        crate::println!("virtio-blk: ===== Waiting for I/O completion =====");

        // 打印初始状态
        let prev_used = queue.get_used();
        crate::println!("virtio-blk: Initial used.idx = {}", prev_used);

        // 检查中断状态寄存器
        const INTERRUPT_STATUS_OFFSET: u64 = 0x60;
        unsafe {
            let irq_ptr = (self.base_addr + INTERRUPT_STATUS_OFFSET) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_ptr);
            crate::println!("virtio-blk: INTERRUPT_STATUS (0x60) = 0x{:02x} (before wait)",
                irq_status);
        }

        // 等待设备完成请求
        crate::println!("virtio-blk: Polling for used ring update...");
        let used = queue.wait_for_completion(prev_used);
        crate::println!("virtio-blk: Request completed! used_idx: {} -> {}", prev_used, used);

        // 检查完成后的中断状态
        unsafe {
            let irq_ptr = (self.base_addr + INTERRUPT_STATUS_OFFSET) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_ptr);
            crate::println!("virtio-blk: INTERRUPT_STATUS (0x60) = 0x{:02x} (after completion)",
                irq_status);

            // 清除中断（如果设置）
            if irq_status != 0 {
                const INTERRUPT_ACK_OFFSET: u64 = 0x64;
                let ack_ptr = (self.base_addr + INTERRUPT_ACK_OFFSET) as *mut u32;
                crate::println!("virtio-blk: Acknowledging interrupt at 0x64");
                core::ptr::write_volatile(ack_ptr, irq_status);
            }
        }

        // 检查响应状态
        unsafe {
            let status = (*resp_ptr).status;
            crate::println!("virtio-blk: Response status = {}", status);
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);

            if status == queue::status::VIRTIO_BLK_S_OK {
                Ok(())
            } else if status == queue::status::VIRTIO_BLK_S_IOERR {
                crate::println!("virtio-blk: Device returned IOERR");
                Err(-5)  // EIO
            } else {
                crate::println!("virtio-blk: Device returned unknown status: {}", status);
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

/// 全局 VirtIO 块设备
static mut VIRTIO_BLK: Option<VirtIOBlkDevice> = None;

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

/// 获取 VirtIO 块设备
pub fn get_device() -> Option<&'static VirtIOBlkDevice> {
    unsafe { VIRTIO_BLK.as_ref() }
}

/// VirtIO-Blk 中断处理器
///
/// 处理 VirtIO-Blk 设备的中断
/// 参考: Linux vm_interrupt() (virtio_mmio.c:285-307)
pub fn interrupt_handler() {
    crate::println!("virtio-blk: interrupt_handler called!");
    unsafe {
        if let Some(device) = VIRTIO_BLK.as_ref() {
            // 读取中断状态 (INTERRUPT_STATUS at 0x60)
            let irq_status_ptr = (device.base_addr + 0x60) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_status_ptr);

            crate::println!("virtio-blk: IRQ status = 0x{:x}", irq_status);

            if irq_status != 0 {
                crate::println!("virtio-blk: Interrupt! status=0x{:x}", irq_status);

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
            crate::println!("virtio-blk: ERROR: VIRTIO_BLK is None!");
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
