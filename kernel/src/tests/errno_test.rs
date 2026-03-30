use crate::errno::{Errno, constants};
use super::{test_pass, test_fail, test_group_start};

pub fn test_errno() {
    test_group_start("errno");

    // Test 1: EPERM
    test_assert_eq!(Errno::OperationNotPermitted.as_i32(), 1, "EPERM == 1");

    // Test 2: ENOENT
    test_assert_eq!(Errno::NoSuchFileOrDirectory.as_i32(), 2, "ENOENT == 2");

    // Test 3: ESRCH
    test_assert_eq!(Errno::NoSuchProcess.as_i32(), 3, "ESRCH == 3");

    // Test 4: EINTR
    test_assert_eq!(Errno::InterruptedSystemCall.as_i32(), 4, "EINTR == 4");

    // Test 5: EIO
    test_assert_eq!(Errno::IOError.as_i32(), 5, "EIO == 5");

    // Test 6: ENOEXEC
    test_assert_eq!(Errno::ExecFormatError.as_i32(), 8, "ENOEXEC == 8");

    // Test 7: EBADF
    test_assert_eq!(Errno::BadFileNumber.as_i32(), 9, "EBADF == 9");

    // Test 8: ENOMEM
    test_assert_eq!(Errno::OutOfMemory.as_i32(), 12, "ENOMEM == 12");

    // Test 9: EACCES
    test_assert_eq!(Errno::PermissionDenied.as_i32(), 13, "EACCES == 13");

    // Test 10: EINVAL
    test_assert_eq!(Errno::InvalidArgument.as_i32(), 22, "EINVAL == 22");

    // Test 11: ENOSPC
    test_assert_eq!(Errno::NoSpaceLeftOnDevice.as_i32(), 28, "ENOSPC == 28");

    // Test 12: EPIPE
    test_assert_eq!(Errno::BrokenPipe.as_i32(), 32, "EPIPE == 32");

    // Test 13: ENOSYS
    test_assert_eq!(Errno::FunctionNotImplemented.as_i32(), 38, "ENOSYS == 38");

    // Test 14: EOVERFLOW
    test_assert_eq!(Errno::ValueTooLarge.as_i32(), 75, "EOVERFLOW == 75");

    // Test 15: as_neg_i32
    test_assert_eq!(Errno::NoSuchFileOrDirectory.as_neg_i32(), -2, "as_neg_i32(ENOENT) == -2");
    test_assert_eq!(Errno::InvalidArgument.as_neg_i32(), -22, "as_neg_i32(EINVAL) == -22");
    test_assert_eq!(Errno::BrokenPipe.as_neg_i32(), -32, "as_neg_i32(EPIPE) == -32");

    // Test 16: as_neg_u64
    test_assert_eq!(Errno::NoSuchFileOrDirectory.as_neg_u64(), (-2i32) as u64, "as_neg_u64(ENOENT)");
    test_assert_eq!(Errno::InvalidArgument.as_neg_u64(), (-22i32) as u64, "as_neg_u64(EINVAL)");

    // Test 17: Constants module
    test_assert_eq!(constants::EPERM, 1, "constants::EPERM == 1");
    test_assert_eq!(constants::ENOENT, 2, "constants::ENOENT == 2");
    test_assert_eq!(constants::EBADF, 9, "constants::EBADF == 9");
    test_assert_eq!(constants::EINVAL, 22, "constants::EINVAL == 22");
    test_assert_eq!(constants::ENOMEM, 12, "constants::ENOMEM == 12");

    // Test 18: EAGAIN == EWOULDBLOCK (aliases)
    test_assert_eq!(constants::EAGAIN, 11, "constants::EAGAIN == 11");
    test_assert_eq!(constants::EWOULDBLOCK, 11, "constants::EWOULDBLOCK == 11");
    test_assert!(constants::EAGAIN == constants::EWOULDBLOCK, "EAGAIN == EWOULDBLOCK");

    // Test 19: Errno enum repr matches constants
    test_assert!(Errno::OperationNotPermitted.as_i32() == constants::EPERM, "Errno::EPERM == constants::EPERM");
    test_assert!(Errno::NoSuchFileOrDirectory.as_i32() == constants::ENOENT, "Errno::ENOENT == constants::ENOENT");
    test_assert!(Errno::InvalidArgument.as_i32() == constants::EINVAL, "Errno::EINVAL == constants::EINVAL");
}
