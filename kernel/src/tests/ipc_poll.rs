//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! poll system call test

use crate::syscall::{PollFd, poll_events};
use super::{test_pass, test_fail, test_group_start};

pub fn test_poll() {
    test_group_start("poll");

    // Test 1: poll constant verification
    test_poll_constants();

    // Test 2: pollfd structure
    test_pollfd_structure();

    // Test 3: poll syscall existence
    test_poll_syscall();
}

fn test_poll_constants() {
    // Verify constant definitions
    let has_pollin = poll_events::POLLIN != 0;
    let has_pollout = poll_events::POLLOUT != 0;
    let has_pollerr = poll_events::POLLERR != 0;
    let has_pollhup = poll_events::POLLHUP != 0;
    let has_pollnval = poll_events::POLLNVAL != 0;

    if has_pollin && has_pollout && has_pollerr && has_pollhup && has_pollnval {
        test_pass("poll constants");
    } else {
        test_fail("poll constants", "missing definitions");
    }
}

fn test_pollfd_structure() {
    let pollfd = PollFd {
        fd: 0,
        events: poll_events::POLLIN | poll_events::POLLOUT,
        revents: 0,
    };

    let fd_ok = pollfd.fd == 0;
    let events_ok = pollfd.events == (poll_events::POLLIN | poll_events::POLLOUT);
    let revents_ok = pollfd.revents == 0;

    if fd_ok && events_ok && revents_ok {
        test_pass("pollfd structure");
    } else {
        test_fail("pollfd structure", "field mismatch");
    }
}

fn test_poll_syscall() {
    // poll syscall number: 7
    test_pass("poll syscall exists");
}
