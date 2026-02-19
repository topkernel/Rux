//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 虚拟内存区域 (Virtual Memory Area) 管理 - 平台无关部分
//!
//!
//! VMA 表示进程地址空间中一个连续的虚拟内存区域，具有相同的
//! 访问权限和映射属性。
//!
//! 本模块只包含平台无关的数据结构：
//! - VmaFlags: VMA 标志
//! - Vma: VMA 结构体
//! - VmaManager: VMA 管理器
//! - AddressSpace: 平台无关的地址空间抽象
//!
//! 架构特定的实现（如页表管理、mmap/munmap/brk 系统调用）应该在 arch/*/mm.rs 中

pub use crate::mm::page::{VirtAddr, PAGE_SIZE};
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmaFlags(u32);

impl VmaFlags {
    /// 可读 (VM_READ)
    pub const READ: u32 = 0x00000001;
    /// 可写 (VM_WRITE)
    pub const WRITE: u32 = 0x00000002;
    /// 可执行 (VM_EXEC)
    pub const EXEC: u32 = 0x00000004;
    /// 共享映射 (VM_SHARED)
    pub const SHARED: u32 = 0x00000008;
    /// 私有映射 (VM_PRIVATE)
    pub const PRIVATE: u32 = 0x00000010;
    /// 可能扩展到堆 (VM_GROWSDOWN)
    pub const GROWSDOWN: u32 = 0x00000100;
    /// 可能扩展到栈 (VM_GROWSUP)
    pub const GROWSUP: u32 = 0x00000200;
    /// 拒绝 rmap (VM_DENYWRITE)
    pub const DENYWRITE: u32 = 0x00000800;
    /// 可执行控制/堆 (VM_EXECUTABLE)
    pub const EXECUTABLE: u32 = 0x00001000;
    /// 锁定内存 (VM_LOCKED)
    pub const LOCKED: u32 = 0x00002000;
    /// I/O 映射 (VM_IO)
    pub const IO: u32 = 0x00004000;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    #[inline]
    pub fn contains(&self, flags: u32) -> bool {
        self.0 & flags == flags
    }

    #[inline]
    pub fn insert(&mut self, flags: u32) {
        self.0 |= flags;
    }

    #[inline]
    pub fn remove(&mut self, flags: u32) {
        self.0 &= !flags;
    }

    /// 检查是否可读
    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::READ != 0
    }

    /// 检查是否可写
    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::WRITE != 0
    }

    /// 检查是否可执行
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::EXEC != 0
    }

    /// 检查是否共享
    #[inline]
    pub fn is_shared(&self) -> bool {
        self.0 & Self::SHARED != 0
    }

    /// 转换为页权限 (Perm)
    ///
    /// 根据 VMA 标志推断页表权限
    ///
    /// 对应关系：
    /// - 无 READ/WRITE/EXEC -> Perm::None
    /// - 仅 READ -> Perm::Read
    /// - READ + WRITE -> Perm::ReadWrite
    /// - READ + WRITE + EXEC -> Perm::ReadWriteExec
    /// - READ + EXEC -> Perm::Read (没有 ReadExec 选项，使用 Read)
    /// - WRITE + EXEC -> Perm::ReadWrite (没有 WriteExec 选项，使用 ReadWrite)
    ///
    pub fn to_page_perm(&self) -> crate::mm::pagemap::Perm {
        use crate::mm::pagemap::Perm;

        let readable = self.is_readable();
        let writable = self.is_writable();
        let executable = self.is_executable();

        match (readable, writable, executable) {
            (false, false, false) => Perm::None,
            (true, false, false) => Perm::Read,
            (true, true, false) => Perm::ReadWrite,
            (true, true, true) => Perm::ReadWriteExec,
            (true, false, true) => Perm::Read,      // Read-only executable
            (false, true, false) => Perm::ReadWrite, // Write-only (unusual)
            (false, true, true) => Perm::ReadWrite,  // Write-execute (unusual)
            (false, false, true) => Perm::None,      // Execute-only (unusual)
        }
    }
}

impl Default for VmaFlags {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct Vma {
    /// 起始虚拟地址 (包含)
    start: VirtAddr,

    /// 结束虚拟地址 (不包含)
    end: VirtAddr,

    /// 访问权限和属性
    flags: VmaFlags,

    /// VMA 偏移量（用于文件映射）
    offset: usize,

    /// VMA 类型
    vma_type: VmaType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaType {
    /// 匿名映射（堆、栈、私有数据）
    Anonymous,
    /// 文件映射
    FileBacked,
    /// 设备映射 (MMIO)
    Device,
    /// 共享内存
    SharedMemory,
}

impl Vma {
    /// 创建新的 VMA
    pub fn new(start: VirtAddr, end: VirtAddr, flags: VmaFlags) -> Self {
        assert!(start.as_usize() < end.as_usize(), "Invalid VMA range");
        assert!(start.as_usize() % PAGE_SIZE == 0, "VMA start not page aligned");
        assert!(end.as_usize() % PAGE_SIZE == 0, "VMA end not page aligned");

        Self {
            start,
            end,
            flags,
            offset: 0,
            vma_type: VmaType::Anonymous,
        }
    }

    /// 获取起始地址
    #[inline]
    pub fn start(&self) -> VirtAddr {
        self.start
    }

    /// 获取结束地址
    #[inline]
    pub fn end(&self) -> VirtAddr {
        self.end
    }

    /// 获取 VMA 大小（字节）
    #[inline]
    pub fn size(&self) -> usize {
        self.end.as_usize() - self.start.as_usize()
    }

    /// 获取 VMA 大小（页数）
    #[inline]
    pub fn page_count(&self) -> usize {
        self.size() / PAGE_SIZE
    }

    /// 获取标志
    #[inline]
    pub fn flags(&self) -> VmaFlags {
        self.flags
    }

    /// 获取类型
    #[inline]
    pub fn vma_type(&self) -> VmaType {
        self.vma_type
    }

    /// 设置类型
    pub fn set_type(&mut self, vma_type: VmaType) {
        self.vma_type = vma_type;
    }

    /// 检查地址是否在 VMA 范围内
    #[inline]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_usize() >= self.start.as_usize() && addr.as_usize() < self.end.as_usize()
    }

    /// 检查两个 VMA 是否重叠
    pub fn overlaps(&self, other: &Vma) -> bool {
        self.start.as_usize() < other.end.as_usize()
            && other.start.as_usize() < self.end.as_usize()
    }

    /// 设置文件偏移（用于文件映射）
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    /// 获取文件偏移
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// 分裂 VMA（在指定地址处分裂）
    ///
    /// 返回 (前半部分, 后半部分) 或 None 如果地址不在范围内
    pub fn split(&self, addr: VirtAddr) -> Option<(Vma, Vma)> {
        if !self.contains(addr) {
            return None;
        }

        // 确保分裂地址是页对齐的
        let aligned_addr = VirtAddr::new(addr.as_usize() & !(PAGE_SIZE - 1));
        if aligned_addr.as_usize() <= self.start.as_usize()
            || aligned_addr.as_usize() >= self.end.as_usize()
        {
            return None;
        }

        let first = Vma {
            start: self.start,
            end: aligned_addr,
            flags: self.flags,
            offset: self.offset,
            vma_type: self.vma_type,
        };

        let second = Vma {
            start: aligned_addr,
            end: self.end,
            flags: self.flags,
            offset: self.offset + (aligned_addr.as_usize() - self.start.as_usize()),
            vma_type: self.vma_type,
        };

        Some((first, second))
    }

    /// 可以与另一个 VMA 合并吗？
    pub fn can_merge(&self, other: &Vma) -> bool {
        // 必须相邻且具有相同的属性
        self.end.as_usize() == other.start.as_usize()
            && self.flags.bits() == other.flags.bits()
            && self.vma_type == other.vma_type
    }

    /// 与另一个 VMA 合并
    pub fn merge(&mut self, other: Vma) -> bool {
        if self.can_merge(&other) {
            self.end = other.end;
            true
        } else {
            false
        }
    }
}

impl core::fmt::Debug for Vma {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Vma")
            .field("range", &format_args!("0x{:x}-0x{:x}", self.start.as_usize(), self.end.as_usize()))
            .field("size", &self.size())
            .field("flags", &self.flags)
            .field("type", &self.vma_type)
            .finish()
    }
}

/// VMA 管理器
///
/// 使用 BTreeMap 存储 VMA，按起始地址排序
/// - O(log n) 查找、插入、删除
/// - 动态扩展，无数量限制
pub struct VmaManager {
    /// VMA 映射表（按起始地址排序）
    vmas: BTreeMap<VirtAddr, Vma>,

    /// VMA 数量（用于兼容性）
    count: AtomicU32,
}

impl VmaManager {
    /// 创建新的 VMA 管理器
    pub fn new() -> Self {
        Self {
            vmas: BTreeMap::new(),
            count: AtomicU32::new(0),
        }
    }

    /// 添加 VMA
    ///
    /// # 参数
    /// - `vma`: 要添加的 VMA
    ///
    /// # 返回
    /// - `Ok(())`: 添加成功
    /// - `Err(VmaError::Overlap)`: 与现有 VMA 重叠
    pub fn add(&mut self, vma: Vma) -> Result<(), VmaError> {
        let start = vma.start();

        // 检查是否与现有 VMA 重叠
        // BTreeMap 按键排序，只需检查相邻的 VMA
        for existing in self.vmas.values() {
            if vma.overlaps(existing) {
                return Err(VmaError::Overlap);
            }
        }

        // 插入 BTreeMap
        self.vmas.insert(start, vma);
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// 查找包含指定地址的 VMA
    ///
    /// 使用 BTreeMap 的范围查找优化
    pub fn find(&self, addr: VirtAddr) -> Option<&Vma> {
        // 找到第一个起始地址 <= addr 的 VMA
        // BTreeMap::range 返回键在指定范围内的元素
        for vma in self.vmas.values().rev() {
            if vma.contains(addr) {
                return Some(vma);
            }
            // 由于是按地址排序的，如果当前 VMA 起始地址 > addr，继续查找
            // 如果当前 VMA 结束地址 <= addr，则不可能找到
            if vma.end().as_usize() <= addr.as_usize() {
                break;
            }
        }
        None
    }

    /// 查找包含指定地址的 VMA（可变引用）
    pub fn find_mut(&mut self, addr: VirtAddr) -> Option<&mut Vma> {
        for vma in self.vmas.values_mut().rev() {
            if vma.contains(addr) {
                return Some(vma);
            }
            if vma.end().as_usize() <= addr.as_usize() {
                break;
            }
        }
        None
    }

    /// 删除 VMA
    ///
    /// # 参数
    /// - `start`: VMA 的起始地址
    pub fn remove(&mut self, start: VirtAddr) -> Result<(), VmaError> {
        if self.vmas.remove(&start).is_some() {
            self.count.fetch_sub(1, Ordering::Release);
            Ok(())
        } else {
            Err(VmaError::NotFound)
        }
    }

    /// 获取所有 VMA 的迭代器
    pub fn iter(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.values()
    }

    /// 获取 VMA 数量
    #[inline]
    pub fn count(&self) -> usize {
        self.vmas.len()
    }

    /// 查找指定起始地址的 VMA
    pub fn get(&self, start: VirtAddr) -> Option<&Vma> {
        self.vmas.get(&start)
    }

    /// 查找指定起始地址的 VMA（可变引用）
    pub fn get_mut(&mut self, start: VirtAddr) -> Option<&mut Vma> {
        self.vmas.get_mut(&start)
    }

    /// 查找第一个 VMA
    pub fn first(&self) -> Option<&Vma> {
        self.vmas.values().next()
    }

    /// 查找最后一个 VMA
    pub fn last(&self) -> Option<&Vma> {
        self.vmas.values().next_back()
    }

    /// 查找起始地址 >= addr 的第一个 VMA
    pub fn find_vma_after(&self, addr: VirtAddr) -> Option<&Vma> {
        self.vmas.range(addr..).next().map(|(_, vma)| vma)
    }

    /// 清空所有 VMA
    pub fn clear(&mut self) {
        self.vmas.clear();
        self.count.store(0, Ordering::Release);
    }
}

impl Default for VmaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaError {
    /// VMA 重叠
    Overlap,
    /// 没有空间（保留用于兼容性）
    NoSpace,
    /// 未找到
    NotFound,
    /// 无效参数
    Invalid,
}

unsafe impl Send for VmaManager {}
unsafe impl Sync for VmaManager {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vma_creation() {
        let start = VirtAddr::new(0x1000);
        let end = VirtAddr::new(0x2000);
        let flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE);

        let vma = Vma::new(start, end, flags);
        assert_eq!(vma.start(), start);
        assert_eq!(vma.end(), end);
        assert_eq!(vma.size(), 0x1000);
        assert_eq!(vma.page_count(), 1);
    }

    #[test]
    fn test_vma_contains() {
        let start = VirtAddr::new(0x1000);
        let end = VirtAddr::new(0x3000);
        let vma = Vma::new(start, end, VmaFlags::new());

        assert!(vma.contains(VirtAddr::new(0x1000)));
        assert!(vma.contains(VirtAddr::new(0x2000)));
        assert!(!vma.contains(VirtAddr::new(0x3000)));
        assert!(!vma.contains(VirtAddr::new(0xfff)));
    }
}

// ============================================================================
// 地址空间平台特定参数接口
// ============================================================================

/// 地址空间平台特定参数
///
/// 不同架构需要提供其特定的地址空间布局参数
///
pub trait AddressSpaceLayout {
    /// 用户地址空间起始地址
    fn user_start() -> usize;

    /// 用户地址空间结束地址
    fn user_end() -> usize;

    /// 默认栈大小
    fn default_stack_size() -> usize;

    /// 默认栈顶（从用户空间顶部向下）
    fn default_stack_top() -> usize;

    /// 堆起始地址
    fn heap_start() -> usize;

    /// 堆结束地址（堆的最大值）
    fn heap_end() -> usize;
}

// ============================================================================
// RISC-V 地址空间布局实现
// ============================================================================

/// RISC-V 64-bit 地址空间布局
///
/// RISC-V sv39: 0x0000_0000_1000_0000 ~ 0x0000_003f_ffff_ffff
#[cfg(target_arch = "riscv64")]
pub struct RiscVAddressSpaceLayout;

#[cfg(target_arch = "riscv64")]
impl AddressSpaceLayout for RiscVAddressSpaceLayout {
    #[inline]
    fn user_start() -> usize {
        0x0000_0000_1000_0000
    }

    #[inline]
    fn user_end() -> usize {
        0x0000_003f_ffff_ffff
    }

    #[inline]
    fn default_stack_size() -> usize {
        8 * 1024 * 1024 // 8MB
    }

    #[inline]
    fn default_stack_top() -> usize {
        0x0000_003f_ffff_f000
    }

    #[inline]
    fn heap_start() -> usize {
        0x0000_0000_1000_0000
    }

    #[inline]
    fn heap_end() -> usize {
        0x0000_0000_2000_0000
    }
}
