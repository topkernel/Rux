//! 页缓存 (Page Cache) 和数据块管理
//!
//! 完全遵循 Linux 内核的页缓存设计 (mm/page_io.c, include/linux/pagemap.h)
//!
//! 核心概念：
//! - `struct page`: 页面，表示一个内存页（通常 4KB）
//! - `struct address_space`: 地址空间，管理一个文件的所有页面
//! - `struct buffer_head`: 缓冲区头，用于块 I/O
//!
//! 简化实现：使用简单的字节缓冲区代替完整的页缓存

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 页面大小 (4KB，与标准架构一致)
pub const PAGE_SIZE: usize = 4096;

/// 页缓存条目
///
/// 对应 Linux 的 struct page (include/linux/mm_types.h)
#[repr(C)]
pub struct Page {
    /// 页面数据
    pub data: Vec<u8>,
    /// 页面状态
    pub flags: AtomicUsize,
    /// 引用计数
    pub ref_count: AtomicUsize,
}

impl Page {
    /// 创建新页面
    pub fn new() -> Self {
        let mut data = Vec::with_capacity(PAGE_SIZE);
        unsafe {
            data.set_len(PAGE_SIZE);
        }
        Self {
            data,
            flags: AtomicUsize::new(0),
            ref_count: AtomicUsize::new(1),
        }
    }

    /// 从数据创建页面
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut data = Vec::with_capacity(PAGE_SIZE);
        unsafe {
            data.set_len(PAGE_SIZE);
        }
        let copy_len = core::cmp::min(bytes.len(), PAGE_SIZE);
        data[..copy_len].copy_from_slice(&bytes[..copy_len]);

        Self {
            data,
            flags: AtomicUsize::new(0),
            ref_count: AtomicUsize::new(1),
        }
    }

    /// 读取页面数据
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= PAGE_SIZE {
            return 0;
        }
        let available = PAGE_SIZE - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.data[offset..offset + to_read]);
        to_read
    }

    /// 写入页面数据
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= PAGE_SIZE {
            return 0;
        }
        let available = PAGE_SIZE - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        to_write
    }

    /// 增加引用计数
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// 减少引用计数
    pub fn put(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }
}

/// 地址空间 - 管理文件的所有缓存页面
///
/// 对应 Linux 的 struct address_space (include/linux/fs.h)
pub struct AddressSpace {
    /// 页面树（简化为数组）
    /// 索引是页号（page index），值是页面
    pages: Mutex<Vec<Option<Box<Page>>>>,
    /// 文件大小（字节）
    size: AtomicUsize,
}

impl AddressSpace {
    /// 创建新的地址空间
    pub fn new() -> Self {
        Self {
            pages: Mutex::new(Vec::new()),
            size: AtomicUsize::new(0),
        }
    }

    /// 获取文件大小
    pub fn get_size(&self) -> usize {
        self.size.load(Ordering::Acquire)
    }

    /// 设置文件大小
    pub fn set_size(&self, size: usize) {
        self.size.store(size, Ordering::Release);
    }

    /// 读取文件数据
    ///
    /// 从指定偏移量读取数据到缓冲区
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        let file_size = self.get_size();
        if offset >= file_size {
            return 0;
        }

        let available = file_size - offset;
        let to_read = core::cmp::min(buf.len(), available);

        let mut total_read = 0;
        let mut current_offset = offset;
        let mut buf_offset = 0;

        while total_read < to_read {
            let page_index = current_offset / PAGE_SIZE;
            let page_offset = current_offset % PAGE_SIZE;

            let pages = self.pages.lock();
            if page_index >= pages.len() {
                break;
            }

            let remaining = to_read - total_read;

            if let Some(ref page) = pages[page_index] {
                let read_in_page = page.read(page_offset, &mut buf[buf_offset..buf_offset + remaining]);
                total_read += read_in_page;
                buf_offset += read_in_page;
                current_offset += read_in_page;

                if read_in_page == 0 {
                    break;
                }
            } else {
                // 页面不存在，视为零填充
                let page_end = ((page_index + 1) * PAGE_SIZE).min(file_size);
                let available_in_page = page_end - current_offset;
                let zero_len = core::cmp::min(available_in_page, remaining);
                buf[buf_offset..buf_offset + zero_len].fill(0);
                total_read += zero_len;
                buf_offset += zero_len;
                current_offset += zero_len;
            }
        }

        total_read
    }

    /// 写入文件数据
    ///
    /// 从缓冲区写入数据到指定偏移量
    pub fn write(&self, offset: usize, buf: &[u8]) -> usize {
        let mut total_written = 0;
        let mut current_offset = offset;
        let mut buf_offset = 0;

        while total_written < buf.len() {
            let page_index = current_offset / PAGE_SIZE;
            let page_offset = current_offset % PAGE_SIZE;

            let mut pages = self.pages.lock();

            // 确保页面存在
            while page_index >= pages.len() {
                pages.push(Some(Box::new(Page::new())));
            }

            if let Some(ref mut page) = pages[page_index] {
                let remaining = buf.len() - total_written;
                let written_in_page = page.write(page_offset, &buf[buf_offset..buf_offset + remaining]);
                total_written += written_in_page;
                buf_offset += written_in_page;
                current_offset += written_in_page;

                // 更新文件大小
                let new_size = self.size.load(Ordering::Acquire).max(current_offset);
                self.size.store(new_size, Ordering::Release);

                if written_in_page == 0 {
                    break;
                }
            } else {
                // 创建新页面
                pages[page_index] = Some(Box::new(Page::new()));
                drop(pages);
                continue;
            }
        }

        total_written
    }

    /// 从字节数据加载文件
    ///
    /// 用于从 ELF 或其他静态数据初始化文件
    pub fn load_from_bytes(&self, data: &[u8]) {
        let mut offset = 0;
        let chunk_size = PAGE_SIZE;

        while offset < data.len() {
            let remaining = data.len() - offset;
            let to_copy = core::cmp::min(remaining, chunk_size);

            let page_index = offset / PAGE_SIZE;
            let mut pages = self.pages.lock();

            while page_index >= pages.len() {
                pages.push(Some(Box::new(Page::new())));
            }

            if let Some(ref mut page) = pages[page_index] {
                let page_offset = offset % PAGE_SIZE;
                page.write(page_offset, &data[offset..offset + to_copy]);
            }

            offset += to_copy;
        }

        // 更新文件大小
        self.size.store(data.len(), Ordering::Release);
    }

    /// 截断文件到指定大小
    pub fn truncate(&self, new_size: usize) {
        let _old_size = self.size.swap(new_size, Ordering::AcqRel);

        // 释放超出新大小的页面
        let new_page_count = (new_size + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut pages = self.pages.lock();
        if pages.len() > new_page_count {
            pages.truncate(new_page_count);
        }
    }
}

impl Default for AddressSpace {
    fn default() -> Self {
        Self::new()
    }
}

/// 简单的文件缓冲区
///
/// 用于小文件的简单存储
pub struct FileBuffer {
    /// 数据
    pub data: Vec<u8>,
}

impl FileBuffer {
    /// 创建新的文件缓冲区
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
        }
    }

    /// 从字节数据创建
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: bytes.to_vec(),
        }
    }

    /// 读取数据
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.data.len() {
            return 0;
        }
        let available = self.data.len() - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.data[offset..offset + to_read]);
        to_read
    }

    /// 写入数据
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= self.data.len() {
            // 扩展缓冲区
            let new_len = offset + buf.len();
            self.data.resize(new_len, 0);
        }
        let available = self.data.len() - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        to_write
    }

    /// 获取大小
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl Default for FileBuffer {
    fn default() -> Self {
        Self::new()
    }
}
