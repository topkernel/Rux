//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The fs journal BIG LOCK (rung-5 relocation from ext4/mod.rs). The
//! lock protocol is FLOOR per the micro-core design (§2: sync
//! primitives' interiors stay core; the whole-kernel lock protocol —
//! owner tracking, waiter queues, force-release on exit, the VW and
//! census diagnostics — is machine core, not filesystem policy), so
//! the STATIC lives framework-side: the exit path, the virtio VW
//! watchdog and dfx taskdump reach it WITHOUT a hook row, and the
//! ext4/jbd2 service crate consumes it as a pub API.
//!
//! Task-recursive: a loop device backed by a file on ANOTHER ext4
//! instance re-enters ext4 from inside the block layer (ext4#2's create
//! writes metadata through /dev/loopN into ext4#1, which takes the same
//! lock). Nested acquire by the SAME task is legal; the outermost guard
//! releases. do_exit() force-drops via release_if_owner (R73) since
//! RAII never runs through do_exit.
pub static EXT4_BIG_LOCK: crate::sync::SleepMutex =
    crate::sync::SleepMutex::new_rt("EXT4-BIGLOCK", true);
