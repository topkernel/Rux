//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/pressure/{cpu,memory,io} — Pressure Stall Information
//! (the reference kernel's fs/proc/pressure.c proc_pressure_* files).
//!
//! Line shape (identical to the reference):
//!
//! ```text
//! some avg10=0.00 avg60=0.00 avg300=0.00 total=0
//! full avg10=0.00 avg60=0.00 avg300=0.00 total=0
//! ```
//!
//! `cpu` carries the `some` line only ("CPU FULL is undefined at the
//! system level" — psi.c psi_show); `memory` and `io` carry both.
//! The accounting and the avg windows live in kernel/src/sched/psi.rs
//! (see that module for the cost model).

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::vec::Vec;

/// /proc/pressure/cpu
pub fn generate_cpu() -> Vec<u8> {
    crate::sched::psi::generate(crate::sched::psi::Resource::Cpu)
}

/// /proc/pressure/memory
pub fn generate_memory() -> Vec<u8> {
    crate::sched::psi::generate(crate::sched::psi::Resource::Memory)
}

/// /proc/pressure/io
pub fn generate_io() -> Vec<u8> {
    crate::sched::psi::generate(crate::sched::psi::Resource::Io)
}
