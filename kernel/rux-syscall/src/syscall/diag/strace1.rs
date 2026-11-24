//! The STRACE1 loader-hunt formatter (moved from
//! kernel/src/syscall/dispatch.rs at the cold-four policy flip — sole
//! copy here; zero behavior change). Reached through the
//! SyscallHooks::strace1_line row behind the EVENT_STRACE1 + pid-1
//! gate at the dispatch call site (disarmed boots never touch the
//! table).

/// rux:strace1 (retired debug=strace1): one console line per loader-relevant syscall of PID 1.
///
/// Numbers are the GENERIC (asm-generic) dispatch numbers — the x86_64
/// native numbers were translated before the dispatch match.
pub fn strace1_line(nr: u32, args: &[u64; 6], result: i64) {
    // (name, pathname argument index or None)
    let entry: Option<(&'static str, Option<usize>)> = match nr {
        48 => Some(("faccessat", Some(1))),   // (dirfd, path, mode)
        56 => Some(("openat", Some(1))),      // (dirfd, path, flags, mode)
        57 => Some(("close", None)),
        61 => Some(("getdents64", None)),
        62 => Some(("lseek", None)),
        63 => Some(("read", None)),
        64 => Some(("write", None)),
        67 => Some(("pread64", None)),
        78 => Some(("readlinkat", Some(1))),
        79 => Some(("fstatat", Some(1))),
        80 => Some(("fstat", None)),
        214 => Some(("brk", None)),
        215 => Some(("munmap", None)),
        221 => Some(("execve", Some(0))),
        222 => Some(("mmap", None)),          // fd/off shown numerically
        226 => Some(("mprotect", None)),
        // 118th ladder hunt: the loop/event family — where does init's
        // job engine diverge from the reference (eventfd/timerfd/epoll/
        // signalfd/rt_sigreturn + the usual wait family).
        9 => Some(("mmap", None)),            // mmap legacy
        10 => Some(("mprotect", None)),       // mprotect legacy
        11 => Some(("munmap", None)),         // munmap legacy
        232 => Some(("epoll_wait", None)),
        281 => Some(("epoll_pwait", None)),
        233 => Some(("epoll_ctl", None)),
        291 => Some(("epoll_create1", None)),
        290 => Some(("eventfd2", None)),
        283 => Some(("timerfd_create", None)),
        286 => Some(("timerfd_settime", None)),
        126 => Some(("sigaltstack", None)),
        34 => Some(("rt_sigprocmask", None)),
        13 => Some(("rt_sigaction", None)),
        135 => Some(("rt_sigprocmask", None)), // legacy sigprocmask
        431 => Some(("clone3", None)),        // modern spawn path
        437 => Some(("clock_nanosleep", None)),
        230 => Some(("clock_nanosleep", None)),
        _ => None,
    };
    let (name, path_arg) = match entry {
        Some(e) => e,
        None => return,
    };

    let mut line: alloc::string::String = alloc::format!("S1 {}(", name);
    match path_arg {
        Some(i) => {
            let mut buf = [0u8; 128];
            let shown = match crate::arch::uaccess::strncpy_from_user(
                args[i] as *const u8, buf.len(), &mut buf,
            ) {
                Ok(s) => core::str::from_utf8(s).unwrap_or("<non-utf8>"),
                Err(_) => "<efault>",
            };
            line.push_str(&alloc::format!("\"{}\"", shown));
            if i > 0 {
                line.push_str(&alloc::format!(", dirfd={}", args[0]));
            }
            if name == "openat" {
                line.push_str(&alloc::format!(", flags={:#x}", args[2]));
            }
        }
        None => match name {
            "mmap" => line.push_str(&alloc::format!(
                "addr={:#x} len={:#x} prot={:#x} flags={:#x} fd={} off={:#x}",
                args[0], args[1], args[2], args[3], args[4] as i64, args[5]
            )),
            "read" | "pread64" => line.push_str(&alloc::format!(
                "fd={} count={:#x}{}", args[0] as i64, args[2],
                if name == "pread64" { alloc::format!(", off={:#x}", args[3]) } else { alloc::string::String::new() }
            )),
            _ => line.push_str(&alloc::format!(
                "{}{}", args[0] as i64,
                if name == "write" || name == "lseek" || name == "fstat" || name == "close" {
                    alloc::format!(", {:#x}", args[1])
                } else if name == "lseek" || name == "mprotect" {
                    alloc::format!(", {:#x}, {:#x}", args[1], args[2])
                } else {
                    alloc::string::String::new()
                }
            )),
        },
    }
    line.push_str(&alloc::format!(") = {}", result));
    crate::pr_warn!("{}", line);
}
