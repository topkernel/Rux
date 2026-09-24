//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! devfs - Device Filesystem
//!
//! - Mounted at /dev
//! - Manages device nodes
//! - Supports character devices and block devices

pub mod registry;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use crate::sync::spinlock::Spinlock;
use crate::fs::file::FileOps;
use super::dev_t::DevNo;

// Re-export device number definitions
pub use super::dev_t;

// ============================================================================
// devfs directory entries
// ============================================================================

/// devfs directory entry type
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DevEntryType {
    /// Directory
    Directory,
    /// Character device
    CharDevice,
    /// Block device (not implemented)
    BlockDevice,
}

/// devfs directory entry
pub struct DevfsEntry {
    /// Name
    pub name: String,
    /// Type
    pub entry_type: DevEntryType,
    /// Child entries (valid only for directory type)
    pub children: Spinlock<BTreeMap<String, Arc<DevfsEntry>>>,
    /// Device number (valid only for device types)
    pub devno: DevNo,
    /// Permissions (default 0666)
    pub mode: u32,
}

impl DevfsEntry {
    /// Create directory
    pub fn new_dir(name: &str) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::Directory,
            children: Spinlock::new(BTreeMap::new()),
            devno: DevNo::default(),
            mode: 0o755,
        }
    }

    /// Create character device
    pub fn new_char_device(name: &str, devno: DevNo) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Spinlock::new(BTreeMap::new()),
            devno,
            mode: 0o666,
        }
    }

    /// Create character device with custom permissions
    pub fn new_char_device_with_mode(name: &str, devno: DevNo, mode: u32) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Spinlock::new(BTreeMap::new()),
            devno,
            mode: mode & 0o777,
        }
    }

    /// Create block device node (P1 mknod): recorded major/minor, but no
    /// block driver registry exists yet — open() on it returns ENXIO via
    /// the VFS device-open gate until a driver claims the number.
    pub fn new_block_device(name: &str, devno: DevNo, mode: u32) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::BlockDevice,
            children: Spinlock::new(BTreeMap::new()),
            devno,
            mode: mode & 0o777,
        }
    }

    /// Is directory
    pub fn is_dir(&self) -> bool {
        self.entry_type == DevEntryType::Directory
    }

    /// Is character device
    pub fn is_char_device(&self) -> bool {
        self.entry_type == DevEntryType::CharDevice
    }

    /// Is block device
    pub fn is_block_device(&self) -> bool {
        self.entry_type == DevEntryType::BlockDevice
    }
}

// ============================================================================
// devfs filesystem
// ============================================================================

/// devfs global instance
static DEVFS_ROOT: Spinlock<Option<Arc<DevfsEntry>>> = Spinlock::new(None);

/// Initialize devfs
/// /dev/null: reads return EOF, writes discard everything. musl's
/// __init_libc opens /dev/null when a stdio fd is missing (POLLNVAL) and
/// DELIBERATELY crashes (NULL store) if the open fails — without this
/// node any process with a closed stdio fd dies at startup.
fn nulldev_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    0 // immediate EOF
}

fn nulldev_write(file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let _ = file;
    buf.len() as isize // pretend everything was swallowed
}

/// /dev/null is ALWAYS readable (immediate EOF) and writable (discard).
/// Explicit poll op (review 5.2 low: relied on the generic "no handler =
/// ready" fallback, which a future default change would silently break).
fn nulldev_poll(_file: &crate::fs::file::File, _events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM
}

static NULLDEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(nulldev_read),
    write: Some(nulldev_write),
    lseek: None,
    close: None,
    poll: Some(nulldev_poll),
};

// ============================================================================
// devtmpfs node stock: console/tty, mem(1) devices, random
// (U2 — Ubuntu boot needs /dev/console writable by PID 1 and the
//  zero/random family present before udev takes over /dev)
// ============================================================================

/// /dev/console, /dev/tty, /dev/ttyS0 — the UART-backed terminal.
///
/// read goes through the shared console TtyDevice (canonical-mode line
/// discipline, termios settings shared with the pty layer); write does
/// OPOST/ONLCR translation then hits the UART atomically per chunk.
fn condev_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    let nonblock = (file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK) != 0;
    crate::fs::tty::console().read_input(buf, nonblock)
}

fn condev_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let onlcr = crate::fs::tty::console().output_translates_nl();
    // Translate \n -> \r\n (OPOST|ONLCR) in 256-byte chunks so a single
    // console write stays atomic per chunk (R20-4 discipline).
    let mut chunk = [0u8; 256];
    let mut chunk_len = 0usize;
    let mut flush = |chunk: &mut [u8], len: &mut usize| -> isize {
        if *len == 0 {
            return 0;
        }
        // SAFETY-free slice call; uart_write only reads the slice.
        let n = unsafe {
            crate::fs::char_dev::uart_write(chunk.as_ptr(), *len)
        };
        *len = 0;
        n
    };
    for &b in buf {
        if onlcr && b == b'\n' {
            if chunk_len + 2 > chunk.len() {
                let n = flush(&mut chunk, &mut chunk_len);
                if n < 0 {
                    return n;
                }
            }
            chunk[chunk_len] = b'\r';
            chunk[chunk_len + 1] = b'\n';
            chunk_len += 2;
        } else {
            if chunk_len + 1 > chunk.len() {
                let n = flush(&mut chunk, &mut chunk_len);
                if n < 0 {
                    return n;
                }
            }
            chunk[chunk_len] = b;
            chunk_len += 1;
        }
    }
    let n = flush(&mut chunk, &mut chunk_len);
    if n < 0 {
        return n;
    }
    buf.len() as isize
}

fn condev_poll(_file: &crate::fs::file::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;
    if events & POLLIN != 0 && crate::fs::tty::console().input_poll_ready() {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    ready
}

static CONDEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(condev_read),
    write: Some(condev_write),
    lseek: None,
    close: None,
    poll: Some(condev_poll),
};

/// /dev/zero — reads return zeros, writes discard.
fn zerodev_read(_file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    buf.fill(0);
    buf.len() as isize
}

fn zerodev_write(file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let _ = file;
    buf.len() as isize
}

fn zerodev_poll(_file: &crate::fs::file::File, _events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM
}

static ZERODEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(zerodev_read),
    write: Some(zerodev_write),
    lseek: None,
    close: None,
    poll: Some(zerodev_poll),
};

/// /dev/full — reads return zeros, writes fail with ENOSPC.
fn fulldev_write(_file: &crate::fs::file::File, _buf: &[u8]) -> isize {
    -(crate::errno::constants::ENOSPC as isize)
}

static FULLDEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(zerodev_read),
    write: Some(fulldev_write),
    lseek: None,
    close: None,
    poll: Some(zerodev_poll),
};

/// xorshift* PRNG state for /dev/random + /dev/urandom nodes (the real
/// CRNG lives behind the getrandom(2) syscall; this node-side source
/// only needs to exist and never block).
static RNG_STATE: Spinlock<u64> = Spinlock::new(0x9e3779b97f4a7c15);

fn rng_fill(buf: &mut [u8]) {
    let mut state = RNG_STATE.lock_irqsave();
    if *state == 0 {
        // Seed from the cycle counter (differs per call site/CPU/time).
        let cycles: u64;
        // SAFETY: rdcycle is a plain CSR read on this hart.
        unsafe { core::arch::asm!("rdcycle {0}", out(reg) cycles, options(nomem, nostack)) };
        *state = cycles ^ 0xa0761d6478bd642f;
    }
    for b in buf.iter_mut() {
        let mut x = *state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *state = x;
        *b = (x.wrapping_mul(0x2545F4914F6CDD1D)) as u8;
    }
}

fn randdev_read(_file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    rng_fill(buf);
    buf.len() as isize
}

fn randdev_poll(_file: &crate::fs::file::File, _events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM
}

static RANDDEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps {
    read: Some(randdev_read),
    write: Some(zerodev_write),
    lseek: None,
    close: None,
    poll: Some(randdev_poll),
};

/// virtio-blk major on this platform (matches sysfs /sys/class/block/vda).
pub const VIRTIO_BLK_MAJOR: u32 = 254;

/// virtio-blk root disk name.
const ROOT_DISK_NAME: &str = "vda";

/// Is a virtio-blk GenDisk present (MMIO or PCI)?
fn root_disk_present() -> bool {
    crate::drivers::virtio::get_pci_gen_disk().is_some()
        || crate::drivers::virtio::get_device().is_some()
}

/// Populate the devtmpfs node set that Ubuntu's early boot expects
/// (called from init() AFTER the block-device probes — main.rs probes
/// virtio-blk before devfs comes up).
///
/// Nodes:
/// - /dev/console, /dev/tty, /dev/ttyS0  (char 5:1 / 5:0 / 4:64)
/// - /dev/zero, /dev/full, /dev/random, /dev/urandom (mem major 1)
/// - /dev/vda (block 254:0) when a virtio-blk disk was probed — THE
///   node the root= boot argument and systemd's root-device wait
///   resolve. Whole-disk only: no partition table is scanned, so
///   /dev/vdaN sub-devices do not appear.
///
/// Network interfaces correctly have NO /dev node (socket-only).
fn devtmpfs_populate() {
    use crate::fs::dev_t::{DevNo, MEM_MAJOR, TTY_MAJOR};

    // --- terminal devices (UART-backed) ---
    let _ = registry::register_char_device(DevNo::new(TTY_MAJOR, 1), &CONDEV_OPS); // console
    let _ = registry::register_char_device(DevNo::new(TTY_MAJOR, 0), &CONDEV_OPS); // tty
    let _ = registry::register_char_device(DevNo::new(TTY_MAJOR, 64), &CONDEV_OPS); // ttyS0

    // --- mem devices ---
    let _ = registry::register_char_device(crate::fs::dev_t::DEV_ZERO, &ZERODEV_OPS);
    let _ = registry::register_char_device(DevNo::new(MEM_MAJOR, 7), &FULLDEV_OPS);
    let _ = registry::register_char_device(crate::fs::dev_t::DEV_RANDOM, &RANDDEV_OPS);
    let _ = registry::register_char_device(crate::fs::dev_t::DEV_URANDOM, &RANDDEV_OPS);

    let mut root = DEVFS_ROOT.lock_irqsave();
    let root_entry = match root.as_ref() {
        Some(r) => r.clone(),
        None => return,
    };
    drop(root);

    let mut children = root_entry.children.lock_irqsave();
    // S_IFCHR = 0o020000.
    let char_nodes: &[(&str, DevNo, u32)] = &[
        ("console", DevNo::new(TTY_MAJOR, 1), 0o600),
        ("tty", DevNo::new(TTY_MAJOR, 0), 0o666),
        ("ttyS0", DevNo::new(TTY_MAJOR, 64), 0o600),
        ("zero", crate::fs::dev_t::DEV_ZERO, 0o666),
        ("full", DevNo::new(MEM_MAJOR, 7), 0o666),
        ("random", crate::fs::dev_t::DEV_RANDOM, 0o666),
        ("urandom", crate::fs::dev_t::DEV_URANDOM, 0o666),
    ];
    for (name, devno, mode) in char_nodes.iter() {
        children.insert(
            String::from(*name),
            Arc::new(DevfsEntry::new_char_device_with_mode(
                name,
                *devno,
                0o020000 | mode,
            )),
        );
    }

    // --- root block device (whole disk) ---
    if root_disk_present() {
        children.insert(
            String::from(ROOT_DISK_NAME),
            Arc::new(DevfsEntry::new_block_device(
                ROOT_DISK_NAME,
                DevNo::new(VIRTIO_BLK_MAJOR, 0),
                0o060000 | 0o660, // S_IFBLK | brw-rw----
            )),
        );
    }
    drop(children);
}

// ============================================================================
// devtmpfs dynamic device node API (U2)
// ============================================================================

/// Evict a cached dentry (negative OR positive) under /dev so freshly
/// created/removed nodes are visible — mirrors the pty layer's
/// evict_pts_dentry discipline. Accepts a devfs-relative path ("sda" or
/// "pts/3").
fn evict_dev_dentry(rel_path: &str) {
    let (parent, name) = match rel_path.rfind('/') {
        Some(i) => (format!("/dev/{}", &rel_path[..i]), &rel_path[i + 1..]),
        None => (String::from("/dev"), rel_path),
    };
    if let Ok(vpath) = crate::fs::vfs::path_lookup(&parent, 0) {
        if let Some(parent_dentry) = vpath.dentry {
            parent_dentry.remove_child(name);
        }
    }
}

/// Register a device node dynamically (devtmpfs discipline): create the
/// /dev/<path> node and broadcast an "add" uevent for udev.
///
/// - `path`: devfs-relative path ("sda", "pts/3", "net/tun")
/// - `is_block`: block vs character device
///
/// Network devices do NOT get /dev nodes — callers must not use this
/// for netdevs (class/net lives in sysfs only).
pub fn devtmpfs_register_device(path: &str, devno: DevNo, is_block: bool) -> Result<(), ()> {
    let mode = if is_block {
        0o060000 | 0o660
    } else {
        0o020000 | 0o666
    };
    mknod(path, devno, mode)?;
    evict_dev_dentry(path);

    // uevent: DEVPATH relative to /sys (block devices live in
    // /sys/class/block/<name>).
    let name = path.rsplit('/').next().unwrap_or(path);
    let subsystem = if is_block { "block" } else { "tty" };
    let maj = format!("{}", devno.major);
    let min = format!("{}", devno.minor);
    let extra: [(&str, &str); 2] = [("MAJOR", maj.as_str()), ("MINOR", min.as_str())];
    let devpath = if is_block {
        format!("/class/block/{}", name)
    } else {
        format!("/class/tty/{}", name)
    };
    crate::fs::sysfs::uevent_send_full(&devpath, "add", subsystem, &extra);
    Ok(())
}

/// Unregister a dynamic device node: remove /dev/<path> and broadcast a
/// "remove" uevent.
pub fn devtmpfs_unregister_device(path: &str, devno: DevNo, is_block: bool) -> Result<(), ()> {
    remove_node(path)?;
    evict_dev_dentry(path);

    let name = path.rsplit('/').next().unwrap_or(path);
    let subsystem = if is_block { "block" } else { "tty" };
    let maj = format!("{}", devno.major);
    let min = format!("{}", devno.minor);
    let extra: [(&str, &str); 2] = [("MAJOR", maj.as_str()), ("MINOR", min.as_str())];
    let devpath = if is_block {
        format!("/class/block/{}", name)
    } else {
        format!("/class/tty/{}", name)
    };
    crate::fs::sysfs::uevent_send_full(&devpath, "remove", subsystem, &extra);
    Ok(())
}

pub fn init() {
    // Register /dev/null before creating the tree
    let _ = registry::register_char_device(crate::fs::dev_t::DEV_NULL, &NULLDEV_OPS);

    let mut root = DEVFS_ROOT.lock_irqsave();

    // Create root directory
    let root_entry = Arc::new(DevfsEntry::new_dir("dev"));

    // /dev/null node
    let null_entry = Arc::new(DevfsEntry::new_char_device_with_mode(
        "null",
        crate::fs::dev_t::DEV_NULL,
        0o666 | 0o020000, // S_IFCHR | rw-rw-rw-
    ));
    root_entry
        .children
        .lock_irqsave()
        .insert(String::from("null"), null_entry);

    // /dev/ptmx: every open allocates a new pty pair (fs/pty.rs ptmx_open).
    let ptmx_entry = Arc::new(DevfsEntry::new_char_device_with_mode(
        "ptmx",
        crate::fs::pty::DEV_PTMX,
        0o666 | 0o020000, // S_IFCHR | rw-rw-rw-
    ));
    root_entry
        .children
        .lock_irqsave()
        .insert(String::from("ptmx"), ptmx_entry);

    // /dev/pts: pty slave nodes appear at /dev/pts/N on posix_openpt and
    // are removed when the pair's last fd closes.
    let pts_dir = Arc::new(DevfsEntry::new_dir("pts"));
    root_entry
        .children
        .lock_irqsave()
        .insert(String::from("pts"), pts_dir);

    // Register the ptmx device ops (slave ops are registered per-pair).
    crate::fs::pty::devfs_register();

    // Create /dev/input directory
    let input_dir = Arc::new(DevfsEntry::new_dir("input"));

    // Add input to root directory
    root_entry.children.lock_irqsave().insert(String::from("input"), input_dir);

    *root = Some(root_entry);
    drop(root);

    // U2 devtmpfs dynamic population: terminal/mem/random nodes and the
    // /dev/vda root-disk node (block devices are probed before devfs
    // comes up in the main.rs boot sequence).
    devtmpfs_populate();
}

/// Create device node
///
/// # Arguments
/// - path: Device path (e.g., "/input/event0")
/// - devno: Device number
/// - mode: File mode (S_IFCHR, etc.)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn mknod(path: &str, devno: DevNo, mode: u32) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    // Collect path components into stack array (avoid Vec allocation)
    const MAX_COMPONENTS: usize = 16;
    let mut components: [&str; MAX_COMPONENTS] = [""; MAX_COMPONENTS];
    let mut ncomponents: usize = 0;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        if ncomponents >= MAX_COMPONENTS {
            return Err(());
        }
        components[ncomponents] = part;
        ncomponents += 1;
    }
    if ncomponents == 0 {
        return Err(());
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Traverse to parent of last component
    let mut current = root.clone();
    let parent_count = ncomponents - 1;  // ncomponents >= 1 guaranteed above
    for i in 0..parent_count {
        let component = components[i];
        let children = current.children.lock_irqsave();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // Create device node. P1 mknod: the S_IFCHR / S_IFBLK type bits select
    // the entry kind (char devices open through the CharDev registry; block
    // nodes are stat-able but have no driver registry yet).
    let device_name = components[ncomponents - 1];
    let is_block = mode & 0o060000 != 0; // S_IFBLK
    let entry = if is_block {
        Arc::new(DevfsEntry::new_block_device(device_name, devno, mode))
    } else {
        Arc::new(DevfsEntry::new_char_device_with_mode(device_name, devno, mode))
    };
    current.children.lock_irqsave().insert(String::from(device_name), entry);

    Ok(())
}

/// Remove a device node (dynamic entries, e.g. /dev/pts/N when a pty pair
/// is destroyed). Traversal rules match mknod(); removing a directory or a
/// missing node fails.
pub fn remove_node(path: &str) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Collect path components into stack array (avoid Vec allocation)
    const MAX_COMPONENTS: usize = 16;
    let mut components: [&str; MAX_COMPONENTS] = [""; MAX_COMPONENTS];
    let mut ncomponents: usize = 0;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        if ncomponents >= MAX_COMPONENTS {
            return Err(());
        }
        components[ncomponents] = part;
        ncomponents += 1;
    }
    if ncomponents == 0 {
        return Err(());
    }

    // Traverse to parent of last component
    let mut current = root.clone();
    let parent_count = ncomponents - 1;
    for i in 0..parent_count {
        let component = components[i];
        let children = current.children.lock_irqsave();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    let node_name = components[ncomponents - 1];
    let removed = current.children.lock_irqsave().remove(node_name);
    match removed {
        Some(entry) => {
            if entry.is_dir() {
                // Re-insert directories: only leaf device nodes may be removed.
                current
                    .children
                    .lock_irqsave()
                    .insert(String::from(node_name), entry);
                return Err(());
            }
            Ok(())
        }
        None => Err(()),
    }
}

/// Create directory
pub fn mkdir(path: &str) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Parse path
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let ncomponents = components.len();
    if ncomponents == 0 {
        return Err(());
    }

    // Traverse to parent of last component
    let mut current = root.clone();
    let parent_count = ncomponents - 1;  // ncomponents >= 1 guaranteed above
    for i in 0..parent_count {
        let component = components[i];
        let children = current.children.lock_irqsave();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // Create directory
    let dir_name = components.last().unwrap();
    let entry = Arc::new(DevfsEntry::new_dir(dir_name));

    current.children.lock_irqsave().insert(String::from(*dir_name), entry);

    Ok(())
}

/// Lookup path
///
/// # Returns
/// Returns (entry, is_char_device, devno) if found
pub fn lookup(path: &str) -> Option<(Arc<DevfsEntry>, bool, DevNo)> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    // Empty path or "." means root directory
    if path.is_empty() || path == "." {
        // Return root directory
        let root = DEVFS_ROOT.lock_irqsave();
        let root = root.as_ref()?;
        return Some((root.clone(), false, DevNo::default()));
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = root.as_ref()?;

    // Parse path, filter out "." and ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // If filtered result is empty, return root directory
    if components.is_empty() {
        return Some((root.clone(), false, DevNo::default()));
    }

    // Traverse path
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock_irqsave();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    Some((
        current.clone(),
        current.is_char_device(),
        current.devno,
    ))
}

/// Check if devfs is initialized
pub fn is_mounted() -> bool {
    DEVFS_ROOT.lock_irqsave().is_some()
}

/// Get the devfs root entry (for dentry tree mount).
pub fn get_root_entry() -> Option<Arc<DevfsEntry>> {
    DEVFS_ROOT.lock_irqsave().clone()
}

/// Directory entry info (name, is_dir, ino)
pub type DevfsDirEntry = (String, bool, u64);

/// List directory contents
///
/// # Arguments
/// - path: devfs internal path (e.g., "" for root directory, "input" for /dev/input)
///
/// # Returns
/// Returns directory entry list on success, None on failure
pub fn list_dir(path: &str) -> Option<Vec<DevfsDirEntry>> {
    let root = DEVFS_ROOT.lock_irqsave();
    let root = root.as_ref()?;

    // Empty path or "." means root directory
    if path.is_empty() || path == "/" || path == "." {
        let children = root.children.lock_irqsave();
        let mut entries = Vec::new();
        for (name, entry) in children.iter() {
            entries.push((name.clone(), entry.is_dir(), devfs_ino_hash(name)));
        }
        return Some(entries);
    }

    // Parse path, filter out "." and ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // Traverse to target directory
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock_irqsave();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    // Check if directory
    if !current.is_dir() {
        return None;
    }

    // List children
    let children = current.children.lock_irqsave();
    let mut entries = Vec::new();
    for (name, entry) in children.iter() {
        entries.push((name.clone(), entry.is_dir(), devfs_ino_hash(name)));
    }
    Some(entries)
}

/// Get device path (check if under /dev)
///
/// If path starts with /dev, returns devfs path (with /dev prefix removed)
pub fn parse_dev_path(path: &str) -> Option<&str> {
    if path == "/dev" {
        return Some("");
    }
    if path.starts_with("/dev/") {
        return Some(&path[5..]);
    }
    None
}

// ============================================================================
// DevFS Inode Operations (for VFS dentry tree integration)
// ============================================================================

use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps};
use crate::errno;

/// Devfs lookup: given a parent directory inode and a child name, return child's ino.
/// We use a simple hash of the name as the inode number since devfs has no real inodes.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let entry_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let entry = &*(entry_ptr as *const DevfsEntry);

    if !entry.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    let name_str = core::str::from_utf8(name)
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    let children = entry.children.lock_irqsave();
    if let Some(child) = children.get(name_str) {
        // Use a simple hash as inode number
        Ok(devfs_ino_hash(name_str))
    } else {
        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }
}

/// Devfs iget: instantiate a VFS Inode from (parent_inode, name, child_ino).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_iget(parent: &Inode, name: &[u8], _ino: Ino) -> Result<alloc::sync::Arc<Inode>, i32> {
    let entry_ptr = parent.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let parent_entry = &*(entry_ptr as *const DevfsEntry);

    let name_str = core::str::from_utf8(name)
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    let children = parent_entry.children.lock_irqsave();
    let child = children.get(name_str)
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    let child = child.clone();
    drop(children);

    let mode = if child.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | child.mode)
    } else if child.is_char_device() {
        InodeMode::new(InodeMode::S_IFCHR | child.mode)
    } else if child.is_block_device() {
        InodeMode::new(InodeMode::S_IFBLK | child.mode)
    } else {
        InodeMode::new(InodeMode::S_IFBLK | child.mode)
    };

    let ino = devfs_ino_hash(name_str);
    let mut inode = Inode::new(ino, mode);
    inode.fs_id = crate::fs::inode::FS_ID_DEVFS;  // icache isolation (VFS-H8)
    inode.ops = Some(&DEVFS_INODE_OPS);
    // Clone the Arc and convert to raw pointer to keep the DevfsEntry alive
    // independently of the BTreeMap entry. The refcount is incremented by
    // clone() and preserved by into_raw() (which doesn't decrement).
    let child_arc = Arc::clone(&child);
    inode.private_data = Some(Arc::into_raw(child_arc) as *mut u8);
    Ok(alloc::sync::Arc::new(inode))
}

/// Devfs getattr: fill stat for a devfs entry.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let entry_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let entry = &*(entry_ptr as *const DevfsEntry);

    stat.st_dev = 0;
    stat.st_ino = inode.ino;
    stat.st_nlink = 1;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = entry.devno.to_u64();
    stat.st_size = 0;
    stat.st_blocks = 0;
    stat.st_blksize = 4096;
    stat.st_mode = inode.mode.bits();
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;
    0
}

/// Simple hash for devfs inode numbers (devfs has no real on-disk inodes).
fn devfs_ino_hash(name: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Fold in length to reduce collisions for strings with shared suffixes.
    hash ^= name.len() as u64;
    // Ensure non-zero
    if hash == 0 { 1 } else { hash }
}

/// DevFS open hook: allocate per-open device state.
/// - /dev/ptmx → allocate a NEW pty pair for this open file description
/// - /dev/pts/N → attach to the live pair N
/// - everything else → no-op (0)
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
    let entry_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return 0,
    };
    let entry = &*(entry_ptr as *const DevfsEntry);

    if entry.devno == crate::fs::pty::DEV_PTMX {
        return crate::fs::pty::ptmx_open(file);
    }
    if entry.devno.major == crate::fs::pty::PTY_SLAVE_MAJOR {
        return crate::fs::pty::slave_open(file, entry.devno.minor);
    }
    0
}

/// DevFS get_file_ops: return device-specific ops for char devices, DIR_FILE_OPS for directories
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_char_device() {
        let entry_ptr = inode.private_data?;
        let entry = &*(entry_ptr as *const DevfsEntry);
        registry::get_char_device_ops(entry.devno)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        // Block device (or unknown): no block-driver registry exists —
        // returning None makes the VFS open gate fail with ENXIO ("no
        // such device or address"), the Linux behavior for a device node
        // with no bound driver.
        None
    }
}

/// DevFS readdir: list directory entries
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    let entry_ptr = inode.private_data?;
    let entry = &*(entry_ptr as *const DevfsEntry);
    if !entry.is_dir() {
        return None;
    }
    let children = entry.children.lock_irqsave();
    let mut entries = alloc::vec::Vec::new();
    for (name, child) in children.iter() {
        let dt = if child.is_dir() {
            file_type::DT_DIR
        } else if child.is_char_device() {
            file_type::DT_CHR
        } else if child.is_block_device() {
            file_type::DT_BLK
        } else {
            file_type::DT_UNKNOWN
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            // Same ino generator as devfs_lookup/devfs_iget (hash of the
            // name) so getdents64+stat agree on the inode number — review
            // 5.4 (devfs lookup/readdir ino 不一致).
            ino: devfs_ino_hash(name),
            name: name.as_bytes().to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

/// DevFS destroy_inode: reclaim the Arc<DevfsEntry> stored in private_data.
// SAFETY: VFS callback contract; called when the inode's refcount drops to zero.
unsafe fn devfs_destroy_inode(inode: &mut Inode) {
    if let Some(ptr) = inode.private_data.take() {
        // Reconstruct the Arc from the raw pointer and let it drop,
        // decrementing the DevfsEntry's reference count.
        let _ = Arc::from_raw(ptr as *const DevfsEntry);
    }
}

/// DevFS inode operations table
pub static DEVFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(devfs_lookup),
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: Some(devfs_get_file_ops),
    readdir: Some(devfs_readdir),
    open: Some(devfs_open),
    permission: None,
    getattr: Some(devfs_getattr),
    setattr: None,
    iget: Some(devfs_iget),
    destroy_inode: Some(devfs_destroy_inode),
};

/// Create a VFS inode for the devfs root entry.
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode(root_entry: &Arc<DevfsEntry>) -> alloc::sync::Arc<Inode> {
    let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.ops = Some(&DEVFS_INODE_OPS);
    inode.private_data = Some(Arc::into_raw(Arc::clone(root_entry)) as *mut u8);
    alloc::sync::Arc::new(inode)
}
