//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 虚拟队列
//!
//! 完全遵循 VirtIO 规范的队列实现

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
    /// 驱动写入下一个可用的描述符索引（使用 volatile 读写）
    pub idx: u16,
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
    /// 设备写入下一个可用的描述符索引（使用 volatile 读写）
    pub idx: u16,
    // 元素数组从这里开始
    // 数组后面跟着 avail_event_idx
}

/// VirtIO 虚拟队列
///
/// 使用 Modern VirtIO (v1.0+) 布局
pub struct VirtQueue {
    /// 队列大小
    pub queue_size: u16,
    /// 队列索引（用于通知设备）
    queue_index: u16,
    /// 队列通知地址
    queue_notify: u64,
    /// 中断状态地址 (VIRTIO_MMIO_INTERRUPT_STATUS - Read Only)
    interrupt_status: u64,
    /// 中断应答地址 (VIRTIO_MMIO_INTERRUPT_ACK - Write Only)
    interrupt_ack: u64,
    /// 描述符表指针 (在连续内存块的开始)
    pub(crate) desc: *mut Desc,
    /// Available Ring 指针
    pub(crate) avail: *mut AvailRing,
    /// Used Ring 指针
    pub(crate) used: *mut UsedRing,
    /// vring 地址
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
    /// - `queue_index`: 队列索引（用于通知设备时写入）
    /// - `queue_notify`: 队列通知寄存器地址
    /// - `interrupt_status`: 中断状态寄存器地址
    /// - `interrupt_ack`: 中断应答寄存器地址
    pub fn new(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64) -> Option<Self> {
        let desc_size = queue_size as usize * 16;
        let avail_size = 2 + 2 + queue_size as usize * 2 + 2;
        let used_size = 2 + 2 + queue_size as usize * 8 + 2;

        // VirtIO 1.0 规范要求：描述符表、可用环和已用环都必须页对齐（至少 4096 字节）
        const PAGE_SIZE: usize = 4096;

        let desc_size_aligned = (desc_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let avail_size_aligned = (avail_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let used_size_aligned = (used_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let total_size = desc_size_aligned + avail_size_aligned + used_size_aligned;

        let layout = alloc::alloc::Layout::from_size_align(total_size, PAGE_SIZE).ok()?;
        let mem_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
        if mem_ptr.is_null() {
            return None;
        }

        let addr = mem_ptr as usize;
        if addr & (PAGE_SIZE - 1) != 0 {
            crate::println!("virtio: ERROR: vring not page-aligned!");
            unsafe { alloc::alloc::dealloc(mem_ptr, layout) };
            return None;
        }

        let desc = mem_ptr as *mut Desc;
        let avail = unsafe { (mem_ptr as usize + desc_size_aligned) as *mut AvailRing };
        let used = unsafe { (mem_ptr as usize + desc_size_aligned + avail_size_aligned) as *mut UsedRing };

        unsafe {
            (*avail).flags = 0;
            (*avail).idx = 0;
            (*used).flags = 0;
            (*used).idx = 0;
        }

        for i in 0..queue_size {
            unsafe {
                *desc.add(i as usize) = Desc { addr: 0, len: 0, flags: 0, next: 0 };
            }
        }

        Some(Self {
            queue_size,
            queue_index,
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
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) }
    }

    /// 获取当前已用索引
    pub fn get_used(&self) -> u16 {
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.used).idx)) }
    }

    /// 通知设备有新的请求
    pub fn notify(&self) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        unsafe {
            let queue_notify = self.queue_notify as *mut u16;
            core::ptr::write_volatile(queue_notify, self.queue_index);
        }
    }

    /// 等待设备完成请求
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        let mut timeout = 10_000_000;

        if self.used.is_null() {
            return prev_used;
        }

        loop {
            // 使用内存屏障确保读取顺序
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

            let used_idx = unsafe {
                let used_idx_ptr = (self.used as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };

            if used_idx != prev_used {
                return used_idx;
            }

            core::hint::spin_loop();

            timeout -= 1;
            if timeout == 0 {
                crate::println!("virtio: I/O timeout (prev={}, idx={})", prev_used, used_idx);
                return used_idx;
            }
        }
    }

    /// 添加描述符链到队列并通知设备
    pub fn submit(&mut self, head_idx: u16) {
        unsafe {
            let avail = &mut *self.avail;
            let idx = core::ptr::read_volatile(core::ptr::addr_of!(avail.idx)) as usize;

            core::sync::atomic::fence(Ordering::Release);

            let ring_ptr = (self.avail as usize + 4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(idx % self.queue_size as usize), head_idx);

            let new_idx = (idx as u16) + 1;
            core::sync::atomic::fence(Ordering::Release);
            core::ptr::write_volatile(&mut (*avail).idx as *mut u16, new_idx);
            core::sync::atomic::fence(Ordering::SeqCst);

            Self::notify(self);

            // 延迟：给 QEMU VirtIO 设备时间处理通知
            // 注意：这个延迟是必要的，因为 QEMU 需要时间来响应 MMIO 写入
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
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

    /// 分配新的描述符
    pub fn alloc_desc(&mut self) -> Option<u16> {
        let idx = self.next_desc.fetch_add(1, Ordering::AcqRel);
        if idx < self.queue_size {
            Some(idx)
        } else {
            None
        }
    }

    /// 重置描述符分配器
    ///
    /// 在开始新的 I/O 操作前调用，以便重用描述符
    /// 注意：这假设没有并发 I/O 操作
    pub fn reset_desc_allocator(&mut self) {
        self.next_desc.store(0, Ordering::Release);
    }

    /// 设置描述符内容
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if idx < self.queue_size {
            unsafe {
                *self.desc.add(idx as usize) = Desc { addr, len, flags, next };
            }
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        }
    }

    /// 获取描述符表地址
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

    /// 获取 vring 基地址
    pub fn get_vring_addr(&self) -> u64 {
        self.vring_addr
    }

    /// 获取队列通知地址
    pub fn get_notify_addr(&self) -> u64 {
        self.queue_notify
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

impl core::fmt::Display for VirtIOBlkResp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.status {
            0 => write!(f, "OK"),
            1 => write!(f, "IOERR"),
            2 => write!(f, "UNSUPPORTED"),
            _ => write!(f, "UNKNOWN({})", self.status),
        }
    }
}

pub mod req_type {
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

pub mod status {
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}
