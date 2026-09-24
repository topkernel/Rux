//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Extended attributes (P1 xattr minimal implementation)
//!
//! Storage model: the name → value map lives on the VFS `Inode`
//! (`Inode.xattrs`), so all filesystems that share VFS inodes get xattrs
//! for free. Values are in-memory only:
//! - ext4 does NOT persist them — after icache eviction of the inode (or
//!   reboot) the attributes are gone (documented limitation; an on-disk
//!   xattr block in the ext4 inode is future work).
//! - rootfs/tmpfs/devfs/procfs inodes are memory-resident, so xattrs on
//!   them survive as long as the inode does.
//!
//! Namespaces (Linux semantics, minimal):
//! - `user.*`        — read/write for everyone subject to file permission
//! - `trusted.*`     — requires CAP_SYS_ADMIN
//! - `security.*`    — requires CAP_SYS_ADMIN (a real LSM would own these;
//!                     we only store them)
//! - `system.*`      — read-only namespace → EPERM on set, ENODATA on get
//!                     unless present (we never store any)
//! - anything else without a `<ns>.` shape → EINVAL; unknown namespace →
//!   EOPNOTSUPP (Linux returns EOPNOTSUPP when no handler matches).

use alloc::vec::Vec;

use crate::errno;
use crate::fs::inode::Inode;

/// Local errno constants not present in errno.rs's primary set.
const ENODATA: i32 = 61;
const EOPNOTSUPP: i32 = 95;

/// XATTR_SIZE_MAX (Linux): 64 KiB per value.
pub const XATTR_SIZE_MAX: usize = 65536;
/// XATTR_NAME_MAX (Linux): 255 bytes.
pub const XATTR_NAME_MAX: usize = 255;
/// XATTR_LIST_MAX (Linux): 64 KiB serialized name list.
pub const XATTR_LIST_MAX: usize = 65536;

/// setxattr flags (UAPI).
pub const XATTR_CREATE: i32 = 1;
pub const XATTR_REPLACE: i32 = 2;

/// Validate an xattr name and check create permission for its namespace.
/// Returns Ok(()) or a negative errno.
fn check_name(name: &[u8]) -> Result<(), i32> {
    if name.is_empty() || name.len() > XATTR_NAME_MAX {
        return Err(-(errno::constants::EINVAL as i32));
    }
    let dot = match name.iter().position(|&b| b == b'.') {
        Some(d) => d,
        None => return Err(-(errno::constants::EINVAL as i32)), // no namespace prefix
    };
    if dot == 0 {
        return Err(-(errno::constants::EINVAL as i32)); // empty namespace
    }
    Ok(())
}

/// Namespace write gate (mirror of Linux xattr_permission, minimal):
/// `user.*` needs write permission on the inode (checked by caller via the
/// plain DAC path — we approximate with MAY_WRITE), `trusted.*` /
/// `security.*` need CAP_SYS_ADMIN, `system.*` is read-only.
fn may_set(name: &[u8]) -> Result<(), i32> {
    let ns: &[u8] = &name[..name.iter().position(|&b| b == b'.').unwrap()];
    match ns {
        b"user" => Ok(()),
        b"trusted" | b"security" => {
            let cred = if let Some(task) = crate::sched::current() {
                task.cred().clone()
            } else {
                crate::process::task::Cred::new_init()
            };
            if crate::security::has_capability(&cred, crate::security::CAP_SYS_ADMIN) {
                Ok(())
            } else {
                Err(-(errno::constants::EPERM as i32))
            }
        }
        b"system" => Err(-(errno::constants::EPERM as i32)), // reserved / read-only
        _ => Err(-EOPNOTSUPP), // unknown namespace: no handler
    }
}

/// setxattr core. `value`/`name` are kernel copies already validated for
/// length. Flags: XATTR_CREATE (fail EEXIST when present), XATTR_REPLACE
/// (fail ENODATA when absent); both at once is EINVAL.
pub fn set(inode: &Inode, name: &[u8], value: &[u8], flags: i32) -> Result<(), i32> {
    if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }
    if flags & XATTR_CREATE != 0 && flags & XATTR_REPLACE != 0 {
        return Err(-(errno::constants::EINVAL as i32));
    }
    if value.len() > XATTR_SIZE_MAX {
        return Err(-(errno::constants::E2BIG as i32));
    }
    check_name(name)?;
    may_set(name)?;

    // user.* requires write permission on the target (Linux
    // xattr_permission MAY_WRITE gate for the user namespace).
    if name.starts_with(b"user.") {
        if !crate::fs::permission::inode_permission(inode, crate::fs::permission::MAY_WRITE) {
            return Err(-(errno::constants::EACCES as i32));
        }
    }

    let mut guard = inode.xattrs.lock();
    if flags & XATTR_CREATE != 0 {
        if let Some(map) = guard.as_ref() {
            if map.contains_key(name) {
                return Err(-(errno::constants::EEXIST as i32));
            }
        }
    }
    if guard.is_none() {
        *guard = Some(alloc::collections::BTreeMap::new());
    }
    let map = guard.as_mut().unwrap();
    if flags & XATTR_REPLACE != 0 && !map.contains_key(name) {
        return Err(-ENODATA);
    }
    map.insert(name.to_vec(), value.to_vec());
    Ok(())
}

/// getxattr core. `out` is the user buffer length (`size == 0` means "query
/// the needed size"). Returns the value length on success.
pub fn get(inode: &Inode, name: &[u8], out: Option<&mut [u8]>) -> Result<usize, i32> {
    check_name(name)?;

    // Read gate (minimal): trusted.* readable only with CAP_SYS_ADMIN.
    if name.starts_with(b"trusted.") {
        let cred = if let Some(task) = crate::sched::current() {
            task.cred().clone()
        } else {
            crate::process::task::Cred::new_init()
        };
        if !crate::security::has_capability(&cred, crate::security::CAP_SYS_ADMIN) {
            return Err(-(errno::constants::EACCES as i32));
        }
    }

    let guard = inode.xattrs.lock();
    let value = match guard.as_ref().and_then(|m| m.get(name)) {
        Some(v) => v,
        None => return Err(-ENODATA),
    };
    match out {
        None => Ok(value.len()),
        Some(buf) => {
            if buf.len() < value.len() {
                return Err(-(errno::constants::ERANGE as i32));
            }
            buf[..value.len()].copy_from_slice(value);
            Ok(value.len())
        }
    }
}

/// listxattr core. Returns the total serialized length (names + NULs);
/// copies into `out` when given, ERANGE when too small.
pub fn list(inode: &Inode, out: Option<&mut [u8]>) -> Result<usize, i32> {
    let guard = inode.xattrs.lock();
    let empty;
    let empty_map: &alloc::collections::BTreeMap<Vec<u8>, Vec<u8>> = match guard.as_ref() {
        Some(m) => m,
        None => {
            empty = alloc::collections::BTreeMap::new();
            &empty
        }
    };

    let total: usize = empty_map.keys().map(|k| k.len() + 1).sum();
    if total > XATTR_LIST_MAX {
        return Err(-(errno::constants::E2BIG as i32));
    }
    match out {
        None => Ok(total),
        Some(buf) => {
            if buf.len() < total {
                return Err(-(errno::constants::ERANGE as i32));
            }
            let mut off = 0;
            for k in empty_map.keys() {
                buf[off..off + k.len()].copy_from_slice(k);
                buf[off + k.len()] = 0;
                off += k.len() + 1;
            }
            Ok(total)
        }
    }
}

/// removexattr core. ENODATA when the attribute is absent.
pub fn remove(inode: &Inode, name: &[u8]) -> Result<(), i32> {
    check_name(name)?;
    may_set(name)?; // same permission gate as set (Linux uses __vfs_setxattr path)

    let mut guard = inode.xattrs.lock();
    match guard.as_mut().and_then(|m| m.remove(name)) {
        Some(_) => {
            // Drop the empty map so untouched inodes never pay the lock'd
            // BTreeMap allocation.
            if guard.as_ref().map(|m| m.is_empty()).unwrap_or(false) {
                *guard = None;
            }
            Ok(())
        }
        None => Err(-ENODATA),
    }
}
