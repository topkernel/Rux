//! Rux 内核配置（自动生成）
//!
//! 此文件由 build.rs 根据 Kernel.toml 自动生成，请勿手动修改

// ============================================================
// 基本信息
// ============================================================

/// 内核名称
pub const KERNEL_NAME: &str = "Rux";

/// 内核版本
pub const KERNEL_VERSION: &str = "0.1.0";

/// 目标平台
pub const TARGET_PLATFORM: &str = "riscv64";

// ============================================================
// 内存配置
// ============================================================

/// 内核堆大小（字节）
pub const KERNEL_HEAP_SIZE: usize = 33554432;

/// 物理内存大小（字节）
pub const PHYS_MEMORY_SIZE: usize = 2147483648;

/// 页大小
pub const PAGE_SIZE: usize = 4096;

/// 页大小位移
pub const PAGE_SHIFT: usize = 12;

// ============================================================
// 驱动配置
// ============================================================

/// 是否启用UART驱动
pub const ENABLE_UART: bool = true;

/// 是否启用定时器驱动
pub const ENABLE_TIMER: bool = true;

/// 是否启用GIC中断控制器
pub const ENABLE_GIC: bool = false;

/// 是否启用VirtIO网络设备探测
pub const ENABLE_VIRTIO_NET_PROBE: bool = true;

// ============================================================
// SMP 配置
// ============================================================

/// 是否启用SMP多核支持
pub const ENABLE_SMP: bool = true;

/// 最大CPU数量
pub const MAX_CPUS: usize = 4;

// ============================================================
// 调度器配置
// ============================================================

/// 是否启用调度器
pub const ENABLE_SCHEDULER: bool = true;

/// 默认时间片 (毫秒)
pub const DEFAULT_TIME_SLICE_MS: u32 = 100;

/// 时间片滴答数
pub const TIME_SLICE_TICKS: u32 = 10;

// ============================================================
// 内存管理配置
// ============================================================

/// 用户栈大小 (字节)
pub const USER_STACK_SIZE: usize = 8388608;

/// 用户栈顶地址
pub const USER_STACK_TOP: u64 = 274877902848;

/// 最大页表数量
pub const MAX_PAGE_TABLES: usize = 1024;

// ============================================================
// 网络配置
// ============================================================

/// 是否启用网络协议栈
pub const ENABLE_NETWORK: bool = true;

/// 以太网 MTU
pub const ETH_MTU: usize = 1500;

/// TCP 套接字表大小
pub const TCP_SOCKET_TABLE_SIZE: usize = 64;

/// UDP 套接字表大小
pub const UDP_SOCKET_TABLE_SIZE: usize = 64;

/// ARP 缓存大小
pub const ARP_CACHE_SIZE: usize = 64;

/// 路由表大小
pub const ROUTE_TABLE_SIZE: usize = 64;

/// IPv4 默认 TTL
pub const IP_DEFAULT_TTL: u8 = 64;

// ============================================================
// 子功能使能
// ============================================================

/// 是否启用 TCP 协议
pub const ENABLE_TCP: bool = true;

/// 是否启用 UDP 协议
pub const ENABLE_UDP: bool = true;

/// 是否启用 ARP 协议
pub const ENABLE_ARP: bool = true;

/// 是否启用 IPv4 协议
pub const ENABLE_IPV4: bool = true;

/// 是否启用以太网
pub const ENABLE_ETHERNET: bool = true;

/// 是否启用信号处理
pub const ENABLE_SIGNAL: bool = true;

/// 是否启用虚拟内存
pub const ENABLE_VM: bool = true;

/// 是否启用 VFS
pub const ENABLE_VFS: bool = true;

/// 是否启用管道
pub const ENABLE_PIPE: bool = true;

// ============================================================
// 挂载配置
// ============================================================

/// ext4 磁盘挂载点
pub const EXT4_MOUNT_POINT: &str = "/";

/// 是否启用 ext4 自动挂载
pub const AUTO_MOUNT_EXT4: bool = true;
