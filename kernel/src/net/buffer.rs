//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络缓冲区 (SkBuff)
//!
//! 完全...

use core::sync::atomic::AtomicU64;

/// 数据包类型
///
/// ...
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// 发送到本机的包
    Host = 0,
    /// 发送到其他主机的包
    Otherhost = 1,
    /// 广播包
    Broadcast = 2,
    /// 多播包
    Multicast = 3,
}

/// 以太网协议类型
///
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum EthProtocol {
    /// IPv4
    ETH_P_IP = 0x0800,
    /// ARP
    ETH_P_ARP = 0x0806,
    /// IPv6
    ETH_P_IPV6 = 0x86DD,
    /// 802.1Q VLAN
    ETH_P_8021Q = 0x8100,
}

impl EthProtocol {
    /// 从 u16 转换
    pub fn from_u16(val: u16) -> Option<Self> {
        match val {
            0x0800 => Some(EthProtocol::ETH_P_IP),
            0x0806 => Some(EthProtocol::ETH_P_ARP),
            0x86DD => Some(EthProtocol::ETH_P_IPV6),
            0x8100 => Some(EthProtocol::ETH_P_8021Q),
            _ => None,
        }
    }

    /// 转换为 u16
    pub fn to_u16(self) -> u16 {
        self as u16
    }
}

/// IP 协议类型
///
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum IpProtocol {
    /// ICMP
    IPPROTO_IP = 0,
    /// ICMP
    IPPROTO_ICMP = 1,
    /// TCP
    IPPROTO_TCP = 6,
    /// UDP
    IPPROTO_UDP = 17,
    /// IPv6
    IPPROTO_IPV6 = 41,
}

impl IpProtocol {
    /// 从 u8 转换
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(IpProtocol::IPPROTO_IP),
            1 => Some(IpProtocol::IPPROTO_ICMP),
            6 => Some(IpProtocol::IPPROTO_TCP),
            17 => Some(IpProtocol::IPPROTO_UDP),
            41 => Some(IpProtocol::IPPROTO_IPV6),
            _ => None,
        }
    }

    /// 转换为 u8
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// 网络缓冲区 (SkBuff)
///
///
/// # 内存布局
/// ```text
/// |<- head                 ->|<- data       ->|<- tail ->|<- end ->|
/// |  (headroom)             |  (实际数据)    | (tailroom) |
/// ```
#[repr(C)]
pub struct SkBuff {
    /// 协议类型 (ETH_P_IP, ETH_P_ARP, etc.)
    pub protocol: u16,
    /// 数据包长度
    pub len: u32,
    /// 数据指针（指向当前协议层的数据起始位置）
    pub data: *mut u8,
    /// 尾部指针（指向数据结束位置）
    pub tail: *mut u8,
    /// 缓冲区结束指针
    pub end: *mut u8,
    /// 缓冲区起始指针
    pub head: *mut u8,
    /// 数据包类型
    pub pkt_type: PacketType,
    /// 时间戳
    pub tstamp: u64,
    /// MAC 地址（用于以太网）
    pub mac_len: u8,
    /// MAC 头指针
    pub mac_header: *mut u8,
    /// 网络层头指针
    pub network_header: *mut u8,
    /// 传输层头指针
    pub transport_header: *mut u8,
}

unsafe impl Send for SkBuff {}

/// SkBuff 全局分配器 ID
static SKBUFF_ALLOCATOR_ID: AtomicU64 = AtomicU64::new(0);

impl SkBuff {
    /// 分配新的 SkBuff
    ///
    /// # 参数
    /// - `size`: 数据大小（字节数）
    ///
    /// # 返回
    /// 返回分配的 SkBuff，如果分配失败则返回 None
    ///
    /// # 说明
    /// - 分配的缓冲区大小为 `size + 2 * NET_SKBUFF_DATA_ALIGN`（预留 headroom 和 tailroom）
    /// - data 和 tail 初始时指向 headroom 之后的位置
    /// - headroom 用于添加协议头（MAC、IP、TCP 等）
    pub fn alloc(size: u32) -> Option<Self> {
        // 对齐到 16 字节边界
        const NET_SKBUFF_DATA_ALIGN: usize = 16;

        // 预留 headroom 和 tailroom，各至少 16 字节
        let headroom = NET_SKBUFF_DATA_ALIGN;
        let data_size = if size == 0 {
            NET_SKBUFF_DATA_ALIGN
        } else {
            ((size as usize) + NET_SKBUFF_DATA_ALIGN - 1) / NET_SKBUFF_DATA_ALIGN * NET_SKBUFF_DATA_ALIGN
        };
        let alloc_size = headroom + data_size + NET_SKBUFF_DATA_ALIGN;

        // 分配缓冲区
        let layout = alloc::alloc::Layout::from_size_align(alloc_size, NET_SKBUFF_DATA_ALIGN)
            .ok()?;

        let head = unsafe { alloc::alloc::alloc(layout) };
        if head.is_null() {
            return None;
        }

        // data 从 headroom 之后开始
        let data = unsafe { head.add(headroom) };
        let tail = data;
        let end = unsafe { head.add(alloc_size) };

        Some(SkBuff {
            protocol: 0,
            len: 0,
            data,
            tail,
            end,
            head,
            pkt_type: PacketType::Host,
            tstamp: 0,
            mac_len: 0,
            mac_header: core::ptr::null_mut(),
            network_header: core::ptr::null_mut(),
            transport_header: core::ptr::null_mut(),
        })
    }

    /// 释放 SkBuff
    ///
    /// # 说明
    /// 释放分配的内存
    pub fn free(self) {
        unsafe {
            let layout = alloc::alloc::Layout::from_size_align(
                (self.end as usize) - (self.head as usize),
                16,
            ).unwrap();
            alloc::alloc::dealloc(self.head, layout);
        }
    }

    /// 在数据尾部添加数据
    ///
    /// # 参数
    /// - `len`: 要添加的数据长度
    ///
    /// # 返回
    /// 返回指向添加位置的指针，如果空间不足则返回 None
    ///
    /// # 说明
    /// - 移动 tail 指针向后
    /// - 增加 len
    pub fn skb_put(&mut self, len: u32) -> Option<*mut u8> {
        if self.tail as usize + len as usize > self.end as usize {
            return None;
        }

        let ptr = self.tail;
        self.tail = unsafe { self.tail.add(len as usize) };
        self.len += len;
        Some(ptr)
    }

    /// 在数据头部添加数据
    ///
    /// # 参数
    /// - `len`: 要添加的数据长度
    ///
    /// # 返回
    /// 返回指向添加位置的指针，如果空间不足则返回 None
    ///
    /// # 说明
    /// - 移动 data 指针向前
    /// - 增加 len
    pub fn skb_push(&mut self, len: u32) -> Option<*mut u8> {
        if (self.data as usize) < (self.head as usize + len as usize) {
            return None;
        }

        self.data = unsafe { self.data.sub(len as usize) };
        self.len += len;
        Some(self.data)
    }

    /// 从数据头部移除数据
    ///
    /// # 参数
    /// - `len`: 要移除的数据长度
    ///
    /// # 返回
    /// 返回移除后的 data 指针
    ///
    /// # 说明
    /// - 移动 data 指针向后
    /// - 减少 len
    pub fn skb_pull(&mut self, len: u32) -> Option<*mut u8> {
        if len > self.len {
            return None;
        }

        self.data = unsafe { self.data.add(len as usize) };
        self.len -= len;
        Some(self.data)
    }

    /// 在数据尾部保留空间
    ///
    /// # 参数
    /// - `len`: 要保留的空间长度
    ///
    /// # 返回
    /// 返回指向保留位置的指针，如果空间不足则返回 None
    ///
    /// # 说明
    /// - 移动 tail 指针向后，但不增加 len
    pub fn skb_reserve(&mut self, len: u32) -> Option<*mut u8> {
        if self.tail as usize + len as usize > self.end as usize {
            return None;
        }

        self.tail = unsafe { self.tail.add(len as usize) };
        self.data = self.tail;
        Some(self.data)
    }

    /// 写入数据到 tail 位置
    ///
    /// # 参数
    /// - `data`: 要写入的数据
    ///
    /// # 返回
    /// 成功返回 Ok(())，失败返回 Err(())
    ///
    /// # 说明
    /// - 先调用 skb_put 获取空间
    /// - 然后复制数据到该空间
    pub fn skb_put_data(&mut self, data: &[u8]) -> Result<(), ()> {
        let len = data.len() as u32;
        let ptr = self.skb_put(len).ok_or(())?;

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        }

        Ok(())
    }

    /// 设置 MAC 头
    ///
    /// # 参数
    /// - `len`: MAC 头长度
    pub fn set_mac_header(&mut self, len: u8) {
        self.mac_header = self.data;
        self.mac_len = len;
    }

    /// 设置网络层头
    ///
    /// # 说明
    /// 当前 data 指针位置即为网络层头
    pub fn set_network_header(&mut self) {
        self.network_header = self.data;
    }

    /// 设置传输层头
    ///
    /// # 说明
    /// 当前 data 指针位置即为传输层头
    pub fn set_transport_header(&mut self) {
        self.transport_header = self.data;
    }

    /// 获取 MAC 头
    pub fn get_mac_header(&self) -> *const u8 {
        self.mac_header
    }

    /// 获取网络层头
    pub fn get_network_header(&self) -> *const u8 {
        self.network_header
    }

    /// 获取传输层头
    pub fn get_transport_header(&self) -> *const u8 {
        self.transport_header
    }

    /// 获取数据指针
    pub fn data(&self) -> *const u8 {
        self.data
    }

    /// 获取可变数据指针
    pub fn data_mut(&mut self) -> *mut u8 {
        self.data
    }

    /// 获取数据长度
    pub fn len(&self) -> u32 {
        self.len
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 复制 SkBuff 数据
    ///
    /// # 参数
    /// - `buf`: 目标缓冲区
    /// - `offset`: 偏移量
    /// - `len`: 复制长度
    ///
    /// # 返回
    /// 返回实际复制的字节数
    pub fn skb_copy_bits(&self, offset: u32, buf: &mut [u8], len: u32) -> u32 {
        if offset > self.len {
            return 0;
        }

        let copy_len = core::cmp::min(len, self.len - offset);
        if copy_len == 0 {
            return 0;
        }

        unsafe {
            let src = self.data.add(offset as usize);
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len as usize);
        }

        copy_len
    }
}

/// 分配 SkBuff 的辅助函数
///
/// # 参数
/// - `size`: 数据大小
///
/// # 返回
/// 返回分配的 SkBuff
pub fn alloc_skb(size: u32) -> Option<SkBuff> {
    SkBuff::alloc(size)
}

/// 释放 SkBuff 的辅助函数
///
/// # 参数
/// - `skb`: 要释放的 SkBuff
pub fn kfree_skb(skb: SkBuff) {
    skb.free();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skb_alloc() {
        let skb = SkBuff::alloc(1500);
        assert!(skb.is_some());
        let skb = skb.unwrap();
        assert_eq!(skb.len(), 0);
        assert!(skb.is_empty());
    }

    #[test]
    fn test_skb_put() {
        let mut skb = SkBuff::alloc(1500).unwrap();
        let data = b"Hello, World!";

        assert!(skb.skb_put_data(data).is_ok());
        assert_eq!(skb.len(), data.len() as u32);
        assert!(!skb.is_empty());
    }

    #[test]
    fn test_skb_push() {
        let mut skb = SkBuff::alloc(1500).unwrap();

        // 先 put 一些数据
        skb.skb_put_data(b"World!").unwrap();

        // 再 push 一些数据
        let ptr = skb.skb_push(7).unwrap();
        unsafe {
            core::ptr::copy_nonoverlapping(b"Hello, ".as_ptr(), ptr, 7);
        }

        assert_eq!(skb.len(), 13);
    }

    #[test]
    fn test_skb_pull() {
        let mut skb = SkBuff::alloc(1500).unwrap();
        skb.skb_put_data(b"Hello, World!").unwrap();

        skb.skb_pull(7);
        assert_eq!(skb.len(), 6);
    }
}
