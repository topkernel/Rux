//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Filesystem layer pins — ported from the dead #[cfg(test)] blocks in
//! kernel/src/fs/{stat,superblock,vt,rootfs}.rs and
//! kernel/src/fs/ext4/{csum,indirect}.rs per the test-layout policy
//! (those host-side blocks never compiled in this no_std crate).

use crate::fs::ext4::csum::{
    crc32c, group_desc_csum_ok, group_desc_csum_set, inode_csum_ok, inode_csum_set,
    superblock_csum_ok, superblock_csum_set, CRC32C_TABLE,
};
use crate::fs::ext4::indirect::{get_indirect_level, max_file_size, Ext4BlockIterator};
use crate::fs::rootfs::{RootFSNode, RootFSSuperBlock};
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
    test_rootfs_pins();
    test_ext4_csum_pins();
    test_ext4_indirect_pins();
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

extern "C" fn test_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    let sb = Box::new(SuperBlock::new(4096, 0x1234));
    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

extern "C" fn test_kill_sb(_sb: *mut SuperBlock) {}

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

// ---- fs/rootfs.rs ------------------------------------------------------

fn test_rootfs_pins() {
    let dir = RootFSNode::new_dir(b"test".to_vec(), 1);
    crate::test_assert!(dir.is_dir() && !dir.is_file(), "dir node type");
    let file = RootFSNode::new_file(b"file.txt".to_vec(), b"hello".to_vec(), 2);
    crate::test_assert!(file.is_file() && !dir.is_file(), "file node type");

    let sb = RootFSSuperBlock::new();
    crate::test_assert!(sb.get_root().map(|r| r.is_dir()).unwrap_or(false), "rootfs root is a dir");

    crate::test_assert!(sb.create_file("/rux-test.txt", b"hello".to_vec()).is_ok(), "create file");
    let f = sb.lookup("/rux-test.txt");
    crate::test_assert!(f.is_some() && f.unwrap().is_file(), "lookup finds created file");

    crate::test_assert!(
        sb.create_file("/dir1/dir2/file.txt", b"data".to_vec()).is_err(),
        "nested create without parent fails"
    );

    let sb = RootFSSuperBlock::new();
    crate::test_assert!(sb.create_file("/file1.txt", b"data1".to_vec()).is_ok(), "create file1");
    crate::test_assert!(sb.create_file("/file2.txt", b"data2".to_vec()).is_ok(), "create file2");
    // list_dir("/") exercises the per-sb path cache. The old GLOBAL
    // cache let this fresh test sb collide with the boot rootfs's cached
    // "/" (the guest run returned a foreign node with 9 children — the
    // dead block's "2 files" pin never matched for that reason). The
    // cache is per-superblock now (Linux dcache alignment), so a fresh
    // sb lists exactly its own two created files.
    let children = sb.list_dir("/").unwrap();
    let names: Vec<&[u8]> = children.iter().map(|c| c.name()).collect();
    crate::test_assert_eq!(children.len(), 2, "fresh root lists exactly the created files");
    crate::test_assert!(names.contains(&&b"file1.txt"[..]), "file1 listed");
    crate::test_assert!(names.contains(&&b"file2.txt"[..]), "file2 listed");
    crate::tests::test_pass("fs-rootfs");
}

// ---- fs/ext4/csum.rs ----------------------------------------------------

fn test_ext4_csum_pins() {
    // Castagnoli table prefix.
    crate::test_assert_eq!(CRC32C_TABLE[0], 0, "crc32c table[0]");
    // The dead block pinned table[1] == 0x82F6_3B78 — that is the POLYNOMIAL
    // constant, not the table entry. The standard reflected Castagnoli
    // table has T[1] = 0xF26B8303 / T[255] = 0xAD7D5351, and the RFC 3720
    // iSCSI vector below passes, so the kernel table is correct.
    crate::test_assert_eq!(CRC32C_TABLE[1], 0xF26B_8303, "crc32c table[1]");
    crate::test_assert_eq!(CRC32C_TABLE[255], 0xAD7D_5351, "crc32c table[255]");

    // set/ok round-trips per object size class.
    for size in [128usize, 256] {
        let mut raw = alloc::vec![0u8; size];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = (i * 7 + 3) as u8;
        }
        inode_csum_set(0x1234_5678, 42, &mut raw);
        crate::test_assert!(inode_csum_ok(0x1234_5678, 42, &raw), "inode csum ok size {size}");
        raw[8] ^= 0xFF;
        crate::test_assert!(!inode_csum_ok(0x1234_5678, 42, &raw), "inode csum breaks on flip size {size}");
    }
    for dsize in [32usize, 64] {
        let mut desc = alloc::vec![0u8; dsize];
        for (i, b) in desc.iter_mut().enumerate() {
            *b = (i * 13 + 5) as u8;
        }
        group_desc_csum_set(0x9ABC_DEF0u32, 7, &mut desc);
        crate::test_assert!(group_desc_csum_ok(0x9ABC_DEF0u32, 7, &desc), "desc csum ok size {dsize}");
        desc[4] ^= 0x01;
        crate::test_assert!(!group_desc_csum_ok(0x9ABC_DEF0u32, 7, &desc), "desc csum breaks on flip size {dsize}");
    }
    let mut sb = alloc::vec![0u8; 1024];
    for (i, b) in sb.iter_mut().enumerate().take(0x3FC) {
        *b = (i * 31 + 11) as u8;
    }
    superblock_csum_set(&mut sb);
    crate::test_assert!(superblock_csum_ok(&sb), "superblock csum ok");
    sb[0] ^= 0x80;
    crate::test_assert!(!superblock_csum_ok(&sb), "superblock csum breaks on flip");

    // RFC 3720 / iSCSI vector: crc32c("123456789").
    let raw = crc32c(0xFFFF_FFFFu32, b"123456789");
    crate::test_assert_eq!(raw ^ 0xFFFF_FFFFu32, 0xE306_9283u32, "iSCSI crc32c vector");
    crate::tests::test_pass("ext4-csum");
}

// ---- fs/ext4/indirect.rs -------------------------------------------------

fn test_ext4_indirect_pins() {
    let block_size = 4096u64;
    let max_size = max_file_size(block_size);
    crate::test_assert!(max_size > 4_000_000_000_000, "max file size over 4TB");

    crate::test_assert_eq!(get_indirect_level(48 * 1024, block_size), 0, "direct only");
    crate::test_assert_eq!(get_indirect_level(100 * 1024, block_size), 1, "single indirect");
    crate::test_assert_eq!(get_indirect_level(4 * 1024 * 1024, block_size), 1, "still single");
    crate::test_assert_eq!(get_indirect_level(5 * 1024 * 1024, block_size), 2, "double indirect");
    crate::test_assert_eq!(get_indirect_level(4 * 1024 * 1024 * 1024, block_size), 2, "still double");
    crate::test_assert_eq!(get_indirect_level(5 * 1024 * 1024 * 1024u64, block_size), 3, "triple indirect");

    let mut iter = Ext4BlockIterator::new(20);
    let mut ok = true;
    for i in 0..12 {
        match iter.next_mapping() {
            Some((level, offset)) => ok &= level == 0 && offset == i,
            None => ok = false,
        }
    }
    for i in 0..8 {
        match iter.next_mapping() {
            Some((level, offset)) => ok &= level == 1 && offset == i,
            None => ok = false,
        }
    }
    ok &= iter.next_mapping().is_none();
    crate::test_assert!(ok, "block iterator level/offset walk");
    crate::tests::test_pass("ext4-indirect");
}
