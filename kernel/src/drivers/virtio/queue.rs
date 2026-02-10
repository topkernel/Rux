//! VirtIO 虚拟队列
//!
//! 完全遵循 VirtIO 规范的队列实现
//! 参考: VirtIO Specification v1.1

use core::mem;
use core::sync::atomic::{AtomicU16, Ordering};

/// VirtQueue 描述符（16字节对齐）
///
/// 对应 VirtIO 规范的 Queue Descriptor
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

/// VirtQueue - 虚拟队列
///
/// 用于与 VirtIO 设备通信的队列结构
pub struct VirtQueue {
    /// 队列描述符表
    desc: &'static mut [Desc],
    /// 队列大小
    queue_size: u16,
    /// 可用环索引（我们下一个可用的描述符）
    avail_idx: AtomicU16,
    /// 已用环索引（设备返回的描述符）
    used_idx: AtomicU16,
    /// 队列通知地址
    queue_notify: u64,
}

impl VirtQueue {
    /// 创建新的 VirtQueue
    ///
    /// # 参数
    /// - `desc`: 描述符表
    /// - `queue_size`: 队列大小（必须是 2 的幂）
    /// - `queue_notify`: 队列通知寄存器地址
    pub fn new(
        desc: &'static mut [Desc],
        queue_size: u16,
        queue_notify: u64,
    ) -> Self {
        Self {
            desc,
            queue_size,
            avail_idx: AtomicU16::new(0),
            used_idx: AtomicU16::new(0),
            queue_notify,
        }
    }

    /// 获取可用描述符索引
    pub fn get_avail(&self) -> u16 {
        self.avail_idx.load(Ordering::Acquire)
    }

    /// 获取已用描述符索引
    pub fn get_used(&self) -> u16 {
        self.used_idx.load(Ordering::Acquire)
    }

    /// 通知设备有新的请求
    ///
    /// 写入队列通知寄存器
    pub fn notify(&self) {
        unsafe {
            let queue_notify = self.queue_notify as *mut u32;
            core::ptr::write_volatile(queue_notify, 1);
        }
    }

    /// 等待设备完成请求
    ///
    /// 轮询 used_idx 直到有新的完成
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        loop {
            let used = self.used_idx.load(Ordering::Acquire);
            if used != prev_used {
                return used;
            }
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    /// 添加描述符到可用环
    ///
    /// # 参数
    /// - `addr`: 数据物理地址
    /// - `len`: 数据长度
    /// - `flags`: 描述符标志
    pub fn add_desc(&mut self, addr: u64, len: u32, flags: u16) -> u16 {
        let idx = self.avail_idx.load(Ordering::Acquire);

        self.desc[idx as usize] = Desc {
            addr,
            len,
            flags,
            next: 0,
        };

        // 更新可用索引
        self.avail_idx.store(idx + 1, Ordering::Release);

        idx
    }

    /// 获取已完成的描述符
    ///
    /// # 参数
    /// - `idx`: 描述符索引
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        if idx < self.queue_size {
            Some(self.desc[idx as usize])
        } else {
            None
        }
    }
}

/// VirtIO 块设备请求头
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

/// VirtIO 块设备响应
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkResp {
    /// 状态（0=OK, 1=IOERR, 2=UNSUPPORTED）
    pub status: u8,
}

/// VirtIO 块设备请求类型
pub mod req_type {
    /// 读
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    /// 写
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    /// 刷新
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

/// VirtIO 块设备状态
pub mod status {
    /// OK
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    /// IOERR
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    /// UNSUPP
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}
