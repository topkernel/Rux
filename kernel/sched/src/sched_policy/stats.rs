//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Per-task scheduler statistics — the runtime gate and the
//! /proc/[pid]/schedstat triple (T7 perf-support slice).
//!
//! The mandate's "perf 支持" asked for the smallest useful per-task
//! counter surface the perf lane's pbench can consume in-guest. That
//! is Linux's `/proc/[pid]/schedstat` (CONFIG_SCHEDSTATS): three
//! numbers — on-cpu time (ns), runqueue wait time (ns), timeslices —
//! that turn a bench's wall time into on-cpu vs waiting per task
//! (P2 ctxsw, P4 fork/exec). `perf_event_open` is deliberately NOT in
//! this slice: the attr/ring/ioctl ABI is a tranche of its own, and a
//! plain file read is what pbench can consume today.
//!
//! The gate mirrors the reference's `/proc/sys/kernel/sched_schedstats`
//! (0/1, default 0 — runtime-toggleable behind a static key since
//! Linux 4.10). Rux's equivalent of the static key is one relaxed
//! atomic load at each schedule edge when off; the clock reads and
//! counter arithmetic run only when on. The accounting edges live in
//! `__schedule` (one `sched_clock` read serves the pick and the put)
//! and in `enqueue_task_locked` (the wait-window open, under the GRQ
//! lock).
//!
//! Devirt lane (2026-11-23): the gate's STORAGE (SCHEDSTATS) and its
//! two accessors (enabled/set_enabled) are framework-native now
//! (sched/entities.rs + sched/sched.rs) — a boolean load is not
//! policy, and the per-schedule read was paying a SchedHooks dispatch
//! for it. The sysctl and __schedule sites call them directly; the
//! module stays as the doc home for the schedstat slice.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]
