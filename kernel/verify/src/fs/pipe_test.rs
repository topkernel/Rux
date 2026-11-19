//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for pipe circular buffer arithmetic.
//! Copied from: kernel/src/fs/pipe.rs

use proptest::prelude::*;

// Copied PipeBuffer (simplified: Vec instead of unsafe, plain usize instead of AtomicUsize)
pub struct PipeBuffer {
    data: Vec<u8>,
    read_pos: usize,
    write_pos: usize,
    size: usize,
}

impl PipeBuffer {
    pub fn new(size: usize) -> Self {
        let mut data = vec![0u8; size];
        // Simulate AtomicUsize with plain usize
        Self {
            data,
            read_pos: 0,
            write_pos: 0,
            size,
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        if self.read_pos == self.write_pos {
            return 0;
        }

        let available = if self.write_pos > self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.size - self.read_pos
        };

        let to_read = core::cmp::min(available, buf.len());

        for i in 0..to_read {
            buf[i] = self.data[(self.read_pos + i) % self.size];
        }

        self.read_pos = (self.read_pos + to_read) % self.size;
        to_read
    }

    pub fn write(&mut self, buf: &[u8]) -> usize {
        let available = if self.write_pos >= self.read_pos {
            self.size - (self.write_pos - self.read_pos) - 1
        } else {
            self.read_pos - self.write_pos - 1
        };

        let to_write = core::cmp::min(available, buf.len());

        for i in 0..to_write {
            self.data[(self.write_pos + i) % self.size] = buf[i];
        }

        self.write_pos = (self.write_pos + to_write) % self.size;
        to_write
    }

    pub fn available_read(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.size - self.read_pos + self.write_pos
        }
    }

    pub fn available_write(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.size - (self.write_pos - self.read_pos) - 1
        } else {
            self.read_pos - self.write_pos - 1
        }
    }
}

proptest! {
    #[test]
    fn test_empty_buffer(size in 16usize..1024usize) {
        let buf = PipeBuffer::new(size);
        assert_eq!(buf.available_read(), 0);
        assert_eq!(buf.available_write(), size - 1);
    }

    #[test]
    fn test_capacity_invariant(size in 16usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        // available_read + available_write + 1 == size always
        assert_eq!(buf.available_read() + buf.available_write() + 1, size);
        // Write some data
        let data = vec![0xAAu8; size / 2];
        buf.write(&data);
        assert_eq!(buf.available_read() + buf.available_write() + 1, size);
        // Read some
        let mut out = vec![0u8; size / 4];
        buf.read(&mut out);
        assert_eq!(buf.available_read() + buf.available_write() + 1, size);
    }

    #[test]
    fn test_write_read_roundtrip(size in 64usize..1024usize, data_len in 1usize..64usize) {
        let mut buf = PipeBuffer::new(size);
        let data: Vec<u8> = (0..data_len).map(|i| (i * 7 + 13) as u8).collect();
        let written = buf.write(&data);
        assert_eq!(written, data_len);

        let mut out = vec![0u8; data_len];
        let read = buf.read(&mut out);
        assert_eq!(read, data_len);
        assert_eq!(out, data);
    }

    #[test]
    fn test_full_buffer_write_returns_zero(size in 16usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        let data = vec![0u8; size - 1];
        assert_eq!(buf.write(&data), size - 1);
        assert_eq!(buf.available_write(), 0);
        // Writing more returns 0
        assert_eq!(buf.write(&[1u8; 10]), 0);
    }

    #[test]
    fn test_empty_buffer_read_returns_zero(size in 16usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        let mut out = vec![0u8; 10];
        assert_eq!(buf.read(&mut out), 0);
    }

    #[test]
    fn test_wraparound_write(size in 64usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        // Fill to near-full
        let fill_data = vec![0xABu8; size - 1];
        buf.write(&fill_data);
        // Drain enough to create wraparound room
        let drain_len = size / 2;
        let mut out = vec![0u8; drain_len];
        buf.read(&mut out);
        // Write new data — wraps around end of buffer
        let new_data = vec![0xCDu8; 5];
        let written = buf.write(&new_data);
        assert_eq!(written, 5);
        // Verify new data is accessible via available_read
        assert!(buf.available_read() >= written);
        // Read in chunks to handle wraparound
        let mut all_read = Vec::new();
        loop {
            let mut chunk = vec![0u8; buf.available_read()];
            let n = buf.read(&mut chunk);
            if n == 0 { break; }
            all_read.extend_from_slice(&chunk[..n]);
        }
        // The last 5 bytes should be 0xCD
        assert!(all_read.len() >= 5);
        assert!(all_read[all_read.len() - 5..].iter().all(|&b| b == 0xCD));
    }

    #[test]
    fn test_fifo_ordering(size in 128usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        // Write multiple chunks
        let chunk1 = vec![1u8; 10];
        let chunk2 = vec![2u8; 10];
        let chunk3 = vec![3u8; 10];
        buf.write(&chunk1);
        buf.write(&chunk2);
        buf.write(&chunk3);
        // Read back in FIFO order
        let mut out = vec![0u8; 30];
        buf.read(&mut out);
        assert_eq!(out[0..10], vec![1u8; 10]);
        assert_eq!(out[10..20], vec![2u8; 10]);
        assert_eq!(out[20..30], vec![3u8; 10]);
    }

    #[test]
    fn test_available_read_write_consistency(size in 64usize..1024usize, write_len in 1usize..64usize) {
        let mut buf = PipeBuffer::new(size);
        let data = vec![0u8; write_len.min(size - 1)];
        let written = buf.write(&data);
        assert_eq!(buf.available_read(), written);
        let mut out = vec![0u8; written];
        let read = buf.read(&mut out);
        assert_eq!(read, written);
        assert_eq!(buf.available_read(), 0);
    }

    #[test]
    fn test_read_respects_buf_len(size in 64usize..1024usize) {
        let mut buf = PipeBuffer::new(size);
        let data = vec![0x55u8; 50];
        buf.write(&data);
        // Read with smaller buffer
        let mut out = vec![0u8; 10];
        assert_eq!(buf.read(&mut out), 10);
        assert_eq!(buf.available_read(), 40);
    }
}
