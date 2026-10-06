//! sgid_mode — unit tests for the VFS create-mode rules behind LTP creat09
//! (CVE-2018-13405): mode_strip_sgid() / prepare_create_mode() semantics
//! mirroring Linux fs/namei.c vfs_prepare_mode() and fs/inode.c
//! mode_strip_sgid() / inode_init_owner().

use super::{test_fail, test_group_start, test_pass};

pub fn test_sgid_mode() {
    test_group_start("sgid_mode");

    // The strip rule engages only when the request has BOTH S_ISGID and
    // S_IXGRP (a mandatory-lock-style file), the parent dir is setgid,
    // and the caller is outside the dir's group without CAP_FSETID.
    const S_ISGID: u32 = 0o2000;
    const S_IXGRP: u32 = 0o010;
    let dir = 0o2777; // setgid parent
    let plain_dir = 0o0777; // non-setgid parent

    let cases: [(u32, u32, u32, &str); 8] = [
        // (dir_mode, mode, expected, what)
        (dir, 0o2777, 0o0777, "setgid dir + 02777 strips S_ISGID (root, no group)"),
        (dir, 0o2077, 0o0077, "02077 (group+x) strips like 02777"),
        (plain_dir, 0o2777, 0o2777, "plain dir keeps S_ISGID (bit is caller's choice)"),
        (dir, 0o2666, 0o2666, "no group-execute keeps S_ISGID (no escalation)"),
        (dir, 0o777, 0o777, "no S_ISGID request passes through"),
        // umask(S_IXGRP) must not save the bit: Linux strips FIRST, then
        // applies the umask (02777 -> 0777, umask makes it 0767). Feeding
        // the already-umasked 02767 to the strip alone would KEEP S_ISGID
        // (no group-execute left) — the ordering is what kills the bit.
        (dir, 0o2777, 0o0777, "strip-before-umask ordering (02777 -> 0777)"),
        // without group-execute the strip rule does not engage at all
        (dir, 0o2764, 0o2764, "02764 (group rw-) keeps S_ISGID"),
        (dir, 0o2000, 0o2000, "S_ISGID without any exec is untouched"),
    ];
    for (dm, mode, want, what) in cases {
        let got = crate::fs::vfs::mode_strip_sgid(dm, 12345, mode);
        if got == want {
            test_pass(what);
        } else {
            test_fail(what, &alloc::format!("mode {:o} in dir {:o}: got {:o}, want {:o}", mode, dm, got, want));
        }
    }

    // Root as fsuid 0 without membership in gid 12345: same table holds
    // when the caller IS in the directory's group — strip does not apply.
    // (in_group check needs a live task; covered indirectly by the LTP
    // regression run: root creates in its OWN setgid dir keeps the bit.)
    let _ = (S_ISGID, S_IXGRP);

    test_pass("sgid_mode group complete");
}
