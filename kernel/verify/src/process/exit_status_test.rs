//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for wait status encoding (POSIX ABI).
//! Copied from: kernel/src/process/exit.rs

use proptest::prelude::*;

// Copied wait status encoding functions
// Normal exit: status = ((exit_code & 0xFF) << 8)
// Signal kill: status = (|signal| & 0x7F)
// Stopped:    status = ((stop_sig << 8) | 0x7F)

pub fn encode_exit_status(exit_code: i32) -> i32 {
    (((exit_code as u32) & 0xFF) << 8) as i32
}

pub fn encode_signal_status(signal: i32) -> i32 {
    // Kernel: (-(raw_exit as i32) as u32 & 0x7F) — raw_exit is negative, so negate to positive
    (signal as u32 & 0x7F) as i32
}

pub fn encode_stopped_status(stop_sig: u32) -> i32 {
    (((stop_sig as u32) << 8) | 0x7F) as i32
}

// WIFEXITED: bit 7 is 0
pub fn wifexited(status: i32) -> bool {
    (status & 0x7F) == 0
}

// WEXITSTATUS: bits 15..8
pub fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xFF
}

// WIFSIGNALED: bit 7 is set and not stopped (0x7F)
pub fn wifsignaled(status: i32) -> bool {
    ((status & 0x7F) + 1) >> 1 > 0 && !wifstopped(status)
}

// WTERMSIG: bits 6..0
pub fn wtermsig(status: i32) -> i32 {
    status & 0x7F
}

// WIFSTOPPED: low byte == 0x7F (POSIX spec)
pub fn wifstopped(status: i32) -> bool {
    (status & 0xFF) == 0x7F
}

// WSTOPSIG: bits 15..8
pub fn wstopsig(status: i32) -> i32 {
    (status >> 8) & 0xFF
}

proptest! {
    #[test]
    fn test_encode_exit_roundtrip(exit_code in 0i32..256i32) {
        let status = encode_exit_status(exit_code);
        assert!(wifexited(status));
        assert_eq!(wexitstatus(status), exit_code & 0xFF);
        assert!(!wifsignaled(status));
        assert!(!wifstopped(status));
    }

    #[test]
    fn test_encode_exit_clamps_high(exit_code in 256i32..1000i32) {
        let status = encode_exit_status(exit_code);
        assert!(wifexited(status));
        // High bits truncated to 8 bits
        assert_eq!(wexitstatus(status), (exit_code & 0xFF) as i32);
    }

    #[test]
    fn test_encode_signal_roundtrip(signal in 1i32..64i32) {
        let status = encode_signal_status(signal);
        assert!(wifsignaled(status));
        assert_eq!(wtermsig(status), signal & 0x7F);
        assert!(!wifexited(status));
        assert!(!wifstopped(status));
    }

    #[test]
    fn test_encode_stopped_roundtrip(stop_sig in 1u32..32u32) {
        let status = encode_stopped_status(stop_sig);
        assert!(wifstopped(status));
        assert_eq!(wstopsig(status), stop_sig as i32);
        assert!(!wifexited(status));
        assert!(!wifsignaled(status));
    }

    #[test]
    fn test_exit_zero(_v in 0u8..1u8) {
        let status = encode_exit_status(0);
        assert_eq!(wexitstatus(status), 0);
        assert!(wifexited(status));
    }

    #[test]
    fn test_exit_255(_v in 0u8..1u8) {
        let status = encode_exit_status(255);
        assert_eq!(wexitstatus(status), 255);
    }

    #[test]
    fn test_signal_sigkill(_v in 0u8..1u8) {
        let status = encode_signal_status(9);
        assert_eq!(wtermsig(status), 9);
    }

    #[test]
    fn test_stopped_sigstop(_v in 0u8..1u8) {
        let status = encode_stopped_status(19);
        assert!(wifstopped(status));
        assert_eq!(wstopsig(status), 19);
    }

    #[test]
    fn test_exit_status_low_byte_only(exit_code in -1000i32..0i32) {
        // Negative exit codes should be treated as signals (in kernel do_wait)
        // But encode_exit_status takes the raw code and masks to 8 bits
        let status = encode_exit_status(exit_code);
        // The & 0xFF mask applies to the u32 cast of a negative number
        let masked = (exit_code as u32) & 0xFF;
        assert_eq!(wexitstatus(status), masked as i32);
    }

    #[test]
    fn test_wifexited_vs_wifsignaled_exclusive(exit_code in 0i32..256i32, signal in 1i32..64i32) {
        let exit_st = encode_exit_status(exit_code);
        let sig_st = encode_signal_status(signal);
        assert!(wifexited(exit_st) != wifsignaled(exit_st));
        assert!(wifsignaled(sig_st) != wifexited(sig_st));
    }

    #[test]
    fn test_wtermsig_mask(signal in 1i32..128i32) {
        let status = encode_signal_status(signal);
        // Signal > 0x7F should be masked
        assert_eq!(wtermsig(status), signal & 0x7F);
    }

    #[test]
    fn test_stopped_preserves_low_bits(_v in 0u8..1u8) {
        // Stopped status always has 0x7F in low byte
        let status = encode_stopped_status(5);
        assert_eq!(status & 0xFF, 0x7F);
    }
}
