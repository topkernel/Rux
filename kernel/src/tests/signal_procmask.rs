use crate::signal::{Signal, SigSet, SigAction, SigActionKind, SigFlags, sigprocmask_how, SIGRTMIN, SIGRTMAX};
use super::{test_pass, test_fail, test_group_start};

pub fn test_sigprocmask() {
    test_group_start("rt_sigprocmask");

    // Test 1: sigprocmask_how constants
    test_assert_eq!(sigprocmask_how::SIG_BLOCK, 0, "SIG_BLOCK == 0");
    test_assert_eq!(sigprocmask_how::SIG_UNBLOCK, 1, "SIG_UNBLOCK == 1");
    test_assert_eq!(sigprocmask_how::SIG_SETMASK, 2, "SIG_SETMASK == 2");

    // Test 2: Signal number constants (Linux ABI)
    test_assert_eq!(Signal::SIGHUP as i32, 1, "SIGHUP == 1");
    test_assert_eq!(Signal::SIGINT as i32, 2, "SIGINT == 2");
    test_assert_eq!(Signal::SIGQUIT as i32, 3, "SIGQUIT == 3");
    test_assert_eq!(Signal::SIGILL as i32, 4, "SIGILL == 4");
    test_assert_eq!(Signal::SIGABRT as i32, 6, "SIGABRT == 6");
    test_assert_eq!(Signal::SIGFPE as i32, 8, "SIGFPE == 8");
    test_assert_eq!(Signal::SIGKILL as i32, 9, "SIGKILL == 9");
    test_assert_eq!(Signal::SIGSEGV as i32, 11, "SIGSEGV == 11");
    test_assert_eq!(Signal::SIGPIPE as i32, 13, "SIGPIPE == 13");
    test_assert_eq!(Signal::SIGALRM as i32, 14, "SIGALRM == 14");
    test_assert_eq!(Signal::SIGTERM as i32, 15, "SIGTERM == 15");
    test_assert_eq!(Signal::SIGCHLD as i32, 17, "SIGCHLD == 17");
    test_assert_eq!(Signal::SIGCONT as i32, 18, "SIGCONT == 18");
    test_assert_eq!(Signal::SIGSTOP as i32, 19, "SIGSTOP == 19");
    test_assert_eq!(Signal::SIGTSTP as i32, 20, "SIGTSTP == 20");
    test_assert_eq!(Signal::SIGUSR1 as i32, 10, "SIGUSR1 == 10");
    test_assert_eq!(Signal::SIGUSR2 as i32, 12, "SIGUSR2 == 12");

    // Test 3: Realtime signal range
    test_assert_eq!(SIGRTMIN, 32, "SIGRTMIN == 32");
    test_assert_eq!(SIGRTMAX, 64, "SIGRTMAX == 64");

    // Test 4: SigSet operations
    let mut set: SigSet = 0;
    test_assert_eq!(set, 0, "SigSet initial == 0");

    // Test 5: SigFlags constants
    test_assert_eq!(SigFlags::SA_NOCLDSTOP, 0x00000001, "SA_NOCLDSTOP");
    test_assert_eq!(SigFlags::SA_NOCLDWAIT, 0x00000002, "SA_NOCLDWAIT");
    test_assert_eq!(SigFlags::SA_SIGINFO, 0x00000004, "SA_SIGINFO");
    test_assert_eq!(SigFlags::SA_ONSTACK, 0x08000000, "SA_ONSTACK");
    test_assert_eq!(SigFlags::SA_RESTART, 0x10000000, "SA_RESTART");
    test_assert_eq!(SigFlags::SA_NODEFER, 0x40000000, "SA_NODEFER");
    test_assert_eq!(SigFlags::SA_RESETHAND, 0x80000000, "SA_RESETHAND");

    // Test 6: SigFlags::new and bits()
    let flags = SigFlags::new(SigFlags::SA_SIGINFO | SigFlags::SA_RESTART);
    test_assert_eq!(flags.bits(), SigFlags::SA_SIGINFO | SigFlags::SA_RESTART, "SigFlags::bits()");

    // Test 7: SigAction::new
    let action = SigAction::new();
    test_assert!(!action.has_handler(), "SigAction::new() has no handler");

    // Test 8: SigAction::ignore
    let ignore = SigAction::ignore();
    match ignore.action() {
        SigActionKind::Ignore => test_pass("SigAction::ignore() is Ignore"),
        _ => test_fail("SigAction::ignore()", "not Ignore"),
    }

    // Test 9: SigAction action kind
    let action = SigAction::new();
    match action.action() {
        SigActionKind::Default => test_pass("SigAction::new() is Default"),
        _ => test_fail("SigAction::new()", "not Default"),
    }
}
