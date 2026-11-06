//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! pty master after slave hangup: buffered slave output is still readable,
//! EIO only once it is drained (reference n_tty_read); TIOCGPTPEER returns
//! the slave fd.

use crate::fs::{file_close, get_file_fd, vfs, FileFlags};
use super::{test_pass, test_fail, test_skip, test_group_start};

const TIOCSPTLCK: u32 = 0x4004_5431;
const TIOCGPTPEER: u32 = 0x5441;
const EIO: isize = -5;

pub fn test_pty_hangup() {
    test_group_start("pty hangup");

    let mfd = match vfs::file_open("/dev/ptmx", FileFlags::O_RDWR, 0) {
        Ok(fd) => fd,
        Err(_) => {
            test_skip("pty hangup", "cannot open /dev/ptmx");
            return;
        }
    };
    // SAFETY: mfd was just installed in this task's table.
    let master = match unsafe { get_file_fd(mfd) } {
        Some(f) => f,
        None => {
            test_fail("pty hangup", "master fd lookup failed");
            return;
        }
    };
    let _ = crate::fs::pty::pty_ioctl(&master, TIOCSPTLCK, 0); // unlockpt
    let sfd = match crate::fs::pty::pty_ioctl(&master, TIOCGPTPEER, 0o2 /* O_RDWR */) {
        Some(fd) if fd >= 0 => fd as usize,
        other => {
            test_fail("TIOCGPTPEER", &alloc::format!("returned {:?}", other));
            let _ = file_close(mfd);
            return;
        }
    };
    test_pass("TIOCGPTPEER returns the slave fd");

    // SAFETY: sfd was just installed by TIOCGPTPEER.
    match unsafe { get_file_fd(sfd) } {
        Some(slave) => unsafe {
            let msg = b"hi\n";
            let _ = slave.write(msg.as_ptr(), msg.len());
        },
        None => test_fail("pty hangup", "slave fd lookup failed"),
    }
    let _ = file_close(sfd); // last slave reference: hangup

    let mut buf = [0u8; 16];
    // SAFETY: buf is a kernel buffer of buf.len() bytes.
    let first = unsafe { master.read(buf.as_mut_ptr(), buf.len()) };
    let second = unsafe { master.read(buf.as_mut_ptr(), buf.len()) };
    if first > 0 && second == EIO {
        test_pass("master reads buffered output, then EIO after slave hangup");
    } else {
        test_fail(
            "master read after hangup",
            &alloc::format!("first={} second={}", first, second),
        );
    }
    drop(master);
    let _ = file_close(mfd);
}
