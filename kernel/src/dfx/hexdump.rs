//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Hex/Memory Dump Utility
//!
//! Provides `print_hex_dump()` for debugging memory contents.
//! Output format follows standard hex dump conventions.

use core::fmt::Write;
use crate::console::putchar_no_lock;

/// Write a single byte as two hex digits to the writer.
fn hex_byte(w: &mut dyn Write, byte: u8) -> core::fmt::Result {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    w.write_str(unsafe {
        core::str::from_utf8_unchecked(&[HEX_CHARS[(byte >> 4) as usize], HEX_CHARS[(byte & 0xf) as usize]])
    })
}

/// Print hex dump of a memory region to the console.
///
/// Output format:
/// ```text
/// 00000000: 7f 45 4c 46 02 01 01 00  00 00 00 00 00 00 00 00  |.ELF............|
/// 00000010: 02 00 b7 00 01 00 00 00  00 10 00 00 00 00 00 00  |................|
/// ```
///
/// # Arguments
/// * `addr` - Start address of the memory region to dump
/// * `len` - Number of bytes to dump
/// * `prefix` - Optional prefix string prepended to each line (pass "" for none)
///
/// # Safety
/// The caller must ensure that the memory range `[addr, addr + len)` is valid and readable.
pub unsafe fn hex_dump_to_console(addr: usize, len: usize, prefix: &str) {
    struct ConsoleW;
    impl Write for ConsoleW {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for b in s.bytes() {
                if b == b'\n' {
                    putchar_no_lock(b'\r');
                }
                putchar_no_lock(b);
            }
            Ok(())
        }
    }

    let mut w = ConsoleW;
    let ptr = addr as *const u8;
    let rowsize: usize = 16;
    let mut offset = 0usize;

    while offset < len {
        // Prefix
        let _ = w.write_str(prefix);

        // Offset
        let _ = write!(w, "{:08x}: ", offset);

        // Hex bytes
        let line_end = (offset + rowsize).min(len);
        let line_len = line_end - offset;

        for i in 0..rowsize {
            if i > 0 && i % 8 == 0 {
                let _ = w.write_str(" ");
            }

            if offset + i < len {
                let byte = *ptr.add(offset + i);
                let _ = hex_byte(&mut w, byte);
                let _ = w.write_str(" ");
            } else {
                let _ = w.write_str("   ");
            }
        }

        // ASCII representation
        let _ = w.write_str(" |");
        for i in 0..line_len {
            let byte = *ptr.add(offset + i);
            if byte >= 0x20 && byte < 0x7f {
                let _ = w.write_str(unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(&byte, 1))
                });
            } else {
                let _ = w.write_str(".");
            }
        }
        let _ = w.write_str("|");

        // Pad ASCII if line is short
        for _ in line_len..rowsize {
            let _ = w.write_str(" ");
        }
        let _ = w.write_str("|\n");

        offset += rowsize;
    }
}
