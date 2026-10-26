//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! F4 route-4 unit group: the getdents64 listing cache
//! (File::dirent_cache + VFS_DIR_EPOCH).
//!
//! The cache lets a cacheable filesystem (tmpfs here — the same
//! property ext4 declares) serve repeat getdents64 calls on one open
//! file description from a snapshot instead of re-materializing the
//! whole directory per call. The rows pin the two safety properties:
//!   - STABILITY: a walk that spans multiple calls (buffer smaller than
//!     the listing) returns a consistent, duplicate-free stream — the
//!     snapshot the later calls read is the one the first call pinned.
//!   - INVALIDATION: a directory mutation (create) between two calls on
//!     the SAME fd bumps VFS_DIR_EPOCH, so the next call re-materializes
//!     and an entry created mid-walk is visible to the SAME stream.
//! Plus the dot-synthesis helper shape (pure function, no fs).

use super::{test_fail, test_group_start, test_pass};

/// Walk `fd` with a 512-byte buffer until exhaustion; returns the set
/// of names seen (panics-free: an Err mid-walk is reported by the
/// caller through the returned Option).
fn walk_all(fd: usize) -> Option<alloc::vec::Vec<alloc::vec::Vec<u8>>> {
    let mut names = alloc::vec::Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match crate::fs::vfs::file_getdents64(fd, &mut buf, 512) {
            Ok(0) => return Some(names),
            Ok(total) => {
                let mut off = 0usize;
                while off + 19 <= total {
                    let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
                    if reclen < 19 || off + reclen > total {
                        return None; // malformed record
                    }
                    let name = &buf[off + 19..off + reclen];
                    let name = name.split(|&b| b == 0).next().unwrap_or(&[]).to_vec();
                    names.push(name);
                    off += reclen;
                }
            }
            Err(_) => return None,
        }
    }
}

pub fn test_getdents_cache() {
    test_group_start("getdents-cache");

    // ---- 1. dot synthesis helper (pure) ---------------------------------
    let dots = crate::fs::vfs::getdents_with_dots(alloc::vec::Vec::new(), 7, 9);
    if dots.len() == 2
        && dots[0].name == b".".as_slice()
        && dots[0].ino == 7
        && dots[1].name == b"..".as_slice()
        && dots[1].ino == 9
    {
        test_pass("with_dots synthesizes the pair");
    } else {
        test_fail("with_dots synthesizes the pair", "wrong shape");
    }
    // A listing that already carries "." is returned untouched.
    let carried = alloc::vec![crate::fs::inode::VfsDirEntry {
        ino: 1,
        name: alloc::vec![b'.'],
        file_type: crate::fs::inode::file_type::DT_DIR,
    }];
    let untouched = crate::fs::vfs::getdents_with_dots(carried, 7, 9);
    if untouched.len() == 1 && untouched[0].name == b".".as_slice() {
        test_pass("with_dots keeps an existing pair");
    } else {
        test_fail("with_dots keeps an existing pair", "re-synthesized");
    }

    // ---- 2. fresh tmpfs with 40 entries; one fd, 512-byte buffer -------
    let mp = "/tmp/getdcachetest";
    let _ = crate::fs::mount::do_mount("none", mp, "tmpfs", 0, None);
    for i in 0..40u32 {
        let p = alloc::format!("{}/a{:02}", mp, i);
        if let Err(e) = crate::fs::vfs::file_open(&p, crate::fs::FileFlags::O_WRONLY | crate::fs::FileFlags::O_CREAT, 0o644) {
            let _ = e;
        }
    }
    let fd = match crate::fs::vfs::file_opendir(mp, 0) {
        Ok(fd) => fd,
        Err(e) => {
            test_fail("opendir tmpfs", &alloc::format!("errno {}", -e));
            return;
        }
    };

    // STABILITY: the multi-call walk (512 B buffer < 40 entries + dots)
    // reads the snapshot from call 2 onward — a duplicate or a missing
    // name means the later call re-materialized mid-stream against a
    // moved position or diverged from the pinned listing.
    let first = match walk_all(fd) {
        Some(n) => n,
        None => {
            test_fail("first walk clean", "malformed stream");
            crate::fs::vfs::file_close(fd).ok();
            return;
        }
    };
    let has_dots = first.iter().any(|n| n == b".".as_slice())
        && first.iter().any(|n| n == b"..".as_slice());
    if has_dots {
        test_pass("walk includes dot pair");
    } else {
        test_fail("walk includes dot pair", "missing . or ..");
    }
    let real: alloc::vec::Vec<&alloc::vec::Vec<u8>> = first.iter().filter(|n| *n != b".".as_slice() && *n != b"..".as_slice()).collect();
    if real.len() == 40 {
        test_pass("walk sees all 40 entries");
    } else {
        test_fail("walk sees all 40 entries", &alloc::format!("got {}", real.len()));
    }
    let mut sorted = first.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() == first.len() {
        test_pass("no duplicate names in stream");
    } else {
        test_fail("no duplicate names in stream", &alloc::format!("{} dups", first.len() - sorted.len()));
    }

    // ---- 3. same-fd invalidation: create mid-stream ---------------------
    // Re-open (a fresh stream), walk ONE call (partial — buffer smaller
    // than the listing), create a new entry (bumps VFS_DIR_EPOCH), then
    // continue the SAME stream: the remaining calls must re-materialize
    // and the new entry must be visible before exhaustion.
    let fd2 = match crate::fs::vfs::file_opendir(mp, 0) {
        Ok(fd) => fd,
        Err(e) => {
            test_fail("reopendir tmpfs", &alloc::format!("errno {}", -e));
            crate::fs::vfs::file_close(fd).ok();
            return;
        }
    };
    let mut buf = [0u8; 512];
    match crate::fs::vfs::file_getdents64(fd2, &mut buf, 512) {
        Ok(n) if n > 0 => {
            test_pass("partial first call consumed");
        }
        _ => test_fail("partial first call consumed", "no bytes"),
    }
    let added = alloc::format!("{}/zz_added", mp);
    match crate::fs::vfs::file_open(&added, crate::fs::FileFlags::O_WRONLY | crate::fs::FileFlags::O_CREAT, 0o644) {
        Ok(fd3) => {
            crate::fs::vfs::file_close(fd3).ok();
            test_pass("mid-stream create");
        }
        Err(e) => test_fail("mid-stream create", &alloc::format!("errno {}", -e)),
    }
    let rest = match walk_all(fd2) {
        Some(n) => n,
        None => {
            test_fail("post-mutation walk clean", "malformed stream");
            crate::fs::vfs::file_close(fd).ok();
            crate::fs::vfs::file_close(fd2).ok();
            return;
        }
    };
    if rest.iter().any(|n| n == b"zz_added".as_slice()) {
        test_pass("same-fd stream sees post-create entry");
    } else {
        test_fail("same-fd stream sees post-create entry", "stale snapshot served");
    }
    crate::fs::vfs::file_close(fd).ok();
    crate::fs::vfs::file_close(fd2).ok();

    // ---- 4. unlink invalidation on a fresh fd ---------------------------
    let gone = alloc::format!("{}/a00", mp);
    match crate::fs::vfs::file_unlink(&gone) {
        Ok(()) => test_pass("unlink a00"),
        Err(e) => test_fail("unlink a00", &alloc::format!("errno {}", -e)),
    }
    let fd4 = crate::fs::vfs::file_opendir(mp, 0).ok();
    if let Some(fd4) = fd4 {
        let after = walk_all(fd4);
        crate::fs::vfs::file_close(fd4).ok();
        match after {
            Some(names) => {
                if !names.iter().any(|n| n == b"a00".as_slice()) {
                    test_pass("unlinked entry gone from next listing");
                } else {
                    test_fail("unlinked entry gone from next listing", "still listed");
                }
            }
            None => test_fail("post-unlink walk clean", "malformed stream"),
        }
    } else {
        test_fail("reopendir after unlink", "open failed");
    }
}
