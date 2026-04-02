//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Block device driver layer
//!
//! Core concepts:
//! - `struct gendisk`: Block device representation
//! - `struct block_device`: Block device instance
//! - `struct request_queue`: Request queue
//! - `struct bio`: I/O descriptor

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
pub struct BlockDeviceOps {
    /// Open block device
    pub open: Option<unsafe fn() -> i32>,
    /// Release block device
    pub release: Option<unsafe fn() -> i32>,
    /// Get geometry info
    pub getgeo: Option<unsafe fn(&mut Geo) -> i32>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Geo {
    /// Number of heads
    pub heads: u8,
    /// Number of sectors
    pub sectors: u8,
    /// Number of cylinders
    pub cylinders: u16,
    /// Start position
    pub start: u32,
}

pub struct GenDisk {
    /// Device name
    pub name: &'static str,
    /// Major device number
    pub major: u32,
    /// First minor device number
    pub first_minor: u32,
    /// Number of minor device numbers
    pub minors: u32,
    /// Capacity (in 512-byte sectors)
    pub capacity: AtomicU32,
    /// Block size
    pub block_size: u32,
    /// Block device operations
    pub ops: Option<&'static BlockDeviceOps>,
    /// Private data
    pub private_data: Option<*mut u8>,
    /// Request handler function
    pub request_fn: Option<unsafe extern "C" fn(&mut Request)>,
    /// Async read function (device-specific async submit without blocking)
    pub async_read_fn: Option<unsafe fn(*const GenDisk, u64, &mut [u8], *mut core::ffi::c_void) -> i32>,
}

unsafe impl Send for GenDisk {}
unsafe impl Sync for GenDisk {}

impl GenDisk {
    /// Create new block device
    pub fn new(
        name: &'static str,
        major: u32,
        minors: u32,
        block_size: u32,
        ops: Option<&'static BlockDeviceOps>,
    ) -> Self {
        Self {
            name,
            major,
            first_minor: 0,
            minors,
            capacity: AtomicU32::new(0),
            block_size,
            ops,
            private_data: None,
            request_fn: None,
            async_read_fn: None,
        }
    }

    /// Set capacity
    pub fn set_capacity(&self, sectors: u32) {
        self.capacity.store(sectors, Ordering::Release);
    }

    /// Get capacity
    pub fn get_capacity(&self) -> u32 {
        self.capacity.load(Ordering::Acquire)
    }

    /// Set private data
    pub fn set_private_data(&mut self, data: *mut u8) {
        self.private_data = Some(data);
    }

    /// Set request handler function
    pub fn set_request_fn(&mut self, f: unsafe extern "C" fn(&mut Request)) {
        self.request_fn = Some(f);
    }

    /// Set async read function (for async I/O submission without blocking)
    pub fn set_async_read_fn(&mut self, f: unsafe fn(*const GenDisk, u64, &mut [u8], *mut core::ffi::c_void) -> i32) {
        self.async_read_fn = Some(f);
    }
}

pub struct Request {
    /// Command type
    pub cmd_type: ReqCmd,
    /// Starting sector
    pub sector: u64,
    /// Data buffer
    pub buffer: Vec<u8>,
    /// Block device pointer
    pub device: *const GenDisk,
    /// Completion callback
    pub end_io: Option<unsafe fn(&Request, i32)>,
    /// Async I/O completion token (set by async submit paths)
    pub completion: Option<*mut core::ffi::c_void>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReqCmd {
    /// Read
    Read,
    /// Write
    Write,
    /// Flush
    Flush,
}

struct BlockDeviceManager {
    /// Block device list
    disks: Spinlock<Vec<Option<Box<GenDisk>>>>,
    /// Device number allocator
    major_next: AtomicU32,
}

unsafe impl Send for BlockDeviceManager {}
unsafe impl Sync for BlockDeviceManager {}

impl BlockDeviceManager {
    const fn new() -> Self {
        Self {
            disks: Spinlock::new(Vec::new()),
            major_next: AtomicU32::new(1),
        }
    }

    /// Register block device
    pub fn register_disk(&self, disk: Box<GenDisk>) -> Result<(), &'static str> {
        let mut disks = self.disks.lock();

        // Check if device number is already in use
        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == disk.major {
                    return Err("Major number already in use");
                }
            }
        }

        disks.push(Some(disk));
        Ok(())
    }

    /// Find block device
    pub fn get_disk(&self, major: u32) -> Option<*const GenDisk> {
        let disks = self.disks.lock();

        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == major {
                    return Some(gd.as_ref() as *const GenDisk);
                }
            }
        }

        None
    }

    /// Submit I/O request
    pub fn submit_request(&self, disk: *const GenDisk, req: &mut Request) -> i32 {
        unsafe {
            let gd = &*disk;

            if let Some(request_fn) = gd.request_fn {
                request_fn(req);
                0  // Success
            } else {
                -6  // ENXIO
            }
        }
    }
}

/// Submit an async block read. Returns immediately; completion is signaled later via interrupt.
///
/// # Arguments
/// * `disk` - Block device
/// * `sector` - Starting sector (512-byte units)
/// * `buf` - Data buffer (must remain valid until completion)
/// * `completion` - IoCompletion to signal on completion
///
/// # Returns
/// Ok(()) if submitted successfully, Err on failure.
pub fn blkdev_read_async(
    disk: *const GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    unsafe {
        let gd = &*disk;
        if let Some(async_fn) = gd.async_read_fn {
            let ret = async_fn(disk, sector, buf, completion as *const _ as *mut _);
            if ret < 0 {
                Err(ret)
            } else {
                Ok(())
            }
        } else {
            Err(-6)  // ENXIO — device doesn't support async I/O
        }
    }
}

static BLOCK_MANAGER: BlockDeviceManager = BlockDeviceManager::new();

pub fn register_disk(disk: Box<GenDisk>) -> Result<(), &'static str> {
    BLOCK_MANAGER.register_disk(disk)
}

pub fn get_disk(major: u32) -> Option<*const GenDisk> {
    BLOCK_MANAGER.get_disk(major)
}

pub fn submit_request(disk: *const GenDisk, req: &mut Request) -> i32 {
    BLOCK_MANAGER.submit_request(disk, req)
}

pub fn blkdev_read(disk: *const GenDisk, sector: u64, buf: &mut [u8]) -> Result<usize, i32> {
    unsafe {
        let _gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Read,
            sector,
            buffer: vec![0u8; buf.len()],
            device: disk,
            end_io: None,
            completion: None,
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }

        // Copy data
        buf.copy_from_slice(&req.buffer);
        Ok(buf.len())
    }
}

pub fn blkdev_write(disk: *const GenDisk, sector: u64, buf: &[u8]) -> Result<usize, i32> {
    unsafe {
        let _gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Write,
            sector,
            buffer: buf.to_vec(),
            device: disk,
            end_io: None,
            completion: None,
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }

        Ok(buf.len())
    }
}
