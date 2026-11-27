//! pick-generation ledger storage.
//!
//! Shrink stage (user method 2026-11-26; whole-subsystem move, user order
//! 2026-11-27): moved from the core's sched/hotpath.rs — zero samples in the
//! p23-hotmap union (docs/development/core-hotmap.md) and no staying core
//! caller. crate-level forbid(unsafe_code) applies.

#![forbid(unsafe_code)]
use crate::config::MAX_CPUS;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::arch;
use crate::config::{CFS_LATENCY_NS, CFS_MIN_GRANULARITY_NS, KERNEL_HZ, MAX_TASKS};
use crate::errno;
use crate::list::ListHead;
use crate::println;
use crate::process::{types::Pid, types::SchedPolicy, types::Task, types::TaskState};
use crate::sync::types::{RawSpinlock, Spinlock};
use crate::trace;


pub static PICK_GEN: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

pub static PICK_GEN_DONE: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

pub static PICK_GEN_TASK: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

pub static PICK_GEN_PID: [core::sync::atomic::AtomicU32; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_CPUS];

pub static PICK_GEN_JIF: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

pub static PICKGEN_LOST: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];
