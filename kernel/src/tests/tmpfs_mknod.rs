//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Boot-time unit tests for tmpfs mknod (OH Phase 1b gap 1).
//!
//! OH's init mounts its own tmpfs over /dev and mknods
//! /dev/{null,random,urandom,kmsg} there. These checks exercise the full
//! chain in the live VFS:
//!   - mknod on a tmpfs creates a device-typed node (stat: S_IFCHR +
//!     st_rdev in userspace dev_t encoding, d_type DT_CHR in readdir),
//!   - open() binds the node to the CharDev registry by rdev (/dev/null
//!     ops discard writes and EOF reads),
//!   - may_mknod parity (EEXIST on a taken name; EPERM on S_IFDIR),
//!   - new_decode_dev parity for the dev_t encoding (from_user_dev).
//!
//! Runs on the boot task (root), so the CAP_MKNOD gate passes.

use super::{test_fail, test_group_start, test_pass};
use crate::fs::inode::InodeMode;
use crate::fs::vfs::{file_read, file_write};

pub fn test_tmpfs_mknod() {
    test_group_start("tmpfs-mknod");

    // ---- 1. dev_t encode/decode parity (new_encode_dev/new_decode_dev) --
    // makedev(1,3) == 0x103; the round trip must preserve major/minor.
    let dn = crate::fs::dev_t::DevNo::new(1, 3);
    if dn.to_user_dev() == 0x103 {
        test_pass("to_user_dev(1:3) == 0x103");
    } else {
        test_fail("to_user_dev(1:3)", &alloc::format!("{:#x}", dn.to_user_dev()));
    }
    let back = crate::fs::dev_t::DevNo::from_user_dev(0x103);
    if back.major == 1 && back.minor == 3 {
        test_pass("from_user_dev(0x103) == 1:3");
    } else {
        test_fail("from_user_dev(0x103)", &alloc::format!("{}:{}", back.major, back.minor));
    }
    // Large minors split across the two fields: minor 511 (0x1FF).
    let dn2 = crate::fs::dev_t::DevNo::new(254, 511);
    let rt = crate::fs::dev_t::DevNo::from_user_dev(dn2.to_user_dev());
    if rt.major == 254 && rt.minor == 511 {
        test_pass("from_user_dev(to_user_dev(254:511)) round trip");
    } else {
        test_fail("dev_t round trip 254:511", &alloc::format!("{}:{}", rt.major, rt.minor));
    }

    // ---- 2. mknod a char node on a fresh tmpfs mounted at /tmp/mknodtest -
    let mp = "/tmp/mknodtest";
    match crate::fs::mount::do_mount("none", mp, "tmpfs", 0) {
        Ok(()) => test_pass("tmpfs mount /tmp/mknodtest"),
        Err(_) => test_fail("tmpfs mount /tmp/mknodtest", "do_mount failed"),
    }
    let null_path = alloc::format!("{}/null", mp);
    // mknod(2) userspace args: mode S_IFCHR|0666, dev makedev(1,3)=0x103.
    match crate::fs::vfs::vfs_mknod(&null_path, InodeMode::S_IFCHR | 0o666, 0x103) {
        Ok(()) => test_pass("vfs_mknod tmpfs char node"),
        Err(e) => test_fail("vfs_mknod tmpfs char node", &alloc::format!("errno {}", -e)),
    }

    // EEXIST on the taken name (do_mknodat user_path_create parity).
    match crate::fs::vfs::vfs_mknod(&null_path, InodeMode::S_IFCHR | 0o666, 0x103) {
        Err(e) if e == -(crate::errno::constants::EEXIST) => {
            test_pass("mknod EEXIST on taken name")
        }
        _ => test_fail("mknod EEXIST on taken name", "expected EEXIST"),
    }

    // EPERM on S_IFDIR (may_mknod).
    let dir_path = alloc::format!("{}/adir", mp);
    match crate::fs::vfs::vfs_mknod(&dir_path, InodeMode::S_IFDIR | 0o755, 0) {
        Err(e) if e == -(crate::errno::constants::EPERM) => {
            test_pass("mknod S_IFDIR -> EPERM")
        }
        _ => test_fail("mknod S_IFDIR -> EPERM", "expected EPERM"),
    }

    // ---- 3. stat: S_IFCHR + st_rdev userspace encoding -------------------
    let mut st = crate::fs::Stat::default();
    let st_ok = crate::fs::vfs::path_lookup(&null_path, 0)
        .ok()
        .and_then(|vp| vp.inode)
        .map(|inode| inode.op_getattr(&mut st));
    if st_ok == Some(0) {
        test_pass("stat tmpfs char node");
        if st.st_mode & InodeMode::S_IFMT == InodeMode::S_IFCHR {
            test_pass("stat mode S_IFCHR");
        } else {
            test_fail("stat mode S_IFCHR", &alloc::format!("{:#o}", st.st_mode));
        }
        if st.st_rdev == 0x103 {
            test_pass("stat st_rdev == 0x103 (makedev(1,3))");
        } else {
            test_fail("stat st_rdev", &alloc::format!("{:#x}", st.st_rdev));
        }
        if st.st_mode & 0o777 == 0o666 {
            test_pass("stat perm 0666");
        } else {
            test_fail("stat perm 0666", &alloc::format!("{:#o}", st.st_mode & 0o777));
        }
    } else {
        test_fail("stat tmpfs char node", "lookup/getattr failed");
    }

    // ---- 4. readdir d_type: DT_CHR ---------------------------------------
    // Walk the directory via file_opendir + getdents64 (the same stream
    // userspace sees) and parse linux_dirent64 records:
    // u64 d_ino | i64 d_off | u16 d_reclen | u8 d_type | char d_name[].
    let dtype_found = crate::fs::vfs::file_opendir(mp, 0 /*O_RDONLY*/).ok().and_then(|fd| {
        let mut buf = [0u8; 512];
        let mut found = false;
        loop {
            match crate::fs::vfs::file_getdents64(fd, &mut buf, 512) {
                Ok(0) | Err(_) => break,
                Ok(total) => {
                    let mut off = 0usize;
                    while off + 19 <= total {
                        let reclen =
                            u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
                        if reclen < 19 || off + reclen > total {
                            break;
                        }
                        let dtype = buf[off + 18];
                        let name = &buf[off + 19..off + reclen];
                        let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
                        if name == b"null".as_slice()
                            && dtype == crate::fs::inode::file_type::DT_CHR
                        {
                            found = true;
                        }
                        off += reclen;
                    }
                    if found {
                        break;
                    }
                }
            }
        }
        // SAFETY: fd is a valid directory descriptor from file_opendir.
        unsafe { crate::fs::close_file_fd(fd); }
        Some(found)
    });
    if dtype_found == Some(true) {
        test_pass("readdir d_type DT_CHR for the node");
    } else {
        test_fail("readdir d_type DT_CHR", "null/DT_CHR not found");
    }

    // ---- 5. open binds through the CharDev registry by rdev --------------
    // /dev/null ops: write discards (full length), read returns EOF (0).
    match crate::fs::file_open(&null_path, 0o2 /*O_RDWR*/, 0) {
        Ok(fd) => {
            test_pass("open tmpfs /dev/null (registry bind)");
            let wbuf = b"discard-me";
            match file_write(fd, wbuf, wbuf.len()) {
                Ok(n) if n == wbuf.len() => test_pass("write via null ops discards all"),
                r => test_fail("write via null ops", &alloc::format!("{:?}", r)),
            }
            let mut rbuf = [0u8; 16];
            match file_read(fd, &mut rbuf, 16) {
                Ok(0) => test_pass("read via null ops returns EOF"),
                r => test_fail("read via null ops", &alloc::format!("{:?}", r)),
            }
            // SAFETY: fd is a valid descriptor from file_open above.
            unsafe { crate::fs::close_file_fd(fd); }
        }
        Err(_) => test_fail("open tmpfs /dev/null (registry bind)", "file_open failed"),
    }

    // ---- 6. hard link preserves the device identity ----------------------
    let link_path = alloc::format!("{}/null-link", mp);
    match crate::fs::vfs::vfs_link(&null_path, &link_path) {
        Ok(()) => test_pass("hard link a device node"),
        Err(_) => test_fail("hard link a device node", "vfs_link failed"),
    }
    let mut st2 = crate::fs::Stat::default();
    let st2_ok = crate::fs::vfs::path_lookup(&link_path, 0)
        .ok()
        .and_then(|vp| vp.inode)
        .map(|inode| inode.op_getattr(&mut st2));
    if st2_ok == Some(0)
        && st2.st_mode & InodeMode::S_IFMT == InodeMode::S_IFCHR
        && st2.st_rdev == 0x103
    {
        test_pass("link preserves S_IFCHR + rdev");
    } else {
        test_fail("link preserves S_IFCHR + rdev", "type/rdev mismatch");
    }
}
