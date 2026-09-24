//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SysFS - device model filesystem + uevent hotplug notification
//!
//! ## Overview
//!
//! SysFS exports the kernel device model (a tree of `KObject`s) to
//! userspace. It is mounted at /sys and is a hard dependency of
//! systemd/udev (device enumeration, coldboot triggering via uevent
//! files, network/block device introspection).
//!
//! ## Device model
//!
//! ```text
//! KObject {
//!     name, parent (Weak), ktype,
//!     children:   BTreeMap<String, Arc<KObject>>,   // directories / symlinks
//!     attributes: BTreeMap<String, Arc<Attribute>>, // regular files
//! }
//! ```
//!
//! Attribute values are generated on demand by a boxed closure reading
//! LIVE state (netdev registry, GenDisk capacity, started-CPU mask) —
//! nothing is cached across reads.
//!
//! ## Hierarchy (Ubuntu userspace expectations)
//!
//! ```text
//! /sys/
//! ├── block/vda -> ../../class/block/vda
//! ├── class/{net/{lo,eth0}, block/vda, tty/{console,pts}}
//! ├── devices/system/cpu/cpu{0..3}/ + cpu{online,present,possible,offline}
//! ├── devices/virtual -> ../../class
//! ├── dev/block/254:0, dev/char/5:1   (device-number mappings)
//! ├── kernel/{uevent, uevent_seqnum, uevent_helper}
//! ├── module/<built-in>/{initstate}
//! ├── power/state                       (writable)
//! ├── fs/, firmware/, hypervisor/       (empty placeholders)
//! ```
//!
//! ## uevent (hotplug)
//!
//! `uevent_send()` broadcasts `action@devpath\0ACTION=..\0DEVPATH=..\0
//! SUBSYSTEM=..\0[MAJOR=..\0MINOR=..\0]SEQNUM=..\0` to every socket bound
//! to the NETLINK_KOBJECT_UEVENT (15) protocol. The payload carries NO
//! nlmsghdr — matching Linux `kobject_uevent_net_broadcast()` exactly, so
//! libudev's `udev_monitor_receive_device()` parses it unmodified.
//! Writing "add"/"remove"/"change" to any device's `uevent` attribute (or
//! to /sys/kernel/uevent) synthesizes a manual event — the path udevadm
//! trigger uses for coldboot.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::errno;
use crate::fs::superblock::{FileSystemType, SuperBlock};
use crate::fs::inode::{file_type, Ino, Inode, InodeMode, INodeOps, VfsDirEntry};
use crate::fs::mount::{MntFlags, VfsMount};
use crate::sync::spinlock::Spinlock;

/// SysFS magic number (STATFS_SYSFS_MAGIC).
const SYSFS_MAGIC: u32 = 0x62656572;

/// icache identity tag for sysfs inodes (VFS-H8: (ino, fs_id) must never
/// collide across filesystems; rootfs/procfs/devfs take 0001-0003).
pub const FS_ID_SYSFS: u64 = 0x5359_5353_0004;

// ============================================================================
// Device model: KObject
// ============================================================================

/// kobject type — drives the uevent SUBSYSTEM= value and stat metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KType {
    /// mount root
    System,
    /// /sys/class
    Class,
    /// /sys/devices
    Devices,
    /// network class devices
    Net,
    /// block class devices
    Block,
    /// tty class devices
    Tty,
    /// /sys/devices/system/cpu
    Cpu,
    /// /sys/module
    Module,
    /// /sys/kernel
    Kernel,
    /// /sys/power
    Power,
    /// /sys/fs
    Fs,
    /// /sys/dev
    Dev,
    /// /sys/firmware
    Firmware,
    /// /sys/hypervisor
    Hypervisor,
    /// unclassified
    Generic,
}

impl KType {
    /// uevent SUBSYSTEM= value (None → not a device-carrying node).
    pub fn subsystem(self) -> Option<&'static str> {
        match self {
            KType::Net => Some("net"),
            KType::Block => Some("block"),
            KType::Tty => Some("tty"),
            KType::Cpu => Some("cpu"),
            KType::Module => Some("module"),
            KType::Kernel => Some("kernel"),
            _ => None,
        }
    }
}

/// Node variant: a plain directory or a symbolic link into the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// directory (children + attributes)
    Directory,
    /// symlink (target path, relative like "../../class/net/lo")
    Link(String),
}

/// A sysfs attribute (regular file). The read side is a boxed generator
/// returning live state; the optional write side executes the store.
pub struct Attribute {
    /// inode number (fs-unique, from the global counter)
    pub ino: u64,
    /// permission bits (0o444 read-only, 0o644 root-writable)
    pub mode: u32,
    /// content generator — called with NO sysfs locks held
    pub show: Option<Box<dyn Fn() -> Vec<u8> + Send + Sync>>,
    /// write handler — returns 0 or negative errno
    pub store: Option<Box<dyn Fn(&[u8]) -> i32 + Send + Sync>>,
    /// cached content length (updated on open; stat reports 4096 like Linux)
    pub cached_size: AtomicU64,
}

/// Kernel object — one node of the /sys tree.
pub struct KObject {
    /// entry name ("" for the root)
    pub name: String,
    /// parent link (Weak: the tree must not form Arc cycles)
    pub parent: Spinlock<Option<Weak<KObject>>>,
    /// object type
    pub ktype: KType,
    /// directory or symlink
    pub kind: NodeKind,
    /// device number for block/char devices (MAJOR=/MINOR= uevent vars)
    pub devno: Spinlock<Option<(u32, u32)>>,
    /// fs-unique inode number
    pub ino: u64,
    /// child objects (BTreeMap → readdir order is deterministic)
    pub children: Spinlock<BTreeMap<String, Arc<KObject>>>,
    /// attribute files
    pub attributes: Spinlock<BTreeMap<String, Arc<Attribute>>>,
}

// SAFETY: all mutable state (children/attributes/parent) sits behind
// Spinlocks; closures are required to be Send + Sync at construction.
unsafe impl Send for KObject {}
unsafe impl Sync for KObject {}

impl KObject {
    /// Create a root object (ino 1).
    pub fn new_root() -> Arc<Self> {
        Arc::new(Self {
            name: String::new(),
            parent: Spinlock::new(None),
            ktype: KType::System,
            kind: NodeKind::Directory,
            devno: Spinlock::new(None),
            ino: 1,
            children: Spinlock::new(BTreeMap::new()),
            attributes: Spinlock::new(BTreeMap::new()),
        })
    }

    /// Create a child object with the next inode number.
    pub fn new(name: &str, ktype: KType, kind: NodeKind) -> Arc<Self> {
        Arc::new(Self {
            name: String::from(name),
            parent: Spinlock::new(None),
            ktype,
            kind,
            devno: Spinlock::new(None),
            ino: alloc_ino(),
            children: Spinlock::new(BTreeMap::new()),
            attributes: Spinlock::new(BTreeMap::new()),
        })
    }

    /// Attach a child (sets its parent backlink). Replaces an existing
    /// child of the same name.
    pub fn add_child(&self, child: Arc<KObject>) {
        *child.parent.lock() = Some(Arc::downgrade(&child));
        self.children.lock().insert(child.name.clone(), child);
    }

    /// Remove (and drop) a child by name. Stale dentries in the VFS layer
    /// simply start returning ENOENT on re-lookup.
    pub fn remove_child(&self, name: &str) -> Option<Arc<KObject>> {
        self.children.lock().remove(name)
    }

    /// Find a child by name.
    pub fn find_child(&self, name: &[u8]) -> Option<Arc<KObject>> {
        let name = core::str::from_utf8(name).ok()?;
        self.children.lock().get(name).cloned()
    }

    /// Register an attribute on this object.
    pub fn set_attr(
        &self,
        name: &str,
        mode: u32,
        show: Option<Box<dyn Fn() -> Vec<u8> + Send + Sync>>,
        store: Option<Box<dyn Fn(&[u8]) -> i32 + Send + Sync>>,
    ) {
        let attr = Arc::new(Attribute {
            ino: alloc_ino(),
            mode,
            show,
            store,
            cached_size: AtomicU64::new(0),
        });
        self.attributes.lock().insert(String::from(name), attr);
    }

    /// Look up an attribute by name.
    pub fn find_attr(&self, name: &[u8]) -> Option<Arc<Attribute>> {
        let name = core::str::from_utf8(name).ok()?;
        self.attributes.lock().get(name).cloned()
    }

    /// Generate an attribute's content. The attributes lock is released
    /// BEFORE the generator runs — generators read device state and must
    /// never run under a sysfs lock.
    pub fn attr_content(&self, name: &[u8]) -> Option<Vec<u8>> {
        let attr = self.find_attr(name)?;
        let content = match attr.show.as_ref() {
            Some(show) => show(),
            None => Vec::new(),
        };
        attr.cached_size.store(content.len() as u64, Ordering::Relaxed);
        Some(content)
    }

    /// Full path inside the sysfs namespace (DEVPATH form: "/class/net/lo").
    pub fn path(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(self.name.clone());
        let mut cur = self.parent.lock().as_ref().and_then(Weak::upgrade);
        let mut depth = 0;
        while let Some(p) = cur {
            if depth > 32 {
                break; // defensive: no cycles expected
            }
            parts.push(p.name.clone());
            cur = p.parent.lock().as_ref().and_then(Weak::upgrade);
            depth += 1;
        }
        parts.reverse();
        let mut path = String::new();
        for p in parts.iter() {
            if p.is_empty() {
                continue; // skip the root's empty name
            }
            path.push('/');
            path.push_str(p);
        }
        if path.is_empty() {
            path.push('/');
        }
        path
    }
}

/// Global inode allocator (1 = root).
static NEXT_INO: AtomicU64 = AtomicU64::new(2);

fn alloc_ino() -> u64 {
    NEXT_INO.fetch_add(1, Ordering::Relaxed)
}

// ============================================================================
// Global tree
// ============================================================================

static SYSFS_ROOT: Spinlock<Option<Arc<KObject>>> = Spinlock::new(None);

/// The global sysfs KObject tree root (None before init_sysfs()).
pub fn sysfs_root() -> Option<Arc<KObject>> {
    SYSFS_ROOT.lock().clone()
}

/// Look up a path like "class/net/lo" from the root (for internal use).
pub fn lookup_path(path: &str) -> Option<Arc<KObject>> {
    let mut cur = sysfs_root()?;
    for comp in path.split('/').filter(|s| !s.is_empty()) {
        cur = cur.find_child(comp.as_bytes())?;
    }
    Some(cur)
}

// ============================================================================
// Tree builders
// ============================================================================

/// Create + attach a directory kobject.
fn mk_dir(parent: &Arc<KObject>, name: &str, ktype: KType) -> Arc<KObject> {
    let child = KObject::new(name, ktype, NodeKind::Directory);
    parent.add_child(child.clone());
    child
}

/// Create + attach a symlink kobject.
fn mk_link(parent: &Arc<KObject>, name: &str, target: &str) -> Arc<KObject> {
    let child = KObject::new(name, KType::Generic, NodeKind::Link(String::from(target)));
    parent.add_child(child.clone());
    child
}

/// Read-only attribute.
fn attr_ro<F>(parent: &Arc<KObject>, name: &str, show: F)
where
    F: Fn() -> Vec<u8> + Send + Sync + 'static,
{
    parent.set_attr(name, 0o444, Some(Box::new(show)), None);
}

/// Read-write attribute (root-writable, 0644 like /sys/power/state).
fn attr_rw<F, G>(parent: &Arc<KObject>, name: &str, show: F, store: G)
where
    F: Fn() -> Vec<u8> + Send + Sync + 'static,
    G: Fn(&[u8]) -> i32 + Send + Sync + 'static,
{
    parent.set_attr(name, 0o644, Some(Box::new(show)), Some(Box::new(store)));
}

/// The "uevent" attribute present on every device kobject: writing
/// "add"/"remove"/"change" synthesizes a userspace-triggered uevent
/// (udevadm trigger coldboot path).
fn add_uevent_attr(dev: &Arc<KObject>) {
    // Copy everything the closure needs — it must be 'static.
    let devpath = dev.path();
    let subsystem = String::from(dev.ktype.subsystem().unwrap_or("kernel"));
    let devno = *dev.devno.lock();
    attr_rw(dev, "uevent", Vec::new, move |buf| {
        let action = match parse_uevent_action(buf) {
            Some(a) => a,
            None => return errno::Errno::InvalidArgument.as_neg_i32(),
        };
        if let Some((major, minor)) = devno {
            let maj = format!("{}", major);
            let min = format!("{}", minor);
            let extra: [(&str, &str); 2] = [("MAJOR", maj.as_str()), ("MINOR", min.as_str())];
            uevent_send_full(&devpath, action, &subsystem, &extra);
        } else {
            uevent_send_full(&devpath, action, &subsystem, &[]);
        }
        0
    });
}

/// Parse a uevent action string ("add\n" → "add").
fn parse_uevent_action(buf: &[u8]) -> Option<&'static str> {
    let s = core::str::from_utf8(buf).ok()?;
    let token = s.trim_matches(|c| c == '\n' || c == '\r' || c == ' ' || c == '\0');
    match token {
        "add" => Some("add"),
        "remove" => Some("remove"),
        "change" => Some("change"),
        "online" => Some("online"),
        "offline" => Some("offline"),
        "bind" => Some("bind"),
        "unbind" => Some("unbind"),
        _ => None,
    }
}

// ============================================================================
// Live-state sources
// ============================================================================

/// Live netdev accessor type (both loopback and virtio-net match).
type NetDevGetter = fn() -> Option<&'static mut crate::drivers::net::space::NetDevice>;

/// Live GenDisk capacity in 512-byte sectors (0 when no disk present).
fn disk_capacity_sectors() -> u64 {
    if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
        return disk.get_capacity();
    }
    if let Some(dev) = crate::drivers::virtio::get_device() {
        return dev.disk.get_capacity();
    }
    0
}

/// "0-3"-style CPU range for /sys/devices/system/cpu/{online,present,...}.
fn cpu_range_str(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => String::from("0"),
        n => format!("0-{}", n - 1),
    }
}

/// Number of possible CPUs (SMP target).
fn possible_cpus() -> usize {
    crate::config::MAX_CPUS
}

/// Build a network device directory under /sys/class/net with live attrs.
fn add_net_device(
    class_net: &Arc<KObject>,
    name: &str,
    ifindex: u32,
    getter: NetDevGetter,
    default_mac: [u8; 6],
    default_mtu: u32,
    default_flags: u32,
    arphrd: u32,
) {
    let dev = mk_dir(class_net, name, KType::Net);

    attr_ro(&dev, "ifindex", move || {
        format!("{}\n", ifindex).into_bytes()
    });
    attr_ro(&dev, "type", move || format!("{}\n", arphrd).into_bytes());
    attr_ro(&dev, "address", move || {
        let mac = getter()
            .map(|d| {
                let mut m = [0u8; 6];
                m.copy_from_slice(&d.addr[..6]);
                m
            })
            .unwrap_or(default_mac);
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )
        .into_bytes()
    });
    attr_ro(&dev, "flags", move || {
        let flags = getter().map(|d| d.flags).unwrap_or(default_flags);
        format!("0x{:04x}\n", flags).into_bytes()
    });
    attr_ro(&dev, "mtu", move || {
        let mtu = getter().map(|d| d.mtu).unwrap_or(default_mtu);
        format!("{}\n", mtu).into_bytes()
    });
    attr_ro(&dev, "operstate", move || {
        let flags = getter().map(|d| d.flags).unwrap_or(default_flags);
        let state = if flags & 0x1 != 0 { "up" } else { "down" };
        format!("{}\n", state).into_bytes()
    });

    // statistics/ subdirectory — live from the NetDevice stats block.
    let stats_dir = mk_dir(&dev, "statistics", KType::Net);
    let fields: &[(&str, fn(&crate::drivers::net::space::DeviceStats) -> u64)] = &[
        ("rx_packets", |s| s.rx_packets),
        ("tx_packets", |s| s.tx_packets),
        ("rx_bytes", |s| s.rx_bytes),
        ("tx_bytes", |s| s.tx_bytes),
        ("rx_errors", |s| s.rx_errors),
        ("tx_errors", |s| s.tx_errors),
        ("rx_dropped", |s| s.rx_dropped),
        ("tx_dropped", |s| s.tx_dropped),
        ("multicast", |s| s.multicast),
    ];
    for (stat_name, get) in fields.iter() {
        let get = *get;
        attr_ro(&stats_dir, stat_name, move || {
            let v = getter().map(|d| get(&d.get_stats())).unwrap_or(0);
            format!("{}\n", v).into_bytes()
        });
    }

    add_uevent_attr(&dev);
}

/// Build the /sys/class/block/vda device directory.
fn add_block_device(class_block: &Arc<KObject>, name: &str, major: u32, minor: u32) {
    let dev = mk_dir(class_block, name, KType::Block);
    *dev.devno.lock() = Some((major, minor));

    attr_ro(&dev, "size", move || {
        format!("{}\n", disk_capacity_sectors()).into_bytes()
    });
    attr_ro(&dev, "dev", move || format!("{}:{}\n", major, minor).into_bytes());
    attr_ro(&dev, "ro", move || b"0\n".to_vec());
    // struct block_device_stats — 17 u64 fields (reads/merges/sectors/ms,
    // writes/..., in-flight, io-time, weighted, discards x4, flushes x2).
    attr_ro(&dev, "stat", move || {
        format!("{}\n", "0 ".repeat(16) + "0").into_bytes()
    });

    add_uevent_attr(&dev);
}

/// Build a /sys/class/tty device directory.
fn add_tty_device(class_tty: &Arc<KObject>, name: &str, major: u32, minor: u32) {
    let dev = mk_dir(class_tty, name, KType::Tty);
    *dev.devno.lock() = Some((major, minor));
    attr_ro(&dev, "dev", move || format!("{}:{}\n", major, minor).into_bytes());
    attr_ro(&dev, "active", move || Vec::new());
    add_uevent_attr(&dev);
}

/// Built-in "modules" listed under /sys/module (subsystems compiled into
/// this kernel image).
const BUILTIN_MODULES: &[&str] = &[
    "ext4",
    "jbd2",
    "virtio",
    "virtio_blk",
    "virtio_mmio",
    "virtio_net",
    "virtio_pci",
    "proc",
    "sysfs",
    "tmpfs",
    "devpts",
    "unix",
    "netlink",
    "ipv6",
];

/// Build the whole default hierarchy. Idempotent at the caller (guarded
/// by SYSFS_ROOT being None).
fn build_tree() -> Arc<KObject> {
    let root = KObject::new_root();

    // ---------------- /sys/class ----------------
    let class = mk_dir(&root, "class", KType::Class);

    let net = mk_dir(&class, "net", KType::Net);
    add_net_device(
        &net,
        "lo",
        1,
        crate::drivers::net::get_loopback_device,
        [0, 0, 0, 0, 0, 0],
        65536,
        0x1 | 0x40 | 0x8 | 0x1000, // UP|RUNNING|LOOPBACK|MULTICAST
        772,                        // ARPHRD_LOOPBACK
    );
    add_net_device(
        &net,
        "eth0",
        2,
        crate::drivers::net::get_virtio_net_device_net,
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        1500,
        0x1 | 0x40 | 0x2, // UP|RUNNING|BROADCAST
        1,                 // ARPHRD_ETHER
    );

    // /sys/class/block/vda — virtio-blk conventionally lands on major 254.
    let block = mk_dir(&class, "block", KType::Block);
    add_block_device(&block, "vda", 254, 0);

    let tty = mk_dir(&class, "tty", KType::Tty);
    add_tty_device(&tty, "console", 5, 1);
    add_tty_device(&tty, "pts", 136, 0);
    // /sys/class/tty/tty0 — the active VT node tools expect to exist.
    add_tty_device(&tty, "tty0", 4, 0);

    // ---------------- /sys/devices ----------------
    let devices = mk_dir(&root, "devices", KType::Devices);

    let system = mk_dir(&devices, "system", KType::System);
    let cpu = mk_dir(&system, "cpu", KType::Cpu);
    attr_ro(&cpu, "online", move || {
        format!("{}\n", cpu_range_str(started_cpus())).into_bytes()
    });
    attr_ro(&cpu, "possible", move || {
        format!("{}\n", cpu_range_str(possible_cpus())).into_bytes()
    });
    attr_ro(&cpu, "present", move || {
        format!("{}\n", cpu_range_str(started_cpus())).into_bytes()
    });
    attr_ro(&cpu, "offline", move || b"\n".to_vec());

    for cpu_id in 0..possible_cpus() {
        let dev = mk_dir(&cpu, &format!("cpu{}", cpu_id), KType::Cpu);
        attr_ro(&dev, "online", move || {
            if cpu_id < started_cpus() {
                b"1\n".to_vec()
            } else {
                b"0\n".to_vec()
            }
        });
        attr_ro(&dev, "cpuinfo", move || {
            // RISC-V /sys cpuinfo format (tab-separated key/value).
            b"isa\t\trv64imafdc\nmmu\t\tsv39\nmvendorid\t0\nmarchid\t\t0\nmimpid\t\t0\n".to_vec()
        });
    }

    // /sys/devices/virtual → ../../class (class entries live under
    // /sys/class; this symlink keeps DEVPATH-style walks working).
    mk_link(&devices, "virtual", "../../class");

    // ---------------- /sys/kernel ----------------
    let kernel = mk_dir(&root, "kernel", KType::Kernel);
    attr_ro(&kernel, "uevent_seqnum", move || {
        format!("{}\n", UEVENT_SEQNUM.load(Ordering::Relaxed)).into_bytes()
    });
    attr_ro(&kernel, "uevent_helper", move || b"\n".to_vec());
    // Manual trigger: echo add > /sys/kernel/uevent
    attr_rw(&kernel, "uevent", Vec::new, |buf| {
        match parse_uevent_action(buf) {
            Some(action) => {
                uevent_send_full("/kernel", action, "kernel", &[]);
                0
            }
            None => errno::Errno::InvalidArgument.as_neg_i32(),
        }
    });

    // ---------------- /sys/module ----------------
    let module = mk_dir(&root, "module", KType::Module);
    for m in BUILTIN_MODULES.iter() {
        let mdir = mk_dir(&module, m, KType::Module);
        attr_ro(&mdir, "initstate", move || b"built-in\n".to_vec());
    }

    // ---------------- /sys/fs, /sys/firmware, /sys/hypervisor ----------
    mk_dir(&root, "fs", KType::Fs);
    mk_dir(&root, "firmware", KType::Firmware);
    mk_dir(&root, "hypervisor", KType::Hypervisor);

    // ---------------- /sys/power ----------------
    let power = mk_dir(&root, "power", KType::Power);
    attr_rw(
        &power,
        "state",
        || {
            let cur = POWER_STATE.lock();
            match cur.as_deref() {
                Some(s) => format!("{}\n", s).into_bytes(),
                None => b"freeze mem disk\n".to_vec(),
            }
        },
        |buf| {
            let s = match core::str::from_utf8(buf) {
                Ok(s) => s,
                Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
            };
            let token = s.trim_matches(|c| c == '\n' || c == '\r' || c == ' ');
            match token {
                "freeze" | "standby" | "mem" | "disk" | "on" | "shutdown" | "" => {
                    *POWER_STATE.lock() = Some(String::from(token));
                    0
                }
                _ => errno::Errno::InvalidArgument.as_neg_i32(),
            }
        },
    );

    // ---------------- /sys/dev + /sys/block ----------------
    let dev_tree = mk_dir(&root, "dev", KType::Dev);
    let dev_block = mk_dir(&dev_tree, "block", KType::Block);
    mk_link(&dev_block, "254:0", "../../../class/block/vda");
    let dev_char = mk_dir(&dev_tree, "char", KType::Tty);
    mk_link(&dev_char, "5:1", "../../../class/tty/console");
    mk_link(&dev_char, "4:0", "../../../class/tty/tty0");

    let sys_block = mk_dir(&root, "block", KType::Block);
    mk_link(&sys_block, "vda", "../../class/block/vda");

    root
}

/// Live started-CPU count.
fn started_cpus() -> usize {
    crate::arch::riscv64::smp::num_started_cpus()
}

/// Last value written to /sys/power/state.
static POWER_STATE: Spinlock<Option<String>> = Spinlock::new(None);

// ============================================================================
// uevent — KOBJ_NETLINK broadcast
// ============================================================================

/// NETLINK_KOBJECT_UEVENT protocol number (see netlink.rs for the socket
/// side; constant repeated here for self-containment).
pub const NETLINK_KOBJECT_UEVENT: i32 = 15;

/// Global uevent sequence counter (starts at 1 like Linux).
static UEVENT_SEQNUM: AtomicU64 = AtomicU64::new(1);

/// Append "KEY=VALUE\0" to the message buffer.
fn push_env(msg: &mut Vec<u8>, key: &str, value: &str) {
    msg.extend_from_slice(key.as_bytes());
    msg.push(b'=');
    msg.extend_from_slice(value.as_bytes());
    msg.push(0);
}

/// Broadcast a uevent for a kobject path.
///
/// Wire format (identical to Linux kobject_uevent_net_broadcast — no
/// nlmsghdr, NUL-separated strings):
///
/// ```text
/// "add@/class/net/lo\0ACTION=add\0DEVPATH=/class/net/lo\0SUBSYSTEM=net\0SEQNUM=42\0"
/// ```
///
/// Returns the assigned SEQNUM.
pub fn uevent_send_full(
    devpath: &str,
    action: &str,
    subsystem: &str,
    extra_env: &[(&str, &str)],
) -> u64 {
    let seq = UEVENT_SEQNUM.fetch_add(1, Ordering::Relaxed);

    let mut msg: Vec<u8> = Vec::new();
    msg.extend_from_slice(action.as_bytes());
    msg.push(b'@');
    msg.extend_from_slice(devpath.as_bytes());
    msg.push(0);
    push_env(&mut msg, "ACTION", action);
    push_env(&mut msg, "DEVPATH", devpath);
    push_env(&mut msg, "SUBSYSTEM", subsystem);
    for (k, v) in extra_env.iter() {
        push_env(&mut msg, k, v);
    }
    push_env(&mut msg, "SEQNUM", &format!("{}", seq));

    crate::net::netlink::uevent_broadcast(&msg);
    seq
}

/// Infer SUBSYSTEM from a DEVPATH ("/class/net/lo" → "net").
fn subsystem_from_devpath(devpath: &str) -> &'static str {
    let rest = devpath.strip_prefix("/class/").unwrap_or(devpath);
    let first = rest.split('/').next().unwrap_or("");
    match first {
        "net" => "net",
        "block" => "block",
        "tty" => "tty",
        "" => "kernel",
        other => {
            let _ = other;
            "kernel"
        }
    }
}

/// uevent_send(devpath, action) — subsystem inferred from the path.
pub fn uevent_send(devpath: &str, action: &str) -> u64 {
    let subsystem = subsystem_from_devpath(devpath);
    uevent_send_full(devpath, action, subsystem, &[])
}

/// Fire a uevent for a KObject (adds MAJOR=/MINOR= for devices).
pub fn kobject_uevent(kobj: &KObject, action: &str) -> u64 {
    let devpath = kobj.path();
    let subsystem = kobj.ktype.subsystem().unwrap_or("kernel");
    if let Some((major, minor)) = *kobj.devno.lock() {
        let maj = format!("{}", major);
        let min = format!("{}", minor);
        let extra: [(&str, &str); 2] = [("MAJOR", maj.as_str()), ("MINOR", min.as_str())];
        uevent_send_full(&devpath, action, subsystem, &extra)
    } else {
        uevent_send_full(&devpath, action, subsystem, &[])
    }
}

/// Network device state change (up/down) — call on IFF_UP transitions.
pub fn netdev_uevent(name: &str, action: &str) -> u64 {
    let devpath = format!("/class/net/{}", name);
    uevent_send_full(&devpath, action, "net", &[])
}

/// Block device discovery/removal notification.
pub fn blockdev_uevent(name: &str, action: &str) -> u64 {
    let devpath = format!("/class/block/{}", name);
    uevent_send_full(&devpath, action, "block", &[])
}

/// Current uevent SEQNUM (for /sys/kernel/uevent_seqnum).
pub fn uevent_seqnum() -> u64 {
    UEVENT_SEQNUM.load(Ordering::Relaxed)
}

// ============================================================================
// Filesystem type / superblock / mount
// ============================================================================

/// SysFS filesystem type (registry entry).
pub static SYSFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "sysfs",
    Some(sysfs_mount_cb),
    Some(sysfs_kill_sb),
    0,
);

/// SysFS superblock wrapper.
pub struct SysfsSuperBlock {
    /// base
    pub sb: SuperBlock,
    /// tree root
    pub root: Arc<KObject>,
}

static GLOBAL_SYSFS_SB: core::sync::atomic::AtomicPtr<SysfsSuperBlock> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

static GLOBAL_SYSFS_MOUNT: core::sync::atomic::AtomicPtr<VfsMount> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

// SAFETY: FsContext is a valid VFS mount callback argument.
unsafe extern "C" fn sysfs_mount_cb(
    _fs_context: &crate::fs::superblock::FsContext<'_>,
) -> Result<*mut SuperBlock, i32> {
    let root = sysfs_root().ok_or(errno::Errno::InvalidArgument.as_neg_i32())?;
    let sb = Box::new(SysfsSuperBlock {
        sb: SuperBlock::new(4096, SYSFS_MAGIC),
        root,
    });
    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

// SAFETY: sb came from sysfs_mount_cb's Box::into_raw.
unsafe extern "C" fn sysfs_kill_sb(sb: *mut SuperBlock) {
    if !sb.is_null() {
        drop(Box::from_raw(sb as *mut SysfsSuperBlock));
    }
}

/// Get the sysfs superblock (if initialized).
pub fn get_sysfs_sb() -> Option<&'static SysfsSuperBlock> {
    let ptr = GLOBAL_SYSFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

/// Initialize sysfs: register the filesystem type and build the KObject
/// tree. Idempotent — safe to call from do_mount and boot paths.
pub fn init_sysfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;

    // Register fs type (ignore "already registered" duplicates — the
    // registry has no lookup-by-name dedup error, so tolerate the push).
    let _ = register_filesystem(&SYSFS_FS_TYPE);

    // Build the tree once.
    {
        let guard = SYSFS_ROOT.lock();
        if guard.is_some() {
            return Ok(());
        }
    }
    let root = build_tree();

    // Block device discovery: emit the initial "add" uevent for vda when
    // a GenDisk is already present (no listeners exist this early, so the
    // broadcast is a no-op — kept for symmetry with the driver path).
    if disk_capacity_sectors() > 0 {
        blockdev_uevent("vda", "add");
    }

    let sb = Box::new(SysfsSuperBlock {
        sb: SuperBlock::new(4096, SYSFS_MAGIC),
        root: root.clone(),
    });
    GLOBAL_SYSFS_SB.store(Box::into_raw(sb), Ordering::Release);
    *SYSFS_ROOT.lock() = Some(root);
    Ok(())
}

/// Prepare the /sys mountpoint (mirrors mount_procfs): create /sys in the
/// rootfs and record a VfsMount.
pub fn mount_sysfs() -> Result<(), i32> {
    // Create /sys directory in rootfs (ignore error if it exists).
    if let Some(rootfs_sb) = crate::fs::rootfs::get_rootfs_sb() {
        // SAFETY: pointer comes from the global rootfs instance.
        unsafe {
            let _ = (*rootfs_sb).create_dir("/sys", 0o755);
        }
    }

    if get_sysfs_sb().is_none() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    let sb_ptr = GLOBAL_SYSFS_SB.load(Ordering::Acquire);
    let mount = Box::new(VfsMount::new(
        b"/sys".to_vec(),
        b"/sys".to_vec(),
        MntFlags::new(0),
        Some(sb_ptr as *mut u8),
    ));
    GLOBAL_SYSFS_MOUNT.store(Box::into_raw(mount) as *mut VfsMount, Ordering::Release);
    Ok(())
}

/// Is sysfs mounted?
pub fn is_mounted() -> bool {
    !GLOBAL_SYSFS_MOUNT.load(Ordering::Acquire).is_null()
}

/// Create the sysfs root directory VFS inode (mount-time).
pub fn create_root_inode() -> Arc<Inode> {
    let root = match sysfs_root() {
        Some(r) => r,
        None => {
            // Fallback minimal directory inode.
            let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o555));
            inode.fs_id = FS_ID_SYSFS;
            inode.ops = Some(&SYSFS_INODE_OPS);
            return Arc::new(inode);
        }
    };
    let entry = Box::new(SysfsEntryData {
        kobj: root.clone(),
        attr: None,
        link: None,
    });
    let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o555));
    inode.fs_id = FS_ID_SYSFS;
    inode.ops = Some(&SYSFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Arc::new(inode)
}

// ============================================================================
// SysFS inode operations
// ============================================================================

/// Per-inode sysfs data (Box leaked into Inode::private_data, reclaimed
/// by sysfs_destroy_inode).
pub struct SysfsEntryData {
    /// owning kobject
    pub kobj: Arc<KObject>,
    /// attribute (regular-file inodes)
    pub attr: Option<Arc<Attribute>>,
    /// symlink target (link inodes)
    pub link: Option<String>,
}

/// Recover the entry data from an inode.
/// SAFETY: private_data must have come from sysfs_iget/create_root_inode.
unsafe fn entry_of(inode: &Inode) -> Option<&'static SysfsEntryData> {
    inode
        .private_data
        .map(|p| &*(p as *const SysfsEntryData))
}

/// Resolve a child of `dir` to (ino, kind) for lookup/iget.
fn resolve_child(dir: &SysfsEntryData, name: &[u8]) -> Option<(u64, Option<Arc<Attribute>>, Option<String>)> {
    if dir.kobj.kind != NodeKind::Directory {
        return None;
    }
    // child kobject first (dirs and links share the kobject ino space)
    if let Some(child) = dir.kobj.find_child(name) {
        let link = if let NodeKind::Link(ref t) = child.kind {
            Some(t.clone())
        } else {
            None
        };
        return Some((child.ino, None, link));
    }
    // then attributes
    if let Some(attr) = dir.kobj.find_attr(name) {
        return Some((attr.ino, Some(attr), None));
    }
    None
}

/// sysfs lookup: child kobject ino, attribute ino, or ENOENT.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let entry = entry_of(dir).ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    match resolve_child(entry, name) {
        Some((ino, _, _)) => Ok(ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// sysfs iget: instantiate the VFS Inode for a resolved child.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, i32> {
    let parent_entry = entry_of(parent).ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let (_, attr, link) = resolve_child(parent_entry, name)
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let (mode, node_ino) = if let Some(attr) = attr.as_ref() {
        (InodeMode::new(InodeMode::S_IFREG | attr.mode), attr.ino)
    } else if link.is_some() {
        (InodeMode::new(InodeMode::S_IFLNK | 0o777), ino)
    } else {
        (InodeMode::new(InodeMode::S_IFDIR | 0o755), ino)
    };

    let entry = Box::new(SysfsEntryData {
        kobj: parent_entry.kobj.clone(),
        attr,
        link,
    });

    let mut inode = Inode::new(node_ino, mode);
    inode.fs_id = FS_ID_SYSFS;
    inode.ops = Some(&SYSFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Ok(Arc::new(inode))
}

/// sysfs getattr.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let entry = match entry_of(inode) {
        Some(e) => e,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };

    stat.st_ino = inode.ino;
    if let Some(attr) = entry.attr.as_ref() {
        stat.st_mode = InodeMode::S_IFREG | attr.mode;
        // Linux sysfs stats attribute files as 4096 regardless of content.
        stat.st_size = 4096;
        stat.st_nlink = 1;
    } else if let Some(ref target) = entry.link {
        stat.st_mode = InodeMode::S_IFLNK | 0o777;
        stat.st_size = target.len() as i64;
        stat.st_nlink = 1;
    } else {
        stat.st_mode = InodeMode::S_IFDIR | 0o755;
        stat.st_size = 4096;
        stat.st_nlink = 2;
    }
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = (stat.st_size + 511) / 512;
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;
    0
}

/// sysfs setattr: ATTR_SIZE is a no-op (O_TRUNC on `echo x > file` must
/// not fail); everything else is EPERM — the tree is kernel-owned.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_setattr(_inode: &Inode, attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    if attr == crate::fs::inode::setattr_attr::ATTR_SIZE {
        return 0;
    }
    errno::Errno::OperationNotPermitted.as_neg_i32()
}

/// sysfs readlink.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let entry = match entry_of(inode) {
        Some(e) => e,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let target = match entry.link.as_ref() {
        Some(t) => t,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let len = target.len().min(buf.len());
    buf[..len].copy_from_slice(&target.as_bytes()[..len]);
    len as isize
}

/// sysfs readdir: children (dirs + links) then attributes.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
    let entry = entry_of(inode)?;
    if entry.kobj.kind != NodeKind::Directory {
        return None;
    }
    let mut out = Vec::new();

    let children = entry.kobj.children.lock();
    for (name, child) in children.iter() {
        let dt = match child.kind {
            NodeKind::Directory => file_type::DT_DIR,
            NodeKind::Link(_) => file_type::DT_LNK,
        };
        out.push(VfsDirEntry {
            ino: child.ino,
            name: name.as_bytes().to_vec(),
            file_type: dt,
        });
    }
    drop(children);

    let attributes = entry.kobj.attributes.lock();
    for (name, attr) in attributes.iter() {
        out.push(VfsDirEntry {
            ino: attr.ino,
            name: name.as_bytes().to_vec(),
            file_type: file_type::DT_REG,
        });
    }

    Some(out)
}

/// sysfs open: snapshot the attribute content into file-private storage
/// (same discipline as procfs — read side never re-enters the tree).
/// SAFETY: VFS callback contract.
unsafe fn sysfs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
    if !inode.mode.is_regular_file() {
        return 0;
    }
    let entry = match entry_of(inode) {
        Some(e) => e,
        None => return 0,
    };
    let attr = match entry.attr.as_ref() {
        Some(a) => a,
        None => return 0,
    };
    let content = match attr.show.as_ref() {
        Some(show) => show(),
        None => Vec::new(),
    };
    attr.cached_size.store(content.len() as u64, Ordering::Relaxed);
    let snapshot = Box::new(SysfsFileContent { data: content });
    file.set_private_data(Box::into_raw(snapshot) as *mut u8);
    0
}

/// sysfs get_file_ops.
/// SAFETY: VFS callback contract.
unsafe fn sysfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&SYSFS_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// sysfs destroy_inode: reclaim the leaked Box<SysfsEntryData>.
/// SAFETY: VFS callback contract; Drop-time hook.
unsafe fn sysfs_destroy_inode(inode: &mut Inode) {
    if let Some(ptr) = inode.private_data.take() {
        drop(Box::from_raw(ptr as *mut SysfsEntryData));
    }
}

/// SysFS inode operations table.
pub static SYSFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(sysfs_lookup),
    create: None,      // sysfs is kernel-owned
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: Some(sysfs_readlink),
    get_file_ops: Some(sysfs_get_file_ops),
    readdir: Some(sysfs_readdir),
    open: Some(sysfs_open),
    permission: None, // default DAC path
    getattr: Some(sysfs_getattr),
    setattr: Some(sysfs_setattr),
    iget: Some(sysfs_iget),
    destroy_inode: Some(sysfs_destroy_inode),
};

// ============================================================================
// SysFS file operations (attribute read/write)
// ============================================================================

/// Attribute content snapshot (File::private_data while open).
pub struct SysfsFileContent {
    /// generated content
    pub data: Vec<u8>,
}

/// read: serve from the open-time snapshot at the file position.
fn sysfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data was installed by sysfs_open under this File.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in sysfs_open.
    let content = unsafe { &*(ptr as *const SysfsFileContent) };
    let offset = file.get_pos() as usize;
    let available = content.data.len().saturating_sub(offset);
    let to_read = buf.len().min(available);
    if to_read > 0 {
        buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);
        file.set_pos((offset + to_read) as u64);
        to_read as isize
    } else {
        0
    }
}

/// write: dispatch to the attribute store handler.
fn sysfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: inode pointer set at open; read-only access here.
    let inode = match unsafe { (*file.inode.get()).clone() } {
        Some(i) => i,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: inode provenance is the sysfs iget path.
    let entry = match unsafe { entry_of(&inode) } {
        Some(e) => e,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    let attr = match entry.attr.as_ref() {
        Some(a) => a,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    match attr.store.as_ref() {
        Some(store) => {
            let ret = store(buf);
            if ret != 0 {
                ret as isize
            } else {
                buf.len() as isize
            }
        }
        None => errno::Errno::InvalidArgument.as_neg_i32() as isize, // read-only node
    }
}

/// lseek within the snapshot.
fn sysfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see sysfs_file_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in sysfs_open.
    let content = unsafe { &*(ptr as *const SysfsFileContent) };
    let file_size = content.data.len() as isize;
    let new_offset = match whence {
        0 => offset,
        1 => file.get_pos() as isize + offset,
        2 => file_size + offset,
        _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    if new_offset < 0 || new_offset > file_size {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }
    file.set_pos(new_offset as u64);
    new_offset
}

/// close: free the content snapshot.
fn sysfs_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see sysfs_file_read; null-out first to close the race window.
    let ptr = unsafe { (*file.private_data.get()).take() };
    if let Some(ptr) = ptr {
        // SAFETY: provenance is Box::into_raw in sysfs_open.
        drop(unsafe { Box::from_raw(ptr as *mut SysfsFileContent) });
    }
    0
}

/// SysFS attribute file operations.
pub static SYSFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(sysfs_file_read),
    write: Some(sysfs_file_write),
    lseek: Some(sysfs_file_lseek),
    close: Some(sysfs_file_close),
    poll: None,
};
