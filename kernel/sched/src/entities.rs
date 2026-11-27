//! entity arithmetic leftovers.
//!
//! Shrink stage (user method 2026-11-26; whole-subsystem move, user order
//! 2026-11-27): moved from the core's sched/hotpath.rs — zero samples in the
//! p23-hotmap union (docs/development/core-hotmap.md) and no staying core
//! caller. crate-level forbid(unsafe_code) applies.

#![forbid(unsafe_code)]

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


pub fn ms_to_ns(ms: u32) -> u64 {
    (ms as u64) * 1_000_000
}
