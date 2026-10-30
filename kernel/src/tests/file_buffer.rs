//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use crate::fs::buffer::FileBuffer;
use super::test_group_start;

/// ftruncate(2): "the extended portion [of the file] reads as null bytes".
/// FileBuffer::resize on a shrink keeps the last chunk's bytes past the new
/// length; a later grow reuses that chunk, so the tail MUST have been
/// zeroed at shrink time or the pre-shrink bytes resurrect (the memfd
/// ftruncate path; the tmpfs twin is test_tmpfs_truncate_zero_tail).
pub fn test_file_buffer() {
    test_group_start("file_buffer");

    // Round-trip + hole zero-fill (grow past written data).
    let mut fb = FileBuffer::new();
    test_assert!(fb.len() == 0, "fresh buffer empty");
    let n = fb.write(0, &[0xAAu8; 64]);
    test_assert!(n == 64, "write 64 bytes");
    let mut buf = [0u8; 128];
    let r = fb.read(0, &mut buf);
    test_assert!(r == 64, "read clipped at len");
    test_assert!(buf[..64].iter().all(|&b| b == 0xAA), "written bytes");

    // Shrink-then-grow: the extended portion reads as zeros.
    test_assert!(fb.resize(16).is_ok(), "shrink to 16");
    test_assert!(fb.len() == 16, "len 16 after shrink");
    let mut b2 = [0u8; 16];
    test_assert!(fb.read(0, &mut b2) == 16, "read after shrink");
    test_assert!(b2[..16].iter().all(|&b| b == 0xAA), "kept head bytes");
    test_assert!(fb.resize(64).is_ok(), "grow back to 64");
    let mut b3 = [0u8; 64];
    test_assert!(fb.read(0, &mut b3) == 64, "read after re-extend");
    test_assert!(b3[..16].iter().all(|&b| b == 0xAA), "head survives");
    test_assert!(b3[16..].iter().all(|&b| b == 0), "tail reads zeros");

    // Offset write extending the buffer zero-fills the gap.
    let mut fb2 = FileBuffer::new();
    fb2.write(4096, b"xy");
    let mut b4 = [0u8; 4098];
    test_assert!(fb2.read(0, &mut b4) == 4098, "gap read");
    test_assert!(b4[..4096].iter().all(|&b| b == 0), "gap zeros");
    test_assert!(&b4[4096..] == b"xy", "tail bytes");
}
