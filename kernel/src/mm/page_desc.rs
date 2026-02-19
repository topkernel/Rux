//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! 页描述符 (Page Descriptor)
//!
//! 为每个物理页帧维护元数据，包括：
//! - 引用计数 (_refcount)
//! - 页标志位 (flags)
//! - 映射计数 (_mapcount)
//! - 其他元数据
//!

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use super::page::{PhysAddr, PhysFrame, PhysFrameNr, VirtAddr, PAGE_SIZE};

/// 页标志位
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    /// 页已锁定，不可访问
    Locked = 1 << 0,
    /// 页正在回写
    Writeback = 1 << 1,
    /// 页已被访问（用于 LRU）
    Referenced = 1 << 2,
    /// 页数据有效（已从磁盘读取）
    UpToDate = 1 << 3,
    /// 页已修改（需要回写）
    Dirty = 1 << 4,
    /// 页在 LRU 链表中
    Lru = 1 << 5,
    /// 复合页的头部页
    Head = 1 << 6,
    /// 页有等待者
    Waiters = 1 << 7,
    /// 页在活跃 LRU 链表
    Active = 1 << 8,
    /// 保留页（内核使用，不可交换）
    Reserved = 1 << 9,
    /// 页有私有数据（存储在 private 字段）
    Private = 1 << 10,
    /// 页将被回收
    Reclaim = 1 << 11,
    /// 页由交换空间支持
    SwapBacked = 1 << 12,
    /// 页不可驱逐
    Unevictable = 1 << 13,
    /// 写时复制页（Rux 扩展）
    Cow = 1 << 14,
    /// 匿名页（Rux 扩展）
    Anonymous = 1 << 15,
}

/// 页标志位集合
#[derive(Debug, Default)]
pub struct PageFlags(AtomicU32);

impl PageFlags {
    /// 创建空的标志位集合
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// 从原始值创建
    pub const fn from_raw(flags: u32) -> Self {
        Self(AtomicU32::new(flags))
    }

    /// 获取原始值
    pub fn raw(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// 测试标志位是否设置
    pub fn test(&self, flag: PageFlag) -> bool {
        self.0.load(Ordering::Relaxed) & (flag as u32) != 0
    }

    /// 设置标志位
    pub fn set(&self, flag: PageFlag) {
        self.0.fetch_or(flag as u32, Ordering::Release);
    }

    /// 清除标志位
    pub fn clear(&self, flag: PageFlag) {
        self.0.fetch_and(!(flag as u32), Ordering::Release);
    }

    /// 测试并设置标志位（返回旧值）
    pub fn test_and_set(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_or(bit, Ordering::AcqRel) & bit) != 0
    }

    /// 测试并清除标志位（返回旧值）
    pub fn test_and_clear(&self, flag: PageFlag) -> bool {
        let bit = flag as u32;
        (self.0.fetch_and(!bit, Ordering::AcqRel) & bit) != 0
    }

    /// 清除所有标志位
    pub fn clear_all(&self) {
        self.0.store(0, Ordering::Release);
    }
}

/// 页类型
///
/// 用于特殊页面的标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageType {
    /// 普通页
    Normal = 0,
    /// 伙伴系统空闲页
    Buddy = 1,
    /// Slab 分配器页
    Slab = 2,
    /// 页缓存页
    PageCache = 3,
    /// 匿名页
    Anonymous = 4,
}

/// 页描述符
///
/// 每个物理页帧对应一个 Page 结构体，用于跟踪页的使用情况。
///
/// 内存布局（约 64 字节，对齐到缓存行）：
/// - flags: 4 字节（原子标志位）
/// - _mapcount: 4 字节（映射计数，-1 表示未映射）
/// - _refcount: 4 字节（引用计数）
/// - private: 8 字节（私有数据）
/// - mapping: 8 字节（关联的 address_space，用于文件映射）
/// - index: 8 字节（在映射中的偏移）
/// - _type: 4 字节（页类型）
/// - _reserved: 4 字节（保留）
/// - next_free: 8 字节（空闲链表指针，用于分配器）
/// - _pad: 12 字节（填充到 64 字节）
///
#[repr(C, align(64))]
pub struct Page {
    /// 原子标志位
    flags: PageFlags,

    /// 映射计数：有多少页表项直接引用此页
    /// -1 表示未映射，0 表示已映射一次，以此类推
    /// 注意：初始值为 -1（PAGE_MAPCOUNT_BIAS）
    _mapcount: AtomicI32,

    /// 引用计数：对此页的引用数
    /// 0 表示空闲，> 0 表示在使用
    _refcount: AtomicI32,

    /// 私有数据
    /// - 伙伴系统：存储 order
    /// - Slab：存储 slab 管理结构
    /// - 文件系统：存储 buffer_head
    private: AtomicUsize,

    /// 关联的地址空间（用于文件映射）
    /// 指向 struct address_space 或 NULL
    mapping: AtomicUsize,

    /// 在映射中的偏移（页单位）
    index: AtomicUsize,

    /// 页类型（用于特殊页面）
    _type: AtomicU32,

    /// 保留字段
    _reserved: AtomicU32,

    /// 空闲链表指针（用于分配器内部使用）
    next_free: AtomicUsize,
}

/// 映射计数的初始偏移值（-1 表示未映射）
const PAGE_MAPCOUNT_BIAS: i32 = -1;

impl Page {
    /// 创建一个新的页描述符（初始化为空闲状态）
    pub const fn new() -> Self {
        Self {
            flags: PageFlags::new(),
            _mapcount: AtomicI32::new(PAGE_MAPCOUNT_BIAS),
            _refcount: AtomicI32::new(0),
            private: AtomicUsize::new(0),
            mapping: AtomicUsize::new(0),
            index: AtomicUsize::new(0),
            _type: AtomicU32::new(PageType::Normal as u32),
            _reserved: AtomicU32::new(0),
            next_free: AtomicUsize::new(0),
        }
    }

    /// 初始化为保留页（内核代码、设备内存等）
    pub fn init_reserved(&self) {
        self.flags.set(PageFlag::Reserved);
        self._refcount.store(1, Ordering::Release);
    }

    /// 初始化为普通可用页
    pub fn init_free(&self) {
        self.flags.clear_all();
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
        self._refcount.store(0, Ordering::Release);
        self.private.store(0, Ordering::Release);
        self.mapping.store(0, Ordering::Release);
        self.index.store(0, Ordering::Release);
    }

    // ========== 标志位操作 ==========

    /// 测试标志位
    #[inline]
    pub fn test_flag(&self, flag: PageFlag) -> bool {
        self.flags.test(flag)
    }

    /// 设置标志位
    #[inline]
    pub fn set_flag(&self, flag: PageFlag) {
        self.flags.set(flag);
    }

    /// 清除标志位
    #[inline]
    pub fn clear_flag(&self, flag: PageFlag) {
        self.flags.clear(flag);
    }

    /// 测试并设置标志位
    #[inline]
    pub fn test_and_set_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_set(flag)
    }

    /// 测试并清除标志位
    #[inline]
    pub fn test_and_clear_flag(&self, flag: PageFlag) -> bool {
        self.flags.test_and_clear(flag)
    }

    /// 检查页是否锁定
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.test_flag(PageFlag::Locked)
    }

    /// 检查页是否保留
    #[inline]
    pub fn is_reserved(&self) -> bool {
        self.test_flag(PageFlag::Reserved)
    }

    /// 检查页是否为脏页
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.test_flag(PageFlag::Dirty)
    }

    /// 检查页是否为写时复制页
    #[inline]
    pub fn is_cow(&self) -> bool {
        self.test_flag(PageFlag::Cow)
    }

    /// 检查页是否为匿名页
    #[inline]
    pub fn is_anonymous(&self) -> bool {
        self.test_flag(PageFlag::Anonymous)
    }

    /// 检查页数据是否有效
    #[inline]
    pub fn is_uptodate(&self) -> bool {
        self.test_flag(PageFlag::UpToDate)
    }

    // ========== 引用计数操作 ==========

    /// 获取引用计数
    #[inline]
    pub fn refcount(&self) -> i32 {
        self._refcount.load(Ordering::Acquire)
    }

    /// 增加引用计数
    /// 返回增加后的值
    #[inline]
    pub fn get_page(&self) -> i32 {
        self._refcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 减少引用计数
    /// 返回减少后的值；如果变为 0，调用者应释放页面
    #[inline]
    pub fn put_page(&self) -> i32 {
        self._refcount.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 尝试增加引用计数（仅当 refcount > 0 时）
    /// 成功返回 true
    #[inline]
    pub fn try_get_page(&self) -> bool {
        loop {
            let old = self._refcount.load(Ordering::Acquire);
            if old <= 0 {
                return false;
            }
            match self._refcount.compare_exchange_weak(
                old,
                old + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// 设置引用计数（仅用于初始化）
    #[inline]
    pub fn set_refcount(&self, count: i32) {
        self._refcount.store(count, Ordering::Release);
    }

    // ========== 映射计数操作 ==========

    /// 获取映射计数（-1 表示未映射）
    #[inline]
    pub fn mapcount(&self) -> i32 {
        self._mapcount.load(Ordering::Acquire)
    }

    /// 增加映射计数
    /// 返回增加后的值
    #[inline]
    pub fn add_mapcount(&self) -> i32 {
        self._mapcount.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 减少映射计数
    /// 返回减少后的值
    #[inline]
    pub fn sub_mapcount(&self) -> i32 {
        self._mapcount.fetch_sub(1, Ordering::AcqRel) - 1
    }

    /// 检查页是否被映射
    #[inline]
    pub fn is_mapped(&self) -> bool {
        self._mapcount.load(Ordering::Acquire) > PAGE_MAPCOUNT_BIAS
    }

    /// 重置映射计数
    #[inline]
    pub fn reset_mapcount(&self) {
        self._mapcount.store(PAGE_MAPCOUNT_BIAS, Ordering::Release);
    }

    // ========== 私有数据操作 ==========

    /// 获取私有数据
    #[inline]
    pub fn private(&self) -> usize {
        self.private.load(Ordering::Acquire)
    }

    /// 设置私有数据
    #[inline]
    pub fn set_private(&self, value: usize) {
        self.private.store(value, Ordering::Release);
    }

    // ========== 映射信息操作 ==========

    /// 获取关联的 address_space
    #[inline]
    pub fn mapping(&self) -> *mut core::ffi::c_void {
        self.mapping.load(Ordering::Acquire) as *mut core::ffi::c_void
    }

    /// 设置关联的 address_space
    #[inline]
    pub fn set_mapping(&self, mapping: *mut core::ffi::c_void) {
        self.mapping.store(mapping as usize, Ordering::Release);
    }

    /// 获取页索引
    #[inline]
    pub fn index(&self) -> usize {
        self.index.load(Ordering::Acquire)
    }

    /// 设置页索引
    #[inline]
    pub fn set_index(&self, index: usize) {
        self.index.store(index, Ordering::Release);
    }

    // ========== 页类型操作 ==========

    /// 获取页类型
    #[inline]
    pub fn page_type(&self) -> PageType {
        match self._type.load(Ordering::Acquire) {
            0 => PageType::Normal,
            1 => PageType::Buddy,
            2 => PageType::Slab,
            3 => PageType::PageCache,
            4 => PageType::Anonymous,
            _ => PageType::Normal,
        }
    }

    /// 设置页类型
    #[inline]
    pub fn set_page_type(&self, page_type: PageType) {
        self._type.store(page_type as u32, Ordering::Release);
    }

    // ========== 空闲链表操作（分配器内部使用） ==========

    /// 获取下一个空闲页的 PFN
    #[inline]
    pub(crate) fn next_free(&self) -> usize {
        self.next_free.load(Ordering::Acquire)
    }

    /// 设置下一个空闲页的 PFN
    #[inline]
    pub(crate) fn set_next_free(&self, pfn: usize) {
        self.next_free.store(pfn, Ordering::Release);
    }
}

// ========== 全局页数组 (mem_map) ==========

/// 物理内存常量
pub const PHYS_MEMORY_BASE: usize = 0x8000_0000; // QEMU virt: 物理内存起始地址
pub const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
pub const MAX_PFN: usize = (PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE) / PAGE_SIZE;

/// 页数组的大小
///
/// 注意：MEM_MAP 数组大小 = MAX_PAGES * sizeof(Page) = MAX_PAGES * 64 字节
/// - 16384 页 = 64MB 物理内存 = 1MB 描述符
/// - 32768 页 = 128MB 物理内存 = 2MB 描述符
/// - 65536 页 = 256MB 物理内存 = 4MB 描述符
///
/// 当前设置为 16384 以避免链接时内存布局冲突
pub const MAX_PAGES: usize = 16384; // 64MB = 16384 页

/// 全局页数组
///
/// 存储所有物理页的描述符。
/// 注意：这是一个静态数组，实际使用大小由物理内存决定。
static mut MEM_MAP: [Page; MAX_PAGES] = {
    // 使用 const fn 初始化
    const INIT: Page = Page::new();
    [INIT; MAX_PAGES]
};

/// 页数组是否已初始化
static MEM_MAP_INIT: AtomicUsize = AtomicUsize::new(0);

/// 获取页数组的起始地址
#[inline]
pub fn mem_map() -> *const Page {
    unsafe { MEM_MAP.as_ptr() }
}

/// 获取页数组的可变起始地址
#[inline]
pub fn mem_map_mut() -> *mut Page {
    unsafe { MEM_MAP.as_mut_ptr() }
}

/// 初始化页数组
///
/// 初始化指定范围内的页描述符。
/// 超出范围的页标记为保留。
///
/// # 参数
/// - `start_pfn`: 可用内存起始 PFN
/// - `nr_pages`: 可用页数
pub fn init_mem_map(start_pfn: PhysFrameNr, nr_pages: usize) {
    // 防止重复初始化
    if MEM_MAP_INIT.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    let mem_map_ptr = mem_map_mut();

    // 标记所有页为保留
    for i in 0..MAX_PAGES {
        unsafe {
            let page = &*mem_map_ptr.add(i);
            page.init_reserved();
        }
    }

    // 初始化可用页
    let init_count = if nr_pages > MAX_PAGES { MAX_PAGES } else { nr_pages };

    for i in 0..init_count {
        unsafe {
            let page = &*mem_map_ptr.add(i);
            page.init_free();
        }
    }
}

// ========== PFN <-> Page 转换 ==========

/// PFN (Page Frame Number) 转换为 Page 指针
///
/// # Safety
/// 调用者必须确保 pfn 在有效范围内
#[inline]
pub fn pfn_to_page(pfn: PhysFrameNr) -> *const Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;
    // 检查 pfn 是否在有效范围内
    if pfn < base_pfn {
        return core::ptr::null();
    }
    let idx = pfn - base_pfn;
    if idx >= MAX_PAGES {
        return core::ptr::null();
    }
    unsafe { MEM_MAP.as_ptr().add(idx) }
}

/// PFN 转换为可变 Page 指针
#[inline]
pub fn pfn_to_page_mut(pfn: PhysFrameNr) -> *mut Page {
    let base_pfn = PHYS_MEMORY_BASE / PAGE_SIZE;
    // 检查 pfn 是否在有效范围内
    if pfn < base_pfn {
        return core::ptr::null_mut();
    }
    let idx = pfn - base_pfn;
    if idx >= MAX_PAGES {
        return core::ptr::null_mut();
    }
    unsafe { MEM_MAP.as_mut_ptr().add(idx) }
}

/// Page 指针转换为 PFN
///
/// # Safety
/// 调用者必须确保 page 指针有效
#[inline]
pub fn page_to_pfn(page: *const Page) -> PhysFrameNr {
    unsafe {
        let base = MEM_MAP.as_ptr() as usize;
        let page_addr = page as usize;
        let idx = (page_addr - base) / core::mem::size_of::<Page>();
        (PHYS_MEMORY_BASE / PAGE_SIZE) + idx
    }
}

/// 物理地址转换为 Page 指针
#[inline]
pub fn phys_to_page(phys: PhysAddr) -> *const Page {
    pfn_to_page(phys.frame_number())
}

/// 物理地址转换为可变 Page 指针
#[inline]
pub fn phys_to_page_mut(phys: PhysAddr) -> *mut Page {
    pfn_to_page_mut(phys.frame_number())
}

/// 物理页帧转换为 Page 指针
#[inline]
pub fn frame_to_page(frame: PhysFrame) -> *const Page {
    pfn_to_page(frame.number)
}

/// 物理页帧转换为可变 Page 指针
#[inline]
pub fn frame_to_page_mut(frame: PhysFrame) -> *mut Page {
    pfn_to_page_mut(frame.number)
}

// ========== 辅助函数 ==========

/// 获取页的总数
#[inline]
pub fn total_pages() -> usize {
    MAX_PAGES
}

/// 获取 Page 结构体的大小（字节）
#[inline]
pub fn page_size() -> usize {
    core::mem::size_of::<Page>()
}

/// 页描述符统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct PageDescStats {
    /// 总页数
    pub total_pages: usize,
    /// 空闲页数（refcount == 0）
    pub free_pages: usize,
    /// 使用中页数（refcount > 0）
    pub used_pages: usize,
    /// 保留页数（Reserved 标志）
    pub reserved_pages: usize,
    /// 已映射页数（mapcount > PAGE_MAPCOUNT_BIAS）
    pub mapped_pages: usize,
    /// 脏页数（Dirty 标志）
    pub dirty_pages: usize,
    /// COW 页数（Cow 标志）
    pub cow_pages: usize,
    /// 匿名页数（Anonymous 标志）
    pub anonymous_pages: usize,
}

/// 获取页描述符统计信息
pub fn page_desc_stats() -> PageDescStats {
    let mut stats = PageDescStats {
        total_pages: MAX_PAGES,
        ..Default::default()
    };

    let mem_map_ptr = mem_map();

    for i in 0..MAX_PAGES {
        unsafe {
            let page = &*mem_map_ptr.add(i);

            if page.refcount() == 0 {
                stats.free_pages += 1;
            } else {
                stats.used_pages += 1;
            }

            if page.is_reserved() {
                stats.reserved_pages += 1;
            }

            if page.is_mapped() {
                stats.mapped_pages += 1;
            }

            if page.is_dirty() {
                stats.dirty_pages += 1;
            }

            if page.is_cow() {
                stats.cow_pages += 1;
            }

            if page.is_anonymous() {
                stats.anonymous_pages += 1;
            }
        }
    }

    stats
}

/// 获取物理页帧对应的 Page 引用
///
/// # Safety
/// 调用者必须确保 pfn 在有效范围内
#[inline]
pub unsafe fn get_page(pfn: PhysFrameNr) -> &'static Page {
    &*pfn_to_page(pfn)
}

/// 获取物理页帧对应的可变 Page 引用
///
/// # Safety
/// 调用者必须确保 pfn 在有效范围内，且没有其他引用
#[inline]
pub unsafe fn get_page_mut(pfn: PhysFrameNr) -> &'static mut Page {
    &mut *pfn_to_page_mut(pfn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_page_flags() {
        let flags = PageFlags::new();

        assert!(!flags.test(PageFlag::Locked));
        assert!(!flags.test(PageFlag::Dirty));

        flags.set(PageFlag::Locked);
        assert!(flags.test(PageFlag::Locked));

        flags.set(PageFlag::Dirty);
        assert!(flags.test(PageFlag::Dirty));

        flags.clear(PageFlag::Locked);
        assert!(!flags.test(PageFlag::Locked));
        assert!(flags.test(PageFlag::Dirty));
    }

    #[test_case]
    fn test_page_refcount() {
        let page = Page::new();

        assert_eq!(page.refcount(), 0);

        page.get_page();
        assert_eq!(page.refcount(), 1);

        page.get_page();
        assert_eq!(page.refcount(), 2);

        page.put_page();
        assert_eq!(page.refcount(), 1);

        page.put_page();
        assert_eq!(page.refcount(), 0);
    }

    #[test_case]
    fn test_page_mapcount() {
        let page = Page::new();

        // 初始映射计数为 -1（未映射）
        assert_eq!(page.mapcount(), -1);
        assert!(!page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 0);
        assert!(page.is_mapped());

        page.add_mapcount();
        assert_eq!(page.mapcount(), 1);

        page.sub_mapcount();
        assert_eq!(page.mapcount(), 0);
    }
}
