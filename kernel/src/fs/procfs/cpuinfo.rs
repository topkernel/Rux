//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cpuinfo - CPU information
//!
//! Reference: Linux arch/riscv/kernel/cpu.c

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Generate /proc/cpuinfo content
///
/// Displays CPU information in the standard Linux format for RISC-V.
pub fn generate() -> Vec<u8> {
    use crate::arch::riscv64::smp::num_started_cpus;
    use core::arch::asm;

    let mut content = String::new();
    let num_cpus = num_started_cpus();

    for cpu in 0..num_cpus {
        // Read CPU information from CSR registers
        let mvendorid: u64;
        let marchid: u64;
        let mimpid: u64;
        let misa: u64;

        unsafe {
            asm!("csrr {}, mvendorid", out(reg) mvendorid);
            asm!("csrr {}, marchid", out(reg) marchid);
            asm!("csrr {}, mimpid", out(reg) mimpid);
            asm!("csrr {}, misa", out(reg) misa);
        }

        // Parse ISA string from misa register
        let isa_str = parse_misa(misa);

        content.push_str(&format!("processor\t: {}\n", cpu));
        content.push_str(&format!("hart\t\t: {}\n", cpu));
        content.push_str(&format!("isa\t\t: {}\n", isa_str));
        content.push_str("mmu\t\t: sv39\n");
        content.push_str(&format!("mvendorid\t: {:#x}\n", mvendorid));
        content.push_str(&format!("marchid\t\t: {:#x}\n", marchid));
        content.push_str(&format!("mimpid\t\t: {:#x}\n", mimpid));

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}

/// Parse MISA register to get ISA string
///
/// MISA register format:
/// - Bits 63:0 - MXL (Machine XLEN) - 1=32, 2=64, 3=128
/// - Bits 31:0 - Extensions bitmap
///
/// Reference: RISC-V Privileged Architecture, section 3.1.1
fn parse_misa(misa: u64) -> alloc::string::String {
    let mxl = (misa >> 62) & 0x3;
    let extensions = misa as u32;

    // Base ISA
    let base = match mxl {
        1 => "rv32",
        2 => "rv64",
        3 => "rv128",
        _ => "rv64",
    };

    // Standard extensions (in alphabetical order as Linux does)
    let mut ext_str = alloc::string::String::new();

    // Extension bitmap (bit position = letter - 'a')
    // A = bit 0, B = bit 1, ..., Z = bit 25
    let extension_names = [
        'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
        'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z'
    ];

    for (i, &name) in extension_names.iter().enumerate() {
        if (extensions >> i) & 1 != 0 {
            // Skip some extensions that are implied or not standard
            // 'i' is implied by base, 'e' is for embedded
            ext_str.push(name);
        }
    }

    format!("{}{}", base, ext_str)
}
