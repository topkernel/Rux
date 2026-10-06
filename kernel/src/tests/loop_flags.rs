//! Loop-device ioctl errno semantics and FS_IOC chattr-flag gating.
//!
//! Covers the r10 "device acquisition" family kernel fixes:
//! - unbound loop devices answer ENXIO (not EINVAL/ENOTTY) for the
//!   loop-status ioctls — LTP's tst_detach_device loops LOOP_CLR_FD
//!   until it sees ENXIO;
//! - FS_IOC_GETFLAGS/SETFLAGS style flags gate unlink/rmdir with EPERM
//!   (IMMUTABLE/APPEND-only; LTP unlink09).

use super::{test_fail, test_group_start, test_pass};

/// Build a synthetic /dev/loopN file (the devfs_open pattern: LOOP_FILE_OPS
/// ops identity + boxed DevNo in private_data) for loop_file_ioctl.
fn make_loop_file(minor: u32) -> alloc::sync::Arc<crate::fs::file::File> {
    let file = alloc::sync::Arc::new(crate::fs::file::File::new(
        crate::fs::file::FileFlags::new(crate::fs::file::FileFlags::O_RDWR),
    ));
    file.set_ops(&crate::fs::devfs::LOOP_FILE_OPS);
    let devno = crate::fs::dev_t::DevNo::new(crate::fs::dev_t::LOOP_MAJOR, minor);
    let b = alloc::boxed::Box::new(devno);
    file.set_private_data(alloc::boxed::Box::into_raw(b) as *mut u8);
    file
}

pub fn test_loop_flags() {
    test_group_start("loop+flags");

    // ---- Wire-layout sanity: the serializers must match the UAPI ----
    // Wire-size sanity is enforced by the ioctl serializer itself
    // (LOOP_INFO64_SIZE constant in loop_dev.rs); no unit assert here.

    // ---- Unbound-loop errno semantics (want ENXIO = -6) ----
    // Loop 7 is never bound by other tests; if it ever is, skip rather
    // than misreport (first probe re-checks).
    if crate::drivers::loop_dev::loop_is_bound(7) {
        super::test_skip("loop7 unbound probes", "loop7 unexpectedly bound");
        return;
    }
    let file = make_loop_file(7);
    let mut scratch = [0u8; 232];
    let arg = scratch.as_mut_ptr() as usize;

    let cases: [(&str, u32); 4] = [
        ("LOOP_GET_STATUS unbound -> ENXIO", 0x4C03),
        ("LOOP_GET_STATUS64 unbound -> ENXIO", 0x4C05),
        ("LOOP_CLR_FD unbound -> ENXIO", 0x4C01),
        ("LOOP_SET_CAPACITY unbound -> ENXIO", 0x4C07),
    ];
    for (name, req) in cases {
        let ret = crate::drivers::loop_dev::loop_file_ioctl(&file, req, arg);
        match ret {
            Some(-6) => test_pass(name),
            Some(other) => test_fail(
                name,
                &alloc::format!("expected -6 (ENXIO), got {}", other),
            ),
            None => test_fail(name, "loop_file_ioctl returned None (not dispatched)"),
        }
    }
    // LOOP_CLR_FD twice: the second must still be ENXIO — EINVAL here was
    // the r10 "unexpectedly failed with: EINVAL" TWARN after every
    // device-using LTP test.
    let _ = crate::drivers::loop_dev::loop_clr_fd(7);
    match crate::drivers::loop_dev::loop_file_ioctl(&file, 0x4C01, 0) {
        Some(-6) => test_pass("LOOP_CLR_FD repeat -> ENXIO"),
        Some(other) => test_fail(
            "LOOP_CLR_FD repeat",
            &alloc::format!("expected -6, got {}", other),
        ),
        None => test_fail("LOOP_CLR_FD repeat", "not dispatched"),
    }

    // ---- FS_IOC chattr flags gate unlink with EPERM ----
    use crate::fs::inode::{FS_APPEND_FL, FS_IMMUTABLE_FL};
    let path = "/unit_loop_flag_test";
    match crate::fs::file_open(path, crate::fs::file::FileFlags::O_WRONLY | crate::fs::file::FileFlags::O_CREAT, 0o600) {
        Ok(fd) => {
            let mut flagged = 0usize;
            if let Some(file) = (unsafe { crate::fs::get_file_fd(fd) }) {
                // SAFETY: inode cell written once at open time; read-only.
                if let Some(inode) = (unsafe { &*file.inode.get() }).as_ref() {
                    inode.ioc_flags.store(FS_IMMUTABLE_FL, core::sync::atomic::Ordering::Release);
                    flagged = 1;
                }
            }
            if flagged == 1 {
                match crate::fs::vfs::file_unlink(path) {
                    Err(e) if e == -crate::syscall::errno::EPERM => {
                        test_pass("unlink IMMUTABLE file -> EPERM");
                        // Append-only next.
                        if let Some(file) = (unsafe { crate::fs::get_file_fd(fd) }) {
                            // SAFETY: inode cell written once at open time.
                            if let Some(inode) = (unsafe { &*file.inode.get() }).as_ref() {
                                inode.ioc_flags.store(FS_APPEND_FL, core::sync::atomic::Ordering::Release);
                            }
                        }
                        match crate::fs::vfs::file_unlink(path) {
                            Err(e) if e == -crate::syscall::errno::EPERM => {
                                test_pass("unlink APPEND file -> EPERM");
                            }
                            _ => test_fail("unlink APPEND file", "expected EPERM"),
                        }
                    }
                    _ => test_fail("unlink IMMUTABLE file", "expected EPERM"),
                }
                // Clear and remove the scratch file.
                if let Some(file) = (unsafe { crate::fs::get_file_fd(fd) }) {
                    // SAFETY: inode cell written once at open time.
                    if let Some(inode) = (unsafe { &*file.inode.get() }).as_ref() {
                        inode.ioc_flags.store(0, core::sync::atomic::Ordering::Release);
                    }
                }
                let _ = crate::fs::vfs::file_unlink(path);
            } else {
                test_fail("FS_IOC gate setup", "could not reach inode of scratch file");
            }
            let _ = crate::fs::file_close(fd);
        }
        Err(_) => super::test_skip("FS_IOC gate", "scratch file create failed"),
    }

    test_pass("loop+flags group complete");
}
