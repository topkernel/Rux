//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The rootfs type pins (moved from the framework's tests/fs_pins.rs at
//! the fs-substrate flip — the section follows its leaf into the crate;
//! runs in-guest through the SERVICE-TEST SEAM).

use crate::rootfs::{RootFSNode, RootFSSuperBlock};
use alloc::vec::Vec;

pub fn test_rootfs_pins() {
    let dir = RootFSNode::new_dir(b"test".to_vec(), 1);
    rux::test_assert!(dir.is_dir() && !dir.is_file(), "dir node type");
    let file = RootFSNode::new_file(b"file.txt".to_vec(), b"hello".to_vec(), 2);
    rux::test_assert!(file.is_file() && !dir.is_file(), "file node type");

    let sb = RootFSSuperBlock::new();
    rux::test_assert!(sb.get_root().map(|r| r.is_dir()).unwrap_or(false), "rootfs root is a dir");

    rux::test_assert!(sb.create_file("/rux-test.txt", b"hello".to_vec()).is_ok(), "create file");
    let f = sb.lookup("/rux-test.txt");
    rux::test_assert!(f.is_some() && f.unwrap().is_file(), "lookup finds created file");

    rux::test_assert!(
        sb.create_file("/dir1/dir2/file.txt", b"data".to_vec()).is_err(),
        "nested create without parent fails"
    );

    let sb = RootFSSuperBlock::new();
    rux::test_assert!(sb.create_file("/file1.txt", b"data1".to_vec()).is_ok(), "create file1");
    rux::test_assert!(sb.create_file("/file2.txt", b"data2".to_vec()).is_ok(), "create file2");
    // list_dir("/") exercises the per-sb path cache. The old GLOBAL
    // cache let this fresh test sb collide with the boot rootfs's cached
    // "/" (the guest run returned a foreign node with 9 children — the
    // dead block's "2 files" pin never matched for that reason). The
    // cache is per-superblock now (Linux dcache alignment), so a fresh
    // sb lists exactly its own two created files.
    let children = sb.list_dir("/").unwrap();
    let names: Vec<Vec<u8>> = children.iter().map(|c| c.name_vec()).collect();
    rux::test_assert_eq!(children.len(), 2, "fresh root lists exactly the created files");
    rux::test_assert!(names.contains(&b"file1.txt".to_vec()), "file1 listed");
    rux::test_assert!(names.contains(&b"file2.txt".to_vec()), "file2 listed");
    rux::tests::test_pass("fs-rootfs");
}
