// Capability-matrix shape tests (rust-design-headroom §1, divergence
// D-6). Pin which ops each builder-converted file type carries: the
// point of the capability declaration is that a type's matrix is a
// STATED fact, so an edit that adds or removes a slot (e.g. giving
// epoll a read op, which once manifested as the wrong-errno spin
// incident) fails here before it can drift into the tree.
use super::{test_fail, test_group_start, test_pass};
use crate::fs::file::FileOps;
use alloc::format;

/// Which slots a table must carry (true) or must NOT carry (false).
struct Expect {
    read: bool,
    write: bool,
    lseek: bool,
    close: bool,
    poll: bool,
    read_at: bool,
}

fn check(name: &str, ops: &FileOps, e: Expect) {
    let rows: [(&str, bool, bool); 6] = [
        ("read", ops.read.is_some(), e.read),
        ("write", ops.write.is_some(), e.write),
        ("lseek", ops.lseek.is_some(), e.lseek),
        ("close", ops.close.is_some(), e.close),
        ("poll", ops.poll.is_some(), e.poll),
        ("read_at", ops.read_at.is_some(), e.read_at),
    ];
    let mut ok = true;
    for (slot, have, want) in rows {
        if have != want {
            test_fail(
                name,
                &format!("slot {} is {}, capability matrix says {}", slot, have, want),
            );
            ok = false;
        }
    }
    if ok {
        test_pass(name);
    }
}

/// Central missing-op errno mapping (the ONE place the ABI decision
/// lives). The pinned pairs are the documented incident classes:
/// read on an op-less fd (epoll) must be EINVAL — an EBADF here once
/// made an OH event loop spin 155,991 times; lseek on a stream
/// (pipe) must be ESPIPE; pread on a stream ESPIPE, on an op-less
/// fd EINVAL.
fn check_errno_matrix() {
    use crate::fs::file::{File, FileFlags, FileOpKind};

    let mk = |ops: Option<&'static FileOps>| -> File {
        let mut f = File::new(FileFlags::new(0));
        if let Some(o) = ops {
            f.set_ops(o);
        }
        f
    };

    // Op-less type: epoll table (close only).
    let epoll_file = mk(Some(crate::syscall::misc::caps_shape::epoll_ops()));
    if epoll_file.missing_op_errno(FileOpKind::Read) != -22 {
        test_fail("errno-matrix", "epoll read: want EINVAL (-22)");
        return;
    }
    if epoll_file.missing_op_errno(FileOpKind::LSeek) != -29 {
        test_fail("errno-matrix", "epoll lseek: want ESPIPE (-29)");
        return;
    }
    if epoll_file.missing_op_errno(FileOpKind::ReadAt) != -22 {
        test_fail("errno-matrix", "epoll pread: want EINVAL (-22)");
        return;
    }

    // Stream type: pipe table (read/write/poll/close, no seek state).
    let pipe_file = mk(Some(crate::syscall::misc::caps_shape::pipe_ops()));
    if pipe_file.missing_op_errno(FileOpKind::ReadAt) != -29 {
        test_fail("errno-matrix", "pipe pread: want ESPIPE (-29)");
        return;
    }

    // poll_dispatch fallback: no poll capability -> always-ready mask.
    let r = epoll_file.poll_dispatch(0x0001 | 0x0004); // POLLIN|POLLOUT
    if r != (0x0001 | 0x0040 | 0x0004 | 0x0100) {
        test_fail("errno-matrix", "poll fallback: want POLLIN|POLLRDNORM|POLLOUT|POLLWRNORM");
        return;
    }

    test_pass("errno-matrix");
}

pub fn test_fileops_caps() {
    test_group_start("fileops-caps");

    // The §1 incident family: epoll carries ONLY close. Every other
    // capability absent — read() takes the central EINVAL path.
    check(
        "epoll caps",
        crate::syscall::misc::caps_shape::epoll_ops(),
        Expect { read: false, write: false, lseek: false, close: true, poll: false, read_at: false },
    );

    check(
        "eventfd caps",
        crate::syscall::misc::caps_shape::eventfd_ops(),
        Expect { read: true, write: true, lseek: false, close: true, poll: true, read_at: false },
    );

    check(
        "timerfd caps",
        crate::syscall::misc::caps_shape::timerfd_ops(),
        Expect { read: true, write: false, lseek: false, close: true, poll: true, read_at: false },
    );

    check(
        "signalfd caps",
        crate::syscall::misc::caps_shape::signalfd_ops(),
        Expect { read: true, write: false, lseek: false, close: true, poll: true, read_at: false },
    );

    // The canonical STREAM set: read/write/poll/close, never seek.
    check(
        "pipe caps",
        crate::syscall::misc::caps_shape::pipe_ops(),
        Expect { read: true, write: true, lseek: false, close: true, poll: true, read_at: false },
    );

    check(
        "inotify caps",
        crate::syscall::misc::caps_shape::inotify_ops(),
        Expect { read: true, write: false, lseek: false, close: true, poll: true, read_at: false },
    );

    // D-6 follow-on conversions (leg-39): the two hot-path regular-file
    // tables. ext4 and tmpfs carry the full positional file set minus
    // poll — poll on a regular file goes through the always-ready
    // fallback (a regular file is always readable/writable per the
    // reference behavior), pinned here so adding a poll slot to either
    // table is a deliberate act that fails this shape first.
    check(
        "ext4 file caps",
        &crate::fs::ext4::file::EXT4_FILE_OPS,
        Expect { read: true, write: true, lseek: true, close: true, poll: false, read_at: true },
    );
    check(
        "tmpfs file caps",
        &crate::fs::tmpfs::TMPFS_FILE_OPS,
        Expect { read: true, write: true, lseek: true, close: true, poll: false, read_at: true },
    );

    // Generic VFS tables (file.rs, D-6 follow-on slice 2). The
    // read-write regular-file table and the read-only twin differ ONLY
    // in write; the directory table carries read (the Eisdir stub) and
    // lseek/close but no write and no read_at (getdents64 is the
    // directory read path).
    check(
        "reg file caps",
        &crate::fs::file::REG_FILE_OPS,
        Expect { read: true, write: true, lseek: true, close: true, poll: false, read_at: false },
    );
    check(
        "reg ro-file caps",
        &crate::fs::file::REG_RO_FILE_OPS,
        Expect { read: true, write: false, lseek: true, close: true, poll: false, read_at: false },
    );
    check(
        "dir file caps",
        &crate::fs::file::DIR_FILE_OPS,
        Expect { read: true, write: false, lseek: true, close: true, poll: false, read_at: false },
    );

    check_errno_matrix();
}
