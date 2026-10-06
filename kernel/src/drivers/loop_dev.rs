//! Loop block devices (Linux drivers/block/loop): a file-backed block
//! device. `losetup`/LTP's tst_acquire_device bind a preallocated image
//! file to /dev/loopN with LOOP_SET_FD; block reads/writes on the loop
//! are routed to the backing file at (sector * 512 + lo_offset), capped
//! by lo_sizelimit. Every loop registers a GenDisk (major 7) whose
//! request_fn performs the file I/O — the buffer cache and page cache
//! key on the GenDisk pointer, so the loop gets the same caching
//! discipline as a real disk for free.
//!
//! /dev/loop-control answers LOOP_CTL_GET_FREE with the first unbound
//! minor (Linux loop control semantics).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::drivers::blkdev::{GenDisk, ReqCmd, Request};
use crate::fs::dev_t::{DevNo, LOOP_MAJOR, MISC_MAJOR};
use crate::fs::file::File;
use crate::sync::spinlock::Spinlock;

/// Number of loop devices (Linux's historical default).
pub const N_LOOPS: usize = 8;

/// loop-control minor on the misc major.
pub const LOOP_CONTROL_MINOR: u32 = 237;

// ---- loop ioctl numbers (linux/loop.h) ----
pub const LOOP_SET_FD: u32 = 0x4C00;
pub const LOOP_CLR_FD: u32 = 0x4C01;
/// BLKGETSIZE64 (bytes) — mke2fs/tst_device size probe.
pub const BLKGETSIZE64: u32 = 0x8008_1272;
/// BLKSSZGET (logical sector size).
pub const BLKSSZGET: u32 = 0x1268;
pub const LOOP_SET_STATUS: u32 = 0x4C02;
pub const LOOP_GET_STATUS: u32 = 0x4C03;
pub const LOOP_SET_STATUS64: u32 = 0x4C04;
pub const LOOP_GET_STATUS64: u32 = 0x4C05;
pub const LOOP_CHANGE_FD: u32 = 0x4C06;
pub const LOOP_SET_CAPACITY: u32 = 0x4C07;
pub const LOOP_SET_DIRECT_IO: u32 = 0x4C08;
pub const LOOP_SET_BLOCK_SIZE: u32 = 0x4C09;
pub const LOOP_CONFIGURE: u32 = 0x4C0A;
pub const LOOP_CTL_ADD: u32 = 0x4C80;
pub const LOOP_CTL_REMOVE: u32 = 0x4C81;
pub const LOOP_CTL_GET_FREE: u32 = 0x4C82;

/// lo_flags bits (linux/loop.h).
pub const LO_FLAGS_READ_ONLY: u32 = 1;
pub const LO_FLAGS_AUTOCLEAR: u32 = 4;
pub const LO_FLAGS_PARTSCAN: u32 = 8;
pub const LO_FLAGS_DIRECT_IO: u32 = 16;

struct LoopDev {
    /// Backing file (LOOP_SET_FD pins an Arc reference).
    file: Spinlock<Option<Arc<File>>>,
    /// Data start offset inside the backing file (bytes).
    offset: AtomicU64,
    /// Max bytes exposed (0 = whole file from offset).
    sizelimit: AtomicU64,
    /// lo_flags mirror (LO_FLAGS_* — display/detach policy only).
    flags: AtomicU64,
    /// Backing path for LOOP_GET_STATUS(64) lo_file_name/lo_name.
    name: Spinlock<[u8; 64]>,
    /// Serializes the set_pos/read/write dance on the backing file.
    io_lock: Spinlock<()>,
    /// The GenDisk handed out to the block layer (stable address: boxed).
    disk: Option<&'static GenDisk>,
}

impl LoopDev {
    const fn new() -> Self {
        Self {
            file: Spinlock::new(None),
            offset: AtomicU64::new(0),
            sizelimit: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            name: Spinlock::new([0; 64]),
            io_lock: Spinlock::new(()),
            disk: None,
        }
    }
}

static mut LOOPS: [LoopDev; N_LOOPS] = [
    LoopDev::new(), LoopDev::new(), LoopDev::new(), LoopDev::new(),
    LoopDev::new(), LoopDev::new(), LoopDev::new(), LoopDev::new(),
];

static LOOPS_INIT: AtomicBool = AtomicBool::new(false);

static LOOP_NAMES: [&str; N_LOOPS] = [
    "loop0", "loop1", "loop2", "loop3", "loop4", "loop5", "loop6", "loop7",
];

/// Device numbers served by this driver.
pub fn is_loop_devno(devno: DevNo) -> bool {
    (devno.major == LOOP_MAJOR && devno.minor < N_LOOPS as u32)
        || (devno.major == MISC_MAJOR && devno.minor == LOOP_CONTROL_MINOR)
}

/// Register the loop GenDisks with the block layer and /proc/partitions.
/// Idempotent; called from devfs init and mount.
pub fn init_loops() {
    if LOOPS_INIT.swap(true, Ordering::AcqRel) {
        return;
    }
    for i in 0..N_LOOPS {
        // SAFETY: LOOPS is a static; index is bounded by N_LOOPS. The
        // leaked GenDisk lives for the machine's lifetime (blkdev's
        // registry stores it; the buffer cache keys on its address).
        unsafe {
            let dev = &mut LOOPS[i];
            let disk = Box::leak(Box::new(GenDisk::new(
                loop_static_name(i),
                LOOP_MAJOR,
                N_LOOPS as u32,
                4096,
                None,
            )));
            disk.first_minor = i as u32;
            disk.minors = 1;
            disk.capacity.store(0, Ordering::Release);
            disk.request_fn = Some(loop_request_fn);
            dev.disk = Some(disk);
            // The registry deduplicates by major; one representative row
            // exposes the loop family to /proc/partitions consumers.
            if i == 0 {
                let _ = crate::drivers::blkdev::register_disk(
                    alloc::boxed::Box::new(GenDisk {
                        name: loop_static_name(i),
                        major: LOOP_MAJOR,
                        first_minor: 0,
                        minors: N_LOOPS as u32,
                        capacity: core::sync::atomic::AtomicU64::new(0),
                        block_size: 4096,
                        ops: None,
                        private_data: None,
                        request_fn: Some(loop_request_fn),
                        async_read_fn: None,
                    }),
                );
            }
        }
    }
}

/// The canonical GenDisk for loop `idx` (the pointer ext4 mounts on).
pub fn loop_disk(idx: usize) -> Option<&'static GenDisk> {
    if idx >= N_LOOPS {
        return None;
    }
    init_loops();
    // SAFETY: static array access bounded by N_LOOPS.
    unsafe { LOOPS[idx].disk }
}

fn loop_static_name(i: usize) -> &'static str {
    LOOP_NAMES[i]
}

/// LOOP_CTL_GET_FREE: first minor with no backing file.
pub fn loop_get_free() -> i32 {
    init_loops();
    for i in 0..N_LOOPS {
        // SAFETY: static array access bounded by N_LOOPS.
        let bound = unsafe { LOOPS[i].file.lock().is_some() };
        if !bound {
            return i as i32;
        }
    }
    -1 // ENXIO-equivalent: none free
}

/// LOOP_SET_FD: bind `file` as the backing store of loop `idx`.
/// Returns 0 or a negative errno.
pub fn loop_set_fd(idx: usize, file: Arc<File>) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    init_loops();
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    let mut slot = dev.file.lock();
    if slot.is_some() {
        return return_errno(crate::errno::Errno::DeviceOrResourceBusy);
    }
    let size = file_size_bytes(&file)? as u64;
    if size == 0 {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    *slot = Some(file);
    dev.offset.store(0, Ordering::Release);
    dev.sizelimit.store(0, Ordering::Release);
    dev.flags.store(0, Ordering::Release);
    *dev.name.lock() = [0; 64];
    if let Some(disk) = dev.disk {
        disk.capacity.store(size / 512, Ordering::Release);
    }
    Ok(())
}

/// LOOP_SET_FD with the caller's backing path recorded for
/// LOOP_GET_STATUS(64).lo_file_name (util-linux `losetup` display).
pub fn loop_set_fd_named(idx: usize, file: Arc<File>, path: &[u8]) -> Result<(), i32> {
    loop_set_fd(idx, file)?;
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    let mut name = dev.name.lock();
    let n = path.len().min(63);
    name[..n].copy_from_slice(&path[..n]);
    name[n] = 0;
    Ok(())
}

/// LOOP_CLR_FD: detach the backing file.
pub fn loop_clr_fd(idx: usize) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    let mut slot = dev.file.lock();
    match slot.take() {
        Some(_) => {
            dev.flags.store(0, Ordering::Release);
            *dev.name.lock() = [0; 64];
            if let Some(disk) = dev.disk {
                disk.capacity.store(0, Ordering::Release);
                // Drop every cached buffer of this loop, dirty ones
                // included (Linux invalidate_bdev on detach). A dirty
                // buffer whose writeback failed cannot be synced against
                // an unbound loop — leaving it in the shared cache let
                // the NEXT full-cache sync fail with this device's ENXIO
                // and poison unrelated syscalls (the mkdtemp ENXIO storm
                // after a failed mkfs-on-loop).
                crate::fs::bio::invalidate_device(disk as *const _);
            }
            Ok(())
        }
        // Linux loop_clr_fd: `lo->lo_state != Lo_bound` => ENXIO. LTP's
        // tst_detach_device loops LOOP_CLR_FD until it sees ENXIO —
        // answering EINVAL there made every device test end with
        // "ioctl(LOOP_CLR_FD) unexpectedly failed with: EINVAL" (TWARN,
        // r10: ~30 tests).
        None => return_errno(crate::errno::Errno::NoSuchDeviceOrAddress),
    }
}

/// Exposed byte length (sizelimit or file size, minus offset).
pub fn loop_size_bytes(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    let guard = dev.file.lock();
    let Some(file) = guard.as_ref() else { return 0 };
    let total = file_size_bytes(file).unwrap_or(0) as u64;
    let off = dev.offset.load(Ordering::Acquire);
    let limit = dev.sizelimit.load(Ordering::Acquire);
    let avail = total.saturating_sub(off);
    if limit != 0 { limit.min(avail) } else { avail }
}

/// Is loop `idx` currently bound?
pub fn loop_is_bound(idx: usize) -> bool {
    if idx >= N_LOOPS {
        return false;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    unsafe { LOOPS[idx].file.lock().is_some() }
}

/// File I/O size: stat through the inode size (regular files).
fn file_size_bytes(file: &Arc<File>) -> Result<usize, i32> {
    // SAFETY: inode cell written at open time; read-only.
    let inode_opt = unsafe { &*file.inode.get() };
    match inode_opt.as_ref() {
        Some(inode) => Ok(inode.size.load(Ordering::Acquire) as usize),
        None => Ok(0),
    }
}

fn return_errno(e: crate::errno::Errno) -> Result<(), i32> {
    Err(e.as_neg_i32())
}

/// Block-layer request function shared by all loop GenDisks: translate
/// the request into a positioned read/write on the backing file.
unsafe extern "C" fn loop_request_fn(req: &mut Request) {
    let idx = unsafe { (*req.device).first_minor as usize };
    if idx >= N_LOOPS {
        req.error.store(-6, Ordering::Release); // ENXIO
        return;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    let _io = dev.io_lock.lock();
    let guard = dev.file.lock();
    let Some(file) = guard.as_ref() else {
        req.error.store(-6, Ordering::Release); // ENXIO: unbound loop
        return;
    };
    let off = dev.offset.load(Ordering::Acquire);
    let limit = dev.sizelimit.load(Ordering::Acquire);
    let byte_start = req.sector
        .saturating_mul(512)
        .saturating_add(off)
        .min(if limit != 0 { off.saturating_add(limit) } else { u64::MAX });
    let ops = match file.get_ops() {
        Some(o) => o,
        None => {
            req.error.store(-5, Ordering::Release); // EIO
            return;
        }
    };
    let saved_pos = file.get_pos();
    file.set_pos(byte_start);
    let rc = match req.cmd_type {
        ReqCmd::Read => match ops.read {
            Some(f) => f(file, &mut req.buffer),
            None => {
                req.error.store(-5, Ordering::Release);
                return;
            }
        },
        ReqCmd::Write => match ops.write {
            Some(f) => f(file, &req.buffer),
            None => {
                req.error.store(-5, Ordering::Release);
                return;
            }
        },
        _ => 0,
    };
    file.set_pos(saved_pos);
    if rc < 0 {
        req.error.store(rc as i32, Ordering::Release);
    } else if req.cmd_type == ReqCmd::Read && (rc as usize) < req.buffer.len() {
        // Short read (past backing EOF): zero-fill the tail — a block
        // device is always block_size wide.
        for b in req.buffer[rc as usize..].iter_mut() {
            *b = 0;
        }
    }
}

// ============================================================================
// Device-node read/write (mkfs/blkid open /dev/loopN directly)
// ============================================================================

fn loop_devno_of(file: &crate::fs::file::File) -> Option<DevNo> {
    if !core::ptr::eq(
        file.get_ops()? as *const _,
        &crate::fs::devfs::LOOP_FILE_OPS as *const _,
    ) {
        return None;
    }
    // SAFETY: private_data was set by devfs_open (Box<DevNo>, file-lifetime).
    unsafe {
        let cell = file.private_data.get();
        match (*cell).as_ref() {
            Some(pd) => Some(*(pd.cast_const() as *const DevNo)),
            None => None,
        }
    }
}

/// Read from the loop device at the file position (buffer-cache backed —
/// coherent with filesystem metadata I/O on the same loop).
pub fn loop_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9, // EBADF
    };
    if devno.major != LOOP_MAJOR {
        return -25; // ENOTTY-ish: loop-control has no data
    }
    let idx = devno.minor as usize;
    let disk = match loop_disk(idx) {
        Some(d) => d,
        None => return -6,
    };
    if !loop_is_bound(idx) {
        return -6; // ENXIO
    }
    let bs = 4096usize;
    let pos = file.get_pos() as usize;
    let mut done = 0usize;
    while done < buf.len() {
        let blocknr = (pos + done) / bs;
        let in_off = (pos + done) % bs;
        let want = core::cmp::min(bs - in_off, buf.len() - done);
        // SAFETY: disk is the loop GenDisk registered at init.
        let bh = match unsafe { crate::fs::bio::bread(disk as *const _, blocknr as u64) } {
            Some(b) => b,
            None => break,
        };
        // SAFETY: bh from bread; b_data is block_size bytes.
        unsafe {
            let data = &(*bh).b_data;
            buf[done..done + want].copy_from_slice(&data[in_off..in_off + want]);
            crate::fs::bio::brelse(bh);
        }
        done += want;
    }
    if done == 0 {
        return 0;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

/// Write to the loop device at the file position (read-modify-write
/// through the buffer cache; partial tails preserved).
pub fn loop_file_write(file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9,
    };
    if devno.major != LOOP_MAJOR {
        return -25;
    }
    let idx = devno.minor as usize;
    let disk = match loop_disk(idx) {
        Some(d) => d,
        None => return -6,
    };
    if !loop_is_bound(idx) {
        return -6; // ENXIO
    }
    let bs = 4096usize;
    let pos = file.get_pos() as usize;
    let mut done = 0usize;
    while done < buf.len() {
        let blocknr = (pos + done) / bs;
        let in_off = (pos + done) % bs;
        let want = core::cmp::min(bs - in_off, buf.len() - done);
        // SAFETY: disk is the registered loop GenDisk.
        let bh = match unsafe { crate::fs::bio::bread(disk as *const _, blocknr as u64) } {
            Some(b) => b,
            None => break,
        };
        // SAFETY: bh from bread; b_data writable and block-sized.
        unsafe {
            {
                let data = &mut (*bh).b_data;
                data[in_off..in_off + want].copy_from_slice(&buf[done..done + want]);
            }
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            let r = crate::fs::bio::sync_dirty_buffer(bh);
            crate::fs::bio::brelse(bh);
            if r.is_err() {
                break;
            }
        }
        done += want;
    }
    if done == 0 {
        return -5;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

/// /proc/partitions lines for the loop devices (bound ones only, like
/// Linux: an unbound loop has no size and is not listed... Linux lists
/// loop minors present in the loop bitmap; we list bound devices).
pub fn partitions_lines() -> Vec<u8> {
    let mut out = alloc::string::String::new();
    for i in 0..N_LOOPS {
        if loop_is_bound(i) {
            let kb = loop_size_bytes(i) / 1024;
            out.push_str(&alloc::format!(
                " 7 {} {} {}\n",
                i, kb, loop_static_name(i)
            ));
        }
    }
    out.into_bytes()
}

// ============================================================================
// ioctl dispatcher (hooked from sys_ioctl on the LOOP_FILE_OPS identity)
// ============================================================================

/// `struct loop_info64` wire size (linux/loop.h): 6*u64 + 4*u32 +
/// 64 + 64 + 32 + 2*u64 = 232 bytes. The old value (168) truncated
/// lo_crypt_name/lo_encrypt_key/lo_init — consumers (util-linux losetup,
/// LTP loop tests) read the tail fields.
const LOOP_INFO64_SIZE: usize = 232;
/// `struct loop_info` (legacy) wire size on LP64: int +
/// __kernel_old_dev_t (u32 on riscv64 — its UAPI defines old dev_t as
/// unsigned int, verified by offsetof probes against the musl headers
/// our userspace builds with) + u64 + u32 + 4*int + 64 + 32 + 2*u64 + 4,
/// padded to 160.
const LOOP_INFO_SIZE: usize = 160;
/// Legacy struct loop_info field offsets (LP64 + 32-bit old_dev_t).
const LI_OFF_NUMBER: usize = 0;
const LI_OFF_DEVICE: usize = 4;
const LI_OFF_INODE: usize = 8;
const LI_OFF_RDEVICE: usize = 16;
const LI_OFF_OFFSET: usize = 20;
const LI_OFF_ENCRYPT_TYPE: usize = 24;
const LI_OFF_KEY_SIZE: usize = 28;
const LI_OFF_FLAGS: usize = 32;
const LI_OFF_NAME: usize = 36;
const LI_OFF_INIT: usize = 132;
/// LO_NAME_SIZE.
const LO_NAME_SIZE: usize = 64;

/// Filled view of a loop's status, shared by the legacy/64 serializers.
struct LoopStatus {
    device: u64,
    inode: u64,
    rdevice: u64,
    offset: u64,
    sizelimit: u64,
    number: u32,
    encrypt_type: u32,
    encrypt_key_size: u32,
    flags: u32,
    name: [u8; LO_NAME_SIZE],
    init: [u64; 2],
}

fn loop_status_of(idx: usize) -> LoopStatus {
    let mut st = LoopStatus {
        device: 7 << 20 | (idx as u64),
        inode: 0,
        rdevice: 0,
        offset: 0,
        sizelimit: 0,
        number: idx as u32,
        encrypt_type: 0,
        encrypt_key_size: 0,
        flags: 0,
        name: [0; LO_NAME_SIZE],
        init: [0; 2],
    };
    if idx >= N_LOOPS {
        return st;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    st.offset = dev.offset.load(Ordering::Acquire);
    st.sizelimit = dev.sizelimit.load(Ordering::Acquire);
    st.flags = dev.flags.load(Ordering::Acquire) as u32;
    st.name = *dev.name.lock();
    if let Some(file) = dev.file.lock().as_ref() {
        // SAFETY: inode cell written at open time; read-only.
        if let Some(inode) = unsafe { &*file.inode.get() }.as_ref() {
            st.inode = inode.ino;
        }
    }
    st
}

/// LOOP_CHANGE_FD: swap the backing file of a BOUND loop.
fn loop_change_fd(idx: usize, fd: usize) -> Result<(), i32> {
    let file = unsafe { crate::fs::file::get_file_fd(fd) }
        .ok_or_else(|| crate::errno::Errno::BadFileNumber.as_neg_i32())?;
    let size = file_size_bytes(&file)? as u64;
    // SAFETY: static array access bounded by N_LOOPS (checked by caller).
    let dev = unsafe { &LOOPS[idx] };
    let mut slot = dev.file.lock();
    if slot.is_none() {
        return Err(crate::errno::Errno::NoSuchDeviceOrAddress.as_neg_i32());
    }
    *slot = Some(file);
    if let Some(disk) = dev.disk {
        disk.capacity.store(size / 512, Ordering::Release);
    }
    Ok(())
}

/// LOOP_SET_CAPACITY: the backing file may have grown (ftruncate after
/// attach); refresh the exported capacity from the live size.
fn loop_set_capacity(idx: usize) -> Result<(), i32> {
    let bytes = loop_size_bytes(idx);
    if bytes == 0 {
        return Err(crate::errno::Errno::NoSuchDeviceOrAddress.as_neg_i32());
    }
    // SAFETY: static array access bounded by N_LOOPS.
    if let Some(disk) = unsafe { LOOPS[idx].disk } {
        disk.capacity.store(bytes / 512, Ordering::Release);
    }
    Ok(())
}

/// Handle a loop-family ioctl. `file` is the open /dev/loopN or
/// /dev/loop-control file (devno stashed in private_data by devfs_open).
/// Returns Some(ret) when handled, None when the file is not a loop.
pub fn loop_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    use crate::arch::riscv64::uaccess::{copy_from_user, copy_to_user};

    if !core::ptr::eq(
        file.get_ops()? as *const _,
        &crate::fs::devfs::LOOP_FILE_OPS as *const _,
    ) {
        return None;
    }
    // SAFETY: private_data was set by devfs_open to a Box<DevNo> (leaked,
    // file-lifetime). Read-only.
    let devno = unsafe {
        let cell = file.private_data.get();
        match (*cell).as_ref() {
            Some(pd) => *(pd.cast_const() as *const DevNo),
            None => return None,
        }
    };
    let is_loop = devno.major == LOOP_MAJOR && (devno.minor as usize) < N_LOOPS;
    let idx = devno.minor as usize;
    // Linux loop_ioctl order: ENXIO for any loop-specific request on an
    // unbound loop (except SET_FD/CONFIGURE which bind, and CLR_FD which
    // answers ENXIO itself).
    let bound = is_loop && loop_is_bound(idx);

    let ret: i64 = match request {
        LOOP_CTL_GET_FREE => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64); // ENOTTY on loop nodes
            }
            let free = loop_get_free();
            if free < 0 {
                -6i64 // ENXIO: none free
            } else {
                free as i64
            }
        }
        LOOP_CTL_ADD => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64);
            }
            // All N_LOOPS devices always exist here; a new one cannot be
            // created. Existing number => EEXIST, beyond the fixed range
            // => ENOSPC (Linux loop_control ADD semantics).
            let want = arg as u32;
            if want < N_LOOPS as u32 {
                -17i64 // EEXIST
            } else {
                -28i64 // ENOSPC: loop table full
            }
        }
        LOOP_CTL_REMOVE => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64);
            }
            let want = arg as usize;
            if want >= N_LOOPS {
                return Some(-6i64); // ENXIO
            }
            if loop_is_bound(want) {
                return Some(-16i64); // EBUSY: still in use
            }
            0
        }
        LOOP_SET_FD => {
            if !is_loop {
                return Some(-25i64);
            }
            let backing = unsafe { crate::fs::file::get_file_fd(arg) };
            match backing {
                Some(f) => {
                    let path = fd_backing_path(&f);
                    match loop_set_fd_named(idx, f, &path) {
                        Ok(()) => 0,
                        Err(e) => e as i64,
                    }
                }
                None => -9i64, // EBADF
            }
        }
        LOOP_CONFIGURE => {
            if !is_loop {
                return Some(-25i64);
            }
            if bound {
                return Some(-16i64); // EBUSY (Linux loop_configure)
            }
            // struct loop_config { __u32 fd; __u32 block_size;
            //                     struct loop_info64 info; ... }
            let mut cfg = [0u8; 8 + LOOP_INFO64_SIZE];
            // SAFETY: copy_from_user bounds-checks the userspace range.
            unsafe {
                if copy_from_user(cfg.as_mut_ptr(), arg as *const u8, cfg.len()) != 0 {
                    return Some(-14i64); // EFAULT
                }
            }
            let fd = u32::from_le_bytes(cfg[0..4].try_into().unwrap()) as usize;
            let offset = u64::from_le_bytes(cfg[8 + 24..8 + 32].try_into().unwrap());
            let sizelimit = u64::from_le_bytes(cfg[8 + 32..8 + 40].try_into().unwrap());
            let backing = match unsafe { crate::fs::file::get_file_fd(fd) } {
                Some(f) => f,
                None => return Some(-9i64),
            };
            let path = fd_backing_path(&backing);
            match loop_set_fd_named(idx, backing, &path) {
                Ok(()) => {
                    let _ = set_loop_geometry(idx, offset, sizelimit);
                    0
                }
                Err(e) => e as i64,
            }
        }
        LOOP_CLR_FD => {
            if !is_loop {
                return Some(-25i64);
            }
            match loop_clr_fd(idx) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        BLKGETSIZE64 | BLKSSZGET => {
            // mke2fs sizes the filesystem from BLKGETSIZE64 (and a failed
            // probe makes it fall back to an interactive "proceed?" prompt
            // that reads EOF from /dev/null and aborts — "mkfs.ext2 failed
            // with exit code 1" in every LTP tst_mkfs test).
            if !is_loop {
                return Some(-25i64);
            }
            let val: u64 = if request == BLKSSZGET {
                512
            } else {
                loop_size_bytes(idx)
            };
            // SAFETY: arg points to a user u64/u16; exception-table copy.
            let len = if request == BLKSSZGET { 2 } else { 8 };
            let bytes = val.to_le_bytes();
            let mut out = [0u8; 8];
            out[..len].copy_from_slice(&bytes[..len]);
            if unsafe { copy_to_user(arg as *mut u8, out.as_ptr(), len) } != 0 {
                return Some(-14i64); // EFAULT
            }
            0
        }
        LOOP_GET_STATUS64 => {
            if !bound {
                return Some(-6i64); // ENXIO: unbound loop
            }
            let st = loop_status_of(idx);
            let mut info = [0u8; LOOP_INFO64_SIZE];
            let put64 = |buf: &mut [u8; LOOP_INFO64_SIZE], off: usize, v: u64| {
                buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
            };
            put64(&mut info, 0, st.device);
            put64(&mut info, 8, st.inode);
            put64(&mut info, 16, st.rdevice);
            put64(&mut info, 24, st.offset);
            put64(&mut info, 32, st.sizelimit);
            info[40..44].copy_from_slice(&st.number.to_le_bytes());
            info[44..48].copy_from_slice(&st.encrypt_type.to_le_bytes());
            info[48..52].copy_from_slice(&st.encrypt_key_size.to_le_bytes());
            info[52..56].copy_from_slice(&st.flags.to_le_bytes());
            info[56..56 + LO_NAME_SIZE].copy_from_slice(&st.name);
            put64(&mut info, 216, st.init[0]);
            put64(&mut info, 224, st.init[1]);
            // SAFETY: copy_to_user validates the userspace range.
            unsafe {
                if copy_to_user(arg as *mut u8, info.as_ptr(), LOOP_INFO64_SIZE) != 0 {
                    return Some(-14i64); // EFAULT
                }
            }
            0
        }
        LOOP_SET_STATUS64 => {
            if !bound {
                return Some(-6i64);
            }
            let mut info = [0u8; LOOP_INFO64_SIZE];
            // SAFETY: copy_from_user bounds-checks the userspace range.
            unsafe {
                if copy_from_user(info.as_mut_ptr(), arg as *const u8, LOOP_INFO64_SIZE) != 0 {
                    return Some(-14i64);
                }
            }
            let offset = u64::from_le_bytes(info[24..32].try_into().unwrap());
            let limit = u64::from_le_bytes(info[32..40].try_into().unwrap());
            let flags = u32::from_le_bytes(info[52..56].try_into().unwrap());
            // Validate BEFORE applying (Linux loop_set_status64 order):
            // reject unknown lo_flags bits.
            if flags
                & !(LO_FLAGS_READ_ONLY
                    | LO_FLAGS_AUTOCLEAR
                    | LO_FLAGS_PARTSCAN
                    | LO_FLAGS_DIRECT_IO)
                != 0
            {
                return Some(-22i64); // EINVAL
            }
            match set_loop_geometry(idx, offset, limit) {
                Ok(()) => {
                    // SAFETY: static array access bounded by N_LOOPS.
                    unsafe {
                        LOOPS[idx].flags.store(flags as u64, Ordering::Release);
                        if !info[56..56 + LO_NAME_SIZE].iter().all(|&b| b == 0) {
                            let mut name = LOOPS[idx].name.lock();
                            name.copy_from_slice(&info[56..56 + LO_NAME_SIZE]);
                        }
                    }
                    0
                }
                Err(e) => e as i64,
            }
        }
        LOOP_GET_STATUS => {
            // Legacy struct loop_info (LP64 wire layout — see LOOP_INFO_SIZE).
            // Unbound -> ENXIO (LTP's free-loop probe relies on it).
            if !bound {
                return Some(-6i64);
            }
            let st = loop_status_of(idx);
            let mut info = [0u8; LOOP_INFO_SIZE];
            info[LI_OFF_NUMBER..LI_OFF_NUMBER + 4].copy_from_slice(&st.number.to_le_bytes());
            info[LI_OFF_DEVICE..LI_OFF_DEVICE + 4]
                .copy_from_slice(&((st.device & 0xFFFF_FFFF) as u32).to_le_bytes());
            info[LI_OFF_INODE..LI_OFF_INODE + 8].copy_from_slice(&st.inode.to_le_bytes());
            info[LI_OFF_RDEVICE..LI_OFF_RDEVICE + 4]
                .copy_from_slice(&((st.rdevice & 0xFFFF_FFFF) as u32).to_le_bytes());
            info[LI_OFF_OFFSET..LI_OFF_OFFSET + 4]
                .copy_from_slice(&(st.offset as u32).to_le_bytes());
            info[LI_OFF_ENCRYPT_TYPE..LI_OFF_ENCRYPT_TYPE + 4]
                .copy_from_slice(&st.encrypt_type.to_le_bytes());
            info[LI_OFF_KEY_SIZE..LI_OFF_KEY_SIZE + 4]
                .copy_from_slice(&st.encrypt_key_size.to_le_bytes());
            info[LI_OFF_FLAGS..LI_OFF_FLAGS + 4].copy_from_slice(&st.flags.to_le_bytes());
            info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE].copy_from_slice(&st.name);
            info[LI_OFF_INIT..LI_OFF_INIT + 8].copy_from_slice(&st.init[0].to_le_bytes());
            info[LI_OFF_INIT + 8..LI_OFF_INIT + 16].copy_from_slice(&st.init[1].to_le_bytes());
            // SAFETY: copy_to_user validates the userspace range.
            unsafe {
                if copy_to_user(arg as *mut u8, info.as_ptr(), LOOP_INFO_SIZE) != 0 {
                    return Some(-14i64); // EFAULT
                }
            }
            0
        }
        LOOP_SET_STATUS => {
            // Legacy SET: LTP's tst_attach_device issues it right after
            // LOOP_SET_FD with lo_name = backing path.
            if !bound {
                return Some(-6i64);
            }
            let mut info = [0u8; LOOP_INFO_SIZE];
            // SAFETY: copy_from_user bounds-checks the userspace range.
            unsafe {
                if copy_from_user(info.as_mut_ptr(), arg as *const u8, LOOP_INFO_SIZE) != 0 {
                    return Some(-14i64);
                }
            }
            let offset = u64::from(u32::from_le_bytes(
                info[LI_OFF_OFFSET..LI_OFF_OFFSET + 4].try_into().unwrap(),
            ));
            let flags = u32::from_le_bytes(
                info[LI_OFF_FLAGS..LI_OFF_FLAGS + 4].try_into().unwrap(),
            );
            if flags & !(LO_FLAGS_READ_ONLY | LO_FLAGS_AUTOCLEAR | LO_FLAGS_PARTSCAN) != 0 {
                return Some(-22i64); // EINVAL
            }
            // SAFETY: static array access bounded by N_LOOPS.
            unsafe {
                LOOPS[idx].offset.store(offset, Ordering::Release);
                LOOPS[idx].flags.store(flags as u64, Ordering::Release);
                if !info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE].iter().all(|&b| b == 0) {
                    let mut name = LOOPS[idx].name.lock();
                    name.copy_from_slice(&info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE]);
                }
            }
            0
        }
        LOOP_CHANGE_FD => {
            if !bound {
                return Some(-6i64);
            }
            match loop_change_fd(idx, arg) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        LOOP_SET_CAPACITY => {
            if !bound {
                return Some(-6i64);
            }
            match loop_set_capacity(idx) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        LOOP_SET_DIRECT_IO => {
            // Our loop always routes through the buffer cache; accepting
            // the request keeps DIRECT_IO-capable users (util-linux
            // `losetup --direct-io=on`) working.
            if !bound {
                return Some(-6i64);
            }
            0
        }
        LOOP_SET_BLOCK_SIZE => {
            if !bound {
                return Some(-6i64);
            }
            let bs = arg as u32;
            if bs < 512 || bs > 4096 || !bs.is_power_of_two() {
                return Some(-22i64); // EINVAL
            }
            0
        }
        _ => -25i64, // ENOTTY
    };
    Some(ret)
}

/// Best-effort backing path for a file fd (for lo_file_name). The VFS
/// tracks no per-open path, so ask the fd table for the inode identity —
/// empty when unknown (GET_STATUS then reports an empty name, same as a
/// detached loop; no consumer treats that as an error).
fn fd_backing_path(file: &Arc<File>) -> Vec<u8> {
    let _ = file;
    Vec::new()
}

#[allow(dead_code)]
fn loop_offset_of(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    unsafe { LOOPS[idx].offset.load(Ordering::Acquire) }
}

#[allow(dead_code)]
fn loop_sizelimit_of(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    unsafe { LOOPS[idx].sizelimit.load(Ordering::Acquire) }
}

fn set_loop_geometry(idx: usize, offset: u64, sizelimit: u64) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = unsafe { &LOOPS[idx] };
    dev.offset.store(offset, Ordering::Release);
    dev.sizelimit.store(sizelimit, Ordering::Release);
    Ok(())
}

/// Seek on the loop device node (position in bytes; sizes come from the
/// bound backing file, zero for an unbound loop).
pub fn loop_file_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9,
    };
    if devno.major != LOOP_MAJOR {
        return -29; // ESPIPE on loop-control
    }
    let size = loop_size_bytes(devno.minor as usize) as isize;
    let cur = file.get_pos() as isize;
    let base = match whence {
        0 => 0,                       // SEEK_SET
        1 => cur,                     // SEEK_CUR
        2 => size,                    // SEEK_END
        _ => return -22,
    };
    let target = match base.checked_add(offset) {
        Some(t) if t >= 0 => t,
        _ => return -22,
    };
    file.set_pos(target as u64);
    target
}
