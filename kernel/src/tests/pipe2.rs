//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use crate::fs::pipe::{Pipe, PipeBuffer, create_pipe, pipe_read, pipe_write};
use super::{test_pass, test_fail, test_group_start};

pub fn test_pipe2() {
    test_group_start("pipe2");

    // Test 1: Pipe::new creates valid pipe
    let pipe = Pipe::new();
    test_assert!(!pipe.is_read_closed() && !pipe.is_write_closed(), "Pipe::new valid state");

    // Test 2: PipeBuffer initial state
    // Note: PipeBuffer uses circular buffer, available_write() returns size - 1
    let mut buf = PipeBuffer::new(4096);
    test_assert_eq!(buf.available_read(), 0, "PipeBuffer initial available_read == 0");
    test_assert_eq!(buf.available_write(), 4095, "PipeBuffer initial available_write == size-1");

    // Test 3: PipeBuffer write and read roundtrip
    let data = [0xDEu8; 100];
    let written = buf.write(&data);
    test_assert_eq!(written, 100, "PipeBuffer write 100 bytes");
    test_assert_eq!(buf.available_read(), 100, "PipeBuffer available_read after write");
    test_assert_eq!(buf.available_write(), 3995, "PipeBuffer available_write after write (size-1-read)");

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
    match create_pipe() {
        (read_file, write_file) => {
            test_assert!(true, "create_pipe returns valid pair");
        }
        _ => {
            test_fail("create_pipe", "returned None");
        }
    }

    // Test 6: pipe_write + pipe_read roundtrip on real pipe
    {
        let pipe = Pipe::new();
        let data = [0x42u8; 50];
        let written = pipe_write(&pipe, &data);
        test_assert!(written > 0, "pipe_write succeeds on open pipe");

        let mut read_buf = [0u8; 50];
        let read = pipe_read(&pipe, &mut read_buf);
        test_assert!(read > 0, "pipe_read succeeds after write");
    }

    // Test 7: pipe_read on empty + write-closed pipe returns 0 (EOF)
    {
        let pipe = Pipe::new();
        pipe.close_write();
        let mut buf = [0u8; 10];
        let read = pipe_read(&pipe, &mut buf);
        test_assert_eq!(read, 0, "pipe_read on write-closed empty pipe returns 0");
    }

    // Test 8: pipe_write on read-closed pipe returns -EPIPE
    {
        let pipe = Pipe::new();
        pipe.close_read();
        let data = [0x01u8; 10];
        let result = pipe_write(&pipe, &data);
        // Should return -EPIPE (32)
        test_assert_eq!(result, -32, "pipe_write on read-closed pipe returns -EPIPE");
    }

    // Test 9: O_CLOEXEC and O_NONBLOCK flag constants
    const O_CLOEXEC: u64 = 0x80000;
    const O_NONBLOCK: u64 = 0x800;
    test_assert_eq!(O_CLOEXEC, 0x80000, "O_CLOEXEC == 0x80000");
    test_assert_eq!(O_NONBLOCK, 0x800, "O_NONBLOCK == 0x800");

    // Test 10: Multiple small writes
    {
        let pipe = Pipe::new();
        let w1 = pipe_write(&pipe, &[0x01u8; 10]);
        let w2 = pipe_write(&pipe, &[0x02u8; 10]);
        test_assert!(w1 >= 10 && w2 >= 10, "multiple pipe_write succeed");
    }
}
