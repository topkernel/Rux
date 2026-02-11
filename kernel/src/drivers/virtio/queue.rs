//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 虚拟队列
//!
//! 完全遵循 VirtIO 规范的队列实现
//! 参考: VirtIO Specification v1.1, Linux vring

use core::sync::atomic::{AtomicU16, Ordering};

/// VirtIO 描述符 (16 字节对齐)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Desc {
    /// 地址（64位）
    pub addr: u64,
    /// 长度（32位）
    pub len: u32,
    /// 标志（16位）
    pub flags: u16,
    /// 下一个（16位）
    pub next: u16,
}

/// Available Ring (2 字节对齐)
#[repr(C)]
pub struct AvailRing {
    /// 标志
    pub flags: u16,
    /// 驱动写入下一个可用的描述符索引
    pub idx: AtomicU16,
    // 描述符索引数组从这里开始
    // 数组后面跟着 used_event_idx
}

/// Used Ring 元素 (4 字节对齐)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsedElem {
    /// 描述符索引
    pub id: u32,
    /// 写入的字节数
    pub len: u32,
}

/// Used Ring (4 字节对齐)
#[repr(C)]
pub struct UsedRing {
    /// 标志
    pub flags: u16,
    /// 设备写入下一个可用的描述符索引
    pub idx: AtomicU16,
    // 元素数组从这里开始
    // 数组后面跟着 avail_event_idx
}

/// VirtIO 虚拟队列
///
/// 使用 legacy VirtIO 布局：所有部分在连续内存中
pub struct VirtQueue {
    /// 队列大小
    pub queue_size: u16,
    /// 队列通知地址
    queue_notify: u64,
    /// 中断状态地址 (VIRTIO_MMIO_INTERRUPT_STATUS - Read Only)
    interrupt_status: u64,
    /// 中断应答地址 (VIRTIO_MMIO_INTERRUPT_ACK - Write Only)
    interrupt_ack: u64,
    /// 描述符表指针 (在连续内存块的开始)
    desc: *mut Desc,
    /// Available Ring 指针
    avail: *mut AvailRing,
    /// Used Ring 指针
    used: *mut UsedRing,
    /// vring 地址（用于调试）
    vring_addr: u64,
    /// 下一个要分配的描述符索引
    next_desc: AtomicU16,
}

unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

impl VirtQueue {
    /// 创建新的 VirtQueue（使用连续内存布局）
    ///
    /// # 参数
    /// - `queue_size`: 队列大小（必须是 2 的幂）
    /// - `queue_notify`: 队列通知寄存器地址 (VIRTIO_MMIO_QUEUE_NOTIFY = 0x050)
    /// - `interrupt_status`: 中断状态寄存器地址 (VIRTIO_MMIO_INTERRUPT_STATUS = 0x060)
    /// - `interrupt_ack`: 中断应答寄存器地址 (VIRTIO_MMIO_INTERRUPT_ACK = 0x064)
    pub fn new(queue_size: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64) -> Option<Self> {
        // 计算需要的内存大小
        // Desc: queue_size * 16 字节
        // Avail: 2 + 2 + queue_size * 2 + 2 (flags + idx + ring[] + used_event)
        // Used: 2 + 2 + queue_size * 8 + 2 (flags + idx + ring[] + avail_event)
        // Padding: 对齐到 4 字节边界

        let desc_size = queue_size as usize * 16;
        let avail_size = 2 + 2 + queue_size as usize * 2 + 2;
        let used_size = 2 + 2 + queue_size as usize * 8 + 2;

        // Avail 之后需要对齐到 4 字节边界
        let avail_size_aligned = (avail_size + 3) & !3;

        let total_size = desc_size + avail_size_aligned + used_size;

        // VirtIO Legacy 要求：整个 vring 必须在页对齐的连续内存中
        // 使用页面大小 (4096 字节) 对齐
        const PAGE_SIZE: usize = 4096;

        // 分配页对齐的连续内存
        let layout = alloc::alloc::Layout::from_size_align(total_size, PAGE_SIZE).ok()?;
        let mem_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
        if mem_ptr.is_null() {
            return None;
        }

        // 验证内存对齐
        let addr = mem_ptr as usize;
        if addr & (PAGE_SIZE - 1) != 0 {
            crate::println!("virtio-blk: ERROR: vring not page-aligned! addr=0x{:x}", addr);
            unsafe { alloc::alloc::dealloc(mem_ptr, layout) };
            return None;
        }

        // 设置各部分指针
        let desc = mem_ptr as *mut Desc;
        let avail = unsafe { (mem_ptr as usize + desc_size) as *mut AvailRing };
        let used = unsafe { (mem_ptr as usize + desc_size + avail_size_aligned) as *mut UsedRing };

        // 初始化 Available Ring
        unsafe {
            (*avail).flags = 0;
            (*avail).idx = AtomicU16::new(0);
        }

        // 初始化 Used Ring
        unsafe {
            (*used).flags = 0;
            (*used).idx = AtomicU16::new(0);
        }

        // 初始化描述符表
        for i in 0..queue_size {
            unsafe {
                *desc.add(i as usize) = Desc {
                    addr: 0,
                    len: 0,
                    flags: 0,
                    next: 0,
                };
            }
        }

        // 打印 vring 布局以验证对齐
        crate::println!("virtio-blk: vring allocation details:");
        crate::println!("  mem_ptr     : 0x{:x}", mem_ptr as u64);
        crate::println!("  page_aligned : {} (addr % 4096 == 0)", (addr as u64) % 4096 == 0);
        crate::println!("  desc offset  : 0 (0x{:x})", desc as u64);
        crate::println!("  avail offset : 0x{:x} (desc_size + aligned_avail = {})",
            avail as u64 - mem_ptr as u64, desc_size + avail_size_aligned);
        crate::println!("  used offset  : 0x{:x} (desc_size + aligned_avail + used_size = {})",
            used as u64 - mem_ptr as u64, desc_size + avail_size_aligned + used_size);

        Some(Self {
            queue_size,
            queue_notify,
            interrupt_status,
            interrupt_ack,
            desc,
            avail,
            used,
            vring_addr: mem_ptr as u64,
            next_desc: AtomicU16::new(0),
        })
    }

    /// 获取当前可用索引
    pub fn get_avail(&self) -> u16 {
        unsafe { (*self.avail).idx.load(Ordering::Acquire) }
    }

    /// 获取当前已用索引
    pub fn get_used(&self) -> u16 {
        unsafe { (*self.used).idx.load(Ordering::Acquire) }
    }

    /// 通知设备有新的请求
    pub fn notify(&self) {
        // 内存屏障：确保所有队列更新对设备可见
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        unsafe {
            let queue_notify = self.queue_notify as *mut u32;
            // 写入队列索引（对于第一个队列是 0）
            core::ptr::write_volatile(queue_notify, 0);
        }
    }

    /// 等待设备完成请求
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        let mut timeout = 100000;
        let mut last_irq = 0u32;
        let mut iterations = 0u32;
        loop {
            // 使用 Acquire 内存序确保看到设备的所有更新
            let used = unsafe { (*self.used).idx.load(Ordering::Acquire) };
            if used != prev_used {
                return used;
            }

            // 读取中断状态（VIRTIO_MMIO_INTERRUPT_STATUS at 0x060 - Read Only）
            unsafe {
                let irq_status_ptr = self.interrupt_status as *const u32;
                let irq_status = core::ptr::read_volatile(irq_status_ptr);
                if irq_status != 0 {
                    if irq_status != last_irq {
                        crate::println!("virtio-blk: IRQ status changed: 0x{:x}", irq_status);
                        last_irq = irq_status;
                    }
                    // 清除中断（VIRTIO_MMIO_INTERRUPT_ACK at 0x064 - Write Only）
                    let irq_ack_ptr = self.interrupt_ack as *mut u32;
                    core::ptr::write_volatile(irq_ack_ptr, irq_status);
                }
            }

            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
            timeout -= 1;
            iterations += 1;

            // 每 10000 次迭代打印一次状态
            if iterations % 10000 == 0 {
                crate::println!("virtio-blk: Still waiting... iterations={}, used.idx={}, avail.idx={}",
                    iterations, used, unsafe { (*self.avail).idx.load(Ordering::Acquire) });
            }

            if timeout == 0 {
                crate::println!("virtio-blk: wait_for_completion timeout! prev_used={}, used={}", prev_used, used);
                // 打印调试信息
                crate::println!("virtio-blk: Device state:");
                crate::println!("  avail.idx = {}", unsafe { (*self.avail).idx.load(Ordering::Acquire) });
                crate::println!("  used.idx = {}", used);
                crate::println!("  last IRQ status = 0x{:x}", last_irq);

                // 尝试读取 used ring 内容
                unsafe {
                    let used_ring_ptr = (self.used as usize + 4) as *const UsedElem;
                    let used_elem = core::ptr::read_volatile(used_ring_ptr);
                    crate::println!("  used[0].id = {}, used[0].len = {}", used_elem.id, used_elem.len);
                }

                return used;
            }
        }
    }

    /// 添加描述符链到队列并通知设备
    pub fn submit(&mut self, head_idx: u16) {
        unsafe {
            let avail = &mut *self.avail;
            let idx = avail.idx.load(Ordering::Acquire) as usize;

            crate::println!("virtio-blk: submit: head_idx={}, avail_idx={}", head_idx, idx);

            // 内存屏障：确保所有描述符更新对设备可见
            core::sync::atomic::fence(Ordering::Release);

            // 获取 ring 数组的指针（在 avail 结构体中的偏移）
            // AvailRing 结构: flags(2) + idx(2) = 4 字节偏移
            let ring_ptr = (self.avail as usize + 4) as *mut u16;

            // 写入描述符索引到 ring
            core::ptr::write_volatile(
                ring_ptr.add(idx % self.queue_size as usize),
                head_idx,
            );

            // 更新索引
            core::sync::atomic::fence(Ordering::Release);
            avail.idx.store((idx as u16) + 1, Ordering::Release);

            crate::println!("virtio-blk: submit: avail.idx updated to {}", (idx as u16) + 1);
        }
    }

    /// 获取描述符
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        if idx < self.queue_size {
            unsafe { Some(*self.desc.add(idx as usize)) }
        } else {
            None
        }
    }

    /// 分配新的描述符（自动清除旧数据）
    pub fn alloc_desc(&mut self) -> Option<u16> {
        let idx = self.next_desc.fetch_add(1, Ordering::AcqRel);
        if idx < self.queue_size {
            // 清除描述符中的旧数据（避免 stale descriptor 导致设备误读）
            // QEMU "Incorrect order for descriptors" 错误的原因：
            //   旧 I/O 的描述符数据（addr=0x0, len=0）被重用
            //   设备处理：Desc[0] → Desc[1](@0x0) → Desc[2]
            //   但 Desc[1] 应该指向有效数据！
            // 解决：分配描述符时清除 addr 和 len
            unsafe {
                let desc = self.desc.add(idx as usize);
                (*desc).addr = 0;
                (*desc).len = 0;
                (*desc).flags = 0;
                (*desc).next = 0;
            }
            Some(idx)
        } else {
            None
        }
    }

    /// 设置描述符内容
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if idx < self.queue_size {
            unsafe {
                *self.desc.add(idx as usize) = Desc { addr, len, flags, next };
            }
            // 确保描述符写入对设备可见
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        }
    }

    /// 获取描述符表地址（用于初始化时告诉设备）
    pub fn get_desc_addr(&self) -> u64 {
        self.desc as u64
    }

    /// 获取 Available Ring 地址
    pub fn get_avail_addr(&self) -> u64 {
        self.avail as u64
    }

    /// 获取 Used Ring 地址
    pub fn get_used_addr(&self) -> u64 {
        self.used as u64
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkReqHeader {
    /// 请求类型（0=读, 1=写, 2=刷新）
    pub type_: u32,
    /// 保留
    pub reserved: u32,
    /// 扇区号
    pub sector: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkResp {
    /// 状态（0=OK, 1=IOERR, 2=UNSUPPORTED）
    pub status: u8,
}

pub mod req_type {
    /// 读
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    /// 写
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    /// 刷新
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

pub mod status {
    /// OK
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    /// IOERR
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    /// UNSUPP
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}
