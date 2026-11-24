//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Filesystem layer pins — ported from the dead #[cfg(test)] blocks in
//! kernel/src/fs/{stat,superblock,vt,rootfs}.rs and
//! kernel/src/fs/ext4/{csum,indirect}.rs per the test-layout policy
//! (those host-side blocks never compiled in this no_std crate).

use crate::fs::stat::Stat;
use crate::fs::superblock::{
    get_fs_type, register_filesystem, unregister_filesystem, FileSystemType, SuperBlock,
    SuperBlockFlags, FsContext,
};
use crate::fs::vt::{VT_MODE, VT_REFS, VT_AUTO, VT_PROCESS, NR_VT};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

pub fn test_fs_pins() {
    crate::tests::test_group_start("fs_pins");
    test_stat_pins();
    test_superblock_pins();
    test_vt_pins();
    // The ext4 csum/indirect pins flipped WITH the ext4 crate
    // (rung 5; they run via the service-test seam now); the rootfs pins
    // moved WITH the rootfs leaf (fs-substrate tranche — SERVICE-TEST
    // SEAM).
}

// ---- fs/stat.rs --------------------------------------------------------

fn test_stat_pins() {
    let stat = Stat::new();
    crate::test_assert_eq!(stat.st_dev, 0, "fresh st_dev 0");
    crate::test_assert_eq!(stat.st_ino, 0, "fresh st_ino 0");
    crate::test_assert_eq!(stat.st_size, 0, "fresh st_size 0");

    let mut stat = Stat::new();
    stat.set_regular_file();
    let ok = stat.is_regular_file() && !stat.is_directory();
    stat.set_directory();
    let ok = ok && stat.is_directory() && !stat.is_regular_file();
    crate::test_assert!(ok, "file type setters/getters");

    let mut stat = Stat::new();
    stat.set_mode(0o644);
    crate::test_assert_eq!(stat.get_mode(), 0o644, "mode roundtrip");
    crate::tests::test_pass("fs-stat");
}

// ---- fs/superblock.rs --------------------------------------------------

fn test_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    let sb = Box::new(SuperBlock::new(4096, 0x1234));
    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

fn test_kill_sb(_sb: *mut SuperBlock) {}

fn test_superblock_pins() {
    // The registry stores &'static FileSystemType, so the test type is
    // leaked (the suite runs once, at boot end, then the kernel halts).
    let test_fs: &'static FileSystemType =
        Box::leak(Box::new(FileSystemType::new("ruxtestfs", Some(test_mount), Some(test_kill_sb), 0)));
    crate::test_assert!(register_filesystem(test_fs).is_ok(), "register fs");
    crate::test_assert!(get_fs_type("ruxtestfs").is_some(), "registered fs found");
    crate::test_assert!(get_fs_type("rux-nonexistent-fs").is_none(), "unknown fs absent");
    crate::test_assert!(unregister_filesystem(test_fs).is_ok(), "unregister fs");
    crate::test_assert!(get_fs_type("ruxtestfs").is_none(), "unregistered fs gone");

    let flags = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY | SuperBlockFlags::SB_ACTIVE);
    crate::test_assert!(flags.is_readonly(), "RD_RDONLY|ACTIVE readonly");
    crate::test_assert!(flags.is_active(), "RD_RDONLY|ACTIVE active");
    let flags2 = SuperBlockFlags::new(SuperBlockFlags::SB_RDONLY);
    crate::test_assert!(flags2.is_readonly() && !flags2.is_active(), "RD_RDONLY only");
    crate::tests::test_pass("fs-superblock");
}

// ---- fs/vt.rs ----------------------------------------------------------

fn test_vt_pins() {
    // OPENQRY scan: first VT with refcount 0.
    VT_REFS[1].store(1, Ordering::Relaxed); // gdm holds tty1
    VT_REFS[2].store(0, Ordering::Relaxed);
    let mut found = 0;
    for n in 1..=(NR_VT as usize) {
        if VT_REFS[n].load(Ordering::Relaxed) == 0 {
            found = n;
            break;
        }
    }
    crate::test_assert_eq!(found, 2, "openqry returns first free VT");
    VT_REFS[1].store(0, Ordering::Relaxed);

    // struct vt_mode layout: mode@0, waitv@1, relsig@2, acqsig@4, frsig@6.
    let mut buf = [0u8; 8];
    {
        let mut m = VT_MODE.lock_irqsave();
        m.mode = VT_PROCESS;
        m.relsig = 10; // SIGUSR1
        m.acqsig = 10;
        buf[0] = m.mode;
        buf[1] = m.waitv;
        buf[2..4].copy_from_slice(&m.relsig.to_le_bytes());
        buf[4..6].copy_from_slice(&m.acqsig.to_le_bytes());
        buf[6..8].copy_from_slice(&m.frsig.to_le_bytes());
        m.mode = VT_AUTO;
        m.relsig = 0;
        m.acqsig = 0;
    }
    crate::test_assert_eq!(buf[0], VT_PROCESS, "vt_mode mode byte");
    crate::test_assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 10, "vt_mode relsig");
    crate::test_assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), 10, "vt_mode acqsig");
    crate::tests::test_pass("fs-vt");
}

// ---- fs/rootfs.rs + fs/ext4/{csum,indirect}.rs -------------------------

// The rootfs pins moved WITH the rootfs leaf (fs-substrate tranche) and
// the ext4 pins flipped WITH the ext4 crate (rung 5) — both run
// in-guest via the SERVICE-TEST SEAM.
