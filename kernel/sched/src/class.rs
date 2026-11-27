//! scheduling-class flag constants (Linux ABI parity values).
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


pub const ENQUEUE_WAKEUP: i32 = 0x0001;

pub const ENQUEUE_RESTORE: i32 = 0x0002;

pub const ENQUEUE_MOVE: i32 = 0x0004;

pub const ENQUEUE_NOCLOCK: i32 = 0x0008;

pub const ENQUEUE_MIGRATING: i32 = 0x0010;

pub const ENQUEUE_HEAD: i32 = 0x00010000;

pub const ENQUEUE_REPLENISH: i32 = 0x00020000;

pub const ENQUEUE_MIGRATED: i32 = 0x00040000;

pub const DEQUEUE_SLEEP: i32 = 0x0001;

pub const DEQUEUE_SAVE: i32 = 0x0002;

pub const DEQUEUE_MOVE: i32 = 0x0004;

pub const DEQUEUE_NOCLOCK: i32 = 0x0008;

pub const DEQUEUE_MIGRATING: i32 = 0x0010;

pub const WF_EXEC: i32 = 0x02;

pub const WF_FORK: i32 = 0x04;

pub const WF_TTWU: i32 = 0x08;

pub const WF_SYNC: i32 = 0x10;

pub const WF_MIGRATED: i32 = 0x20;

pub const WF_CURRENT_CPU: i32 = 0x40;
