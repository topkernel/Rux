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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
    pub capacity: AtomicU64,
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
            capacity: AtomicU64::new(0),
            block_size,
            ops,
            private_data: None,
            request_fn: None,
            async_read_fn: None,
        }
    }

    /// Set capacity
    pub fn set_capacity(&self, sectors: u64) {
        self.capacity.store(sectors, Ordering::Release);
    }

    /// Get capacity
    pub fn get_capacity(&self) -> u64 {
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
    /// Final device status (R8-M2): set by the driver before end_io fires.
    /// submit_request returns 0 whenever a request_fn is registered, so a
    /// device-side error used to vanish and the caller copied a ZERO
    /// buffer as success (cached as BH_Uptodate!).
    pub error: core::sync::atomic::AtomicI32,
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
        // SAFETY: disk is a valid pointer registered via register_disk; caller ensures it remains valid.
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
// SAFETY: disk is a valid GenDisk pointer from register_disk; completion outlives the async operation.
/// Per-device I/O statistics (Linux's /proc/diskstats counters).
pub mod iostat;

pub fn blkdev_read_async(
    disk: *const GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    unsafe {
        let gd = &*disk;
        if let Some(async_fn) = gd.async_read_fn {
            // /proc/diskstats: the async path does not go through
            // submit_request, so it is counted here.
            let major = gd.major;
            let minor = gd.first_minor;
            let sectors = (buf.len() / 512) as u64;
            let t0 = crate::drivers::timer::read_time();
            // block:block_rq_issue (async read): the completion lands
            // later in the device's IRQ walker, which carries the
            // geometry the submit stores in the pending entry.
            if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
                // SAFETY: current task's comm is read for the record only.
                let comm = crate::sched::current().map(|cur| (*cur).comm());
                if let Some(comm) = comm {
                    crate::trace::trace_block_rq_issue(
                        (major << 20) | minor,
                        sector,
                        sectors as u32,
                        buf.len() as u32,
                        0,
                        "R",
                        comm,
                    );
                }
            }
            let ret = async_fn(disk, sector, buf, completion as *const _ as *mut _);
            let elapsed_ns = crate::drivers::timer::read_time().saturating_sub(t0) * 100
                / crate::config::TIMER_CLOCK_FREQ_HZ;
            iostat::note_io(major, minor, false, sectors, elapsed_ns);
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
    // /proc/diskstats lists every registered device.
    iostat::register_device(disk.major, disk.first_minor, disk.name);
    BLOCK_MANAGER.register_disk(disk)
}

pub fn get_disk(major: u32) -> Option<*const GenDisk> {
    BLOCK_MANAGER.get_disk(major)
}

pub fn submit_request(disk: *const GenDisk, req: &mut Request) -> i32 {
    // /proc/diskstats: count the request and the time it took. The
    // request carries the transfer size, so the sector count is exact.
    let major = unsafe { (*disk).major };
    let minor = unsafe { (*disk).first_minor };
    let is_write = matches!(req.cmd_type, ReqCmd::Write);
    let sectors = (req.buffer.len() / 512) as u64;
    let t0 = crate::drivers::timer::read_time();
    iostat::note_start(major, minor);
    // block:block_rq_issue / block_rq_complete — the synchronous
    // request path: the registered request_fn completes the transfer
    // before it returns, so issue is recorded at entry and complete
    // right after the device status is known. Both record calls carry
    // their armed() check inside; the comm copy is marshalled only
    // behind the issue event's own check.
    if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
        // SAFETY: current task's comm is read for the record only.
        let comm = crate::sched::current().map(|cur| unsafe { (*cur).comm() });
        if let Some(comm) = comm {
            crate::trace::trace_block_rq_issue(
                (major << 20) | minor,
                req.sector,
                sectors as u32,
                req.buffer.len() as u32,
                0,
                req_rwbs(&req.cmd_type),
                comm,
            );
        }
    }
    let ret = BLOCK_MANAGER.submit_request(disk, req);
    if crate::trace::EVENT_BLOCK_RQ_COMPLETE.armed() {
        // The device status (R8-M2: submit_request returns 0 whenever a
        // request_fn is registered — the real completion status is the
        // request's error word).
        let error = req.error.load(core::sync::atomic::Ordering::Acquire);
        crate::trace::trace_block_rq_complete(
            (major << 20) | minor,
            req.sector,
            sectors as u32,
            error,
            0,
            req_rwbs(&req.cmd_type),
        );
    }
    iostat::note_end(major, minor);
    let elapsed_ns = crate::drivers::timer::read_time().saturating_sub(t0) * 100
        / crate::config::TIMER_CLOCK_FREQ_HZ;
    iostat::note_io(major, minor, is_write, sectors, elapsed_ns);
    ret
}

/// The rwbs column of the block events: the transfer direction only
/// (the reference's sync/meta bits have no counterpart here — Rux
/// merges no requests and passes no write hints).
fn req_rwbs(cmd: &ReqCmd) -> &'static str {
    match cmd {
        ReqCmd::Read => "R",
        ReqCmd::Write => "W",
        ReqCmd::Flush => "F",
    }
}

pub fn blkdev_read(disk: *const GenDisk, sector: u64, buf: &mut [u8]) -> Result<usize, i32> {
    // SAFETY: disk is a valid GenDisk pointer from register_disk; req is a local Request.
    unsafe {
        let _gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Read,
            sector,
            buffer: vec![0u8; buf.len()],
            device: disk,
            end_io: None,
            completion: None,
            error: core::sync::atomic::AtomicI32::new(0),
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }
        // R8-M2: device-side errors reach the caller (was swallowed).
        let dev_err = req.error.load(core::sync::atomic::Ordering::Acquire);
        if dev_err != 0 {
            return Err(dev_err);
        }

        // Copy data
        buf.copy_from_slice(&req.buffer);
        Ok(buf.len())
    }
}

pub fn blkdev_write(disk: *const GenDisk, sector: u64, buf: &[u8]) -> Result<usize, i32> {
    // SAFETY: disk is a valid GenDisk pointer from register_disk; req is a local Request.
    unsafe {
        let _gd = &*disk;

        let mut req = Request {
            cmd_type: ReqCmd::Write,
            sector,
            buffer: buf.to_vec(),
            device: disk,
            end_io: None,
            completion: None,
            error: core::sync::atomic::AtomicI32::new(0),
        };

        let ret = submit_request(disk, &mut req);
        if ret < 0 {
            return Err(ret);
        }
        // R9-4: device-side errors reach the caller (read path had this
        // since R8-M2a; without it a failed write was marked clean by
        // BufferHead::sync and swap-out silently discarded page data).
        let dev_err = req.error.load(core::sync::atomic::Ordering::Acquire);
        if dev_err != 0 {
            return Err(dev_err);
        }

        Ok(buf.len())
    }
}
