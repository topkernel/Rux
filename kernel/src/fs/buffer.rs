//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Page Cache and data block management
//!
//!
//! Core concepts:
//! - `struct page`: Page, representing a memory page (typically 4KB)
//! - `struct address_space`: Address space, managing all pages of a file
//! - `struct buffer_head`: Buffer head, used for block I/O
//!
//! Simplified implementation: Uses simple byte buffers instead of full page cache

use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const PAGE_SIZE: usize = 4096;

#[repr(C)]
pub struct Page {
    /// Page data
    pub data: Vec<u8>,
    /// Page status
    pub flags: AtomicUsize,
    /// Reference count
    pub ref_count: AtomicUsize,
}

impl Page {
    /// Create new page
    pub fn new() -> Self {
        let mut data = Vec::with_capacity(PAGE_SIZE);
        unsafe {
            core::ptr::write_bytes(data.as_mut_ptr(), 0, PAGE_SIZE);
            data.set_len(PAGE_SIZE);
        }
        Self {
            data,
            flags: AtomicUsize::new(0),
            ref_count: AtomicUsize::new(1),
        }
    }

    /// Create page from data
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

    /// Read page data
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= PAGE_SIZE {
            return 0;
        }
        let available = PAGE_SIZE - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.data[offset..offset + to_read]);
        to_read
    }

    /// Write page data
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= PAGE_SIZE {
            return 0;
        }
        let available = PAGE_SIZE - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        to_write
    }

    /// Increment reference count
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn put(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::AcqRel) - 1
    }
}

pub struct AddressSpace {
    /// Page tree (simplified to array)
    /// Index is page number, value is page
    pages: Spinlock<Vec<Option<Box<Page>>>>,
    /// File size (bytes)
    size: AtomicUsize,
}

impl AddressSpace {
    /// Create new address space
    pub fn new() -> Self {
        Self {
            pages: Spinlock::new(Vec::new()),
            size: AtomicUsize::new(0),
        }
    }

    /// Get file size
    pub fn get_size(&self) -> usize {
        self.size.load(Ordering::Acquire)
    }

    /// Set file size
    pub fn set_size(&self, size: usize) {
        self.size.store(size, Ordering::Release);
    }

    /// Read file data
    ///
    /// Reads data from specified offset into buffer
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
                // Page does not exist, treat as zero-filled
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

    /// Write file data
    ///
    /// Writes data from buffer to specified offset
    pub fn write(&self, offset: usize, buf: &[u8]) -> usize {
        let mut total_written = 0;
        let mut current_offset = offset;
        let mut buf_offset = 0;

        while total_written < buf.len() {
            let page_index = current_offset / PAGE_SIZE;
            let page_offset = current_offset % PAGE_SIZE;

            let mut pages = self.pages.lock();

            // Ensure page exists
            while page_index >= pages.len() {
                pages.push(Some(Box::new(Page::new())));
            }

            if let Some(ref mut page) = pages[page_index] {
                let remaining = buf.len() - total_written;
                let written_in_page = page.write(page_offset, &buf[buf_offset..buf_offset + remaining]);
                total_written += written_in_page;
                buf_offset += written_in_page;
                current_offset += written_in_page;

                // Update file size
                let new_size = self.size.load(Ordering::Acquire).max(current_offset);
                self.size.store(new_size, Ordering::Release);

                if written_in_page == 0 {
                    break;
                }
            } else {
                // Create new page
                pages[page_index] = Some(Box::new(Page::new()));
                drop(pages);
                continue;
            }
        }

        total_written
    }

    /// Load file from byte data
    ///
    /// Used to initialize file from ELF or other static data
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

        // Update file size
        self.size.store(data.len(), Ordering::Release);
    }

    /// Truncate file to specified size
    pub fn truncate(&self, new_size: usize) {
        let _old_size = self.size.swap(new_size, Ordering::AcqRel);

        // Release pages beyond new size
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

pub struct FileBuffer {
    /// Data
    pub data: Vec<u8>,
}

impl FileBuffer {
    /// Create new file buffer
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
        }
    }

    /// Create from byte data
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: bytes.to_vec(),
        }
    }

    /// Read data
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.data.len() {
            return 0;
        }
        let available = self.data.len() - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&self.data[offset..offset + to_read]);
        to_read
    }

    /// Write data
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if offset >= self.data.len() {
            // Extend buffer
            let new_len = offset + buf.len();
            self.data.resize(new_len, 0);
        }
        let available = self.data.len() - offset;
        let to_write = core::cmp::min(buf.len(), available);
        self.data[offset..offset + to_write].copy_from_slice(&buf[..to_write]);
        to_write
    }

    /// Get size
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl Default for FileBuffer {
    fn default() -> Self {
        Self::new()
    }
}
