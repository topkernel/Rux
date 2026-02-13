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

        // VirtIO 要求：整个 vring 在连续内存中（支持 Modern VirtIO v1.0+）
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
            (*avail).flags = 1;  // 轮询模式（VIRTQ_AVAIL_F_NO_INTERRUPT = 1）
            (*avail).idx = 0;
        }

        // 初始化 Used Ring
        unsafe {
            (*used).flags = 0;
            (*used).idx = 0;
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
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) }
    }

    /// 获取当前已用索引
    pub fn get_used(&self) -> u16 {
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.used).idx)) }
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
        let mut iterations = 0u32;

        // 调试：检查指针有效性
        crate::println!("virtio-blk: wait_for_completion: self.used=0x{:x}, self.avail=0x{:x}",
            self.used as usize, self.avail as usize);

        if self.used.is_null() {
            crate::println!("virtio-blk: ERROR: used pointer is NULL!");
            return prev_used;
        }

        // 检查是否使用轮询模式
        let poll_mode = unsafe { (*self.avail).flags & 1 == 1 };
        crate::println!("virtio-blk: poll_mode = {}", poll_mode);

        loop {
            // 直接从 UsedRing 结构读取 used.idx（偏移 2）
            let used_idx = unsafe {
                let used_idx_ptr = (self.used as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };

            if used_idx != prev_used {
                crate::println!("virtio-blk: used.idx changed: {} -> {}", prev_used, used_idx);
                return used_idx;
            }

            // 在轮询模式下，使用 CPU pause 而不是 WFI
            if poll_mode {
                unsafe {
                    // 使用轻量级的 pause 指令（如果支持）
                    // 或者直接继续循环
                    core::arch::asm!("nop", options(nomem, nostack));
                }
            } else {
                // 中断模式：等待中断
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }

            timeout -= 1;
            iterations += 1;

            // 每 10000 次迭代打印一次状态
            if iterations % 10000 == 0 {
                crate::println!("virtio-blk: Still waiting... iterations={}, used.idx={}, avail.idx={}",
                    iterations, used_idx, unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) });
            }

            if timeout == 0 {
                crate::println!("virtio-blk: wait_for_completion timeout! prev_used={}, used_idx={}", prev_used, used_idx);
                // 打印调试信息
                crate::println!("virtio-blk: Device state:");
                crate::println!("  avail.idx = {}", unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) });
                crate::println!("  used.idx = {}", used_idx);

                // 尝试读取 used ring 内容
                unsafe {
                    let used_ring_ptr = (self.used as usize + 4) as *const UsedElem;
                    let used_elem = core::ptr::read_volatile(used_ring_ptr);
                    crate::println!("  used[0].id = {}, used[0].len = {}", used_elem.id, used_elem.len);
                }

                return used_idx;
            }
        }
    }

    /// 添加描述符链到队列并通知设备
    pub fn submit(&mut self, head_idx: u16) {
        unsafe {
            let avail = &mut *self.avail;
            let idx = core::ptr::read_volatile(core::ptr::addr_of!(avail.idx)) as usize;

            // 调试：检查 avail flags 字段（控制中断行为）
            let flags = core::ptr::read_volatile(core::ptr::addr_of!(avail.flags));
            crate::println!("virtio-blk: submit: head_idx={}, avail_idx={}, flags={}", head_idx, idx, flags);

            // 内存屏障：确保所有描述符更新对设备可见
            core::sync::atomic::fence(Ordering::Release);

            // 获取 ring 数组的指针（在 avail 结构体中的偏移）
            // AvailRing 结构: flags(2) + idx(2) = 4 字节偏移
            let ring_ptr = (self.avail as usize + 4) as *mut u16;

            // 先读取旧值
            let old_val = core::ptr::read_volatile(ring_ptr);
            crate::println!("virtio-blk: avail_ring[{}] before write = {}", idx % self.queue_size as usize, old_val);

            // 写入描述符索引到 ring
            core::ptr::write_volatile(
                ring_ptr.add(idx % self.queue_size as usize),
                head_idx,
            );

            // 调试：验证写入
            let written = core::ptr::read_volatile(ring_ptr.add(idx % self.queue_size as usize));
            crate::println!("virtio-blk: Wrote head_idx={} to avail_ring[{}], read back={}",
                head_idx, idx % self.queue_size as usize, written);

            // 内存屏障：确保 idx 更新对设备可见
            core::sync::atomic::fence(Ordering::Release);

            // 更新索引
            unsafe {
                core::ptr::write_volatile(&mut (*avail).idx as *mut u16, (idx as u16) + 1);
            }

            // 最终内存屏障：确保所有写入对设备可见
            core::sync::atomic::fence(Ordering::SeqCst);

            // 调试：验证最终状态
            let final_idx = core::ptr::read_volatile(core::ptr::addr_of!((*avail).idx));
            crate::println!("virtio-blk: submit: avail.idx updated to {} (readback={})", (idx as u16) + 1, final_idx);
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
            // 不在这里清除描述符，让调用者通过 set_desc 设置所有字段
            // 这样避免在 alloc_desc 和 set_desc 之间出现 addr=0 的中间状态
            Some(idx)
        } else {
            None
        }
    }

    /// 设置描述符内容
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if idx < self.queue_size {
            unsafe {
                crate::println!("set_desc: idx={}, writing Desc {{ addr: 0x{:x}, len: {}, flags: {}, next: {} }}",
                    idx, addr, len, flags, next);
                *self.desc.add(idx as usize) = Desc { addr, len, flags, next };
                // 立即读回验证
                let read_back = *self.desc.add(idx as usize);
                crate::println!("set_desc: read back Desc {{ addr: 0x{:x}, len: {}, flags: {}, next: {} }}",
                    read_back.addr, read_back.len, read_back.flags, read_back.next);
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

// Implement Display for VirtIOBlkResp
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
