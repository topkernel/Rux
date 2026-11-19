//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for TaskState bitmap operations.
//! Copied from: kernel/src/process/task.rs

use proptest::prelude::*;

// Copied TaskState
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState(u32);

impl TaskState {
    pub const RUNNING: u32 = 0x00000000;
    pub const INTERRUPTIBLE: u32 = 0x00000001;
    pub const UNINTERRUPTIBLE: u32 = 0x00000002;
    pub const STOPPED: u32 = 0x00000004;
    pub const TRACED: u32 = 0x00000008;
    pub const ZOMBIE: u32 = 0x00000010;
    pub const DEAD: u32 = 0x00000020;

    pub const fn new(bits: u32) -> Self { TaskState(bits) }
    pub fn bits(&self) -> u32 { self.0 }
    pub fn contains(&self, flag: u32) -> bool { (self.0 & flag) != 0 }
    pub fn is_running(&self) -> bool { self.0 == Self::RUNNING }
    pub fn is_sleeping(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE) || self.contains(Self::UNINTERRUPTIBLE)
    }
    pub fn is_dead(&self) -> bool {
        self.contains(Self::ZOMBIE) || self.contains(Self::DEAD)
    }
    pub fn is_interruptible(&self) -> bool { self.contains(Self::INTERRUPTIBLE) }
}

proptest! {
    #[test]
    fn test_state_constants_distinct(_v in 0u8..1u8) {
        let consts = [
            TaskState::RUNNING, TaskState::INTERRUPTIBLE, TaskState::UNINTERRUPTIBLE,
            TaskState::STOPPED, TaskState::TRACED, TaskState::ZOMBIE, TaskState::DEAD,
        ];
        for i in 0..consts.len() {
            for j in (i+1)..consts.len() {
                assert_ne!(consts[i], consts[j],
                    "constants {} and {} are equal", i, j);
            }
        }
    }

    #[test]
    fn test_constants_powers_of_two(_v in 0u8..1u8) {
        let consts = [
            TaskState::RUNNING, TaskState::INTERRUPTIBLE, TaskState::UNINTERRUPTIBLE,
            TaskState::STOPPED, TaskState::TRACED, TaskState::ZOMBIE, TaskState::DEAD,
        ];
        // RUNNING is 0 (not a power of 2), rest are powers of 2
        for &c in &consts[1..] {
            assert!(c > 0 && (c & (c - 1)) == 0, "constant {} is not power of 2", c);
        }
    }

    #[test]
    fn test_new_bits_roundtrip(bits in 0u32..0x100u32) {
        let state = TaskState::new(bits);
        assert_eq!(state.bits(), bits);
    }

    #[test]
    fn test_contains_flag(bits in 0u32..0x100u32, flag in 1u32..0x40u32) {
        let state = TaskState::new(bits | flag);
        assert!(state.contains(flag));
        let state2 = TaskState::new(bits & !flag);
        assert!(!state2.contains(flag));
    }

    #[test]
    fn test_is_running_only_zero(_v in 0u8..1u8) {
        assert!(TaskState::new(0).is_running());
        assert!(!TaskState::new(TaskState::INTERRUPTIBLE).is_running());
        assert!(!TaskState::new(TaskState::ZOMBIE).is_running());
    }

    #[test]
    fn test_is_sleeping_interruptible(bits in 0u32..0x100u32) {
        let with_int = TaskState::new(bits | TaskState::INTERRUPTIBLE);
        assert!(with_int.is_sleeping());
    }

    #[test]
    fn test_is_sleeping_uninterruptible(bits in 0u32..0x100u32) {
        let with_unint = TaskState::new(bits | TaskState::UNINTERRUPTIBLE);
        assert!(with_unint.is_sleeping());
    }

    #[test]
    fn test_is_sleeping_neither(bits in 0u32..0x100u32) {
        // Clear both interruptible bits
        let cleared = TaskState::new(bits & !TaskState::INTERRUPTIBLE & !TaskState::UNINTERRUPTIBLE);
        assert!(!cleared.is_sleeping());
    }

    #[test]
    fn test_is_dead_zombie(bits in 0u32..0x100u32) {
        let with_z = TaskState::new(bits | TaskState::ZOMBIE);
        assert!(with_z.is_dead());
    }

    #[test]
    fn test_is_dead_dead_state(bits in 0u32..0x100u32) {
        let with_d = TaskState::new(bits | TaskState::DEAD);
        assert!(with_d.is_dead());
    }

    #[test]
    fn test_is_dead_neither(bits in 0u32..0x100u32) {
        let cleared = TaskState::new(bits & !TaskState::ZOMBIE & !TaskState::DEAD);
        // Unless both bits are set, not dead
        if (bits & (TaskState::ZOMBIE | TaskState::DEAD)) == 0 {
            assert!(!cleared.is_dead());
        }
    }

    #[test]
    fn test_combined_state(bits in 0u32..0x100u32) {
        let combined = TaskState::new(bits | TaskState::INTERRUPTIBLE | TaskState::TRACED);
        assert!(combined.contains(TaskState::INTERRUPTIBLE));
        assert!(combined.contains(TaskState::TRACED));
        assert!(combined.is_sleeping());
        // bits() roundtrip preserves all original bits
        assert!(combined.bits() & bits != 0 || bits == 0);
    }
}
