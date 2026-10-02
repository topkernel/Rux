//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/config.gz — CONFIG_IKCONFIG_PROC-style embedded kernel config.
//!
//! Rux has no Kbuild/Kconfig; this file reports the feature set the
//! kernel actually implements so LTP's tst_kconfig (`.needs_kconfigs`)
//! can gate tests accurately instead of TBROK-ing on "Couldn't locate
//! kernel config". Entries are hand-maintained to mirror implemented
//! syscalls ONLY — declaring a feature that is not implemented would
//! move gated tests from TBROK to hard failures.
//!
//! The payload is gzip-wrapped with STORED (uncompressed) DEFLATE
//! blocks — valid gzip that any zlib/toybox zcat decompresses, without
//! a compressor in the kernel.

/// The config text served under /proc/config.gz.
const CONFIG_TEXT: &[u8] = b"#\n\
# Rux kernel configuration (IKCONFIG-style; mirrors implemented features)\n\
#\n\
CONFIG_EVENTFD=y\n\
CONFIG_SIGNALFD=y\n\
CONFIG_TIMERFD=y\n\
CONFIG_EPOLL=y\n\
CONFIG_INOTIFY_USER=y\n\
CONFIG_SHMEM=y\n\
CONFIG_SYSVIPC=y\n\
CONFIG_PROC_SYSCTL=y\n\
";

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — gzip trailer.
/// Bitwise implementation; the config payload is tiny.
fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Generate the gzip image of the embedded config.
pub fn generate() -> alloc::vec::Vec<u8> {
    let data = CONFIG_TEXT;
    let len = data.len();
    if len > u16::MAX as usize {
        // Cannot happen for the static payload; guard the format anyway.
        return alloc::vec::Vec::new();
    }
    let mut out = alloc::vec::Vec::with_capacity(len + 32);
    // gzip header: magic, CM=deflate, FLG=0, MTIME=0, XFL=0, OS=3 (Unix)
    out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03]);
    // One STORED block, BFINAL=1, BTYPE=00.
    out.push(0x01);
    out.extend_from_slice(&(len as u16).to_le_bytes());
    out.extend_from_slice(&(!(len as u16)).to_le_bytes());
    out.extend_from_slice(data);
    // Trailer: CRC32 + ISIZE of the UNcompressed payload.
    out.extend_from_slice(&crc32_ieee(data).to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out
}
