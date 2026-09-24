//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Loadable kernel module registry (kmod support, Ubuntu wave U2)
//!
//! ## Scope
//!
//! This is a REGISTRATION-ONLY implementation of the Linux module
//! syscalls (`init_module` / `finit_module` / `delete_module`). No code
//! is linked or executed from the blob — the kernel has no runtime
//! module loader. The contract that matters for udev/systemd boot is:
//!
//! 1. `modprobe` (kmod) must succeed when told to load a module. kmod
//!    opens `/lib/modules/<release>/<name>.ko`, `mmap`s it and calls
//!    `finit_module(fd, "", 0)`. A returned 0 makes modprobe exit 0,
//!    which is all udev's coldboot requires.
//! 2. Loaded modules must be listed in `/proc/modules` and appear under
//!    `/sys/module/<name>/` — the two places `lsmod`/udev inspect.
//!
//! ## ELF handling
//!
//! The blob is validated as an ELF64 relocatable object (ET_REL — every
//! Linux `.ko` is ET_REL) and the module NAME is extracted from the
//! `.modinfo` section (`name=<value>\0`), mirroring
//! `kernel/module/main.c:find_module()`. If `.modinfo` carries no name
//! the load fails with EINVAL — delete_module(2) needs a name, so
//! registering an anonymous module would be un-removable.
//!
//! finit_module reads the blob from a file descriptor with
//! `File::read_at` (position-invariant pread — it must not consume the
//! shared file offset, kmod keeps the fd open and may retry).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::errno;
use crate::sync::spinlock::Spinlock;

/// Upper bound on the module blob we copy into the kernel (Linux uses
/// 16 MiB as a sanity limit for init_module; real RISC-V .ko files are
/// a few hundred KiB).
const MAX_MODULE_SIZE: usize = 16 * 1024 * 1024;

/// Per-module bookkeeping.
pub struct ModuleInfo {
    /// Module name (from .modinfo "name=")
    pub name: String,
    /// Blob size in bytes (what /proc/modules reports)
    pub size: usize,
    /// Reference count (always 0 — nothing can take one)
    pub refcnt: u32,
}

/// The module table: name → info. BTreeMap keeps /proc/modules sorted
/// by name, matching Linux.
static MODULES_TABLE: Spinlock<BTreeMap<String, ModuleInfo>> =
    Spinlock::new(BTreeMap::new());

// ============================================================================
// ELF parsing (minimal: header + section table + .modinfo scan)
// ============================================================================

/// ELF64 section header (only the fields we read).
#[repr(C)]
struct Elf64Shdr {
    sh_name: u32,
    sh_type: u32,
    _sh_flags: u64,
    sh_offset: u64,
    sh_size: u64,
}

/// Read a big-endian-independent (ELF is little-endian on RISC-V) u16/u32
/// from a byte slice at the given offset.
fn rd16(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn rd32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd64(buf: &[u8], off: usize) -> Option<u64> {
    let b = buf.get(off..off + 8)?;
    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Section header entry size + table location from the ELF header.
struct SectionTable {
    shoff: u64,
    shentsize: u16,
    shnum: u16,
    shstrndx: u16,
}

fn parse_ehdr(buf: &[u8]) -> Result<SectionTable, i32> {
    // ELF magic + 64-bit + little-endian + relocatable object.
    if buf.len() < 64 {
        return Err(errno::Errno::ExecFormatError.as_neg_i32());
    }
    if &buf[0..4] != b"\x7fELF" {
        return Err(errno::Errno::ExecFormatError.as_neg_i32());
    }
    if buf[4] != 2 {
        return Err(errno::Errno::ExecFormatError.as_neg_i32()); // not ELFCLASS64
    }
    let e_type = rd16(buf, 16).ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;
    const ET_REL: u16 = 1;
    if e_type != ET_REL {
        // Not a relocatable object — not a loadable module.
        return Err(errno::Errno::ExecFormatError.as_neg_i32());
    }

    // e_shoff @ 0x28, e_shentsize @ 0x3A, e_shnum @ 0x3C, e_shstrndx @ 0x3E.
    let shoff = rd64(buf, 0x28).ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;
    let shentsize = rd16(buf, 0x3A).ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;
    let shnum = rd16(buf, 0x3C).ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;
    let shstrndx = rd16(buf, 0x3E).ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;

    if shoff == 0 || shentsize < core::mem::size_of::<Elf64Shdr>() as u16 || shnum == 0 {
        return Err(errno::Errno::ExecFormatError.as_neg_i32());
    }
    Ok(SectionTable {
        shoff,
        shentsize,
        shnum,
        shstrndx,
    })
}

/// Read section header `idx` from the blob.
fn shdr(buf: &[u8], st: &SectionTable, idx: u16) -> Option<Elf64Shdr> {
    if idx as u64 >= st.shnum as u64 {
        return None;
    }
    let off = st.shoff as usize + idx as usize * st.shentsize as usize;
    // SAFETY-free manual parse (repr(C) layout is little-endian RISC-V):
    // read field-by-field to avoid unaligned reads of packed kernels'
    // section tables (.ko files align shdrs to 8, but be conservative).
    Some(Elf64Shdr {
        sh_name: rd32(buf, off)?,
        sh_type: rd32(buf, off + 4)?,
        _sh_flags: rd64(buf, off + 8)?,
        sh_offset: rd64(buf, off + 0x18)?,
        sh_size: rd64(buf, off + 0x20)?,
    })
}

/// Extract the module name from the `.modinfo` section: a sequence of
/// NUL-terminated "key=value" strings; we need `name=<value>`.
fn module_name_from_blob(buf: &[u8]) -> Result<String, i32> {
    let st = parse_ehdr(buf)?;

    // Section header string table (section names).
    let shstr = shdr(buf, &st, st.shstrndx)
        .ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;
    let names_off = shstr.sh_offset as usize;
    let names_len = shstr.sh_size as usize;
    let names = buf
        .get(names_off..names_off + names_len)
        .ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;

    let name_from_strtab = |stroff: u32| -> Option<&str> {
        let start = stroff as usize;
        if start >= names.len() {
            return None;
        }
        let end = names[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)?;
        core::str::from_utf8(&names[start..end]).ok()
    };

    // Walk sections looking for .modinfo (type PROGBITS, name ".modinfo").
    const SHT_PROGBITS: u32 = 1;
    for idx in 0..st.shnum {
        let sh = match shdr(buf, &st, idx) {
            Some(s) => s,
            None => break,
        };
        if sh.sh_type != SHT_PROGBITS {
            continue;
        }
        if name_from_strtab(sh.sh_name) != Some(".modinfo") {
            continue;
        }
        let mstart = sh.sh_offset as usize;
        let mlen = sh.sh_size as usize;
        let info = buf
            .get(mstart..mstart + mlen)
            .ok_or(errno::Errno::ExecFormatError.as_neg_i32())?;

        // info = "key=value\0key=value\0..."
        let mut pos = 0usize;
        while pos < info.len() {
            let end = info[pos..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| pos + p)
                .unwrap_or(info.len());
            if let Ok(kv) = core::str::from_utf8(&info[pos..end]) {
                if let Some(value) = kv.strip_prefix("name=") {
                    if !value.is_empty() {
                        return Ok(String::from(value));
                    }
                }
            }
            pos = end + 1;
        }
        // .modinfo present but no name= entry.
        break;
    }

    Err(errno::Errno::InvalidArgument.as_neg_i32())
}

// ============================================================================
// sysfs exposure (/sys/module/<name>)
// ============================================================================

/// Create /sys/module/<name> with the attributes lsmod/systemd expect:
/// `initstate` ("live") and `refcnt` ("0"). Uses only the public sysfs
/// KObject API; idempotent.
fn sysfs_publish(name: &str) {
    use crate::fs::sysfs::{sysfs_root, KObject, KType, NodeKind};

    let root = match sysfs_root() {
        Some(r) => r,
        None => return,
    };
    let module_dir = match root.find_child(b"module") {
        Some(d) => d,
        None => return,
    };
    if module_dir.find_child(name.as_bytes()).is_some() {
        return; // already published
    }

    let dir = KObject::new(name, KType::Module, NodeKind::Directory);
    let initstate = String::from("live\n");
    dir.set_attr("initstate", 0o444, Some(alloc::boxed::Box::new(move || {
        initstate.as_bytes().to_vec()
    })), None);
    dir.set_attr("refcnt", 0o444, Some(alloc::boxed::Box::new(|| b"0\n".to_vec())), None);
    module_dir.add_child(dir);
}

/// Remove /sys/module/<name> (delete_module path).
fn sysfs_unpublish(name: &str) {
    let root = match crate::fs::sysfs::sysfs_root() {
        Some(r) => r,
        None => return,
    };
    if let Some(module_dir) = root.find_child(b"module") {
        module_dir.remove_child(name);
    }
}

// ============================================================================
// Load / unload
// ============================================================================

/// Register a module blob. Returns the module name on success.
///
/// This performs NO relocation or execution — see the module doc header.
pub fn load_module_blob(blob: &[u8]) -> Result<String, i32> {
    if blob.is_empty() || blob.len() > MAX_MODULE_SIZE {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    let name = module_name_from_blob(blob)?;

    let mut table = MODULES_TABLE.lock_irqsave();
    if table.contains_key(&name) {
        // Linux: EEXIST for a duplicate live module. modprobe maps
        // EEXIST to "already loaded" success.
        return Err(errno::Errno::FileExists.as_neg_i32());
    }
    table.insert(
        name.clone(),
        ModuleInfo {
            name: name.clone(),
            size: blob.len(),
            refcnt: 0,
        },
    );
    drop(table);

    sysfs_publish(&name);
    crate::printk::printk(
        crate::printk::loglevel::KERN_INFO,
        format_args!("module: registered '{}' ({} bytes, no linking)\n", name, blob.len()),
    );
    Ok(name)
}

/// Remove a module by name. Fails with EBUSY while referenced (nothing
/// takes references today, so this only guards the future).
pub fn delete_module_by_name(name: &str) -> Result<(), i32> {
    let mut table = MODULES_TABLE.lock_irqsave();
    match table.get(name) {
        Some(info) => {
            if info.refcnt > 0 {
                return Err(errno::Errno::DeviceOrResourceBusy.as_neg_i32());
            }
        }
        None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
    table.remove(name);
    drop(table);

    sysfs_unpublish(name);
    Ok(())
}

// ============================================================================
// /proc/modules
// ============================================================================

/// Generate /proc/modules content.
///
/// Linux format: `<name> <size> <refcnt> <deps> <state> <address>`, one
/// line per module, e.g. `virtio_blk 20480 0 - Live 0x000...`.
pub fn generate_proc_modules() -> Vec<u8> {
    let table = MODULES_TABLE.lock_irqsave();
    let mut out = String::new();
    for info in table.values() {
        out.push_str(&format!(
            "{} {} {} - Live 0x0000000000000000\n",
            info.name, info.size, info.refcnt
        ));
    }
    out.into_bytes()
}

// ============================================================================
// Syscall backing (user-memory plumbing)
// ============================================================================

/// Capability gate shared by all three module syscalls.
fn check_cap_sys_module() -> bool {
    crate::security::capable(crate::security::CAP_SYS_MODULE)
}

/// init_module(2): blob passed directly from user memory.
pub fn sys_init_module_impl(umod: usize, len: usize) -> i64 {
    if !check_cap_sys_module() {
        return -(errno::Errno::OperationNotPermitted.as_i32() as i64);
    }
    if len == 0 || len > MAX_MODULE_SIZE {
        return -(errno::Errno::InvalidArgument.as_i32() as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(umod, len) {
        return -(errno::Errno::BadAddress.as_i32() as i64);
    }

    let mut blob = vec![0u8; len];
    // SAFETY: umod/len validated with access_ok; copy_from_user faults
    // safely on bad pages and reports the uncopied tail.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(blob.as_mut_ptr(), umod as *const u8, len)
    };
    if uncopied > 0 {
        return -(errno::Errno::BadAddress.as_i32() as i64);
    }

    match load_module_blob(&blob) {
        Ok(_) => 0,
        Err(e) => -(e as i64),
    }
}

/// finit_module(2): read the blob from a file descriptor.
///
/// Uses read_at (pread semantics) so the shared file offset is not
/// consumed — kmod keeps the fd open and may retry the load.
pub fn sys_finit_module_impl(fd: usize, _param_flags: u64) -> i64 {
    if !check_cap_sys_module() {
        return -(errno::Errno::OperationNotPermitted.as_i32() as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::Errno::BadFileNumber.as_i32() as i64),
    };
    let file = match fdtable.get_file(fd) {
        Some(f) => f,
        None => return -(errno::Errno::BadFileNumber.as_i32() as i64),
    };

    // File size from the backing inode.
    let size = {
        // SAFETY: inode is set at open time; read-only access here.
        let inode = unsafe { (*file.inode.get()).clone() };
        match inode {
            Some(i) => i.size.load(core::sync::atomic::Ordering::Relaxed) as usize,
            None => return -(errno::Errno::InvalidArgument.as_i32() as i64),
        }
    };
    if size == 0 || size > MAX_MODULE_SIZE {
        return -(errno::Errno::InvalidArgument.as_i32() as i64);
    }

    let mut blob = vec![0u8; size];
    // SAFETY: blob is a valid kernel buffer of `size` bytes; read_at
    // validates its own inode references.
    let n = unsafe { file.read_at(0, blob.as_mut_ptr(), size) };
    if n < 0 {
        return n as i64;
    }
    if n as usize != size {
        return -(errno::Errno::InvalidArgument.as_i32() as i64);
    }

    match load_module_blob(&blob) {
        Ok(_) => 0,
        Err(e) => -(e as i64),
    }
}

/// delete_module(2): NUL-terminated module name from user memory.
pub fn sys_delete_module_impl(name_user: usize, _flags: u64) -> i64 {
    if !check_cap_sys_module() {
        return -(errno::Errno::OperationNotPermitted.as_i32() as i64);
    }
    if !crate::arch::riscv64::uaccess::access_ok(name_user, 1) {
        return -(errno::Errno::BadAddress.as_i32() as i64);
    }

    // Copy up to MODULE_NAME_LEN+1 bytes (Linux: 56), stopping at NUL.
    const MODULE_NAME_LEN: usize = 56;
    let mut name_buf = [0u8; MODULE_NAME_LEN + 1];
    for i in 0..=MODULE_NAME_LEN {
        // SAFETY: byte-by-byte access_ok-guarded user read; copy_from_user
        // handles faulting pages (returns uncopied count).
        let mut one = [0u8; 1];
        let uncopied = unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(
                one.as_mut_ptr(),
                (name_user + i) as *const u8,
                1,
            )
        };
        if uncopied > 0 {
            return -(errno::Errno::BadAddress.as_i32() as i64);
        }
        if one[0] == 0 {
            break;
        }
        name_buf[i] = one[0];
    }
    if name_buf[0] == 0 {
        return -(errno::Errno::InvalidArgument.as_i32() as i64); // empty name
    }
    // Reject a non-terminated over-long name.
    if name_buf[MODULE_NAME_LEN] != 0 {
        return -(errno::Errno::InvalidArgument.as_i32() as i64);
    }

    let name = match core::str::from_utf8(&name_buf[..MODULE_NAME_LEN]) {
        Ok(s) => s.trim_end_matches('\0'),
        Err(_) => return -(errno::Errno::InvalidArgument.as_i32() as i64),
    };

    match delete_module_by_name(name) {
        Ok(()) => 0,
        Err(e) => -(e as i64),
    }
}
