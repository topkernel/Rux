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

/// FileBuffer chunk granularity (R76). Chunks are order-4 (64 KiB) page
/// allocations from the zone allocator when available — Linux serves
/// memfd/shmem contents from tmpfs pages, never from a contiguous kmalloc.
/// The pre-R76 single `Vec::resize` made a 64 MiB ftruncate (observed from
/// gnome-shell init: ALLOCTHROW size=67108864, bigalloc3 run) one oversized
/// heap block that the 16 MiB buddy MAX_ORDER cannot serve; the interim
/// chunked-heap version then exhausted the 128 MB kernel heap outright
/// (bigalloc5: ALLOCTHROW size=65536 with the heap full). Page-backed
/// chunks draw from the 2 GiB zone allocator instead.
pub const FILE_BUFFER_CHUNK: usize = 64 * 1024;

/// log2(FILE_BUFFER_CHUNK / PAGE_SIZE) — order passed to alloc_pages.
const CHUNK_ORDER: usize = 4;

/// One fixed-size chunk: a page-allocation when the zone allocator can
/// serve it, a heap Vec as the pre-zone-init / fallback path.
enum FileChunk {
    /// Physical address of an order-CHUNK_ORDER allocation.
    Page(usize),
    Heap(Vec<u8>),
}

impl FileChunk {
    fn new() -> Option<Self> {
        let phys = crate::mm::page_alloc::alloc_pages(
            crate::mm::zone::GfpFlags::GFP_KERNEL,
            CHUNK_ORDER,
        );
        if phys != 0 {
            return Some(FileChunk::Page(phys));
        }
        // Fallback (early boot before zone init, or zone exhausted):
        // heap chunk. May itself fail on an exhausted heap — the caller
        // treats None as ENOMEM.
        let mut c = Vec::with_capacity(FILE_BUFFER_CHUNK);
        unsafe {
            core::ptr::write_bytes(c.as_mut_ptr(), 0, FILE_BUFFER_CHUNK);
            c.set_len(FILE_BUFFER_CHUNK);
        }
        Some(FileChunk::Heap(c))
    }

    /// Base pointer of the chunk storage.
    ///
    /// # SAFETY: callers must not hold two live mutable slices into the
    /// same chunk (the FileBuffer lock serializes all access in practice).
    unsafe fn base_ptr(&mut self) -> *mut u8 {
        match self {
            FileChunk::Page(phys) => {
                crate::arch::mm::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr(*phys as u64),
                )
                .as_usize() as *mut u8
            }
            FileChunk::Heap(v) => v.as_mut_ptr(),
        }
    }
}

impl Drop for FileChunk {
    fn drop(&mut self) {
        if let FileChunk::Page(phys) = *self {
            crate::mm::page_alloc::free_pages(phys, CHUNK_ORDER);
        }
    }
}

pub struct FileBuffer {
    /// Logical length in bytes (chunks may hold zeros past it).
    len: usize,
    /// Fixed-size chunks covering [0, chunks.len()*CHUNK). All chunks
    /// up to ceil(len/CHUNK) are allocated; a shrink keeps them until the
    /// trailing ones are dropped.
    chunks: Vec<FileChunk>,
}

impl FileBuffer {
    /// One zeroed chunk, allocated without transiting the (small) kernel
    /// stack: with_capacity mmaps from the heap directly.
    fn new_chunk() -> Vec<u8> {
        let mut c = Vec::with_capacity(FILE_BUFFER_CHUNK);
        unsafe {
            core::ptr::write_bytes(c.as_mut_ptr(), 0, FILE_BUFFER_CHUNK);
            c.set_len(FILE_BUFFER_CHUNK);
        }
        c
    }

    /// Create new file buffer
    pub fn new() -> Self {
        Self {
            len: 0,
            chunks: Vec::new(),
        }
    }

    /// Create from byte data
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut fb = Self::new();
        fb.write(0, bytes);
        fb
    }

    /// Read data
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.len {
            return 0;
        }
        let available = self.len - offset;
        let to_read = core::cmp::min(buf.len(), available);
        let mut done = 0;
        while done < to_read {
            let pos = offset + done;
            let ci = pos / FILE_BUFFER_CHUNK;
            let co = pos % FILE_BUFFER_CHUNK;
            let n = core::cmp::min(FILE_BUFFER_CHUNK - co, to_read - done);
            // SAFETY: const cast of the chunk reference — read-only use;
            // all FileBuffer access is serialized by the inode data lock.
            let src: &[u8] = unsafe {
                let fc = &self.chunks[ci] as *const FileChunk as *mut FileChunk;
                let base = (*fc).base_ptr();
                core::slice::from_raw_parts(base.add(co), n)
            };
            buf[done..done + n].copy_from_slice(src);
            done += n;
        }
        to_read
    }

    /// Write data (extends the buffer like Linux when offset+len exceeds
    /// the current size, zero-filling any gap).
    pub fn write(&mut self, offset: usize, buf: &[u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        let end = offset + buf.len();
        if end > self.len {
            if self.resize(end).is_err() {
                // Could not grow (zone + heap both exhausted): write only
                // what the current extent holds, POSIX partial-write.
                if offset >= self.len {
                    return 0;
                }
            }
        }
        let limit = core::cmp::min(end, self.len);
        let writable = limit.saturating_sub(offset);
        let mut done = 0;
        while done < writable {
            let pos = offset + done;
            let ci = pos / FILE_BUFFER_CHUNK;
            let co = pos % FILE_BUFFER_CHUNK;
            let n = core::cmp::min(FILE_BUFFER_CHUNK - co, writable - done);
            // SAFETY: &mut access; serialized by the inode data lock.
            let dst: &mut [u8] = unsafe {
                let base = self.chunks[ci].base_ptr();
                core::slice::from_raw_parts_mut(base.add(co), n)
            };
            dst.copy_from_slice(&buf[done..done + n]);
            done += n;
        }
        done
    }

    /// Resize (ftruncate semantics: grow-with-zeros / shrink drops bytes).
    /// Growth allocates one chunk at a time; Err(ENOMEM) when the zone
    /// allocator and the heap both fail (R76).
    ///
    /// Shrink zeroes the tail of the kept last chunk: ftruncate(2) says the
    /// file "is extended, and the extended portion reads as null bytes" —
    /// a shrink-then-grow would otherwise serve the pre-shrink bytes back
    /// (the chunks are kept, only `len` bounds reads). Same discipline as
    /// TmpfsNode::truncate.
    pub fn resize(&mut self, new_len: usize) -> Result<(), ()> {
        let new_chunks = new_len.div_ceil(FILE_BUFFER_CHUNK);
        while self.chunks.len() < new_chunks {
            match FileChunk::new() {
                Some(c) => self.chunks.push(c),
                None => return Err(()),
            }
        }
        if new_len < self.len {
            // Shrink: drop whole trailing chunks, then zero the kept last
            // chunk's tail so a later re-extend exposes zeros, not stale
            // data.
            let keep = new_len.div_ceil(FILE_BUFFER_CHUNK);
            self.chunks.truncate(keep);
            let tail = new_len % FILE_BUFFER_CHUNK;
            if tail != 0 {
                if let Some(c) = self.chunks.last_mut() {
                    // SAFETY: &mut access to the chunk we own; the write
                    // stays inside the FILE_BUFFER_CHUNK-sized chunk.
                    unsafe {
                        let base = c.base_ptr();
                        core::ptr::write_bytes(
                            base.add(tail),
                            0,
                            FILE_BUFFER_CHUNK - tail,
                        );
                    }
                }
            }
        }
        self.len = new_len;
        Ok(())
    }

    /// Get size
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for FileBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ftruncate(2): "the extended portion reads as null bytes" — a
    /// shrink-then-grow of a FileBuffer (the memfd ftruncate path) must not
    /// resurrect the pre-shrink tail. Mirrors test_tmpfs_truncate_zero_tail.
    #[test]
    fn test_file_buffer_truncate_zero_tail() {
        let mut fb = FileBuffer::new();
        fb.write(0, &[0xAAu8; 64]);
        assert_eq!(fb.resize(16), Ok(()));
        assert_eq!(fb.len(), 16);
        assert_eq!(fb.resize(64), Ok(()));
        let mut buf = [0u8; 64];
        assert_eq!(fb.read(0, &mut buf), 64);
        assert!(buf[..16].iter().all(|&b| b == 0xAA));
        assert!(buf[16..].iter().all(|&b| b == 0));
    }
}
