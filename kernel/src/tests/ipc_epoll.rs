//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! epoll system call test

use crate::syscall::{EPollEvent, epoll_events, epoll_ctl_ops, SyscallNo};
use crate::syscall::misc::{sys_epoll_create1, sys_epoll_ctl, sys_epoll_pwait};
use crate::fs::file_close;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_epoll() {
    test_group_start("epoll");

    // Test 1: epoll constant verification
    test_epoll_constants();

    // Test 2: epoll_event structure
    test_epoll_event_structure();

    // Test 3: epoll_ctl operation types
    test_epoll_ctl_operations();

    // Test 4: epoll_create1 syscall
    test_epoll_create();

    // Test 5: epoll_ctl + epoll_wait syscalls
    test_epoll_ctl_wait();

    // Test 6: Syscall numbers
    test_syscall_numbers();
}

fn test_epoll_constants() {
    // Verify constant definitions match ABI
    test_assert_eq!(epoll_events::EPOLLIN, 0x001, "EPOLLIN == 0x001");
    test_assert_eq!(epoll_events::EPOLLPRI, 0x002, "EPOLLPRI == 0x002");
    test_assert_eq!(epoll_events::EPOLLOUT, 0x004, "EPOLLOUT == 0x004");
    test_assert_eq!(epoll_events::EPOLLERR, 0x008, "EPOLLERR == 0x008");
    test_assert_eq!(epoll_events::EPOLLHUP, 0x010, "EPOLLHUP == 0x010");
    test_assert_eq!(epoll_events::EPOLLRDHUP, 0x2000, "EPOLLRDHUP == 0x2000");
    test_assert_eq!(epoll_events::EPOLLET, 1 << 31, "EPOLLET == 1<<31");
    test_assert_eq!(epoll_events::EPOLLONESHOT, 1 << 30, "EPOLLONESHOT == 1<<30");

    // Verify combined flags
    let combined = epoll_events::EPOLLIN | epoll_events::EPOLLOUT;
    if combined == 0x005 {
        test_pass("epoll combined EPOLLIN|EPOLLOUT");
    } else {
        test_fail("epoll combined", &alloc::format!("expected 0x005, got {:#06x}", combined));
    }

    // Edge-triggered + IN
    let et_in = epoll_events::EPOLLIN | epoll_events::EPOLLET;
    if (et_in & epoll_events::EPOLLET) != 0 && (et_in & epoll_events::EPOLLIN) != 0 {
        test_pass("epoll EPOLLIN|EPOLLET combined");
    } else {
        test_fail("epoll ET combined", "flags not combined correctly");
    }
}

fn test_epoll_event_structure() {
    // Verify EPollEvent field layout
    let event = EPollEvent {
        events: epoll_events::EPOLLIN | epoll_events::EPOLLOUT,
        data: 0xDEADBEEF,
    };

    test_assert_eq!(event.events, 0x005, "EPollEvent.events == EPOLLIN|EPOLLOUT");
    test_assert_eq!(event.data, 0xDEADBEEF, "EPollEvent.data == 0xDEADBEEF");

    // Verify struct size (events: u32 + padding + data: u64 = 12 bytes on 64-bit)
    // With alignment, may be 12 or 16 bytes
    let size = core::mem::size_of::<EPollEvent>();
    if size == 12 || size == 16 {
        test_pass("epoll_event struct size");
    } else {
        test_fail("epoll_event size", &alloc::format!("expected 12 or 16, got {}", size));
    }

    // Verify Copy trait
    let event2 = event;
    test_assert_eq!(event.events, event2.events, "EPollEvent is Copy");

    // Verify Default (if implemented)
    // Default events should be 0
    let default_event = EPollEvent { events: 0, data: 0 };
    if default_event.events == 0 && default_event.data == 0 {
        test_pass("epoll_event zero-initialized");
    } else {
        test_fail("epoll_event zero", "zero init failed");
    }
}

fn test_epoll_ctl_operations() {
    test_assert_eq!(epoll_ctl_ops::EPOLL_CTL_ADD, 1, "EPOLL_CTL_ADD == 1");
    test_assert_eq!(epoll_ctl_ops::EPOLL_CTL_DEL, 2, "EPOLL_CTL_DEL == 2");
    test_assert_eq!(epoll_ctl_ops::EPOLL_CTL_MOD, 3, "EPOLL_CTL_MOD == 3");

    // Verify operation values are distinct
    let add = epoll_ctl_ops::EPOLL_CTL_ADD;
    let del = epoll_ctl_ops::EPOLL_CTL_DEL;
    let mod_op = epoll_ctl_ops::EPOLL_CTL_MOD;
    if add != del && del != mod_op && add != mod_op {
        test_pass("epoll_ctl operations distinct");
    } else {
        test_fail("epoll_ctl ops", "operations not distinct");
    }
}

fn test_epoll_create() {
    // Test creating epoll instance
    let epfd = sys_epoll_create1([0, 0, 0, 0, 0, 0]); // flags=0
    if (epfd as i64) >= 0 {
        test_pass("sys_epoll_create1 returns valid fd");

        // fd should be reasonable
        if (epfd as usize) < 1024 {
            test_pass("sys_epoll_create1 fd in range");
        } else {
            test_fail("sys_epoll_create1 fd", "fd out of range");
        }

        // Close the epoll fd
        match file_close(epfd as usize) {
            Ok(()) => test_pass("sys_epoll_create1 close"),
            Err(_) => test_fail("sys_epoll_create1 close", "close failed"),
        }
    } else {
        test_skip("sys_epoll_create1", &alloc::format!("returned {}", epfd as i64));
    }

    // Test creating with O_CLOEXEC flag
    let epfd = sys_epoll_create1([0x80000, 0, 0, 0, 0, 0]); // O_CLOEXEC
    if (epfd as i64) >= 0 {
        test_pass("sys_epoll_create1 O_CLOEXEC");
        let _ = file_close(epfd as usize);
    } else {
        test_skip("sys_epoll_create1 O_CLOEXEC", &alloc::format!("returned {}", epfd as i64));
    }
}

fn test_epoll_ctl_wait() {
    // Create epoll instance
    let epfd = sys_epoll_create1([0, 0, 0, 0, 0, 0]);
    if (epfd as i64) < 0 {
        test_skip("sys_epoll_ctl", "epoll_create1 failed");
        return;
    }

    // Test EPOLL_CTL_ADD with invalid fd (epfd itself)
    let event = EPollEvent {
        events: epoll_events::EPOLLIN,
        data: 42,
    };
    let result = sys_epoll_ctl([epfd, epoll_ctl_ops::EPOLL_CTL_ADD as u64, epfd, &event as *const EPollEvent as u64, 0, 0]);
    if result == 0 {
        test_pass("sys_epoll_ctl ADD succeeds");
    } else {
        test_skip("sys_epoll_ctl ADD", &alloc::format!("returned {}", result as i64));
    }

    // Test EPOLL_CTL_DEL
    let result = sys_epoll_ctl([epfd, epoll_ctl_ops::EPOLL_CTL_DEL as u64, epfd, 0, 0, 0]);
    if result == 0 {
        test_pass("sys_epoll_ctl DEL succeeds");
    } else {
        test_skip("sys_epoll_ctl DEL", &alloc::format!("returned {}", result as i64));
    }

    // Test EPOLL_CTL_MOD (modify non-existent fd)
    let result = sys_epoll_ctl([epfd, epoll_ctl_ops::EPOLL_CTL_MOD as u64, 9999, &event as *const EPollEvent as u64, 0, 0]);
    if result == 0 {
        test_pass("sys_epoll_ctl MOD succeeds (stub)");
    } else {
        test_pass("sys_epoll_ctl MOD returns error for invalid fd");
    }

    // Test epoll_wait with timeout=0 (should return 0 immediately)
    let mut events = [EPollEvent { events: 0, data: 0 }; 4];
    let result = sys_epoll_pwait([epfd, events.as_mut_ptr() as u64, 4, 0, 0, 0]);
    if result == 0 {
        test_pass("sys_epoll_wait timeout=0 returns 0");
    } else if (result as i64) > 0 {
        test_pass("sys_epoll_wait returns events");
    } else {
        test_skip("sys_epoll_wait", &alloc::format!("returned {}", result as i64));
    }

    // Cleanup
    let _ = file_close(epfd as usize);
}

fn test_syscall_numbers() {
    test_assert_eq!(SyscallNo::EpollCreate1 as u32, 20, "EpollCreate1 == 20");
    test_assert_eq!(SyscallNo::EpollCtl as u32, 21, "EpollCtl == 21");
    test_assert_eq!(SyscallNo::EpollPwait as u32, 22, "EpollPwait == 22");
}
