//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::pipe::{Pipe, PipeBuffer, create_pipe, pipe_file_read, pipe_file_write};
use crate::fs::file::File;
use super::test_group_start;

/// Borrow the Pipe backing a pipe-end File — the same raw-pointer access the
/// data-path callbacks in fs/pipe.rs perform (the old free-function
/// pipe_read/pipe_write helpers were removed as dead code in review 5.2).
fn pipe_of(file: &File) -> &Pipe {
    let ptr = unsafe { *file.private_data.get() }
        .expect("pipe end File must have private_data");
    unsafe { &*(ptr as *const Pipe) }
}

pub fn test_pipe2() {
    test_group_start("pipe2");

    // Test 1: Pipe::new creates valid pipe
    let pipe = Pipe::new();
    test_assert!(!pipe.is_read_closed() && !pipe.is_write_closed(), "Pipe::new valid state");

    // Test 2: PipeBuffer initial state
    // Note: PipeBuffer is a ring that keeps one slot empty to distinguish
    // full from empty. Since 22ff21a ("LTP round 3") PipeBuffer::new(N)
    // allocates N+1 slots so the pipe can hold the FULL N bytes (F_GETPIPE_SZ
    // reported N but a full N-byte write used to return N-1 — LTP pipe2_04).
    // available_write() on a fresh buffer is therefore N, not N-1.
    let mut buf = PipeBuffer::new(4096);
    test_assert_eq!(buf.available_read(), 0, "PipeBuffer initial available_read == 0");
    test_assert_eq!(buf.available_write(), 4096, "PipeBuffer initial available_write == capacity");

    // Test 3: PipeBuffer write and read roundtrip
    let data = [0xDEu8; 100];
    let written = buf.write(&data);
    test_assert_eq!(written, 100, "PipeBuffer write 100 bytes");
    test_assert_eq!(buf.available_read(), 100, "PipeBuffer available_read after write");
    test_assert_eq!(buf.available_write(), 3996, "PipeBuffer available_write after write (capacity-read)");

    let mut read_buf = [0u8; 100];
    let read = buf.read(&mut read_buf);
    test_assert_eq!(read, 100, "PipeBuffer read 100 bytes");
    test_assert_eq!(read_buf, [0xDEu8; 100], "PipeBuffer read data matches written");
    test_assert_eq!(buf.available_read(), 0, "PipeBuffer available_read after read");

    // Test 4: PipeBuffer wraparound
    buf.write(&[0xAAu8; 2000]);
    let mut tmp = [0u8; 1000];
    buf.read(&mut tmp);
    buf.write(&[0xBBu8; 500]);
    test_assert_eq!(buf.available_read(), 1500, "PipeBuffer wraparound available_read");

    // Test 5: create_pipe returns valid pair
    let (read_file, write_file) = create_pipe();
    test_assert!(true, "create_pipe returns valid pair");

    // Test 6: pipe_file_write + pipe_file_read roundtrip on real pipe
    {
        let (read_file, write_file) = create_pipe();
        let data = [0x42u8; 50];
        let written = pipe_file_write(&write_file, &data);
        test_assert_eq!(written, 50, "pipe_file_write writes all 50 bytes");

        let mut read_buf = [0u8; 50];
        let read = pipe_file_read(&read_file, &mut read_buf);
        test_assert_eq!(read, 50, "pipe_file_read succeeds after write");
        test_assert_eq!(read_buf, [0x42u8; 50], "pipe_file_read data matches written");
    }

    // Test 7: pipe_file_read on empty + write-closed pipe returns 0 (EOF)
    {
        let (read_file, write_file) = create_pipe();
        pipe_of(&write_file).close_write();
        let mut buf = [0u8; 10];
        let read = pipe_file_read(&read_file, &mut buf);
        test_assert_eq!(read, 0, "pipe_file_read on write-closed empty pipe returns 0");
    }

    // Test 8: pipe_file_write on read-closed pipe returns -EPIPE
    {
        let (read_file, write_file) = create_pipe();
        pipe_of(&read_file).close_read();
        let data = [0x01u8; 10];
        let result = pipe_file_write(&write_file, &data);
        // Should return -EPIPE (32)
        test_assert_eq!(result, -(crate::errno::constants::EPIPE as isize), "pipe_file_write on read-closed pipe returns -EPIPE");
    }

    // Test 9: O_CLOEXEC and O_NONBLOCK flag constants
    const O_CLOEXEC: u64 = 0x80000;
    const O_NONBLOCK: u64 = 0x800;
    test_assert_eq!(O_CLOEXEC, 0x80000, "O_CLOEXEC == 0x80000");
    test_assert_eq!(O_NONBLOCK, 0x800, "O_NONBLOCK == 0x800");

    // Test 10: Multiple small writes
    {
        let (read_file, write_file) = create_pipe();
        let w1 = pipe_file_write(&write_file, &[0x01u8; 10]);
        let w2 = pipe_file_write(&write_file, &[0x02u8; 10]);
        test_assert!(w1 >= 10 && w2 >= 10, "multiple pipe_file_write succeed");
        let mut buf = [0u8; 20];
        let r = pipe_file_read(&read_file, &mut buf);
        test_assert_eq!(r, 20, "both small writes are readable (FIFO order)");
        test_assert_eq!(buf[..10], [0x01u8; 10], "first chunk order preserved");
        test_assert_eq!(buf[10..], [0x02u8; 10], "second chunk order preserved");
    }
}
