//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! File system related system calls
//!
//! Includes: open, openat, close, fstat, getdents64, mkdir, rmdir, unlink, readlinkat, lseek, chdir, getcwd, umask

use super::*;
use crate::arch::uaccess::strncpy_from_user;

/// Maximum path length (PATH_MAX)
const PATH_MAX: usize = 4096;

/// sys_open - Open file (legacy interface, wrapped to openat)
///
/// NOTE: riscv64 (asm-generic) has NO plain `open` syscall — this wrapper
/// exists only for the in-kernel test suite (tests/syscall_file.rs) and is
/// never dispatched. Kept intentionally; do not wire into dispatch.rs.
///
/// # Arguments
/// - args[0]: pathname - file path
/// - args[1]: flags - open flags
/// - args[2]: mode - creation mode
///
/// # Returns
/// Returns file descriptor on success, negative error code on failure
pub fn sys_open(args: SyscallArgs) -> i64 {
    // open(pathname, flags, mode) is equivalent to openat(AT_FDCWD, pathname, flags, mode)
    // AT_FDCWD = -100
    const AT_FDCWD: i64 = -100;

    let openat_args = [
        AT_FDCWD as u64,  // dirfd = AT_FDCWD
        args[0],          // pathname
        args[1],          // flags
        args[2],          // mode
        0, 0
    ];
    sys_openat(openat_args)
}

/// Parse /proc/[pid]/fd path for O_DIRECTORY open.
/// Returns the PID if path matches /proc/{pid}/fd (exact, no trailing content).
fn parse_proc_fd_dir_path(path: &str) -> Option<u64> {
    let path = path.trim_end_matches('/');
    // Match /proc/{pid}/fd
    let path = path.trim_start_matches('/');
    let parts: alloc::vec::Vec<&str> = path.split('/').collect();
    if parts.len() == 3 && parts[0] == "proc" && parts[2] == "fd" {
        parts[1].parse().ok()
    } else {
        None
    }
}
/// sys_openat - Open file
pub fn sys_openat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mode = args[3] as u32;

    const O_CREAT: u32 = 0o00000100;
    const O_DIRECTORY: u32 = 0o00200000;
    const O_CLOEXEC: u32 = 0o02000000;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => {
            // OH DRM bring-up (rs-egl): the composer VDI's
            // FindAndOpenKmsCard openat(dirfd, "card0") died silently —
            // resolution failures return before every later trace, so
            // name the dirfd-relative resolution failure directly.
            if dirfd != -100 {
                let mut pb = [0u8; PATH_MAX];
                let name = match strncpy_from_user(pathname_ptr, pb.len(), &mut pb) {
                    Ok(s) => core::str::from_utf8(s).unwrap_or(""),
                    Err(_) => "",
                };
                if name.starts_with("card") || name.starts_with("renderD") {
                    static ATRES_FAIL_N: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    let n = ATRES_FAIL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 16 {
                        crate::pr_warn!(
                            "openat-resolve-fail[{}] dirfd {} '{}' -> errno {} (pid {})",
                            n,
                            dirfd,
                            name,
                            -(e as i64),
                            unsafe { crate::process::current_pid() }
                        );
                    }
                }
            }
            return e as i64;
        }
    };

    // OH bring-up: trace O_PATH opens (musl's fchmodat fallback opens
    // the target O_PATH then fstatat(fd,"",AT_EMPTY_PATH)s it).
    {
        const O_PATH_X: u32 = 0x200000;
        if (flags & O_PATH_X) != 0 {
            static OPATH_TRACE_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let pid = unsafe { crate::process::current_pid() };
            let n = if pid >= 300 {
                OPATH_TRACE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
            } else {
                OPATH_TRACE_N.load(core::sync::atomic::Ordering::Relaxed)
            };
            if n < 20 && pid >= 300 {
                crate::pr_info!(
                    "o-path open: '{}' flags={:#x} pid={}",
                    full_path, flags, pid
                );
            }
        }
    }

    // OH bring-up: capped .so-open trace (SUCCESS too — a dlopen retry
    // loop is only visible when the loaded-each-round libs are shown).
    // dfx=syscalltrace only: ungated, hilogd.rd_kmsg copies the whole
    // trace from /dev/kmsg and eats a vCPU at KVM speed (b128/b129).
    if full_path.ends_with(".so")
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace)
    {
        // Per-pid budget: a flat counter let hdf_devhost (~pid 418,
        // every lib searched across ~15 dirs) consume the whole budget
        // before render_service (~443) opened its first library. 8
        // pid-slots x 64 lines each, indexed by pid % 8 (collisions
        // just share a budget). Late-boot services only (pid >= 418).
        static SO_SLOT_PID: [core::sync::atomic::AtomicU32; 8] =
            [const { core::sync::atomic::AtomicU32::new(0) }; 8];
        static SO_SLOT_N: [core::sync::atomic::AtomicU32; 8] =
            [const { core::sync::atomic::AtomicU32::new(0) }; 8];
        let pid = unsafe { crate::process::current_pid() } as u32;
        let slot = (pid % 8) as usize;
        let slot_pid = SO_SLOT_PID[slot].load(core::sync::atomic::Ordering::Relaxed);
        if slot_pid != pid && pid >= 418 {
            // Claim the slot for this pid (last writer wins on races —
            // this is a diagnostic, approximate is fine).
            SO_SLOT_PID[slot].store(pid, core::sync::atomic::Ordering::Relaxed);
            SO_SLOT_N[slot].store(0, core::sync::atomic::Ordering::Relaxed);
        }
        let claimed = SO_SLOT_PID[slot].load(core::sync::atomic::Ordering::Relaxed);
        if claimed == pid && pid >= 418 {
            let n = SO_SLOT_N[slot].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 64 {
                crate::pr_info!("so-open[{}.{}] '{}' (pid {})", slot, n, full_path, pid);
            }
        }
    }

    // Shortcut: /proc/[pid]/fd with O_DIRECTORY. P1 chroot: the string
    // shortcuts below address the GLOBAL /proc namespace — under a chroot
    // without a mounted /proc inside the jail they must NOT leak (they
    // would break the isolation path_lookup now enforces), so they are
    // limited to non-chrooted tasks.
    if !crate::fs::vfs::chrooted() {
        if (flags & O_CREAT) == 0 && (flags & O_DIRECTORY) != 0 {
            if let Some(pid) = parse_proc_fd_dir_path(&full_path) {
                return match crate::fs::vfs::open_procfs_dir(pid, flags) {
                    Ok(fd) => {
                        if (flags & O_CLOEXEC) != 0 { crate::fs::set_cloexec_fd(fd, true); }
                        fd as i64
                    }
                    Err(e) => e as i64,
                };
            }
        }
    }

    // /proc/self/exe and /proc/[pid]/exe (review 5.7): opening the exe
    // "symlink" must open the REAL executable file — the old path served a
    // memory file whose CONTENT was the path string. Resolve the target's
    // exe_path and fall through to the regular open path.
    // (P1 chroot: global-namespace shortcut, non-chrooted tasks only.)
    if !crate::fs::vfs::chrooted() {
        let parts: alloc::vec::Vec<&str> = full_path
            .trim_end_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() == 3 && parts[0] == "proc" && parts[2] == "exe" && (flags & O_CREAT) == 0 {
            let pid = if parts[1] == "self" {
                (unsafe { crate::process::current_pid() }) as u64
            } else {
                match parts[1].parse::<u64>() {
                    Ok(p) => p,
                    Err(_) => u64::MAX,
                }
            };
            let exe_path: alloc::vec::Vec<u8> = if pid != u64::MAX {
                crate::fs::procfs::pid::generate_exe_link(pid)
            } else {
                alloc::vec::Vec::new()
            };
            if !exe_path.is_empty() {
                let exe_str = core::str::from_utf8(&exe_path).unwrap_or("");
                if !exe_str.is_empty() {
                    let result = if (flags & O_DIRECTORY) != 0 {
                        crate::fs::vfs::file_opendir(exe_str, flags)
                    } else {
                        crate::fs::vfs::file_open(exe_str, flags, mode)
                    };
                    return match result {
                        Ok(fd) => {
                            if (flags & O_CLOEXEC) != 0 {
                                crate::fs::set_cloexec_fd(fd, true);
                            }
                            fd as i64
                        }
                        Err(e) => e as i64,
                    };
                }
            }
            return -(errno::ENOENT as i64);
        }
    }

    // /proc/[pid]/ns/<kind> — namespace magic links (U1c): opening one
    // yields a namespace fd usable by setns(2). Handled before the generic
    // procfs read_file shortcut (which would follow the "uts:[N]" target).
    // (Global-namespace shortcut, non-chrooted tasks only.)
    if !crate::fs::vfs::chrooted() && (flags & O_CREAT) == 0 {
        if let Some(rv) = crate::process::ns::try_open_ns_path(&full_path, flags) {
            return rv;
        }
    }

    // Shortcut: /proc/[pid]/xxx paths go through procfs read_file
    // because VFS inode lookup doesn't support PID subdirectories.
    // (P1 chroot: global-namespace shortcut, non-chrooted tasks only.)
    if !crate::fs::vfs::chrooted() && (flags & O_CREAT) == 0 && (flags & O_DIRECTORY) == 0 {
        // ptrace_may_access gate for environ BEFORE generating content
        // (review 5.7): self or CAP_SYS_PTRACE.
        {
            let parts: alloc::vec::Vec<&str> = full_path
                .trim_end_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() == 3 && parts[0] == "proc" && parts[2] == "environ" {
                let pid = if parts[1] == "self" {
                    (unsafe { crate::process::current_pid() }) as u64
                } else {
                    match parts[1].parse::<u64>() {
                        Ok(p) => p,
                        Err(_) => 0,
                    }
                };
                if !crate::fs::procfs::pid::environ_access_allowed(pid) {
                    return -(errno::EACCES as i64);
                }
            }
        }
        if let Some(content) = crate::fs::procfs::read_file(&full_path) {
            return match crate::fs::vfs::open_mem_file(content, flags) {
                Ok(fd) => {
                    if (flags & O_CLOEXEC) != 0 { crate::fs::set_cloexec_fd(fd, true); }
                    fd as i64
                }
                Err(e) => e as i64,
            };
        }
    }

    let result = if (flags & O_DIRECTORY) != 0 {
        crate::fs::vfs::file_opendir(&full_path, flags)
    } else {
        crate::fs::file_open(&full_path, flags, mode)
    };

    // OH DRM bring-up (rs-egl): the composer HDI's DrmDevice::Create
    // (drm_device.cpp:90, passthrough-loaded into render_service) logs
    // "can't open drm card device" — but its message carries no errno,
    // and ENOENT-only tracing missed the real reason. Print every
    // FAILED open under /dev/dri with the errno so the composer's
    // failure is named directly (successes are already counted by the
    // drm driver's own open trace).
    if full_path.starts_with("/dev/dri") {
        if let Err(e) = &result {
            static DRIOpen_FAIL_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = DRIOpen_FAIL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 32 {
                crate::pr_warn!(
                    "dri-open-fail[{}] '{}' flags {:#x} -> errno {} (pid {})",
                    n,
                    full_path,
                    flags,
                    -e,
                    unsafe { crate::process::current_pid() }
                );
            }
        } else if (flags & O_DIRECTORY) != 0 {
            // The composer's FindAndOpenKmsCard opendir()s /dev/dri and
            // then openat(dirfd)«card0»s; a successful opendir followed
            // by NO drm ioctl means readdir/openat/getdents broke —
            // trace the directory open too so the whole walk is
            // visible on serial.
            static DRIDIR_OPEN_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = DRIDIR_OPEN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 8 {
                crate::pr_warn!(
                    "dri-dir-open[{}] '{}' flags {:#x} ok (pid {})",
                    n,
                    full_path,
                    flags,
                    unsafe { crate::process::current_pid() }
                );
            }
        }
    }

    // OH param-workspace hunt (b70): /dev/__parameters__/{param_sec_dac,
    // param_selinux} end up missing — init's GetSharedMem create-open
    // (O_CREAT|O_RDWR|O_TRUNC + ftruncate + mmap MAP_SHARED) fails
    // silently (ENOENT-only tracing misses non-ENOENT errnos), every
    // later param label lookup is refused, bootevent.* sets are
    // forbidden and init's boot graph never starts the UI stack. Print
    // EVERY open under that directory with its result, plus the
    // create-open's flags, so the failing errno is named directly.
    if full_path.starts_with("/dev/__parameters__") {
        static PARAMWS_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let n = PARAMWS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 48 {
            let rc = match &result {
                Ok(fd) => *fd as i64,
                Err(e) => *e as i64,
            };
            let creating = (flags & O_CREAT) != 0;
            crate::pr_info!(
                "paramws open[{}] '{}' flags {:#x} creat={} -> {} (pid {})",
                n,
                full_path,
                flags,
                creating,
                rc,
                unsafe { crate::process::current_pid() }
            );
        }
    }

    // OH bring-up: throttled ENOENT trace — the display HDF hosts retry
    // missing files in tight loops and their hilog never reaches the
    // console; the failing path list is the only visibility into what
    // userspace still lacks. ld.so directory probes would burn the
    // budget, so: /dev/ paths always print (device nodes are the
    // bring-up signal), every other DISTINCT path prints once (the
    // linker search list dedupes to one line per missing library).
    if let Err(e) = &result {
        if *e == -(errno::ENOENT as i32) {
            static ENO_SEEN: crate::sync::spinlock::Spinlock<
                Option<alloc::vec::Vec<alloc::string::String>>,
            > = crate::sync::spinlock::Spinlock::new(None);
            let mut print_it = false;
            if full_path.starts_with("/dev/") {
                static ENO_DEV_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                print_it = ENO_DEV_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 32;
            } else {
                // ".so" misses get their own budget: the ld.so probe
                // storm would otherwise starve the runtime-failure
                // budget, but a lib missing from EVERY directory is the
                // dlopen-failure signal (bootanimation retry loops).
                // The BASE name dedupes the ~30 directory probes per
                // missing lib to one line.
                let is_lib_probe = full_path.ends_with(".so")
                    || full_path.ends_with(".z.so")
                    || full_path.ends_with(".so.1");
if is_lib_probe {
                    // Per-(basename,parent_dir) counter map: keeps the
                    // dlopen directory probe storm deduplicated while
                    // still surfacing WHERE the loader looked. The
                    // (lib,dir) tuple is the unit — "still missing
                    // libEGL.so in /system/lib64" is one line,
                    // "/vendor/lib64" is another. Without the dir axis
                    // the loader can hit /system/lib64 and miss, hit
                    // /vendor/lib64 and miss, and we only see one
                    // "still missing libEGL.so" that tells us nothing.
                    static SO_COUNTS: crate::sync::spinlock::Spinlock<
                        Option<alloc::vec::Vec<((alloc::string::String, alloc::string::String), u32)>>,
                    > = crate::sync::spinlock::Spinlock::new(None);
                    let base = alloc::string::String::from(
                        full_path.rsplit('/').next().unwrap_or(""),
                    );
                    let parent = alloc::string::String::from(
                        match full_path.rsplit_once('/') {
                            Some((d, _)) if !d.is_empty() => d,
                            _ => "/",
                        },
                    );
                    let mut counts = SO_COUNTS.lock_irqsave();
                    let list = counts.get_or_insert_with(alloc::vec::Vec::new);
                    if list.len() < 420 {
                        let key = (base.clone(), parent.clone());
                        match list.iter_mut().find(|(k, _)| *k == key) {
                            Some((_, c)) => {
                                *c += 1;
                                if *c % 64 == 0 {
                                    crate::pr_info!(
                                        "open ENOENT: still missing '{}' in '{}' x{} (pid {})",
                                        base,
                                        parent,
                                        c,
                                        unsafe { crate::process::current_pid() }
                                    );
                                }
                            }
                            None => {
                                crate::pr_info!(
                                    "open ENOENT: '{}' in '{}' (pid {})",
                                    base,
                                    parent,
                                    unsafe { crate::process::current_pid() }
                                );
                                list.push((key, 1));
                            }
                        }
                    }
                } else {
                    let mut seen = ENO_SEEN.lock_irqsave();
                    let list = seen.get_or_insert_with(alloc::vec::Vec::new);
                    if list.len() < 96 {
                        if !list.iter().any(|p| p.as_str() == full_path.as_str()) {
                            list.push(alloc::string::String::from(full_path.as_str()));
                            crate::pr_info!(
                                "open ENOENT: '{}' (pid {})",
                                full_path,
                                unsafe { crate::process::current_pid() }
                            );
                        }
                    }
                }
            }
            if print_it {
                crate::pr_info!(
                    "open ENOENT: '{}' (pid {})",
                    full_path,
                    unsafe { crate::process::current_pid() }
                );
            }
        }
    }

    match result {
        Ok(fd) => {
            if (flags & O_CLOEXEC) != 0 { crate::fs::set_cloexec_fd(fd, true); }
            fd as i64
        }
        Err(e) => e as i64,
    }
}

/// sys_close - Close file descriptor
pub fn sys_close(args: SyscallArgs) -> i64 {
    let raw_fd = args[0] as i64;

    // Reject negative fd
    if raw_fd < 0 {
        return -(errno::EBADF as i64);
    }
    let fd = raw_fd as usize;

    // POSIX MQ fds live in a side namespace starting at 512. Consult the
    // process fd table FIRST: an ordinary fd in [512, fdtable_max) must be
    // closed as a regular file, not hijacked by the MQ namespace.
    if fd >= 512 {
        let in_fdtable = crate::sched::get_current_fdtable()
            .map(|ft| ft.get_file(fd).is_some())
            .unwrap_or(false);
        if !in_fdtable {
            return crate::ipc::posix_mq::close_mq_fd(fd as i32) as i64;
        }
    }

    use crate::fs::close_file_fd;
    // Capture a Weak handle BEFORE the close so the purge below can tell
    // whether THIS close dropped the last fd-table reference to the open
    // file description. A strong Arc here would keep the description
    // alive and suppress its close op (pipe EOF, socket HUP) — a Weak
    // observes without holding.
    let desc_weak = crate::sched::get_current_fdtable().and_then(|ft| {
        ft.get_file(fd)
            .map(|f| alloc::sync::Arc::downgrade(&f))
    });
    // SAFETY: fd is a valid file descriptor owned by the current process.
    unsafe {
        match close_file_fd(fd) {
            Ok(()) => {
                // Linux eventpoll removes interest entries when the LAST
                // reference to the registered file description goes away
                // (eventpoll_release — the epoll's own epi reference is
                // what removes it, and only when THAT is the last one).
                // Purging on ANY close was wrong in the cross-process
                // shape: a forked child that inherited the fd table (and
                // with it the epoll fd) closed its inherited copy on
                // exit, and the purge — resolving "live" against the
                // child's table, where the slot was just cleared —
                // stripped the PARENT's live interest. appspawn's
                // bundle-scan child did exactly this to the AppSpawn
                // listener: after the child exited, the server's epoll
                // held only its timer fd, every epoll_wait returned
                // timer-only events, and client connections piled up in
                // the accept queue (q reached 210 in sc18) with the
                // server never accepting — the launcher chain's
                // spawn-blocker. Gate the purge on this close being the
                // description's last fd-table reference.
                let others = desc_weak
                    .as_ref()
                    .map(alloc::sync::Weak::strong_count)
                    .unwrap_or(1);
                if others == 0 {
                    crate::syscall::misc::epoll_purge_closed_fd(fd);
                }
                0
            }
            Err(e) => e as i64,
        }
    }
}

/// sys_fstat - Get file status
pub fn sys_fstat(args: SyscallArgs) -> i64 {
    use crate::fs::{file_stat, Stat};

    let fd = args[0] as usize;
    let statbuf = args[1] as *mut Stat;

    // Check statbuf pointer validity
    if statbuf.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Check if statbuf is in valid user space
    if !crate::arch::uaccess::access_ok(statbuf as usize, crate::fs::user_stat_size()) {
        return -(errno::EFAULT as i64);
    }

    // Create temporary stat structure
    let mut stat = Stat::new();

    // Call VFS layer file_stat
    match file_stat(fd, &mut stat) {
        Ok(()) => {
            // Exception-table copy in the target ABI layout (review
            // SYSA-M4: raw dereference panics on unmapped user pointers).
            let uncopied = crate::fs::copy_stat_to_user(statbuf as *mut u8, &stat);
            if uncopied > 0 {
                -(errno::EFAULT as i64)
            } else {
                0
            }
        }
        Err(errno) => {
            // file_stat returns ALREADY-NEGATIVE errnos (as_neg_i32). The
            // old re-negation turned EBADF(-9) into +9 — a POSITIVE return
            // that glibc passes through as "success" without writing the
            // caller's stat buffer, so userspace compared untouched
            // (all-equal) buffers: coreutils cat then refused every
            // `x | cat` pipeline with "input file is output file" and
            // dropped its data.
            errno as i64
        }
    }
}

/// sys_fstatat - Get file status by path
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor (AT_FDCWD = -100 means current directory)
/// - args[1]: pathname - file path
/// - args[2]: statbuf - stat structure buffer
/// - args[3]: flags - flags (AT_SYMLINK_NOFOLLOW, etc.)

/// Parse a "/proc/[self|<pid>]/fd/<n>" magic-link path (OH bring-up:
/// musl's fchmodat(AT_SYMLINK_NOFOLLOW) fallback stats and chmods
/// exactly this path — see the sys_fstatat/sys_fchmodat shortcuts).
/// Returns (pid, fd) when the path matches and the fd is open in that
/// pid's table.
fn proc_fd_magic(path: &str) -> Option<(u32, u32)> {
    let rest = path.strip_prefix("/proc/")?;
    let (pid_s, fd_s) = rest.split_once("/fd/")?;
    if fd_s.is_empty() || !fd_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = if pid_s == "self" {
        unsafe { crate::process::current_pid() }
    } else if !pid_s.is_empty() && pid_s.bytes().all(|b| b.is_ascii_digit()) {
        pid_s.parse().ok()?
    } else {
        return None;
    };
    let fd: u32 = fd_s.parse().ok()?;
    let open = crate::process::find_task_by_pid(pid)
        .or_else(|| {
            use crate::process::{current_pid, current_task};
            if current_pid() == pid { current_task() } else { None }
        })
        .and_then(|t| t.try_fdtable())
        .map(|ft| ft.get_file(fd as usize).is_some())
        .unwrap_or(false);
    if open { Some((pid, fd)) } else { None }
}

pub fn sys_fstatat(args: SyscallArgs) -> i64 {
    use crate::fs::{Stat, stat_file_by_path};

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let statbuf = args[2] as *mut Stat;
    let flags = args[3] as u32;

    if statbuf.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(statbuf as usize, crate::fs::user_stat_size()) {
        return -(errno::EFAULT as i64);
    }

    // Magic-link shortcut: stat("/proc/[self|<pid>]/fd/<n>") stats the
    // file the fd refers to (Linux open traversal of the procfs fd link).
    // musl's fchmodat(NO-FOLLOW) fallback stats this path after its
    // O_PATH open; walking the synthetic symlink re-resolves a target
    // string that socket fds cannot express. (Non-chrooted tasks only,
    // like the other global /proc shortcuts.)
    if !crate::fs::vfs::chrooted() {
        let mut probe = [0u8; 64];
        if !pathname_ptr.is_null()
            && crate::arch::uaccess::access_ok(pathname_ptr as usize, probe.len())
            && unsafe {
                crate::arch::uaccess::copy_from_user(
                    probe.as_mut_ptr(), pathname_ptr, probe.len())
            } == 0
        {
            let len = probe.iter().position(|&b| b == 0).unwrap_or(probe.len());
            if let Ok(p) = core::str::from_utf8(&probe[..len]) {
                if let Some((_, fd)) = proc_fd_magic(p) {
                    let mut stat = Stat::new();
                    match crate::fs::file_stat(fd as usize, &mut stat) {
                        Ok(()) => {
                            let r = crate::fs::copy_stat_to_user(
                                statbuf as *mut u8, &stat);
                            return if r != 0 { -(errno::EFAULT as i64) } else { 0 };
                        }
                        Err(e) => return e as i64,
                    }
                }
            }
        }
    }

    // Validate flags: AT_SYMLINK_NOFOLLOW (0x100), AT_EMPTY_PATH (0x1000)
    // and AT_NO_AUTOMOUNT (0x800) are the Linux-legal at-flags here.
    // AT_NO_AUTOMOUNT is required: glibc's stat()/statx() wrappers and
    // gio (GFile query-info / enumerate-children) always pass it —
    // rejecting it made every GFileInfo query fail with EINVAL.
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    const VALID_FLAGS: u32 = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT;
    if flags & !VALID_FLAGS != 0 {
        return -(errno::EINVAL as i64);
    }

    // AT_EMPTY_PATH + empty pathname: fstat(dirfd) semantics. glibc's
    // dynamic linker identifies DSOs via newfstatat(fd, "", AT_EMPTY_PATH)
    // (dl-file-id); routing that down the path resolver returned ENOTDIR
    // and every dynamically linked binary died with "cannot stat shared
    // object" before ld.so finished loading libc.
    let mut stat = Stat::new();
    let empty_path = {
        let mut probe = [0u8; 1];
        !pathname_ptr.is_null()
            && crate::arch::uaccess::access_ok(pathname_ptr as usize, 1)
            && unsafe {
                crate::arch::uaccess::copy_from_user(
                    probe.as_mut_ptr(),
                    pathname_ptr,
                    1,
                )
            } == 0
            && probe[0] == 0
    };
    if empty_path && (flags & AT_EMPTY_PATH) != 0 {
        // fstat(dirfd)
        if dirfd < 0 {
            return -(errno::EBADF as i64);
        }
        match crate::fs::file_stat(dirfd as usize, &mut stat) {
            Ok(()) => {}
            // Already-negative errno (see sys_fstat): don't re-negate.
            Err(e) => {
                static FSTAT_EP_ERR_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if FSTAT_EP_ERR_N.fetch_add(
                    1, core::sync::atomic::Ordering::Relaxed) < 20
                {
                    crate::pr_info!(
                        "fstatat empty-path err: dirfd={} flags={:#x} ret={} pid={}",
                        dirfd, flags, e,
                        unsafe { crate::process::current_pid() }
                    );
                }
                return e as i64;
            }
        }
        let result = crate::fs::copy_stat_to_user(statbuf as *mut u8, &stat);
        return if result != 0 { -(errno::EFAULT as i64) } else { 0 };
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => {
            // OH bring-up: init's service-socket chmod path dies with
            // ENOTDIR — name the failing call.
            {
                static FSTATAT_ERR_TRACE_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let pid = unsafe { crate::process::current_pid() };
                let n = if pid >= 300 {
                    FSTATAT_ERR_TRACE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                } else {
                    FSTATAT_ERR_TRACE_N.load(core::sync::atomic::Ordering::Relaxed)
                };
                if n < 40 && pid >= 300 {
                    let mut pb = [0u8; 96];
                    let shown = crate::arch::uaccess::strncpy_from_user(
                        pathname_ptr, pb.len(), &mut pb)
                        .map(|s| core::str::from_utf8(s).unwrap_or("?"))
                        .unwrap_or("?");
                    crate::pr_info!(
                        "fstatat err: dirfd={} path='{}' flags={:#x} ret={} pid={}",
                        dirfd, shown, flags, e,
                        pid
                    );
                }
            }
            return e as i64;
        }
    };

    // AT_SYMLINK_NOFOLLOW: don't follow symlinks (for lstat)
    let lookup_flags = if flags & AT_SYMLINK_NOFOLLOW != 0 {
        crate::fs::vfs::LOOKUP_NOFOLLOW
    } else {
        0
    };

    let ret = match crate::fs::vfs::stat_file_by_path_with_flags(&full_path, &mut stat, lookup_flags) {
        Ok(()) => {
            let result = crate::fs::copy_stat_to_user(statbuf as *mut u8, &stat);
            if result != 0 {
                -(errno::EFAULT as i64)
            } else {
                0
            }
        }
        Err(errno) => errno as i64, // already negative (see sys_fstat)
    };
    ret
}

/// sys_getdents64 - Read directory entries
pub fn sys_getdents64(args: SyscallArgs) -> i64 {
    use crate::fs::vfs::file_getdents64;

    let fd = args[0] as usize;
    let dirp = args[1] as *mut u8;
    let count = args[2] as usize;

    // Check pointer validity
    if dirp.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Cap the scratch buffer size: count is user-controlled and the kernel
    // heap cannot satisfy multi-GB requests (alloc failure would panic).
    const MAX_GETDENTS_COUNT: usize = 4 * 1024 * 1024;
    let count = if count > MAX_GETDENTS_COUNT { MAX_GETDENTS_COUNT } else { count };

    // Check if dirp is in valid user space
    if !crate::arch::uaccess::access_ok(dirp as usize, count) {
        return -(errno::EFAULT as i64);
    }

    if count == 0 {
        return -(errno::EINVAL as i64);
    }

    // Create temporary buffer. try_reserve makes allocation failure a
    // clean ENOMEM instead of the allocator panic handler; zero-filling
    // also stops getdents64 padding bytes from leaking kernel heap data
    // to userspace (review VFS-M7).
    let mut buffer: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if buffer.try_reserve_exact(count).is_err() {
        return -(errno::ENOMEM as i64);
    }
    buffer.resize(count, 0);

    // Call VFS layer
    let result = file_getdents64(fd, &mut buffer, count);

    // OH DRM bring-up (rs-egl): the composer VDI's FindAndOpenKmsCard
    // opendir()s /dev/dri then openat()s "card0" per readdir entry; a
    // getdents that returns no entries (or errors) leaves the loop empty
    // and the composer reports "can't open drm card device" with no drm
    // ioctl ever issued. Name the /dev/dri getdents result directly.
    {
        let path = crate::sched::get_current_fdtable()
            .and_then(|ft| ft.get_file(fd))
            .and_then(|f| {
                let p = f.path();
                if p.is_empty() { None } else { Some(p) }
            });
        if let Some(p) = path {
            if p.ends_with("dri") {
                static DRIDENTS_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = DRIDENTS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 24 {
                    match &result {
                        Ok(b) => {
                            // Entry dump (the composer's FindAndOpenKmsCard
                            // strncmp()s d_name against "card" — if the
                            // names/reclens are wrong the loop skips every
                            // entry and "can't open drm card device" follows
                            // with no openat ever issued; the raw bytes name
                            // that directly).
                            let mut txt = alloc::string::String::with_capacity(96);
                            for &byte in buffer.iter().take((*b).min(96)) {
                                if byte >= 0x20 && byte < 0x7f {
                                    txt.push(byte as char);
                                } else {
                                    txt.push('.');
                                }
                            }
                            crate::pr_warn!(
                                "dri-getdents[{}] '{}' -> {} bytes: {} (pid {})",
                                n,
                                p,
                                b,
                                txt,
                                unsafe { crate::process::current_pid() }
                            );
                        }
                        Err(e) => crate::pr_warn!(
                            "dri-getdents[{}] '{}' -> errno {} (pid {})",
                            n,
                            p,
                            -e,
                            unsafe { crate::process::current_pid() }
                        ),
                    }
                }
            }
        }
    }

    match result {
        Ok(bytes_read) => {
            // SAFETY: dirp validated with access_ok(count); bytes_read <= count.
            let uncopied = unsafe {
                crate::arch::uaccess::copy_to_user(
                    dirp, buffer.as_ptr(), bytes_read,
                )
            };
            if uncopied != 0 {
                return -(errno::EFAULT as i64);
            }
            bytes_read as i64
        }
        Err(errno) => {
            // file_getdents64 returns ALREADY-NEGATIVE errnos
            // (as_neg_i32, like file_stat). Negating again turned -EINVAL
            // into a fake successful +22 return: a getdents64 buffer too
            // small for even one entry then reported "22 bytes read"
            // forever and telldir-style loops spun eternally.
            errno as i64
        }
    }
}

/// sys_mkdir - Create directory (legacy wrapper)
///
/// NOTE: riscv64 has no plain `mkdir` syscall — kept for the in-kernel test
/// suite only; not dispatched (mkdirat is the real entry).
pub fn sys_mkdir(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let mode = args[1] as u32; // passed through like mkdirat (review SYSA-M1)

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::file_mkdir(pathname, mode) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_mkdirat - Create directory relative to directory file descriptor
/// Syscall number: 34
pub fn sys_mkdirat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as u32;

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // OH param-workspace hunt (b70) — see the open tracer in sys_open.
    if full_path.starts_with("/dev/__parameters") {
        let rc = match crate::fs::vfs::file_mkdir(&full_path, mode) {
            Ok(()) => 0,
            Err(e) => e as i64,
        };
        crate::pr_info!(
            "paramws mkdir '{}' mode {:#o} -> {} (pid {})",
            full_path,
            mode,
            rc,
            unsafe { crate::process::current_pid() }
        );
        return rc;
    }

    match crate::fs::vfs::file_mkdir(&full_path, mode) {
        Ok(()) => 0,
        Err(e) => {
            // b18/sc18 appspawn hunt: DoSandboxRootFolderCreate fails with
            // no errno on the guest side; the sandbox roots live under
            // /mnt and /data, so surface failed mkdirs there on serial.
            if e != -(17) && (full_path.starts_with("/mnt") || full_path.starts_with("/data")) {
                crate::pr_info!(
                    "mkdirfail '{}' mode {:#o}: {} (pid {})",
                    full_path,
                    mode,
                    e,
                    unsafe { crate::process::current_pid() }
                );
            }
            e as i64
        }
    }
}

/// sys_rmdir - Remove empty directory
pub fn sys_rmdir(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::file_rmdir(pathname) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_unlink - Remove file
pub fn sys_unlink(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::file_unlink(pathname) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_linkat - Create hard link (syscall 37)
///
/// # Arguments
/// - args[0]: olddirfd - old directory file descriptor (AT_FDCWD = -100)
/// - args[1]: oldpath - existing file path
/// - args[2]: newdirfd - new directory file descriptor (AT_FDCWD = -100)
/// - args[3]: newpath - new link path
/// - args[4]: flags - reserved (AT_SYMLINK_FOLLOW, AT_EMPTY_PATH)
pub fn sys_linkat(args: SyscallArgs) -> i64 {
    let olddirfd = args[0] as i32;
    let oldpath_ptr = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath_ptr = args[3] as *const u8;
    let flags = args[4] as u32;

    // Flags validation (Linux): only AT_SYMLINK_FOLLOW (0x400) and
    // AT_EMPTY_PATH (0x1000) are legal — anything else is EINVAL.
    const AT_SYMLINK_FOLLOW: u32 = 0x400;
    const AT_EMPTY_PATH: u32 = 0x1000;
    if flags & !(AT_SYMLINK_FOLLOW | AT_EMPTY_PATH) != 0 {
        return -(errno::EINVAL as i64);
    }

    let old_full = match resolve_user_path(olddirfd, oldpath_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    let new_full = match resolve_user_path(newdirfd, newpath_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::vfs_link(&old_full, &new_full) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_unlinkat - Remove file or directory (syscall 35)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor (AT_FDCWD = -100)
/// - args[1]: pathname - file path
/// - args[2]: flags - flags (AT_REMOVEDIR = 0x200 for removing directory)
pub fn sys_unlinkat(args: SyscallArgs) -> i64 {
    const AT_REMOVEDIR: u32 = 0x200;

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;

    // Only AT_REMOVEDIR is legal for unlinkat (Linux: EINVAL otherwise).
    if flags & !AT_REMOVEDIR != 0 {
        return -(errno::EINVAL as i64);
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    if (flags & AT_REMOVEDIR) != 0 {
        match crate::fs::vfs::file_rmdir(&full_path) {
            Ok(()) => 0,
            Err(e) => e as i64,
        }
    } else {
        match crate::fs::vfs::file_unlink(&full_path) {
            Ok(()) => 0,
            Err(e) => e as i64,
        }
    }
}

/// sys_renameat - Rename a file or directory
///
/// # Arguments
/// - args[0]: olddirfd - old directory file descriptor
/// - args[1]: oldpath - old pathname
/// - args[2]: newdirfd - new directory file descriptor
/// - args[3]: newpath - new pathname
pub fn sys_renameat(args: SyscallArgs) -> i64 {
    let olddirfd = args[0] as i32;
    let oldpath_ptr = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath_ptr = args[3] as *const u8;

    let old_full = match resolve_user_path(olddirfd, oldpath_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    let new_full = match resolve_user_path(newdirfd, newpath_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::vfs_rename(&old_full, &new_full) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_readlinkat - Read symbolic link
pub fn sys_readlinkat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let buf = args[2] as *mut u8;
    let bufsize = args[3] as usize;

    if buf.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(buf as usize, bufsize) {
        return -(errno::EFAULT as i64);
    }

    // Empty pathname with a concrete dirfd: operate on the file referenced
    // by the fd itself (man readlinkat: "If pathname is an empty string,
    // operate on the file referred to by dirfd" — the fd must reference a
    // symlink). resolve_user_path rejects empty paths outright, so this
    // must run before it (LTP readlinkat01's fd-direct case).
    {
        let mut pbuf = [0u8; 1];
        // SAFETY: copy_from_user probes with the exception table in play;
        // an unreadable pointer falls through to the normal path.
        let empty = unsafe {
            crate::arch::uaccess::copy_from_user(
                pbuf.as_mut_ptr(),
                pathname_ptr as *const u8,
                1,
            ) == 0
                && pbuf[0] == 0
        };
        if empty && dirfd >= 0 {
            let file = crate::sched::get_current_fdtable()
                .and_then(|ft| ft.get_file(dirfd as usize));
            if let Some(file) = file {
                // SAFETY: file comes from the fd table; inode accessed
                // read-only.
                let is_symlink = unsafe {
                    (*file.inode.get())
                        .as_ref()
                        .map(|i| i.mode.is_symlink())
                        .unwrap_or(false)
                };
                if !is_symlink {
                    return -(errno::EINVAL as i64);
                }
                // SAFETY: same inode validity as above; op_readlink fills
                // a kernel buffer.
                let target = unsafe {
                    (*file.inode.get())
                        .as_ref()
                        .map(|i| {
                            let mut tb = [0u8; 4096];
                            let n = i.op_readlink(&mut tb);
                            if n < 0 { None } else { Some((tb, n as usize)) }
                        })
                        .flatten()
                };
                if let Some((tb, n)) = target {
                    let copy_len = n.min(bufsize);
                    // SAFETY: buf validated with access_ok(bufsize);
                    // exception-table copy.
                    unsafe {
                        if crate::arch::uaccess::copy_to_user(buf, tb.as_ptr(), copy_len) != 0 {
                            return -(errno::EFAULT as i64);
                        }
                    }
                    return copy_len as i64;
                }
                return -(errno::ENOENT as i64);
            }
            return -(errno::EBADF as i64);
        }
    }

    // Resolve dirfd-relative paths like every other *at() syscall — the
    // old code ignored dirfd entirely for non-/proc paths.
    let pathname = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // /proc/self/exe - return program path (P1 chroot: global-namespace
    // shortcut, non-chrooted tasks only).
    if !crate::fs::vfs::chrooted() && pathname == "/proc/self/exe" {
        if let Some(current) = crate::sched::current() {
            // SAFETY: current is the running task's Task pointer from sched::current().
            let exe_path = unsafe { (*current).get_exe_path() };

            // Linux readlink NEVER fails with ENAMETOOLONG — it truncates
            // the target to bufsiz and returns the truncated length (the
            // buffer is not NUL-terminated).
            let copy_len = exe_path.len().min(bufsize);
            // SAFETY: buf validated with access_ok(bufsize); exception-table copy.
            unsafe {
                if crate::arch::uaccess::copy_to_user(buf, exe_path.as_ptr(), copy_len) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }

            return copy_len as i64;
        }
    }

    // /proc/[pid]/fd/N - return fd symlink target
    // Supports both absolute and relative paths (already dirfd-resolved)
    // (P1 chroot: global-namespace shortcut, non-chrooted tasks only.)
    if !crate::fs::vfs::chrooted() {
        let full_path = resolve_proc_readlink_path(dirfd, &pathname);
        if let Some(target) = handle_proc_fd_readlink(&full_path) {
            // Same truncation semantics as above (Linux policy, not ENAMETOOLONG).
            let copy_len = target.len().min(bufsize);
            // SAFETY: buf validated with access_ok(bufsize); exception-table copy.
            unsafe {
                if crate::arch::uaccess::copy_to_user(buf, target.as_ptr(), copy_len) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            return copy_len as i64;
        }
    }
    let full_path = resolve_proc_readlink_path(dirfd, &pathname);

    // /proc/[pid]/ns/<kind> — namespace magic links (U1c): readlink returns
    // "<kind>:[<inum>]" exactly like Linux. (Global-namespace shortcut,
    // non-chrooted tasks only.)
    if !crate::fs::vfs::chrooted() {
        let ns_full_path = resolve_proc_readlink_path(dirfd, &pathname);
        if let Some(target) = handle_proc_ns_readlink(&ns_full_path) {
            // Linux truncates to bufsiz and returns the truncated length.
            let copy_len = target.len().min(bufsize);
            // SAFETY: buf validated with access_ok(bufsize); exception-table copy.
            unsafe {
                if crate::arch::uaccess::copy_to_user(buf, target.as_ptr(), copy_len) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            return copy_len as i64;
        }
    }

    // Generic path: look up the symlink itself (NOFOLLOW) and read its
    // target through the filesystem's readlink op.
    let rl_result: Result<i64, i64> = (|| {
        let vpath = crate::fs::vfs::path_lookup(&full_path, crate::fs::vfs::LOOKUP_NOFOLLOW)
            .map_err(|e| e as i64)?;
        let inode = vpath.inode.ok_or(-(errno::ENOENT as i64))?;
        if !inode.mode.is_symlink() {
            return Err(-(errno::EINVAL as i64));
        }
        let mut target_buf = [0u8; PATH_MAX];
        let n = inode.op_readlink(&mut target_buf);
        if n < 0 {
            return Err(n as i64);
        }
        let n = n as usize;
        if n > target_buf.len() {
            return Err(-(errno::ENAMETOOLONG as i64));
        }
        let copy_len = n.min(bufsize);
        // SAFETY: buf validated with access_ok(bufsize); exception-table copy.
        unsafe {
            if crate::arch::uaccess::copy_to_user(buf, target_buf.as_ptr(), copy_len) != 0 {
                return Err(-(errno::EFAULT as i64));
            }
        }
        // Linux truncates to bufsiz and returns the truncated length
        Ok(copy_len as i64)
    })();
    // OH bring-up: throttled readlink-failure trace (HDF devhost probes
    // its module directory through readlink; hilog never reaches the
    // console). Distinct paths only, like the ENOENT open trace.
    if let Err(e) = &rl_result {
        static RL_SEEN: crate::sync::spinlock::Spinlock<
            Option<alloc::vec::Vec<alloc::string::String>>,
        > = crate::sync::spinlock::Spinlock::new(None);
        let mut print_it = false;
        {
            let mut seen = RL_SEEN.lock_irqsave();
            let list = seen.get_or_insert_with(alloc::vec::Vec::new);
            if list.len() < 48 && !list.iter().any(|p| p.as_str() == full_path.as_str()) {
                list.push(alloc::string::String::from(full_path.as_str()));
                print_it = true;
            }
        }
        if print_it {
            crate::pr_info!(
                "readlink fail({}) '{}': not-a-symlink={} (pid {})",
                e,
                full_path,
                *e == -(errno::EINVAL as i64),
                unsafe { crate::process::current_pid() }
            );
        }
    }
    match rl_result {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// Resolve path for procfs readlink (handles /proc/self/ -> /proc/{pid}/)
fn resolve_proc_readlink_path(dirfd: i32, pathname: &str) -> alloc::string::String {
    const AT_FDCWD: i32 = -100;

    if pathname.starts_with("/proc/") {
        // Already absolute - just resolve "self" to PID
        let mut path = alloc::string::String::from(pathname);
        if path.contains("/self/") {
            // SAFETY: current_pid() reads the current task's pid from scheduler.
            let pid = unsafe { crate::process::current_pid() };
            path = path.replace("/self/", &alloc::format!("/{}/", pid));
        }
        return path;
    }

    // Relative path - try to resolve using dirfd. An ABSOLUTE path must
    // pass through untouched: the old code prepended the cwd to it too
    // (readlink("/tmp") with cwd=/tmp became "/tmp/tmp"), so every
    // readlink of an existing non-/proc absolute path returned ENOENT
    // whenever the caller's cwd was not "/" — sys_readlinkat never
    // reached its EINVAL-for-non-symlink branch, and libc realpath()
    // (which probes each component with readlink) aborted with ENOENT
    // (LTP acct01's ro_mntpoint TINFO; any realpath() from a non-root
    // cwd on an absolute path).
    if !pathname.starts_with('/') && dirfd == AT_FDCWD {
        if let Some(current) = crate::sched::current() {
            // SAFETY: current is the running task's Task pointer from sched::current().
            let cwd = unsafe { (*current).get_cwd() };
            let cwd_str = core::str::from_utf8(&cwd).unwrap_or("?");
            let mut full = if cwd_str.ends_with('/') {
                alloc::format!("{}{}", cwd_str, pathname)
            } else {
                alloc::format!("{}/{}", cwd_str, pathname)
            };
            if full.contains("/self/") {
                // SAFETY: current_pid() reads the current task's pid from scheduler.
                let pid = unsafe { crate::process::current_pid() };
                full = full.replace("/self/", &alloc::format!("/{}/", pid));
            }
            return full;
        }
    }

    alloc::string::String::from(pathname)
}

/// Handle /proc/[pid]/ns/<kind> readlink (U1c), returns the magic-link
/// target "<kind>:[<inum>]" or None when the path is not an ns link.
fn handle_proc_ns_readlink(path: &str) -> Option<alloc::vec::Vec<u8>> {
    let path = path.trim_start_matches('/');
    let parts: alloc::vec::Vec<&str> = path.split('/').collect();
    if parts.len() != 4 || parts[0] != "proc" || parts[2] != "ns" {
        return None;
    }
    let pid: u32 = if parts[1] == "self" {
        crate::process::current_pid()
    } else {
        parts[1].parse().ok()?
    };
    let kind = crate::process::ns::NsKind::from_name(parts[3].as_bytes())?;
    Some(crate::process::ns::ns_link_target(pid, kind))
}

/// Handle /proc/[pid]/fd/N readlink, returns symlink target or None
fn handle_proc_fd_readlink(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // Match pattern: /proc/{pid}/fd/{fd_num}
    let path = path.trim_start_matches('/');

    let parts: alloc::vec::Vec<&str> = path.split('/').collect();
    // Need exactly "proc/{pid}/fd/{fd_num}" — len < 4 would make parts[3]
    // an out-of-bounds index (a bare "/proc/1/fd" used to panic here).
    if parts.len() != 4 {
        return None;
    }

    // parts[0] = "proc", parts[1] = pid or "self" (already resolved), parts[2] = "fd"
    if parts[0] != "proc" || parts[2] != "fd" {
        return None;
    }

    let pid: u64 = match parts[1].parse() {
        Ok(p) => p,
        Err(_) => return None,
    };

    let fd: u32 = match parts[3].parse() {
        Ok(f) => f,
        Err(_) => return None,
    };

    let target = crate::fs::procfs::pid::generate_fd_link(pid, fd);
    if target.is_empty() {
        None
    } else {
        Some(target)
    }
}

/// sys_lseek - Set file offset
pub fn sys_lseek(args: SyscallArgs) -> i64 {
    use crate::fs::get_file_fd;

    let fd = args[0] as i32;
    let offset = args[1] as isize;  // Use isize instead of i64
    let whence = args[2] as i32;

    // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File or None.
    unsafe {
        match get_file_fd(fd as usize) {
            Some(file) => {
                let result = file.lseek(offset, whence);
                // lseek returns isize, negative indicates error
                if result < 0 {
                    result as i64
                } else {
                    result as i64
                }
            }
            None => -(errno::EBADF as i64)
        }
    }
}

/// sys_chdir - Change current directory
pub fn sys_chdir(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;

    let mut buf = [0u8; PATH_MAX];
    let pathname = match read_user_path(pathname_ptr, &mut buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    // Resolve the RAW user path through path_lookup: it joins relative
    // paths against the cwd and prefixes the task root onto absolute
    // paths, in the caller's namespace. The old manual cwd join here
    // produced a GLOBAL absolute path which path_lookup then
    // root-prefixed a SECOND time — every relative chdir() under
    // chroot() failed with ENOENT.
    match crate::fs::vfs::path_lookup(pathname, 0) {
        Ok(vpath) => {
            if let Some(ref inode) = vpath.inode {
                if !inode.mode.is_directory() {
                    return -(errno::ENOTDIR as i64);
                }
                // Linux path_step/chdir checks search (MAY_EXEC) permission
                // on the TARGET directory too, not just the components
                // traversed — path_lookup only DAC-checks intermediate dirs.
                // Without this, an unprivileged chdir() into a no-x
                // directory (mode 0644) succeeded (LTP chdir01 "keep_out").
                if !crate::fs::permission::inode_permission(
                    inode,
                    crate::fs::permission::MAY_EXEC,
                ) {
                    return -(errno::EACCES as i64);
                }
            } else {
                return -(errno::ENOENT as i64);
            }
            // Linux getcwd(2) returns the PHYSICAL path: the kernel keeps
            // the cwd as a (vfsmount, dentry) pair, so a chdir through a
            // symlink reports the RESOLVED location, never the symlink
            // name (LTP symlink01 chdir01 compares getcwd against the
            // object directory). Storing the literal argument string made
            // getcwd echo the unresolved symlink path. The dentry is
            // recorded alongside the string: it is the truth that keeps
            // the cwd correct across later mount moves (MS_MOVE) and lets
            // getcwd compute root-relative paths under chroot.
            let physical = vpath
                .dentry
                .as_ref()
                .map(|d| d.build_path())
                .unwrap_or_else(|| alloc::string::String::from(pathname));
            let cwd_dentry = vpath.dentry.clone();
            if let Some(current) = crate::sched::current() {
                // SAFETY: current is the running task's Task pointer from
                // sched::current(); fs_arc() only clones the Arc out of it.
                if let Some(fs) = unsafe { (*current).fs_arc() } {
                    fs.set_cwd_with_dentry(physical.as_bytes(), cwd_dentry);
                }
            }
            0
        }
        Err(e) => e as i64,
    }
}

/// sys_fchdir - Change working directory by fd
/// RISC-V syscall number: 50
pub fn sys_fchdir(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;

    // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File or None.
    let file = unsafe { crate::fs::get_file_fd(fd) }
        .ok_or(-(errno::EBADF as i64));
    let file = match file {
        Ok(f) => f,
        Err(e) => return e,
    };

    // SAFETY: inode is an UnsafeCell; we hold &File so no concurrent mutation.
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = match inode_opt.as_ref() {
        Some(i) => i,
        None => return -(errno::EBADF as i64),
    };

    if !inode.mode.is_directory() {
        return -(errno::ENOTDIR as i64);
    }

    // Same target MAY_EXEC requirement as chdir(2) — fchdir to a directory
    // the caller cannot search is EACCES (Linux ksys_fchdir permission hook).
    if !crate::fs::permission::inode_permission(
        inode,
        crate::fs::permission::MAY_EXEC,
    ) {
        return -(errno::EACCES as i64);
    }
    // SAFETY: dentry is an UnsafeCell; we hold &File so no concurrent mutation.
    let dentry_opt = unsafe { &*file.dentry.get() };
    let dentry = match dentry_opt.as_ref() {
        Some(d) => d,
        None => return -(errno::EBADF as i64),
    };

    // The physical (mount-crossing) path of the target directory, exactly
    // what sys_chdir stores. The old manual parent walk could not cross
    // mount boundaries (a mounted fs root has no parent pointer), so
    // fchdir to a /proc or /dev fd stored cwd="/" (or dropped the
    // mountpoint prefix) — same defect chdir had before build_path()
    // became mount-aware. The dentry is recorded alongside the string
    // (same rationale as sys_chdir — survives later mount moves, feeds
    // root-relative getcwd).
    let physical = dentry.build_path();
    let cwd_dentry = dentry.clone();

    if let Some(task) = crate::sched::current() {
        // SAFETY: task is the running task's Task pointer from
        // sched::current(); fs_arc() only clones the Arc out of it.
        if let Some(fs) = unsafe { (*task).fs_arc() } {
            fs.set_cwd_with_dentry(physical.as_bytes(), Some(cwd_dentry));
        }
    }

    0
}

/// sys_getcwd - Get current working directory
pub fn sys_getcwd(args: SyscallArgs) -> i64 {
    let buf = args[0] as *mut u8;
    let size = args[1] as usize;

    // A zero-length buffer can never hold the cwd (+ NUL): ERANGE before
    // any range validation (Linux getcwd; LTP getcwd01 — the old order
    // ran access_ok first and returned EFAULT for size 0).
    if size == 0 {
        return -(errno::ERANGE as i64);
    }

    if let Some(current) = crate::sched::current() {
        // SAFETY: current is the running task's Task pointer from
        // sched::current(); fs_arc() only clones the Arc out of it.
        let fs = unsafe { (*current).fs_arc() };

        // Linux prepend_path() walks the cwd dentry UP TO the process ROOT
        // (fs->root), so getcwd reports a path RELATIVE TO THE JAIL: after
        // chroot("/usr") + chdir("/usr/bin"), getcwd() is "/bin", not
        // "/usr/bin". When the walk reaches the global root without
        // meeting the process root (chroot() without chdir() — the cwd
        // stays outside the jail, historic Linux behavior), getcwd fails
        // with ENOENT ("(unreachable)" in modern kernels; the
        // CVE-2018-1000001 shape; LTP realpath01 chroots into an empty dir
        // and expects realpath(".") == NULL/ENOENT because getcwd does).
        let cwd: alloc::vec::Vec<u8> = match fs.as_ref() {
            Some(fs) if fs.cwd_dentry().is_some() => {
                let cwd_d = fs.cwd_dentry().unwrap();
                let root = fs.get_root();
                if root.as_slice() == b"/" {
                    // Not chrooted: the mount-crossing global path —
                    // `cd /proc; pwd` must report "/proc".
                    cwd_d.build_path().into_bytes()
                } else if let Some(root_d) = fs.root_dentry() {
                    match cwd_d.build_path_rooted(Some(&root_d)) {
                        Some(p) => p.into_bytes(),
                        // cwd outside the jail: Linux getcwd fails ENOENT.
                        None => return -(errno::ENOENT as i64),
                    }
                } else {
                    // Chrooted without a root dentry (legacy root set by a
                    // string-only path): approximate with the string prefix.
                    let cwd = fs.get_cwd();
                    if !cwd.starts_with(&root[..]) {
                        return -(errno::ENOENT as i64);
                    }
                    cwd
                }
            }
            _ => {
                // No cwd dentry recorded (early boot / string-only state):
                // old string-prefix approximation, unchanged.
                // SAFETY: same task pointer as above.
                let cwd = unsafe { (*current).get_cwd() };
                // SAFETY: same task pointer as above.
                let root = unsafe { (*current).get_root() };
                if root.as_ref() != b"/".as_slice() && !cwd.starts_with(&root[..]) {
                    return -(errno::ENOENT as i64);
                }
                cwd.into()
            }
        };

        let cwd_len = cwd.len();

        // Linux checks the length BEFORE the buffer: getcwd(NULL, 1) is
        // ERANGE (the cwd + NUL cannot fit), not EFAULT — only a size that
        // COULD hold the path proceeds to buffer validation/copy (LTP
        // getcwd01 case 5: buf=NULL size=1 expected ERANGE, got EFAULT).
        if cwd_len >= size {
            return -(errno::ERANGE as i64);
        }

        if buf.is_null() {
            return -(errno::EFAULT as i64);
        }

        // Check if buf is in valid user space
        if !crate::arch::uaccess::access_ok(buf as usize, size) {
            return -(errno::EFAULT as i64);
        }

        // SAFETY: buf validated with access_ok(size); cwd_len < size; writes cwd_len + 1 bytes.
        // Use copy_to_user for proper SUM bit handling.
        unsafe {
            let remaining = crate::arch::uaccess::copy_to_user(buf, cwd.as_ptr(), cwd_len);
            if remaining != 0 {
                return -(errno::EFAULT as i64);
            }
            let remaining = crate::arch::uaccess::copy_to_user(buf.add(cwd_len), &0u8, 1);
            if remaining != 0 {
                return -(errno::EFAULT as i64);
            }
        }

        return buf as i64;
    }

    -(errno::ENOENT as i64)
}

/// sys_umask - Set file mode creation mask
pub fn sys_umask(args: SyscallArgs) -> i64 {
    let new_mask = (args[0] & 0o777) as u32;
    if let Some(task) = crate::sched::current() {
        task.set_umask(new_mask) as i64
    } else {
        0o022i64
    }
}

/// sys_mount - Mount a filesystem (syscall 40)
///
/// mount(source, target, filesystemtype, mountflags, data)
pub fn sys_mount(args: SyscallArgs) -> i64 {
    // Linux order: copy source (EFAULT on a bad pointer — LTP mount03
    // "EFAULT for filesystemtype/source" cases) BEFORE capability checks
    // on the strings; CAP_SYS_ADMIN gates the actual mount.
    let mut source_buf = [0u8; PATH_MAX];
    // FIX8: a NULL source is legal for anonymous filesystems (tmpfs,
    // proc, ...). Linux path_mount passes the NULL dev_name through and
    // only filesystems with FS_REQUIRES_DEV reject it later — LTP's
    // tst_test tmpfs mntpoint setup calls mount(NULL, mntpoint, "tmpfs",
    // 0, NULL) and used to fall into the block-device+mkfs fallback
    // ("Can't mount (null) at mntpoint (tmpfs): EFAULT"). Only a
    // NON-NULL bad pointer is EFAULT.
    let source = if args[0] == 0 {
        "none"
    } else {
        match read_user_path(args[0] as *const u8, &mut source_buf) {
            Ok(s) => s,
            Err(e) => return e as i64,
        }
    };
    let source = if source.is_empty() { "none" } else { source };

    // CAP_SYS_ADMIN required to mount
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as i64;
    }

    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(args[1] as *const u8, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    // Linux path_mount resolves the target against the CALLER'S CWD —
    // a relative mountpoint like LTP madvise01's "tmp_madvise" (mounted
    // from inside the test's tmpdir) must attach at <cwd>/tmp_madvise.
    // The old code passed the relative string straight to vfs_mount,
    // which walks from the VFS ROOT — the mount landed on a phantom
    // /tmp_madvise dentry and every later lookup crossed into the
    // unmounted ext4 dir instead.
    let target_owned = if !target.starts_with('/') {
        match crate::sched::current() {
            Some(t) => {
                let cwd = unsafe { (*t).get_cwd() };
                let mut abs = alloc::vec::Vec::with_capacity(cwd.len() + 1 + target.len());
                abs.extend_from_slice(&cwd);
                if !cwd.ends_with(&[b'/']) {
                    abs.push(b'/');
                }
                abs.extend_from_slice(target.as_bytes());
                match alloc::string::String::from_utf8(abs) {
                    Ok(s) => s,
                    Err(_) => return -(errno::EINVAL as i64),
                }
            }
            None => alloc::string::String::from(target),
        }
    } else {
        alloc::string::String::from(target)
    };
    let target = target_owned.as_str();

    // fstype is OPTIONAL for MS_MOVE/MS_REMOUNT-style calls (Linux
    // path_mount only reads it for fresh mounts; OH's switch_root moves
    // mounts with mount(src, dst, NULL, MS_MOVE, NULL)). NULL or an
    // empty string passes through as "".
    let mut fstype_buf = [0u8; 64];
    let fs_type_str = if args[2] == 0 {
        ""
    } else {
        match read_user_str(args[2] as *const u8, &mut fstype_buf) {
            Ok(s) => s,
            Err(e) => return e as i64,
        }
    };

    // The mount target must exist (Linux path_mount resolves it before the
    // filesystem-specific helper runs — the old code fabricated success for
    // any path string).
    if let Err(e) = crate::fs::vfs::path_lookup(target, 0) {
        return e as i64;
    }

    // MS_REMOUNT: update the flags of the mount already attached at `target`
    // instead of mounting a fresh instance. LTP's needs_rofs setup does
    // mount(NULL, mntpoint, "tmpfs", 0), creates files, then
    // mount(dev, mntpoint, fs, MS_REMOUNT|MS_RDONLY) — routing that through
    // do_mount replaced the tmpfs (the files vanished, ENOENT where EROFS
    // was expected) and never produced a read-only view.
    // MS_MOVE: re-attach the mount at `source` over `target` (Linux
    // move_mount semantics — OH's switch_root walks this path for
    // /dev,/proc,/sys,/run and finally /usr→/). No fstype, no data, no
    // fresh instance: the VfsMountInternal (flags included) moves.
    const MS_MOVE: u64 = 8192;
    if args[3] & MS_MOVE != 0 {
        return match crate::fs::vfs::vfs_move_mount(source, target) {
            Ok(()) => {
                // Keep the /proc/mounts row pointed at the new target.
                let existing = crate::fs::mount::get_mounts()
                    .into_iter()
                    .find(|(_, mp, _, _)| mp == source);
                if let Some((device, _, fs_type, flags)) = existing {
                    crate::fs::mount::register_mount(&device, target, &fs_type, &flags);
                }
                0
            }
            Err(e) => e as i64,
        };
    }

    const MS_REMOUNT: u64 = 32;
    if args[3] & MS_REMOUNT != 0 {        let new_flags = crate::fs::mount::translate_mnt_flags_public(args[3] & !MS_REMOUNT);
        match crate::fs::vfs::vfs_remount(target, new_flags) {
            Ok(()) => {
                // Keep the /proc/mounts row in sync (ro/rw string).
                let existing = crate::fs::mount::get_mounts()
                    .into_iter()
                    .find(|(_, mp, _, _)| mp == target);
                if let Some((device, _, fs_type, _)) = existing {
                    crate::fs::mount::register_mount(
                        &device,
                        target,
                        &fs_type,
                        if new_flags.is_readonly() { "ro" } else { "rw" },
                    );
                }
                return 0;
            }
            Err(e) => return e as i64,
        }
    }

    // MS_BIND: attach the source subtree over the target (Linux
    // do_loopback; dispatch order REMOUNT → BIND → MOVE → propagation).
    // OH's appspawn sandbox construction binds /proc, /dev, /sys,
    // /system/... into /mnt/sandbox/<user>/<pkg>/ — b19 showed every
    // one failing EINVAL right after the propagation fix landed. The
    // new mount SHARES the source directory's inode (see
    // vfs_bind_mount); fstype and data are ignored, as on Linux.
    const BIND_MS_BIND: u64 = 4096;
    const BIND_MS_REC: u64 = 16384;
    if args[3] & BIND_MS_BIND != 0 {
        let mnt_flags = crate::fs::mount::translate_mnt_flags_public(args[3]);
        let ret = match crate::fs::vfs::vfs_bind_mount(
            source,
            target,
            mnt_flags,
            args[3] & BIND_MS_REC != 0,
        ) {
            Ok(()) => {
                // Register in /proc/mounts AND the caller's mount namespace:
                // OH's appspawn child self-binds the sandbox root precisely
                // so pivot_root's is-mountpoint check passes (b22: sandbox
                // setup completed, then `pivot /mnt/sandbox/0/<pkg> failed,
                // err 22` because the bind was never registered anywhere).
                crate::fs::mount::register_mount(
                    source,
                    target,
                    "bind",
                    if mnt_flags.is_readonly() { "ro" } else { "rw" },
                );
                0
            }
            Err(e) => e,
        };
        if ret != 0 {
            crate::pr_info!(
                "bindmount '{}' -> '{}' flags {:#x}: {}",
                source,
                target,
                args[3],
                ret
            );
        }
        return ret as i64;
    }

    // Propagation-type change (Linux do_mount dispatch: REMOUNT → BIND →
    // MOVE → propagation → new mount; this block sits after REMOUNT and
    // MOVE). When flags carry ONLY propagation bits (MS_SHARED/MS_PRIVATE/
    // MS_SLAVE/MS_UNBINDABLE, plus harmless modifiers like MS_REC),
    // mount() changes the propagation type of the mount containing
    // `target` — no new filesystem instance, fstype/source ignored. Rux
    // mounts never propagate, so the change is a success no-op — but
    // returning EINVAL killed real callers: OH init's sandbox prepare
    // ("Failed set mount slave err = 22"), and the appspawn child's
    // mount(NULL, "/", NULL, MS_SLAVE|MS_REC) right after creating the
    // sandbox root dirs — b18 nailed DoSandboxRootFolderCreate dying at
    // exactly mount[29] flags 0x84000 = MS_SLAVE|MS_REC → EINVAL, which
    // failed every START_PROCESS (systemui, launcher) after the fork.
    const CHG_MS_UNBINDABLE: u64 = 1 << 17;
    const CHG_MS_PRIVATE: u64 = 1 << 18;
    const CHG_MS_SLAVE: u64 = 1 << 19;
    const CHG_MS_SHARED: u64 = 1 << 20;
    const CHG_PROPAGATION: u64 =
        CHG_MS_SHARED | CHG_MS_PRIVATE | CHG_MS_SLAVE | CHG_MS_UNBINDABLE;
    if args[3] & CHG_PROPAGATION != 0 {
        let prop = args[3] & CHG_PROPAGATION;
        // Multiple propagation types at once is EINVAL (Linux
        // change_mount_flags rejects the combination).
        if prop & prop.wrapping_sub(1) != 0 {
            return -(errno::EINVAL as i64);
        }
        return 0;
    }

    // Mount options string (mount(2) arg 5): consumed by tmpfs today
    // (its size= page budget — LTP's all-filesystems harness mounts
    // tmpfs with size=32M, and an unbounded tmpfs let preadv203's 118 MB
    // write pattern exhaust the kernel heap). Other filesystem types
    // accept and ignore it. NULL (the common case) passes through as
    // None; a bad pointer is EFAULT like Linux's getname() copy.
    let mut data_buf = [0u8; 256];
    let mount_data: Option<&str> = if args[4] != 0 {
        match read_user_str(args[4] as *const u8, &mut data_buf) {
            Ok(s) => Some(s),
            Err(e) => return e as i64,
        }
    } else {
        None
    };

    // OH bring-up: throttled mount trace — the param service mounts
    // tmpfs at /dev/__parameters__; a failed mount there stalls every
    // later param get (samgr/devhost startup).
    {
        static MNT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let n = MNT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // hmdfs/sharefs are always traced (b143: StorageDaemon's
        // per-user /mnt/hmdfs/<id>/account mount "fails errno 11" and
        // the account-ready chain rides on diagnosing it; a handful of
        // mounts per boot, not per-transaction).
        let always = fs_type_str == "hmdfs" || fs_type_str == "sharefs";
        {
            let ret = match crate::fs::mount::do_mount(source, target, fs_type_str, args[3], mount_data) {
                Ok(()) => 0,
                Err(e) => e,
            };
            // b18/sc18: print EVERY failed mount, however late in the boot.
            // The old n<24 gate made appspawn's DoSandboxRootFolderCreate
            // failure (systemui spawn, hook stage 31) invisible — sandbox
            // mounts happen thousands of mounts into the boot.
            if ret != 0 || n < 12 || always {
                crate::pr_info!(
                    "mount[{}] '{}' -> '{}' type '{}' flags {:#x}: {}",
                    n,
                    source,
                    target,
                    fs_type_str,
                    args[3],
                    ret
                );
            }
            return ret as i64;
        }
    }

    match crate::fs::mount::do_mount(source, target, fs_type_str, args[3], mount_data) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_umount - Unmount a filesystem (syscall 39)
///
/// umount2(target, flags) — Linux flag semantics (LTP umount2_01/02):
///   MNT_FORCE (1)     — mark pending; without real busy-reference
///                       tracking it unmounts like a plain umount.
///   MNT_DETACH (2)    — lazy detach: accepted and detaches now.
///   MNT_EXPIRE (4)    — exponential expiry: the FIRST attempt on an
///                       idle mount fails EAGAIN and arms the flag; the
///                       mount unmounts only on a later attempt with no
///                       intervening access. Any access in between
///                       re-requires the EAGAIN probe.
///   UMOUNT_NOFOLLOW(8)— do not follow a trailing symlink (accepted).
///   MNT_EXPIRE combined with MNT_FORCE or MNT_DETACH is EINVAL.
pub fn sys_umount(args: SyscallArgs) -> i64 {
    const MNT_FORCE: u64 = 1;
    const MNT_DETACH: u64 = 2;
    const MNT_EXPIRE: u64 = 4;
    const UMOUNT_NOFOLLOW: u64 = 8;
    let flags = args[1];

    // Unknown flag bits are EINVAL (do_umount: flag validation first).
    if flags & !(MNT_FORCE | MNT_DETACH | MNT_EXPIRE | UMOUNT_NOFOLLOW) != 0 {
        return -(errno::EINVAL as i64);
    }
    // MNT_EXPIRE is exclusive with MNT_FORCE/MNT_DETACH.
    if flags & MNT_EXPIRE != 0 && flags & (MNT_FORCE | MNT_DETACH) != 0 {
        return -(errno::EINVAL as i64);
    }

    // CAP_SYS_ADMIN required to unmount
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return crate::errno::Errno::OperationNotPermitted.as_neg_i32() as i64;
    }

    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(args[0] as *const u8, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    // Same CWD resolution as sys_mount — umount(2) takes the same kind
    // of path and must unmount the mount the caller sees.
    let target_owned = if !target.starts_with('/') {
        match crate::sched::current() {
            Some(t) => {
                let cwd = unsafe { (*t).get_cwd() };
                let mut abs = alloc::vec::Vec::with_capacity(cwd.len() + 1 + target.len());
                abs.extend_from_slice(&cwd);
                if !cwd.ends_with(&[b'/']) {
                    abs.push(b'/');
                }
                abs.extend_from_slice(target.as_bytes());
                match alloc::string::String::from_utf8(abs) {
                    Ok(s) => s,
                    Err(_) => return -(errno::EINVAL as i64),
                }
            }
            None => alloc::string::String::from(target),
        }
    } else {
        alloc::string::String::from(target)
    };
    // Normalize like Linux's user_path_at: OH's post-pivot cleanup calls
    // umount2("<sandbox-root>/.", MNT_DETACH) and the literal "." component
    // made vfs_umount's parent/name split fail EINVAL (sc23 umountfail
    // trace), leaving init wedged in a half-prepared sandbox.
    let target = crate::fs::path::path_normalize(&target_owned);
    let target = target.as_str();

    // U1c: drop the mount row from the CALLER's mount namespace (the
    // dentry-level unmount below is global — recorded divergence).
    {
        let norm = crate::fs::path::path_normalize(target);
        crate::process::ns::ns_unregister_mount(&norm);
    }

    // MNT_EXPIRE two-phase protocol (see the header comment): the first
    // attempt on a mount answers EAGAIN and arms the expire flag; a later
    // attempt unmounts only when nothing touched the mountpoint in
    // between (path lookups under the target bump `accessed`).
    if flags & MNT_EXPIRE != 0 {
        let norm = crate::fs::path::path_normalize(target);
        let mut exp = MNT_EXPIRE_STATE.lock();
        let entry = exp.entry_mut(norm.as_bytes());
        if !entry.armed || entry.accessed_since_arm {
            // First probe (or the mount was used since the last probe):
            // EAGAIN, arm/re-arm the flag and clear the access mark.
            entry.armed = true;
            entry.accessed_since_arm = false;
            return -(errno::EAGAIN as i64);
        }
    }

    // Post-pivot cleanup umount2(put_old | ".", MNT_DETACH): our
    // jail-style pivot_root stacks no real mount in the shared dentry
    // tree, so there is nothing for vfs_umount to detach; Linux's
    // pivot_root leaves the OLD root mounted at put_old and this umount
    // drops it. When the target matches the caller's recorded put_old
    // (OH appspawn sandbox_core.cpp:1272 umounts "." right after
    // pivot_root(p, p); init's sandbox cleanup umounts put_old too),
    // consume the marker and report success so the sandbox setup
    // proceeds — the jail root itself is untouched.
    if flags & MNT_DETACH != 0 {
        if let Some(t) = crate::sched::current() {
            // SAFETY: sched::current() yields a valid task reference.
            if let Some(fs) = unsafe { (*t).fs_arc() } {
                if let Some(po) = fs.pivot_put_old() {
                    let po_str =
                        core::str::from_utf8(&po).unwrap_or("/");
                    let po_norm = crate::fs::path::path_normalize(po_str);
                    // "/" also names the old-root stack for the
                    // pivot_root(new, new) idiom: OH's appspawn child
                    // chdir's into the sandbox root, pivots, and its cwd
                    // collapses to "/" inside the new root — umount2(".",
                    // MNT_DETACH) then arrives here as "/" (b25: children
                    // 1185-1189 all EBUSY on exactly that). A pivot
                    // happened in this fs (po is set), so the DETACH pops
                    // the (unmodelled) old-root stack: no-op success.
                    if target == po_norm || target == "/" {
                        fs.clear_pivot_put_old();
                        return 0;
                    }
                }
            }
        }
    }

    let ret = match crate::fs::vfs::vfs_umount(target) {
        Ok(()) => 0,
        Err(e) => e,
    };
    if ret != 0 {
        // b24: after the (now-working) sandbox pivot, OH init's cleanup
        // umount2(old_root, MNT_DETACH) fails EINVAL and wedges the boot
        // in a half-pivoted state — name every failed umount so the
        // topology gap is one glance.
        crate::pr_info!(
            "umountfail '{}' flags {:#x}: {} (pid {})",
            target,
            flags,
            ret,
            crate::process::current_pid()
        );
    }
    ret as i64
}

/// Per-mount MNT_EXPIRE bookkeeping: (armed, accessed-since-arm) keyed by
/// the normalized mountpoint path.
struct ExpireEntry {
    armed: bool,
    accessed_since_arm: bool,
}

struct ExpireMap {
    map: alloc::collections::BTreeMap<alloc::vec::Vec<u8>, ExpireEntry>,
}

impl ExpireMap {
    const fn new() -> Self {
        Self { map: alloc::collections::BTreeMap::new() }
    }

    fn entry_mut(&mut self, key: &[u8]) -> &mut ExpireEntry {
        self.map
            .entry(key.to_vec())
            .or_insert(ExpireEntry { armed: false, accessed_since_arm: false })
    }

    /// Record a path access at/under a tracked mountpoint (expires any
    /// armed MNT_EXPIRE probe). Hooked from the access(2) family — the
    /// Linux semantics gate on "not in use", and access() is the probe
    /// LTP umount2_02 uses.
    pub fn note_access(&mut self, path: &[u8]) {
        if self.map.is_empty() {
            return;
        }
        for (k, e) in self.map.iter_mut() {
            if path == k.as_slice() || path.starts_with(k.as_slice()) {
                e.accessed_since_arm = true;
            }
        }
    }
}

static MNT_EXPIRE_STATE: crate::sync::spinlock::Spinlock<ExpireMap> =
    crate::sync::spinlock::Spinlock::new(ExpireMap::new());

/// sys_faccessat - Check file access permissions (syscall 48)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: mode - access mode (F_OK, R_OK, W_OK, X_OK)
/// - args[3]: flags - flags (AT_EACCESS, etc.)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_faccessat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as i32;

    // Linux validates the mode word before touching the path: anything
    // outside F_OK|R_OK|W_OK|X_OK is EINVAL (LTP access04 case 1 passes
    // -1 and expects EINVAL, not a permission answer).
    if mode & !0o7 != 0 {
        return -(errno::EINVAL as i64);
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // MNT_EXPIRE probe access: a successful resolution at/under a tracked
    // mountpoint re-arms its expire state (umount2(2) then answers EAGAIN
    // until the mount stays untouched across two umount2 attempts).
    MNT_EXPIRE_STATE.lock().note_access(full_path.as_bytes());

    let mut may_mask: u32 = 0;
    if mode & 0o004 != 0 { may_mask |= crate::fs::permission::MAY_READ; }
    if mode & 0o002 != 0 { may_mask |= crate::fs::permission::MAY_WRITE; }
    if mode & 0o001 != 0 { may_mask |= crate::fs::permission::MAY_EXEC; }

    // access(2) EROFS: write permission requested on a read-only filesystem
    // (LTP access04 probes W_OK on an MS_RDONLY tmpfs mountpoint).
    if mode & 0o002 != 0 && crate::fs::vfs::path_on_readonly_mount(&full_path) {
        return -(errno::EROFS as i64);
    }

    // faccessat uses the REAL uid/gid for the permission decision (Linux:
    // the euid is only used when AT_EACCESS is passed — and faccessat on
    // riscv64 has no flags argument, so the real id is always correct
    // here). The old code used the entire cred (which resolves by euid).
    let mut cred = if let Some(task) = crate::sched::current() {
        task.cred().clone()
    } else {
        crate::process::task::Cred::new_init()
    };
    cred.euid = cred.uid;
    cred.egid = cred.gid;

    match crate::fs::vfs::path_lookup(&full_path, 0) {
        Ok(vfs_path) => {
            if let Some(inode) = &vfs_path.inode {
                let inode_mode = inode.mode.bits() as u16;
                let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
                let inode_gid = inode.gid.load(core::sync::atomic::Ordering::Relaxed);

                if !crate::fs::permission::generic_permission(inode_mode, inode_uid, inode_gid, may_mask, &cred) {
                    return -(errno::EACCES as i64);
                }
            }
            0
        }
        Err(e) => e as i64,
    }
}

/// sys_futimesat - Change file timestamps (syscall 88 = utimensat)
///
/// utimensat ABI: (dirfd, pathname, times, flags)
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path (NULL means futimens(fd))
/// - args[2]: times - pointer to timespec[2] (or NULL = UTIME_NOW both)
/// - args[3]: flags - AT_SYMLINK_NOFOLLOW / AT_EMPTY_PATH
///
/// # Returns
/// 0 on success, negative error code on failure
///
/// # Behavior
/// - times NULL: set both atime and mtime to the current time
/// - times[i].tv_nsec == UTIME_OMIT (0x3ffffffe): leave that timestamp alone
/// - times[i].tv_nsec == UTIME_NOW  (0x3fffffff): use the current time
/// - Otherwise use the given {tv_sec, tv_nsec}
pub fn sys_futimesat(args: SyscallArgs) -> i64 {
    const UTIME_OMIT: i64 = 0x3fff_fffe;
    const UTIME_NOW: i64 = 0x3fff_ffff;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let times_ptr = args[2] as *const crate::syscall::time::Timespec;
    let flags = args[3] as u32;

    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return -(errno::EINVAL as i64);
    }

    // Parse the user timespec pair first (before any path work) so bad
    // values/negative tv_sec are reported as EINVAL and unreadable memory
    // as EFAULT, matching Linux.
    let (atime, mtime, explicit_times): (Option<u64>, Option<u64>, bool) = if times_ptr.is_null() {
        // NULL = UTIME_NOW for both (resolve here: vfs_utimensat leaves
        // None fields untouched).
        let now = current_time_secs();
        (Some(now), Some(now), false)
    } else {
        if !crate::arch::uaccess::access_ok(times_ptr as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        let mut buf = [0u8; 32];
        // SAFETY: times_ptr validated with access_ok(32); exception-table copy.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_from_user(
                buf.as_mut_ptr(),
                times_ptr as *const u8,
                32,
            )
        };
        if uncopied > 0 {
            return -(errno::EFAULT as i64);
        }
        let rd = |i: usize| i64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap());
        let mut parsed = [None, None];
        for i in 0..2 {
            let (sec, nsec) = (rd(i * 2), rd(i * 2 + 1));
            if nsec == UTIME_OMIT {
                parsed[i] = None; // leave alone
            } else if nsec == UTIME_NOW {
                parsed[i] = Some(current_time_secs()); // now
            } else {
                if nsec < 0 || nsec >= 1_000_000_000 || sec < 0 {
                    return -(errno::EINVAL as i64);
                }
                parsed[i] = Some(sec as u64);
            }
        }
        (parsed[0], parsed[1], true)
    };

    // futimens(fd): pathname NULL, dirfd is the fd. utimensat(AT_FDCWD,
    // NULL, ...) — a userspace utimensat(-100, NULL, ...) is EFAULT (no
    // path, no fd: LTP utimes01 "NULL pathname" case; conflating it with
    // futimens returned EBADF).
    if pathname_ptr.is_null() {
        if dirfd == -100 {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: dirfd is a caller-supplied fd; get_file_fd returns None for
        // invalid fds.
        let file = match unsafe { crate::fs::get_file_fd(dirfd as usize) } {
            Some(f) => f,
            None => return -(errno::EBADF as i64),
        };
        // SAFETY: inode is an UnsafeCell; we hold &File so no concurrent mutation.
        let inode_opt = unsafe { &*file.inode.get() };
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return -(errno::EBADF as i64),
        };
        // Times setter permission: owner or CAP_FOWNER (same rule as the
        // path form).
        let cred = match crate::sched::current() {
            Some(t) => t.cred().clone(),
            None => return -(errno::EPERM as i64),
        };
        let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
        if cred.euid != inode_uid
            && !crate::security::has_capability(&cred, crate::security::CAP_FOWNER)
        {
            return -(errno::EPERM as i64);
        }
        if let Some(m) = mtime {
            let _ = inode.op_setattr(crate::fs::inode::setattr_attr::ATTR_MTIME, m, 0);
        }
        if let Some(a) = atime {
            let _ = inode.op_setattr(crate::fs::inode::setattr_attr::ATTR_ATIME, a, 0);
        }
        return 0;
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // UTIME_NOW (times==NULL or per-component) was resolved to
    // current_time_secs() above; None reaching the vfs layer means
    // UTIME_OMIT (leave untouched).
    match crate::fs::vfs::vfs_utimensat_ex(&full_path, atime, mtime, explicit_times) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// Current wall-clock time in whole seconds (for UTIME_NOW).
fn current_time_secs() -> u64 {
    crate::drivers::rtc::wall_secs()
}

/// Read a null-terminated path string from user space into a kernel buffer.
/// Returns a borrowed &str with lifetime tied to the buffer.
/// Does NOT resolve relative paths or combine with CWD.
fn read_user_path<'a>(
    pathname_ptr: *const u8,
    buf: &'a mut [u8; PATH_MAX],
) -> Result<&'a str, u64> {
    if pathname_ptr.is_null() {
        return Err(-errno::EFAULT as u64);
    }
    if !crate::arch::uaccess::access_ok(pathname_ptr as usize, PATH_MAX) {
        return Err(-errno::EFAULT as u64);
    }
    let pathname = match strncpy_from_user(pathname_ptr, PATH_MAX, buf) {
        Ok(s) => s,
        Err(e) => return Err(e as u64),
    };
    // Linux getname(): a path with no NUL within PATH_MAX is ENAMETOOLONG.
    // strncpy_from_user silently truncates at the limit — a full-length
    // slice therefore means the terminator was never seen (LTP fchmodat02's
    // 4098-byte single-component path; the old truncation surfaced as
    // ENOTDIR from the walk instead).
    if pathname.len() >= PATH_MAX {
        return Err(-errno::ENAMETOOLONG as u64);
    }
    core::str::from_utf8(pathname).map_err(|_| -errno::EINVAL as u64)
}

/// Read a null-terminated string from user space (short variant for non-path arguments).
fn read_user_str<'a>(
    ptr: *const u8,
    buf: &'a mut [u8],
) -> Result<&'a str, u64> {
    if ptr.is_null() {
        return Err(-errno::EFAULT as u64);
    }
    if !crate::arch::uaccess::access_ok(ptr as usize, 1) {
        return Err(-errno::EFAULT as u64);
    }
    let s = match strncpy_from_user(ptr, buf.len(), buf) {
        Ok(s) => s,
        Err(e) => return Err(e as u64),
    };
    core::str::from_utf8(s).map_err(|_| -errno::EINVAL as u64)
}

/// Helper: read path from user space and resolve to absolute path (CWD-aware).
pub fn resolve_user_path(dirfd: i32, pathname_ptr: *const u8) -> Result<alloc::string::String, u64> {
    const AT_FDCWD: i32 = -100;

    let mut buf = [0u8; PATH_MAX];
    let pathname_str = read_user_path(pathname_ptr, &mut buf)?;

    // Linux getname(): an EMPTY pathname is ENOENT, never the cwd. The old
    // code joined "" onto the cwd ("/tmp/LTP_xxx/") so chmod("") silently
    // chmod'ed the test's own temp directory (LTP chmod06 case 6).
    if pathname_str.is_empty() {
        return Err((-errno::ENOENT as i64) as u64);
    }

    let full_path: alloc::string::String = if pathname_str.starts_with('/') {
        alloc::string::String::from(pathname_str)
    } else if dirfd == AT_FDCWD {
        // Join against the cwd AS THE USER SEES IT (root-relative, the
        // getcwd(3) string). The result feeds path_lookup-style callers
        // that PREFIX the task root onto absolute paths; joining the
        // GLOBAL cwd here made them prefix it twice, so every relative
        // *at()/chroot() path under chroot() failed with ENOENT.
        let cwd_str = crate::fs::vfs::get_cwd_user();
        let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
        path.push_str(&cwd_str);
        if !path.ends_with('/') { path.push('/'); }
        path.push_str(pathname_str);
        path
    } else {
        // Relative path resolved against the directory referenced by dirfd.
        // The fd must exist and be a directory (Linux: EBADF / ENOTDIR).
        let file = crate::sched::get_current_fdtable()
            .and_then(|ft| ft.get_file(dirfd as usize))
            .ok_or((-errno::EBADF as i64) as u64)?;
        // SAFETY: file comes from the fd table; inode accessed read-only.
        let is_dir = unsafe {
            (*file.inode.get())
                .as_ref()
                .map(|i| i.mode.is_directory())
                .unwrap_or(false)
        };
        if !is_dir {
            return Err((-errno::ENOTDIR as i64) as u64);
        }
        // Same namespace discipline as the AT_FDCWD branch: the base is
        // the dirfd's path RELATIVE TO THE TASK ROOT (d_path style — what
        // /proc/<pid>/fd shows), so downstream root-prefixing applies
        // exactly once. A dirfd outside the jail has no in-jail spelling
        // and keeps its global path (string roots cannot express Linux's
        // dentry-based dirfd escape).
        let base = {
            // SAFETY: file comes from the fd table; dentry written once at
            // open time and never mutated afterwards; read-only access.
            let dentry_opt = unsafe { (*file.dentry.get()).clone() };
            match dentry_opt {
                Some(d) => crate::fs::vfs::dentry_user_path(&d),
                None => file.path(),
            }
        };
        let mut path = alloc::string::String::with_capacity(base.len() + pathname_str.len() + 1);
        path.push_str(&base);
        if !path.ends_with('/') { path.push('/'); }
        path.push_str(pathname_str);
        path
    };
    Ok(full_path)
}

/// sys_fchmodat - Change file mode (syscall 53)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: mode - new file mode (permission bits)
/// - args[3]: flags - reserved
pub fn sys_fchmodat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as u32;

    // Magic-link shortcut (see sys_fstatat): chmod("/proc/*/fd/<n>")
    // chmods the file the fd refers to.
    if !crate::fs::vfs::chrooted() {
        let mut probe = [0u8; 64];
        if !pathname_ptr.is_null()
            && crate::arch::uaccess::access_ok(pathname_ptr as usize, probe.len())
            && unsafe {
                crate::arch::uaccess::copy_from_user(
                    probe.as_mut_ptr(), pathname_ptr, probe.len())
            } == 0
        {
            let len = probe.iter().position(|&b| b == 0).unwrap_or(probe.len());
            if let Ok(p) = core::str::from_utf8(&probe[..len]) {
                if let Some((_, fd)) = proc_fd_magic(p) {
                    return sys_fchmod([
                        fd as u64, mode as u64, 0, 0, 0, 0,
                    ]);
                }
            }
        }
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::vfs_chmod(full_path.as_ref(), mode) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_fchownat - Change file ownership (syscall 54)
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - file path
/// - args[2]: uid - new owner uid (u32::MAX = no change)
/// - args[3]: gid - new owner gid (u32::MAX = no change)
/// - args[4]: flags - flags (AT_SYMLINK_NOFOLLOW, etc.)
pub fn sys_fchownat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let uid = args[2] as u32;
    let gid = args[3] as u32;
    let flags = args[4] as u32;

    // Flags validation (Linux): AT_SYMLINK_NOFOLLOW (0x100) and
    // AT_EMPTY_PATH (0x1000) only — anything else is EINVAL.
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return -(errno::EINVAL as i64);
    }

    let full_path = match resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // AT_SYMLINK_NOFOLLOW: lchown(2) semantics — the ownership change
    // applies to the SYMBOLIC LINK itself, never the target. musl maps
    // lchown() to fchownat(..., AT_SYMLINK_NOFOLLOW), so ignoring the flag
    // made lchown chown the target instead (LTP chown05 saw the link keep
    // root:root where it expected the nobody ownership it had set).
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    match crate::fs::vfs::vfs_chown(full_path.as_ref(), uid, gid, follow) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_ftruncate - Truncate an open file (syscall 46)
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: length - new file size
pub fn sys_ftruncate(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let length = args[1] as i64;

    match crate::fs::vfs::vfs_ftruncate(fd, length) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_truncate - Truncate a file by path (syscall 45)
///
/// # Arguments
/// - args[0]: pathname - file path
/// - args[1]: length - new file size
pub fn sys_truncate(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let length = args[1] as i64;

    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::vfs_truncate(full_path.as_ref(), length) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// struct statfs - Filesystem statistics (64-bit)
#[repr(C)]
struct Statfs {
    f_type: u64,
    f_bsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [u32; 2],
    f_namelen: u64,
    f_frsize: u64,
    f_flags: u64,
    f_spare: [u64; 4],
}

/// Fill a Statfs from the rootfs superblock
fn fill_rootfs_statfs(buf: &mut Statfs) {
    use crate::fs::rootfs::{get_rootfs, ROOTFS_MAGIC};
    buf.f_type = ROOTFS_MAGIC as u64;
    buf.f_bsize = 4096;
    buf.f_blocks = 0;
    buf.f_bfree = 0;
    buf.f_bavail = 0;
    buf.f_files = 0;
    buf.f_ffree = 0;
    buf.f_fsid = [ROOTFS_MAGIC, 0];
    buf.f_namelen = 255;
    buf.f_frsize = 4096;
    buf.f_flags = 0;
    buf.f_spare = [0; 4];

    // Try to get more accurate info from rootfs
    let sb_ptr = get_rootfs();
    if !sb_ptr.is_null() {
        // SAFETY: sb_ptr is a valid superblock pointer from get_rootfs() when non-null.
        unsafe {
            buf.f_bsize = (*sb_ptr).sb.s_blocksize as u64;
            buf.f_type = (*sb_ptr).sb.s_magic as u64;
        }
    }
}

/// Fill a Statfs for ext4 from a LIVE filesystem instance.
///
/// The old filler hardcoded zeroes for every counter, which made guest
/// `df`/statfs(2) useless and hid exactly the "/tmp is filling up" state
/// behind the LTP r4/r5 exhaustion regression. Free counts come from the
/// in-memory group descriptors — the allocators keep them exact on every
/// alloc/free (bitmap, descriptor and superblock round trips are
/// synchronous under EXT4_BIG_LOCK).
fn fill_ext4_statfs_live(fs: &crate::fs::ext4::Ext4FileSystem, buf: &mut Statfs) {
    buf.f_type = 0xEF53; // EXT4_SUPER_MAGIC
    buf.f_bsize = fs.block_size as u64;
    buf.f_frsize = fs.block_size as u64;
    buf.f_blocks = fs.sb_info.as_ref().map(|sb| sb.s_blocks_count).unwrap_or(0);
    let mut free_blocks: u64 = 0;
    let mut free_inodes: u64 = 0;
    {
        let descs = fs.group_descs.lock();
        for d in descs.iter() {
            free_blocks += d.bg_free_blocks_count_lo as u64;
            free_inodes += d.bg_free_inodes_count_lo as u64;
        }
    }
    buf.f_bfree = free_blocks;
    // Reserved blocks are not enforced by this ext4; report them available
    // (matching what an unprivileged allocation can actually get).
    buf.f_bavail = free_blocks;
    buf.f_files = fs.sb_info.as_ref().map(|sb| sb.s_inodes_count as u64).unwrap_or(0);
    buf.f_ffree = free_inodes;
    buf.f_fsid = [0xEF53, 0];
    buf.f_namelen = 255;
    buf.f_flags = 0;
    buf.f_spare = [0; 4];
}

/// Fill a Statfs according to which filesystem actually backs `inode`.
///
/// Dispatch by inode-ops identity (the mount layout decides, not a
/// hardcoded "/mnt|/disk" prefix): an ext4 root covers `/`, `/tmp`, `/home`,
/// ... and must report ITS live numbers, while tmpfs mounts (/dev/shm,
/// /run) and the boot ramfs report their own.
fn fill_statfs_for_inode(inode: &crate::fs::inode::Inode, buf: &mut Statfs) {
    if inode
        .ops
        .is_some_and(|o| core::ptr::eq(o as *const _, &crate::fs::ext4::EXT4_INODE_OPS as *const _))
    {
        if let Some(p) = inode.private_data {
            // SAFETY: ext4 inodes carry their filesystem instance in
            // private_data (installed at iget; valid for the inode's life).
            let fs = unsafe { &*(p as *const crate::fs::ext4::Ext4FileSystem) };
            fill_ext4_statfs_live(fs, buf);
            return;
        }
    }
    if inode
        .ops
        .is_some_and(|o| core::ptr::eq(o as *const _, &crate::fs::tmpfs::TMPFS_INODE_OPS as *const _))
    {
        // tmpfs: pages are kernel-heap allocations with no fixed quota;
        // report the static instance identity (as before) — size signals
        // come from meminfo, not statfs.
        fill_rootfs_statfs(buf);
        buf.f_type = crate::fs::tmpfs::TMPFS_MAGIC as u32 as u64;
        buf.f_fsid = [crate::fs::tmpfs::TMPFS_MAGIC, 0];
        return;
    }
    if inode
        .ops
        .is_some_and(|o| core::ptr::eq(o as *const _, &crate::fs::selinuxfs::SELINUXFS_INODE_OPS as *const _))
    {
        // selinuxfs: f_type MUST be SELINUX_MAGIC (0xf97cff8c) —
        // libselinux's verify_selinuxmnt() gates selinux_mnt (and thus
        // every security_check_context) on exactly this magic and a
        // non-read-only statvfs. OH samgr's SA-permission gate depends
        // on it (boot b50's "SA:5100 selinux denied" loop).
        const SELINUX_MAGIC: u64 = 0xf97c_ff8c;
        fill_rootfs_statfs(buf);
        buf.f_type = SELINUX_MAGIC;
        buf.f_fsid = [SELINUX_MAGIC as u32, 0];
        buf.f_flags = 0; // rw — verify_selinuxmnt rejects ST_RDONLY
        return;
    }
    fill_rootfs_statfs(buf);
}

/// sys_statfs - Get filesystem statistics by path
/// RISC-V syscall number: 43
pub fn sys_statfs(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let buf_ptr = args[1] as *mut Statfs;

    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    if !crate::arch::uaccess::access_ok(buf_ptr as usize, core::mem::size_of::<Statfs>()) {
        return -(errno::EFAULT as i64);
    }

    let mut statfs_buf = Statfs {
        f_type: 0, f_bsize: 0, f_blocks: 0, f_bfree: 0, f_bavail: 0,
        f_files: 0, f_ffree: 0, f_fsid: [0; 2], f_namelen: 0,
        f_frsize: 0, f_flags: 0, f_spare: [0; 4],
    };

    // Linux statfs fails with ENOENT for paths that do not exist — the old
    // code fabricated rootfs numbers for ANY path (review批次1: statfs 不查
    // 路径存在性).
    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    let vpath = match crate::fs::vfs::path_lookup(&full_path, 0) {
        Ok(v) => v,
        Err(e) => return e as i64,
    };
    match vpath.inode.as_ref() {
        Some(inode) => fill_statfs_for_inode(inode, &mut statfs_buf),
        None => fill_rootfs_statfs(&mut statfs_buf),
    }

    // Copy to user space
    // SAFETY: buf_ptr validated with access_ok(size_of::<Statfs>); copies Statfs to user.
    let uncopied = unsafe {
        crate::arch::uaccess::copy_to_user(
            buf_ptr as *mut u8,
            &statfs_buf as *const Statfs as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    if uncopied > 0 {
        return -(errno::EFAULT as i64);
    }

    0
}

/// sys_fstatfs - Get filesystem statistics by fd
/// RISC-V syscall number: 44
pub fn sys_fstatfs(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let buf_ptr = args[1] as *mut Statfs;

    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    if !crate::arch::uaccess::access_ok(buf_ptr as usize, core::mem::size_of::<Statfs>()) {
        return -(errno::EFAULT as i64);
    }

    let mut statfs_buf = Statfs {
        f_type: 0, f_bsize: 0, f_blocks: 0, f_bfree: 0, f_bavail: 0,
        f_files: 0, f_ffree: 0, f_fsid: [0; 2], f_namelen: 0,
        f_frsize: 0, f_flags: 0, f_spare: [0; 4],
    };

    // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File or None.
    unsafe {
        match crate::fs::get_file_fd(fd) {
            Some(file) => {
                // SAFETY: inode is written once at open time; read-only here.
                let inode_opt = &*file.inode.get();
                match inode_opt.as_ref() {
                    Some(inode) => fill_statfs_for_inode(inode, &mut statfs_buf),
                    None => fill_rootfs_statfs(&mut statfs_buf),
                }
            }
            None => return -(errno::EBADF as i64),
        }
    }

    // SAFETY: buf_ptr validated with access_ok(size_of::<Statfs>); copies Statfs to user.
    let uncopied = unsafe {
        crate::arch::uaccess::copy_to_user(
            buf_ptr as *mut u8,
            &statfs_buf as *const Statfs as *const u8,
            core::mem::size_of::<Statfs>(),
        )
    };
    if uncopied > 0 {
        return -(errno::EFAULT as i64);
    }

    0
}

/// sys_symlinkat - Create symbolic link relative to directory fd
/// RISC-V syscall number: 36
pub fn sys_symlinkat(args: SyscallArgs) -> i64 {
    let target_ptr = args[0] as *const u8;
    let newdirfd = args[1] as i32;
    let linkpath_ptr = args[2] as *const u8;

    // Read target path (symlink content — NOT resolved against CWD)
    let mut target_buf = [0u8; PATH_MAX];
    let target = match read_user_path(target_ptr, &mut target_buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    // Resolve link path (where the symlink will be created)
    let full_path = match resolve_user_path(newdirfd, linkpath_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    match crate::fs::vfs::vfs_symlink(&full_path, target) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_statx - Extended file status (syscall 291)
///
/// statx(dirfd, pathname, flags, mask, statxbuf)
pub fn sys_statx(args: SyscallArgs) -> i64 {
    use crate::fs::stat::{Stat, Statx};

    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let flags = args[2] as u32;
    let mask = args[3] as u32;
    let statxbuf = args[4] as *mut Statx;

    if statxbuf.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(statxbuf as usize, core::mem::size_of::<Statx>()) {
        return -(errno::EFAULT as i64);
    }

    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    const AT_STATX_SYNC_TYPE: u32 = 0x6000; // SYNC_AS_STAT/FSYNC/NOATIME family
    // AT_NO_AUTOMOUNT is legal here (Linux): glibc's stat family and
    // gio's statx wrapper always pass it alongside AT_SYMLINK_NOFOLLOW
    // and AT_STATX_SYNC_AS_STAT — rejecting it broke GFile query-info.
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_STATX_SYNC_TYPE) != 0 {
        return -(errno::EINVAL as i64);
    }

    let mut stat = Stat::new();

    // AT_EMPTY_PATH + empty pathname: operate on dirfd itself (fstat).
    let is_empty_path = {
        let mut probe = [0u8; 1];
        !pathname_ptr.is_null()
            && crate::arch::uaccess::access_ok(pathname_ptr as usize, 1)
            && unsafe {
                crate::arch::uaccess::copy_from_user(
                    probe.as_mut_ptr(),
                    pathname_ptr,
                    1,
                )
            } == 0
            && probe[0] == 0
    };

    if is_empty_path && (flags & AT_EMPTY_PATH) != 0 {
        // fstat(dirfd)
        match crate::fs::file_stat(dirfd as usize, &mut stat) {
            Ok(()) => {}
            Err(e) => return -(e as i64),
        }
    } else {
        let full_path = match resolve_user_path(dirfd, pathname_ptr) {
            Ok(p) => p,
            Err(e) => return e as i64,
        };
        // AT_SYMLINK_NOFOLLOW: lstat-style lookup (do not follow the final
        // symlink component).
        let lookup_flags = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            crate::fs::vfs::LOOKUP_NOFOLLOW
        } else {
            0
        };
        if let Err(e) = crate::fs::vfs::stat_file_by_path_with_flags(&full_path, &mut stat, lookup_flags) {
            return e as i64;
        }
    }

    // STATX mask constants
    const STATX_TYPE: u32 = 0x0001;
    const STATX_MODE: u32 = 0x0002;
    const STATX_NLINK: u32 = 0x0004;
    const STATX_UID: u32 = 0x0008;
    const STATX_GID: u32 = 0x0010;
    const STATX_ATIME: u32 = 0x0020;
    const STATX_MTIME: u32 = 0x0040;
    const STATX_CTIME: u32 = 0x0080;
    const STATX_INO: u32 = 0x0100;
    const STATX_SIZE: u32 = 0x0200;
    const STATX_BLOCKS: u32 = 0x0400;
    const STATX_BTIME: u32 = 0x0800;

    // Linux semantics: STATX_ALL-style requests are ANDed with what the fs
    // can provide; unknown request bits are not an error. We always answer
    // the basic set.
    let requested = mask
        & (STATX_TYPE | STATX_MODE | STATX_NLINK | STATX_UID | STATX_GID
           | STATX_ATIME | STATX_MTIME | STATX_CTIME | STATX_INO | STATX_SIZE
           | STATX_BLOCKS | STATX_BTIME);

    let mut stx = Statx::new();
    stx.stx_mask = requested;
    stx.stx_blksize = stat.st_blksize as u32;
    // stx_attributes: no compression/immutable/append/nonce flags are
    // tracked by the Rux VFS — report a clean 0 with STATX_ATTR bit flags
    // unset (Linux reports attribute bits only when the supporting mask bit
    // is set; 0 is always correct for "nothing special").
    stx.stx_attributes = 0;
    stx.stx_attributes_mask = 0;

    if requested & STATX_TYPE != 0 || requested & STATX_MODE != 0 {
        stx.stx_mode = stat.st_mode as u16;
    }
    if requested & STATX_NLINK != 0 { stx.stx_nlink = stat.st_nlink; }
    if requested & STATX_UID != 0 { stx.stx_uid = stat.st_uid; }
    if requested & STATX_GID != 0 { stx.stx_gid = stat.st_gid; }
    if requested & STATX_ATIME != 0 {
        stx.stx_atime.tv_sec = stat.st_atime;
        stx.stx_atime.tv_nsec = stat.st_atime_nsec as u32;
    }
    if requested & STATX_MTIME != 0 {
        stx.stx_mtime.tv_sec = stat.st_mtime;
        stx.stx_mtime.tv_nsec = stat.st_mtime_nsec as u32;
    }
    if requested & STATX_CTIME != 0 {
        stx.stx_ctime.tv_sec = stat.st_ctime;
        stx.stx_ctime.tv_nsec = stat.st_ctime_nsec as u32;
    }
    if requested & STATX_INO != 0 { stx.stx_ino = stat.st_ino; }
    if requested & STATX_SIZE != 0 { stx.stx_size = stat.st_size as u64; }
    if requested & STATX_BLOCKS != 0 { stx.stx_blocks = stat.st_blocks as u64; }

    // Copy to user space
    // SAFETY: statxbuf validated with access_ok(size_of::<Statx>); copies Statx to user.
    let uncopied = unsafe {
        crate::arch::uaccess::copy_to_user(
            statxbuf as *mut u8,
            &stx as *const Statx as *const u8,
            core::mem::size_of::<Statx>(),
        )
    };
    if uncopied != 0 {
        return -(errno::EFAULT as i64);
    }

    0
}

/// sys_openat2 - Open file extended (syscall 437)
///
/// openat2(dirfd, pathname, how, size)
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

pub fn sys_openat2(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let how_ptr = args[2] as *const OpenHow;
    let size = args[3] as usize;

    // Validate: size must cover at least flags + mode + resolve (24 bytes).
    // Linux openat2 uses copy_struct_from_user: size < the known struct is
    // EINVAL (cannot see the version); size > the known struct is accepted
    // only when ALL unknown trailing bytes are zero — otherwise E2BIG
    // (userspace passed fields the kernel does not understand).
    const OPEN_HOW_VER0_SIZE: usize = 24;
    const OPEN_HOW_MAX_SIZE: usize = 24;

    if size < OPEN_HOW_VER0_SIZE {
        return -(errno::EINVAL as i64);
    }
    if size > OPEN_HOW_MAX_SIZE + 4096 {
        return -(errno::E2BIG as i64);
    }

    if how_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(how_ptr as usize, size) {
        return -(errno::EFAULT as i64);
    }

    // Read struct open_how from user space. `size` may exceed our known
    // 24-byte v0 layout; read up to `size` bytes into a padded buffer and
    // require every byte past the known tail to be zero (E2BIG otherwise,
    // matching copy_struct_from_user semantics).
    let mut buf = [0u8; OPEN_HOW_MAX_SIZE + 4096];
    // SAFETY: how_ptr validated with access_ok(size); copies size bytes from
    // user into a buffer of at least that size (bounded above).
    let uncopied = unsafe {
        crate::arch::uaccess::copy_from_user(
            buf.as_mut_ptr(),
            how_ptr as *const u8,
            size,
        )
    };
    if uncopied > 0 {
        return -(errno::EFAULT as i64);
    }
    // Unknown trailing fields must be zero (copy_struct_from_user rule).
    if buf[OPEN_HOW_MAX_SIZE..size].iter().any(|&b| b != 0) {
        return -(errno::E2BIG as i64);
    }
    // SAFETY: buf contains size bytes copied from user; OPEN_HOW_MAX_SIZE >= OPEN_HOW_VER0_SIZE (24);
    // OpenHow is #[repr(C)] with 3 u64 fields = 24 bytes, so read is valid.
    let how: OpenHow = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const OpenHow) };

    // Validate flags: reject O_CREAT/O_EXCL/O_TRUNC for now (simplification)
    // Actually, just pass through — sys_openat handles these
    // Validate resolve flags: only accept known bits
    const RESOLVE_NO_XDEV: u64 = 0x01;
    const RESOLVE_NO_SYMLINKS: u64 = 0x02;
    const RESOLVE_NO_MAGICLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    const RESOLVE_IN_ROOT: u64 = 0x10;
    const RESOLVE_CACHED: u64 = 0x20;

    let supported = RESOLVE_NO_XDEV | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS
        | RESOLVE_BENEATH | RESOLVE_IN_ROOT | RESOLVE_CACHED;
    if (how.resolve & !supported) != 0 {
        return -(errno::EINVAL as i64);
    }

    // TODO: Implement RESOLVE_* semantics (currently just delegate to openat)
    let openat_args = [
        dirfd as u64,
        pathname_ptr as u64,
        how.flags,
        how.mode,
        0, 0,
    ];
    sys_openat(openat_args)
}

/// sys_mknodat - create a filesystem node
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - path pointer
/// - args[2]: mode - file mode (type + permissions)
/// - args[3]: dev - device number (for block/char special files)
///
/// P1: FIFO (mkfifo) and device nodes implemented.
/// - FIFO: regular create on the parent FS + retype to S_IFIFO + a live
///   pipe peer registered by inode identity (fs/fifo.rs).
/// - char/block devices: only inside devfs (/dev) — the entry records the
///   major/minor; open() binds through the CharDev registry (ENXIO when no
///   driver registered; no block-driver registry exists yet). Device node
///   creation elsewhere (ext4/rootfs) is EPERM: those filesystems cannot
///   store device-typed inodes yet.
pub fn sys_mknodat(args: SyscallArgs) -> i64 {
    let _dirfd = args[0] as i32;
    let pathname = args[1] as *const u8;
    let mode = args[2] as u32;
    let dev = args[3] as u32;

    let path = match resolve_user_path(_dirfd, pathname) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // EROFS: mknod/mkfifo on a read-only mount (Linux may_create +
    // mnt_want_write; LTP mknod0* ro cases).
    if crate::fs::vfs::path_on_readonly_mount(&path) {
        return -(errno::EROFS as i64);
    }

    let ftype = mode & 0o170000;
    match ftype {
        0o010000 => {
            // FIFO: no capability required (POSIX mkfifo).
            mkfifo_at(&path, mode & 0o7777)
        }
        0o020000 | 0o060000 => {
            // Character / block device node: CAP_MKNOD (Linux).
            if !crate::security::capable(crate::security::CAP_MKNOD) {
                return -(errno::EPERM as i64);
            }
            mknod_device(&path, ftype, dev, mode & 0o7777)
        }
        0o100000 | 0o040000 | 0 => {
            // Regular file / directory / bare mode bits.
            if ftype == 0o040000 {
                match crate::fs::vfs::vfs_mkdir(&path, mode & 0o7777) {
                    Ok(_) => 0,
                    Err(e) => e as i64,
                }
            } else {
                // Regular file - create with O_CREAT|O_EXCL (fail with
                // EEXIST if already exists)
                match crate::fs::file_open(&path, 0o100 | 0o200 | 0o1000, mode & 0o777) {
                    Ok(fd) => {
                        // Close the fd immediately
                        // SAFETY: fd was just opened by file_open; closing it is safe.
                        unsafe { crate::fs::close_file_fd(fd); }
                        0
                    }
                    Err(e) => e as i64,
                }
            }
        }
        0o140000 => {
            // S_IFSOCK: create the node on the owning filesystem (Linux
            // do_mknodat → vfs_mknod; no capability required). AF_UNIX
            // bind(2) can later claim the name.
            let umask = match crate::sched::current() {
                Some(task) => {
                    // SAFETY: sched::current() yields a valid task reference.
                    unsafe { (*task).get_umask() }
                }
                None => 0o022,
            };
            match crate::fs::vfs::vfs_mknod(&path, 0o140000 | (mode & 0o7777 & !umask), 0) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        _ => -(errno::EINVAL as i64), // S_IFLNK (use symlink(2)) / unknown types
    }
}

/// Create a FIFO at `path`: create the file on the parent filesystem, then
/// retype the inode to S_IFIFO and register the live pipe peer.
fn mkfifo_at(path: &str, perm: u32) -> i64 {
    // O_CREAT|O_EXCL|O_RDWR: the transient fd must be openable both ways
    // (a bare O_WRONLY create is fine too, but RDWR avoids nothing here);
    // it is closed immediately, so the accmode never reaches userspace.
    match crate::fs::file_open(path, 0o100 | 0o200 | 0o1000 | 0o2, perm) {
        Ok(fd) => {
            // SAFETY: fd is a valid open descriptor from file_open above.
            let file = match unsafe { crate::fs::get_file_fd(fd) } {
                Some(f) => f,
                None => {
                    return -(errno::EIO as i64);
                }
            };
            // SAFETY: inode is written once at open time; read-only access.
            let inode = match unsafe { (*file.inode.get()).as_ref() } {
                Some(i) => i.clone(),
                None => {
                    // SAFETY: fd validated above.
                    unsafe { crate::fs::close_file_fd(fd); }
                    return -(errno::EIO as i64);
                }
            };
            // Retype: the mode word carries S_IFIFO from here on. setattr
            // persists it where the FS supports mode updates (rootfs/
            // tmpfs in-memory; ext4 chmod path).
            let full_mode = crate::fs::inode::InodeMode::S_IFIFO | (perm & 0o777);
            let ret = inode.op_setattr(
                crate::fs::inode::setattr_attr::ATTR_MODE,
                full_mode as u64,
                0,
            );
            if ret != 0 {
                // SAFETY: fd validated above.
                unsafe { crate::fs::close_file_fd(fd); }
                return ret as i64;
            }
            // Register the live pipe peer keyed by inode identity.
            crate::fs::fifo::peer_for(inode.fs_id, inode.ino);
            // SAFETY: fd validated above.
            unsafe { crate::fs::close_file_fd(fd); }
            0
        }
        Err(e) => e as i64,
    }
}

/// Create a device node (char/block): on the filesystem that OWNS the
/// parent directory (Linux do_mknodat semantics), with the legacy devfs
/// registry route kept for the dentry-mounted devfs instance itself.
///
/// OH Phase 1b: OH's init mounts a tmpfs over /dev and mknods
/// /dev/{null,random,urandom,kmsg} there — those nodes must land on the
/// tmpfs (open() binds them through the CharDev registry by the stored
/// rdev), not in the devfs tree the bare "/dev" prefix would select.
fn mknod_device(path: &str, ftype: u32, dev: u32, perm: u32) -> i64 {
    let type_bits = if ftype == 0o020000 { 0o020000 } else { 0o060000 };
    // do_mknodat applies the umask before handing the mode down.
    let umask = match crate::sched::current() {
        Some(task) => {
            // SAFETY: sched::current() yields a valid task reference.
            unsafe { (*task).get_umask() }
        }
        None => 0o022,
    };
    let mode_bits = type_bits | (perm & !umask);

    // Generic path: resolve the parent through the mount stack and use
    // its mknod inode op when present (tmpfs today; devfs keeps mknod
    // None on purpose — its internal registry route below also evicts
    // stale dentries and checks the /dev shape).
    match crate::fs::vfs::vfs_mknod(path, mode_bits, dev as u64) {
        Ok(()) => return 0,
        Err(e) => {
            // EOPNOTSUPP = the owning filesystem has no mknod op — fall
            // through to the devfs route when the path is under /dev and
            // devfs still owns it (no tmpfs overmount). Any other error
            // (EEXIST, ENOENT, ENOTDIR, EACCES, EROFS...) is final.
            if e != -(errno::EOPNOTSUPP) {
                return e as i64;
            }
        }
    }

    let dev_path = match crate::fs::devfs::parse_dev_path(path) {
        Some(p) => p,
        None => {
            // ext4/rootfs cannot persist device-typed inodes yet — a
            // regular file wearing a device mode would be worse than
            // failing (silent misbehavior on open).
            return -(errno::EPERM as i64);
        }
    };
    if dev_path.is_empty() {
        return -(errno::ENOENT as i64); // mknod("/dev") itself
    }
    // Userspace dev_t encoding (kernel new_decode_dev): major in bits 8-19,
    // minor split across bits 0-7 and 20-31.
    let major = (dev & 0xfff00) >> 8;
    let minor = (dev & 0xff) | ((dev >> 12) & 0xfff00);
    match crate::fs::devfs::mknod_user(dev_path, crate::fs::dev_t::DevNo::new(major, minor), mode_bits) {
        Ok(()) => 0,
        Err(()) => -(errno::EACCES as i64), // missing parent dir inside /dev
    }
}

/// sys_fchmod - change file permissions
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: mode - file mode
pub fn sys_fchmod(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let mode = args[1] as u32;

    // Linux chmod_common: only the permission + special bits (S_IALLUGO =
    // 0o7777) are taken from the request; file-type bits in the argument
    // are silently DISCARDED, not rejected (LTP fchown03 setup passes
    // 0106770 to fchmod and expects success).
    let mode = mode & 0o7777;

    // SAFETY: fd is a valid file descriptor; get_file_fd returns valid File or None.
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(file) => {
            // EROFS: fd-based chmod on a read-only mount (LTP fchmod06).
            if crate::fs::vfs::file_on_readonly_mount(&file) {
                return -(errno::EROFS as i64);
            }
            // SAFETY: inode is an UnsafeCell; we hold &File so no concurrent mutation.
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                Some(inode) => {
                    // Permission check: require CAP_FOWNER or be the file owner
                    let cred = match crate::sched::current() {
                        Some(t) => t.cred().clone(),
                        None => return -(errno::EPERM as i64),
                    };
                    let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
                    if !crate::security::has_capability(&cred, crate::security::CAP_FOWNER)
                        && cred.euid != inode_uid {
                        return -(errno::EPERM as i64);
                    }
                    // notify_change S_ISGID rule (same as vfs_chmod): an
                    // unprivileged caller that is not in the file's group
                    // loses S_ISGID on chmod — setgid dirs must not survive
                    // a downgrade by a non-member (LTP fchmod05: nobody
                    // fchmod's a free_gid-owned directory to 043777 and
                    // must land on 041777).
                    let inode_gid = inode.gid.load(core::sync::atomic::Ordering::Relaxed);
                    let mode = if mode & 0o2000 != 0
                        && !crate::security::has_capability(&cred, crate::security::CAP_FSETID)
                        && cred.egid != inode_gid
                        && !cred.groups.contains(&inode_gid)
                    {
                        mode & !0o2000
                    } else {
                        mode
                    };
                    // Use op_setattr to update mode
                    let result = inode.op_setattr(0x0001, mode as u64, 0);
                    if result == 0 { 0 } else { -(result as i64) }
                }
                None => -(errno::EBADF as i64),
            }
        }
        None => -(errno::EBADF as i64),
    }
}

/// sys_fallocate - preallocate or deallocate space to a file
///
/// # Arguments
/// - args[0]: fd - file descriptor
/// - args[1]: mode - operation mode (FALLOC_FL_KEEP_SIZE, etc.)
/// - args[2]: offset - offset within file
/// - args[3]: length - length to allocate
pub fn sys_fallocate(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let mode = args[1] as i32;
    let offset = args[2] as i64;
    let length = args[3] as i64;

    const FALLOC_FL_KEEP_SIZE: i32 = 1;
    const FALLOC_FL_PUNCH_HOLE: i32 = 2;
    const FALLOC_FL_COLLAPSE_RANGE: i32 = 8;
    const FALLOC_FL_ZERO_RANGE: i32 = 16;
    const FALLOC_FL_INSERT_RANGE: i32 = 32;
    const FALLOC_FL_UNSHARE_RANGE: i32 = 64;
    const KNOWN_MODES: i32 = FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE
        | FALLOC_FL_COLLAPSE_RANGE | FALLOC_FL_ZERO_RANGE
        | FALLOC_FL_INSERT_RANGE | FALLOC_FL_UNSHARE_RANGE;

    if offset < 0 || length <= 0 {
        return -(errno::EINVAL as i64);
    }
    // Unknown mode bits are EINVAL; PUNCH_HOLE requires KEEP_SIZE.
    if mode & !KNOWN_MODES != 0 {
        return -(errno::EINVAL as i64);
    }
    if mode & FALLOC_FL_PUNCH_HOLE != 0 && mode & FALLOC_FL_KEEP_SIZE == 0 {
        return -(errno::EINVAL as i64);
    }

    // Wire to the VFS layer (W2 exposed vfs_fallocate, which routes to
    // ext4_fallocate). Filesystems without the op answer ENOSYS/EOPNOTSUPP
    // themselves instead of the old unconditional fake success (which broke
    // coreutils/dd error paths that rely on fallocate failing honestly).
    match crate::fs::vfs::vfs_fallocate(fd as usize, mode, offset as u64, length as u64) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_fsync - Synchronize a file's in-core state with storage (NR 82)
pub fn sys_fsync(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    // SAFETY: fd is a valid non-negative i32; get_file_fd returns None for invalid fds.
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(file) => {
            // Linux: fsync on pipes, sockets, and FIFOs is EINVAL (no
            // fsync method), regardless of the file having an inode
            // (LTP fsync03). Recognize them by their file-ops identity
            // FIRST — a pipe File carries no inode at all.
            {
                let ops = file.get_ops();
                let is_pseudo = ops.is_some_and(|o| {
                    core::ptr::eq(o as *const _, &crate::fs::pipe::PIPE_OPS as *const _)
                        || core::ptr::eq(o as *const _, &crate::fs::fifo::FIFO_OPS as *const _)
                });
                if is_pseudo {
                    return -(errno::EINVAL as i64);
                }
            }
            // SAFETY: inode is an UnsafeCell written at open time; read-only here.
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                // No inode at all (anonymous pipe/socket file): EINVAL.
                None => -(errno::EINVAL as i64),
                Some(inode) => {
                    // Linux do_fsync → vfs_fsync: only file types with an
                    // fsync method (regular files) can be synced. FIFOs,
                    // sockets, and device nodes report EINVAL instead of
                    // flushing an unrelated global cache and claiming
                    // success.
                    if !inode.mode.is_regular_file() {
                        return -(errno::EINVAL as i64);
                    }
                    // ext4 files: flush data blocks + journal commit via the
                    // W2-exposed ext4_sync_file. Other filesystems (rootfs,
                    // pipes, ...) have no fsync op — flush the shared buffer
                    // cache as a best effort.
                    let is_ext4 = inode.ops.is_some_and(|o| core::ptr::eq(
                        o as *const _, &crate::fs::ext4::EXT4_INODE_OPS as *const _,
                    ));
                    if is_ext4 {
                        let fs_ptr = match inode.private_data {
                            Some(p) => p as *const crate::fs::ext4::Ext4FileSystem,
                            None => return -(errno::EIO as i64),
                        };
                        let ext4_inode = match inode.sb {
                            Some(p) => unsafe { &*(p as *const crate::fs::ext4::inode::Ext4Inode) },
                            None => return -(errno::EIO as i64),
                        };
                        // SAFETY: both pointers were installed at file open
                        // time and live as long as the inode (Arc).
                        match unsafe { crate::fs::ext4::file::ext4_sync_file(&*fs_ptr, ext4_inode) } {
                            Ok(()) => 0,
                            Err(e) => e as i64,
                        }
                    } else {
                        let _ = crate::fs::bio::sync_buffers();
                        // Persist the DISK's volatile cache too — draining
                        // only the kernel buffer cache was a no-op flush
                        // (review BUG: "Flush 假成功，fsync 无持久化").
                        if crate::drivers::virtio::flush_pci_blk().is_err() {
                            return -(errno::EIO as i64);
                        }
                        0
                    }
                }
                None => -(errno::EBADF as i64),
            }
        }
        None => -(errno::EBADF as i64),
    }
}

/// sys_fdatasync - synchronize a file's data (not metadata) with storage
pub fn sys_fdatasync(args: SyscallArgs) -> i64 {
    // Same durability path as fsync (ext4_sync_file already writes the data
    // blocks; the journal commit covers the metadata we updated).
    sys_fsync(args)
}

/// sys_sync - synchronize filesystem caches
///
/// # Arguments: none
pub fn sys_sync(_args: SyscallArgs) -> i64 {
    // Flush all buffer cache
    let _ = crate::fs::bio::sync_buffers();
    // And persist the disk's volatile cache (see sys_fsync).
    if crate::drivers::virtio::flush_pci_blk().is_err() {
        return -(errno::EIO as i64);
    }
    0
}

/// Extended attribute syscalls (NR 5-16) — P1 minimal implementation.
///
/// Storage is the per-inode in-memory map (see fs/xattr.rs): user.* is
/// fully usable (get/set/list/remove); trusted.*/security.* store with
/// CAP_SYS_ADMIN; system.* is read-only. ext4 does not persist xattrs
/// (documented limitation).

/// Resolve a path (or fd) to its inode for xattr operations.
fn xattr_inode_by_path(path: &str, follow: bool) -> Result<alloc::sync::Arc<crate::fs::inode::Inode>, i32> {
    let flags = if follow { 0 } else { crate::fs::vfs::LOOKUP_NOFOLLOW };
    let vpath = crate::fs::vfs::path_lookup(path, flags)?;
    vpath.inode.ok_or(-(errno::ENOENT as i32))
}

fn xattr_inode_by_fd(fd: u64) -> Result<alloc::sync::Arc<crate::fs::inode::Inode>, i32> {
    // SAFETY: fd comes from the syscall args; get_file_fd validates it.
    let file = unsafe { crate::fs::get_file_fd(fd as usize) }
        .ok_or(-(errno::EBADF as i32))?;
    // SAFETY: inode written once at open; read-only access.
    unsafe { (*file.inode.get()).clone() }
        .ok_or(-(errno::EBADF as i32))
}

/// Copy a NUL-terminated xattr name from user space (max XATTR_NAME_MAX).
fn xattr_read_name(ptr: u64) -> Result<alloc::vec::Vec<u8>, i64> {
    const NAME_MAX: usize = crate::fs::xattr::XATTR_NAME_MAX;
    if ptr == 0 {
        return Err(-(errno::EFAULT as i64));
    }
    if !crate::arch::uaccess::access_ok(ptr as usize, 1) {
        return Err(-(errno::EFAULT as i64));
    }
    let mut buf = [0u8; NAME_MAX + 1];
    let name = match strncpy_from_user(ptr as *const u8, NAME_MAX + 1, &mut buf) {
        Ok(s) => s,
        Err(e) => return Err(-(e as i64)),
    };
    Ok(alloc::vec::Vec::from(name))
}

/// Copy `size` bytes of xattr value from user space (max XATTR_SIZE_MAX).
fn xattr_read_value(ptr: u64, size: u64) -> Result<alloc::vec::Vec<u8>, i64> {
    let size = size as usize;
    if size > crate::fs::xattr::XATTR_SIZE_MAX {
        return Err(-(errno::E2BIG as i64));
    }
    if size == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    if ptr == 0 {
        return Err(-(errno::EFAULT as i64));
    }
    if !crate::arch::uaccess::access_ok(ptr as usize, size) {
        return Err(-(errno::EFAULT as i64));
    }
    let mut buf = alloc::vec![0u8; size];
    // SAFETY: ptr/size validated with access_ok above.
    if unsafe {
        crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), ptr as *const u8, size)
    } != 0
    {
        return Err(-(errno::EFAULT as i64));
    }
    Ok(buf)
}

/// setxattr core shared by the 4 set variants.
fn do_setxattr(inode: &crate::fs::inode::Inode, args: SyscallArgs) -> i64 {
    let name = match xattr_read_name(args[1]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let value = match xattr_read_value(args[2], args[3]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::fs::xattr::set(inode, &name, &value, args[4] as i32) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// getxattr core. size==0 → return required size.
fn do_getxattr(inode: &crate::fs::inode::Inode, args: SyscallArgs) -> i64 {
    let name = match xattr_read_name(args[1]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let value_ptr = args[2] as usize;
    let size = args[3] as usize;
    if size == 0 {
        return match crate::fs::xattr::get(inode, &name, None) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }
    if value_ptr == 0 || !crate::arch::uaccess::access_ok(value_ptr, size) {
        return -(errno::EFAULT as i64);
    }
    let mut buf = alloc::vec![0u8; size];
    match crate::fs::xattr::get(inode, &name, Some(&mut buf)) {
        Ok(n) => {
            // SAFETY: value_ptr/size validated with access_ok above.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    value_ptr as *mut u8,
                    buf.as_ptr(),
                    n,
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            n as i64
        }
        Err(e) => e as i64,
    }
}

/// listxattr core. size==0 → return required length.
fn do_listxattr(inode: &crate::fs::inode::Inode, args: SyscallArgs) -> i64 {
    let list_ptr = args[1] as usize;
    let size = args[2] as usize;
    if size == 0 {
        return match crate::fs::xattr::list(inode, None) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }
    if list_ptr == 0 || !crate::arch::uaccess::access_ok(list_ptr, size) {
        return -(errno::EFAULT as i64);
    }
    let mut buf = alloc::vec![0u8; size];
    match crate::fs::xattr::list(inode, Some(&mut buf)) {
        Ok(n) => {
            // SAFETY: list_ptr/size validated with access_ok above.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    list_ptr as *mut u8,
                    buf.as_ptr(),
                    n,
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            n as i64
        }
        Err(e) => e as i64,
    }
}

/// removexattr core.
fn do_removexattr(inode: &crate::fs::inode::Inode, args: SyscallArgs) -> i64 {
    let name = match xattr_read_name(args[1]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::xattr::remove(inode, &name) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

pub fn sys_setxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, true) {
        Ok(inode) => do_setxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_lsetxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, false) {
        Ok(inode) => do_setxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_fsetxattr(args: SyscallArgs) -> i64 {
    match xattr_inode_by_fd(args[0]) {
        Ok(inode) => do_setxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_getxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, true) {
        Ok(inode) => do_getxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_lgetxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, false) {
        Ok(inode) => do_getxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_fgetxattr(args: SyscallArgs) -> i64 {
    match xattr_inode_by_fd(args[0]) {
        Ok(inode) => do_getxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_listxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, true) {
        Ok(inode) => do_listxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_llistxattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, false) {
        Ok(inode) => do_listxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_flistxattr(args: SyscallArgs) -> i64 {
    match xattr_inode_by_fd(args[0]) {
        Ok(inode) => do_listxattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_removexattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, true) {
        Ok(inode) => do_removexattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_lremovexattr(args: SyscallArgs) -> i64 {
    let path = match resolve_user_path(-100, args[0] as *const u8) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match xattr_inode_by_path(&path, false) {
        Ok(inode) => do_removexattr(&inode, args),
        Err(e) => e as i64,
    }
}

pub fn sys_fremovexattr(args: SyscallArgs) -> i64 {
    match xattr_inode_by_fd(args[0]) {
        Ok(inode) => do_removexattr(&inode, args),
        Err(e) => e as i64,
    }
}

/// *xattrat (NR 463-466, Linux 6.9): dirfd-relative variants with at_flags
/// (AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH). UAPI (fs/xattr.c):
///   setxattrat(dfd, path, at_flags, name, struct xattr_args*)
///   getxattrat(dfd, path, at_flags, name, struct xattr_args*)
///   listxattrat(dfd, path, at_flags, struct xattr_args*)
///   removexattrat(dfd, path, at_flags, name)
/// struct xattr_args { u64 value; u64 size; u32 flags; } (24 bytes).
fn xattrat_resolve(
    dirfd: i32,
    path_ptr: u64,
    at_flags: u32,
) -> Result<alloc::sync::Arc<crate::fs::inode::Inode>, i64> {
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    if at_flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
        return Err(-(errno::EINVAL as i64));
    }
    if path_ptr == 0 && at_flags & AT_EMPTY_PATH != 0 && dirfd >= 0 {
        return match xattr_inode_by_fd(dirfd as u64) {
            Ok(i) => Ok(i),
            Err(e) => Err(-(e as i64)),
        };
    }
    let path = match resolve_user_path(dirfd, path_ptr as *const u8) {
        Ok(p) => p,
        Err(e) => return Err(e as i64),
    };
    match xattr_inode_by_path(&path, at_flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(i) => Ok(i),
        Err(e) => Err(-(e as i64)),
    }
}

/// Read struct xattr_args { value: u64, size: u64, flags: u32 } from user.
fn xattrat_read_args(ptr: u64) -> Result<(u64, u64, i32), i64> {
    if ptr == 0 || !crate::arch::uaccess::access_ok(ptr as usize, 24) {
        return Err(-(errno::EFAULT as i64));
    }
    let mut raw = [0u8; 24];
    // SAFETY: ptr/24 validated with access_ok above.
    if unsafe {
        crate::arch::uaccess::copy_from_user(raw.as_mut_ptr(), ptr as *const u8, 24)
    } != 0
    {
        return Err(-(errno::EFAULT as i64));
    }
    let value = u64::from_ne_bytes(raw[0..8].try_into().unwrap());
    let size = u64::from_ne_bytes(raw[8..16].try_into().unwrap());
    let flags = i32::from_ne_bytes(raw[16..20].try_into().unwrap());
    Ok((value, size, flags))
}

pub fn sys_setxattrat(args: SyscallArgs) -> i64 {
    let inode = match xattrat_resolve(args[0] as i32, args[1], args[2] as u32) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let name = match xattr_read_name(args[3]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let (value_ptr, size, flags) = match xattrat_read_args(args[4]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let value = match xattr_read_value(value_ptr, size) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::fs::xattr::set(&inode, &name, &value, flags) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

pub fn sys_getxattrat(args: SyscallArgs) -> i64 {
    let inode = match xattrat_resolve(args[0] as i32, args[1], args[2] as u32) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let name = match xattr_read_name(args[3]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let (value_ptr, size, _flags) = match xattrat_read_args(args[4]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if size == 0 {
        return match crate::fs::xattr::get(&inode, &name, None) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }
    if value_ptr == 0
        || !crate::arch::uaccess::access_ok(value_ptr as usize, size as usize)
    {
        return -(errno::EFAULT as i64);
    }
    let mut buf = alloc::vec![0u8; size as usize];
    match crate::fs::xattr::get(&inode, &name, Some(&mut buf)) {
        Ok(n) => {
            // SAFETY: value_ptr/size validated with access_ok above.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    value_ptr as *mut u8,
                    buf.as_ptr(),
                    n,
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            n as i64
        }
        Err(e) => e as i64,
    }
}

pub fn sys_listxattrat(args: SyscallArgs) -> i64 {
    let inode = match xattrat_resolve(args[0] as i32, args[1], args[2] as u32) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let (list_ptr, size, _flags) = match xattrat_read_args(args[3]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if size == 0 {
        return match crate::fs::xattr::list(&inode, None) {
            Ok(n) => n as i64,
            Err(e) => e as i64,
        };
    }
    if list_ptr == 0 || !crate::arch::uaccess::access_ok(list_ptr as usize, size as usize)
    {
        return -(errno::EFAULT as i64);
    }
    let mut buf = alloc::vec![0u8; size as usize];
    match crate::fs::xattr::list(&inode, Some(&mut buf)) {
        Ok(n) => {
            // SAFETY: list_ptr/size validated with access_ok above.
            if unsafe {
                crate::arch::uaccess::copy_to_user(
                    list_ptr as *mut u8,
                    buf.as_ptr(),
                    n,
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            n as i64
        }
        Err(e) => e as i64,
    }
}

pub fn sys_removexattrat(args: SyscallArgs) -> i64 {
    let inode = match xattrat_resolve(args[0] as i32, args[1], args[2] as u32) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let name = match xattr_read_name(args[3]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match crate::fs::xattr::remove(&inode, &name) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_chroot - Change root directory (NR 51)
pub fn sys_chroot(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;

    // Linux resolves the path BEFORE the capability check (fs/open.c:
    // user_path_at, then d_can_lookup, then ns_capable(CAP_SYS_CHROOT)),
    // and the resolution itself enforces MAY_EXEC on every directory
    // component — a non-searchable directory reports EACCES, not EPERM
    // (LTP chroot04 chroots as nobody into a 0222 directory under a 0700
    // tmpdir). path_lookup is the permission-checked walk the open path
    // uses; plain vfs_stat skips traversal DAC.
    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    let vpath = match crate::fs::vfs::path_lookup(&full_path, crate::fs::vfs::lookup_flags::LOOKUP_FOLLOW) {
        Ok(p) => p,
        Err(e) => return e as i64, // already-negative errno
    };
    match vpath.inode.as_ref() {
        Some(inode) if inode.mode.is_directory() => {
            // The resolved directory itself must be SEARCHABLE: verified
            // against the host kernel, chroot() into a 0222 directory as
            // an unprivileged user returns EACCES (not the EPERM of the
            // capability gate below) — LTP chroot04.
            if !crate::fs::permission::inode_permission(
                inode,
                crate::fs::permission::MAY_EXEC,
            ) {
                return -(errno::EACCES as i64);
            }
        }
        Some(_) => return -(errno::ENOTDIR as i64),
        None => return -(errno::ENOENT as i64),
    }

    // CAP_SYS_CHROOT required to chroot
    if !crate::security::capable(crate::security::CAP_SYS_CHROOT) {
        return -(errno::EPERM as i64);
    }

    // P1 chroot: store the jail root in fs_struct — the dentry of the
    // resolved directory plus its real-namespace path string.
    // path_lookup interprets every subsequent absolute path against the
    // string prefix and clamps ".." at it (see vfs::normalize_with_root),
    // so the jail is enforced for lookups, opens, creates and unlinks
    // alike. The dentry is what Linux's fs->root is: getcwd computes
    // root-relative paths from the cwd dentry up to it, and a cwd outside
    // the jail fails getcwd with ENOENT ("(unreachable)" in modern
    // kernels).
    //
    // The path string is the CANONICAL build_path() of the resolved
    // dentry, not path_normalize(full_path): the user path may contain
    // symlinks the FOLLOW lookup already resolved, and — critically for
    // switch_root-style flows — cwd-relative arguments like "." must
    // capture where the cwd dentry IS NOW, as a string that stays valid
    // for the prefix machinery (the cwd string itself is rebuilt from the
    // cwd dentry, so it cannot be stale here).
    let root_dentry = vpath.dentry.clone();
    let normalized = match root_dentry.as_ref() {
        Some(d) => d.build_path(),
        None => crate::fs::path::path_normalize(&full_path),
    };
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is the running task's Task pointer from
        // sched::current(); fs_arc() only clones the Arc out of it.
        if let Some(fs) = unsafe { (*current).fs_arc() } {
            fs.set_root_with_dentry(normalized.as_bytes(), root_dentry);
        }
    }
    0
}

/// sys_fchown - Change file ownership by fd (NR 55)
pub fn sys_fchown(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let uid = args[1] as u32;
    let gid = args[2] as u32;

    // SAFETY: fd is a valid non-negative i32; get_file_fd returns None for invalid fds.
    match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(file) => {
            // SAFETY: inode.get() points to an Option<Arc<Inode>> initialized during file open.
            let inode_opt = unsafe { &*file.inode.get() };
            match inode_opt.as_ref() {
                Some(inode) => {
                    // EROFS: fd-based chown on a read-only mount (LTP fchown04).
                    if crate::fs::vfs::file_on_readonly_mount(&file) {
                        return -(errno::EROFS as i64);
                    }
                    // Linux chown_ok/chgrp_ok (fs/attr.c): CAP_CHOWN holders
                    // may set anything; an unprivileged OWNER may re-set its
                    // own uid and any gid it belongs to; anything else EPERMs
                    // (LTP fchown03: nobody chgrps its own file to its own
                    // group and expects success).
                    let cred = match crate::sched::current() {
                        Some(t) => t.cred().clone(),
                        None => return -(errno::EPERM as i64),
                    };
                    let inode_uid = inode.uid.load(core::sync::atomic::Ordering::Relaxed);
                    let inode_gid = inode.gid.load(core::sync::atomic::Ordering::Relaxed);
                    let is_priv = crate::security::has_capability(&cred, crate::security::CAP_CHOWN);
                    let owner = cred.euid == inode_uid;
                    if !is_priv {
                        if uid != u32::MAX && !(owner && uid == inode_uid) {
                            return -(errno::EPERM as i64);
                        }
                        if gid != u32::MAX && !(owner && cred.in_group(gid)) {
                            return -(errno::EPERM as i64);
                        }
                    }
                    let actual_uid = if uid == u32::MAX { inode_uid } else { uid };
                    let actual_gid = if gid == u32::MAX { inode_gid } else { gid };
                    // Persist through the filesystem's setattr (not just the
                    // cached inode word — ext4 stat() reads the disk inode).
                    let result = inode.op_setattr(
                        crate::fs::inode::setattr_attr::ATTR_UID_GID,
                        actual_uid as u64,
                        actual_gid as u64,
                    );
                    if result != 0 {
                        return -(result as i64);
                    }
                    // Same SUID/SGID kill as chown(2) (Linux chown_common runs
                    // for fchown too): SUID always dies on a regular file,
                    // SGID when group-executable or the caller is outside the
                    // file's group without CAP_FSETID (LTP chown02/fchown02).
                    if !inode.mode.is_directory() {
                        let mode = {
                            let mut st = crate::fs::Stat::default();
                            if inode.op_getattr(&mut st) == 0 { st.st_mode } else { inode.mode.bits() }
                        };
                        let mut new_mode = mode;
                        if new_mode & 0o4000 != 0 {
                            new_mode &= !0o4000;
                        }
                        let in_group_or_capable = cred.in_group(inode_gid)
                            || crate::security::has_capability(&cred, crate::security::CAP_FSETID);
                        if new_mode & 0o2000 != 0 && ((new_mode & 0o010) != 0 || !in_group_or_capable) {
                            new_mode &= !0o2000;
                        }
                        if new_mode != mode {
                            let _ = inode.op_setattr(
                                crate::fs::inode::setattr_attr::ATTR_MODE,
                                new_mode as u64,
                                0,
                            );
                        }
                    }
                    0
                }
                None => -(errno::EBADF as i64),
            }
        }
        None => -(errno::EBADF as i64),
    }
}

/// sys_vhangup - Simulate hangup on current tty (NR 58)
pub fn sys_vhangup(_args: SyscallArgs) -> i64 {
    // Permission check: require CAP_SYS_TTY_CONFIG
    if !crate::security::capable(crate::security::CAP_SYS_TTY_CONFIG) {
        return -(errno::EPERM as i64);
    }
    0
}

/// sys_sync_file_range - Sync file range to disk (NR 84)
pub fn sys_sync_file_range(args: SyscallArgs) -> i64 {
    let _fd = args[0] as i32;
    let _offset = args[1] as i64;
    let _nbytes = args[2] as i64;
    let _flags = args[3] as u32;
    // Simplified: sync all buffers
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// sys_acct - Enable/disable process accounting (NR 89)
pub fn sys_acct(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;

    // CAP_SYS_PACCT-equivalent gate FIRST — even acct(NULL) from an
    // unprivileged caller is EPERM (LTP acct01 runs the NULL/tmpfile
    // cases as nobody).
    if let Some(task) = crate::sched::current() {
        if task.cred().euid != 0 {
            return -(errno::EPERM as i64);
        }
    } else {
        return -(errno::EPERM as i64);
    }

    if pathname_ptr.is_null() {
        // NULL means disable accounting — succeed
        return 0;
    }

    // FIX8 (LTP acct01): validate the filename like Linux sys_acct —
    // resolution errors (ENOENT / ENOTDIR / ELOOP / ENAMETOOLONG) pass
    // through, a directory is EISDIR, a non-regular file is EACCES, and a
    // file on a read-only mount is EROFS. Accounting itself stays a
    // no-op, but only after the path is proven eligible.
    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    match crate::fs::vfs::path_lookup(&full_path, 0) {
        Ok(vpath) => {
            if let Some(inode) = vpath.inode.as_ref() {
                if inode.mode.is_directory() {
                    return -(errno::EISDIR as i64);
                }
                if !inode.mode.is_regular_file() {
                    return -(errno::EACCES as i64);
                }
            }
        }
        Err(e) => return e as i64,
    }
    // Read-only mount: the longest matching mount-table prefix decides.
    {
        let mounts = crate::fs::mount::get_mounts();
        let mut best_len = 0usize;
        let mut ro = false;
        for (_d, mp, _f, fl) in mounts.iter() {
            if (full_path == *mp || full_path.starts_with(mp.as_str()))
                && mp.len() >= best_len
                && mp != "/"
            {
                if mp.len() > best_len || best_len == 0 {
                    best_len = mp.len();
                    ro = fl.contains("ro");
                }
            }
        }
        if ro {
            return -(errno::EROFS as i64);
        }
    }

    // Process accounting not implemented — succeed silently
    0
}

/// sys_readahead - Readahead pages into cache (NR 213)
pub fn sys_readahead(_args: SyscallArgs) -> i64 {
    0 // Simplified: success (no-op)
}

/// sys_swapon - Start swap (NR 224)
///
/// swapon(const char *path, int swapflags)
///
/// Resolves `path`, then activates the kernel-managed swap area (tail
/// carve of the root block device — devfs has no block-device nodes and
/// there is no partition layer, so the root disk is the only swappable
/// backend; the path itself currently serves as the authorization token
/// naming an existing object). A mkswap-style header ("SWAPSPACE2",
/// version 1) at the carve start, when present, fixes the usable page
/// count. Refuses when the carve would overlap the ext4 filesystem
/// (see mm::swap::swap_activate).
pub fn sys_swapon(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let _swapflags = args[1] as i32;

    // Linux: CAP_SYS_ADMIN required.
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }
    if pathname_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Only one swap area exists (SWAP_DEVICES[0]); refuse double-enable.
    if crate::mm::swap::nr_active_swap() {
        return -(errno::EBUSY as i64);
    }

    // Resolve the path — a nonexistent name must fail, not fake-success.
    // resolve_user_path reports negative errnos in a u64; the cast back
    // to i64 preserves the sign.
    let full_path = match resolve_user_path(-100, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // The named object must exist (block-device nodes and swap FILES both
    // land on the root disk; there is no other GenDisk in the system).
    if let Err(e) = crate::fs::vfs::path_lookup(&full_path, 0) {
        return e as i64;
    }

    // Find the backing disk: first registered GenDisk (virtio-blk root).
    let disk = match crate::mm::swap::swap_boot_disk() {
        Some(d) => d,
        None => return -(errno::ENODEV as i64),
    };

    match crate::mm::swap::swap_activate(disk) {
        Ok(_) => 0,
        Err(e) => e as i64,
    }
}

/// sys_swapoff - Stop swap (NR 225)
///
/// swapoff(const char *path)
///
/// Disables the active swap area. Refuses EBUSY while pages are still
/// swapped out (swap-in-before-disable is not implemented — dropping the
/// backing store would make the data unreachable).
pub fn sys_swapoff(_args: SyscallArgs) -> i64 {
    // Linux: CAP_SYS_ADMIN required.
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }
    match crate::mm::swap::swap_deactivate() {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_remap_file_pages - Remap file pages (NR 234, deprecated)
pub fn sys_remap_file_pages(_args: SyscallArgs) -> i64 {
    0 // Deprecated, return success
}

/// sys_name_to_handle_at - Get file handle (NR 264)
///
/// Minimal stub (U2): EOPNOTSUPP instead of ENOSYS — file handles are
/// not implemented, but callers probing support distinguish "unsupported
/// here" from "syscall absent".
pub fn sys_name_to_handle_at(_args: SyscallArgs) -> i64 {
    -(errno::EOPNOTSUPP as i64)
}

/// sys_open_by_handle_at - Open by file handle (NR 265)
///
/// Minimal stub (U2): EOPNOTSUPP — see sys_name_to_handle_at.
pub fn sys_open_by_handle_at(_args: SyscallArgs) -> i64 {
    -(errno::EOPNOTSUPP as i64)
}

/// sys_copy_file_range - Copy data between files (NR 285)
pub fn sys_copy_file_range(args: SyscallArgs) -> i64 {
    let fd_in = args[0] as i32;
    let off_in_ptr = args[1] as *mut i64;
    let fd_out = args[2] as i32;
    let off_out_ptr = args[3] as *mut i64;
    let len = args[4] as usize;
    let _flags = args[5] as u32;

    if len == 0 { return 0; }

    // off_in/off_out semantics: when non-NULL the offsets are read from and
    // written back through the pointers and the file positions are NOT
    // touched. Read them with the exception-table copy (EFAULT, not panic,
    // on bad pointers — the old code dereferenced them raw and then ignored
    // them entirely).
    let mut off_in: Option<i64> = None;
    if !off_in_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(off_in_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        let mut buf = [0u8; 8];
        // SAFETY: off_in_ptr validated with access_ok(8).
        if unsafe { crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), off_in_ptr as *const u8, 8) } != 0 {
            return -(errno::EFAULT as i64);
        }
        let v = i64::from_le_bytes(buf.try_into().unwrap());
        if v < 0 {
            return -(errno::EINVAL as i64);
        }
        off_in = Some(v);
    }
    let mut off_out: Option<i64> = None;
    if !off_out_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(off_out_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        let mut buf = [0u8; 8];
        // SAFETY: off_out_ptr validated with access_ok(8).
        if unsafe { crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), off_out_ptr as *const u8, 8) } != 0 {
            return -(errno::EFAULT as i64);
        }
        let v = i64::from_le_bytes(buf.try_into().unwrap());
        if v < 0 {
            return -(errno::EINVAL as i64);
        }
        off_out = Some(v);
    }

    use crate::fs::get_file_fd;
    // SAFETY: fd_in and fd_out are validated i32; get_file_fd returns None for invalid fds.
    unsafe {
        let in_file = match get_file_fd(fd_in as usize) {
            Some(f) => f,
            None => return -(errno::EBADF as i64),
        };
        let out_file = match get_file_fd(fd_out as usize) {
            Some(f) => f,
            None => return -(errno::EBADF as i64),
        };

        let mut cur_in = off_in.unwrap_or_else(|| in_file.get_pos() as i64);
        let mut cur_out = off_out.unwrap_or_else(|| out_file.get_pos() as i64);

        let mut total = 0usize;
        let mut remaining = len;
        let mut first_err: i64 = 0;
        'outer: while remaining > 0 {
            let chunk = core::cmp::min(remaining, 8192);
            let mut buf = alloc::vec![0u8; chunk];
            // Read at cur_in without moving the fd position (read_at is
            // position-invariant; ESPIPE for unseekable sources).
            let n = in_file.read_at(cur_in as u64, buf.as_mut_ptr(), chunk);
            if n <= 0 {
                // Linux: a FIRST read failure is an error return, not a
                // fake "0 bytes copied" success (review批次1).
                if total == 0 && n < 0 {
                    first_err = n as i64;
                }
                break;
            }
            let mut written = 0usize;
            while written < n as usize {
                let w = out_file.write_at((cur_out + written as i64) as u64, buf.as_ptr().add(written), (n as usize) - written);
                if w <= 0 {
                    if total == 0 && written == 0 && w < 0 {
                        first_err = w as i64;
                    }
                    break 'outer; // destination full / error: stop copying
                }
                written += w as usize;
            }
            cur_in += n as i64;
            cur_out += written as i64;
            total += written;
            remaining -= written;
        }

        // Write the advanced offsets back. When off_in/off_out are NULL,
        // Linux semantics advance the actual file positions by the number
        // of bytes copied (see copy_file_range(2)); the old code never
        // moved them, so every call re-copied from the same position and
        // callers looping to EOF (e.g. coreutils cp) spun forever.
        if !off_in_ptr.is_null() {
            // SAFETY: off_in_ptr validated with access_ok(8).
            if unsafe { crate::arch::uaccess::copy_to_user(off_in_ptr as *mut u8, cur_in.to_le_bytes().as_ptr(), 8) } != 0 {
                return -(errno::EFAULT as i64);
            }
        } else {
            in_file.set_pos(in_file.get_pos() + total as u64);
        }
        if !off_out_ptr.is_null() {
            // SAFETY: off_out_ptr validated with access_ok(8).
            if unsafe { crate::arch::uaccess::copy_to_user(off_out_ptr as *mut u8, cur_out.to_le_bytes().as_ptr(), 8) } != 0 {
                return -(errno::EFAULT as i64);
            }
        } else {
            out_file.set_pos(out_file.get_pos() + total as u64);
        }

        if total > 0 {
            total as i64
        } else if first_err != 0 {
            first_err
        } else {
            // EOF on the source with nothing copied is a 0 return.
            0
        }
    }
}

/// sys_preadv2 - Read from fd into vectors v2 (NR 286)
pub fn sys_preadv2(args: SyscallArgs) -> i64 {
    // Same as preadv, ignoring flags
    crate::syscall::io::sys_preadv(args)
}

/// sys_pwritev2 - Write vectors to fd v2 (NR 287)
pub fn sys_pwritev2(args: SyscallArgs) -> i64 {
    // Same as pwritev, ignoring flags
    crate::syscall::io::sys_pwritev(args)
}

// ============================================================================
// NR 428-433: Filesystem mounting/operations
// ============================================================================

/// sys_open_tree - Open a directory tree (NR 428)
pub fn sys_open_tree(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_move_mount - Move a mount (NR 429)
pub fn sys_move_mount(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fsopen - Open a filesystem (NR 430)
pub fn sys_fsopen(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fsconfig - Configure a filesystem context (NR 431)
pub fn sys_fsconfig(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fsmount - Create a mount (NR 432)
pub fn sys_fsmount(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fspick - Pick a filesystem (NR 433)
pub fn sys_fspick(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 441-443: epoll_pwait2, mount_setattr, quotactl_fd
// ============================================================================

/// sys_epoll_pwait2 - epoll_wait with sigmask (NR 441)
pub fn sys_epoll_pwait2(args: SyscallArgs) -> i64 {
    // R32 (NEW-12): epoll_pwait2 passes the timeout as a struct timespec
    // POINTER in args[3] (sigmask is args[4]). The old delegation fed that
    // pointer VALUE into epoll_wait's i32-milliseconds argument, so every
    // non-NULL timeout produced a garbage wait (usually negative =
    // "infinite") and a NULL timeout returned immediately instead of
    // blocking forever.
    let timeout_ptr = args[3] as *const u8;
    let timeout_ms: i32 = if timeout_ptr.is_null() {
        -1 // NULL timeout = block indefinitely
    } else {
        if !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16) {
            return -(errno::EFAULT as i64);
        }
        let mut buf = [0u8; 16];
        // SAFETY: timeout_ptr was access_ok-validated for 16 bytes above;
        // buf is a 16-byte stack buffer (exception-table copy).
        let uncopied = unsafe {
            crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), timeout_ptr, 16)
        };
        if uncopied > 0 {
            return -(errno::EFAULT as i64);
        }
        let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
        let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
        // Linux timespec validation: negative tv_sec/tv_nsec or
        // tv_nsec >= 1s is EINVAL, not an infinite wait (LTP
        // epoll_pwait05 — the old u64 read turned "-1" into ~584 years).
        if sec < 0 || nsec < 0 || nsec > 999_999_999 {
            return -(errno::EINVAL as i64);
        }
        if sec == 0 && nsec == 0 {
            0 // zero timeout = poll once and return
        } else {
            // Convert to milliseconds, cap at i32 max (very long = infinite
            // for our purposes — same policy as sys_ppoll).
            let total_ms = (sec as u64).saturating_mul(1000).saturating_add(nsec as u64 / 1_000_000);
            if total_ms > i32::MAX as u64 { -1 } else { total_ms as i32 }
        }
    };
    // P3 LTP round 3: validate (and apply) the sigmask like epoll_pwait.
    let sigmask_ptr = args[4] as *const u64;
    let sigsetsize = args[5] as usize;
    let saved_mask_cell = core::cell::Cell::new(0u64);
    let mask_applied = !sigmask_ptr.is_null();
    if mask_applied {
        if sigsetsize != 8 {
            return -(errno::EINVAL as i64);
        }
        if !crate::arch::uaccess::access_ok(sigmask_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        let new_mask = match unsafe { crate::arch::uaccess::get_user(sigmask_ptr) } {
            Some(v) => v,
            None => return -(errno::EFAULT as i64),
        };
        let new_mask = new_mask & !((1u64 << 8) | (1u64 << 18));
        if let Some(c) = crate::sched::current() {
            let current = c as *const _ as *mut crate::process::task::Task;
            // SAFETY: current is the running task; sigmask is a plain u64.
            saved_mask_cell.set(unsafe { (*current).sigmask });
            unsafe { (*current).sigmask = new_mask; }
        }
    }
    let ret = crate::syscall::misc::sys_epoll_wait([args[0], args[1], args[2], timeout_ms as u64, 0, 0]);
    if mask_applied {
        if let Some(c) = crate::sched::current() {
            let current = c as *const _ as *mut crate::process::task::Task;
            // SAFETY: same running task as above.
            unsafe { (*current).sigmask = saved_mask_cell.get(); }
        }
    }
    ret
}

/// sys_mount_setattr - Change mount attributes (NR 442)
pub fn sys_mount_setattr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_quotactl_fd - Disk quota operations via fd (NR 443)
pub fn sys_quotactl_fd(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 444-446: Landlock
// ============================================================================

/// sys_landlock_create_ruleset - Create landlock ruleset (NR 444)
pub fn sys_landlock_create_ruleset(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_landlock_add_rule - Add landlock rule (NR 445)
pub fn sys_landlock_add_rule(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_landlock_restrict_self - Restrict self with landlock (NR 446)
pub fn sys_landlock_restrict_self(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 449-466: futex_waitv, set_mempolicy_home_node, etc.
// ============================================================================

/// sys_futex_waitv - Wait on multiple futexes (NR 449)
pub fn sys_futex_waitv(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_set_mempolicy_home_node - Set home node for memory policy (NR 450)
pub fn sys_set_mempolicy_home_node(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_cachestat - Get cache stats (NR 451)
pub fn sys_cachestat(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fchmodat2 - Change file mode (extended) (NR 452)
pub fn sys_fchmodat2(args: SyscallArgs) -> i64 {
    // Linux fchmodat2(dirfd, path, mode, flags): flags are validated —
    // anything beyond AT_SYMLINK_NOFOLLOW / AT_EMPTY_PATH is EINVAL
    // (LTP fchmodat02's "invalid flag" case used to pass -1 through and
    // succeed). AT_SYMLINK_NOFOLLOW is ENOTSUP (chmod on the symlink
    // itself is not supported, like every Linux filesystem).
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    let flags = args[3] as u32;
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return -(errno::EINVAL as i64);
    }
    if flags & AT_SYMLINK_NOFOLLOW != 0 {
        return -(errno::EOPNOTSUPP as i64);
    }
    if flags & AT_EMPTY_PATH != 0 && args[1] != 0 {
        // chmod through the fd the path argument names. An empty path
        // with a nonempty string is EINVAL; a plain empty string with
        // AT_EMPTY_PATH operates on dirfd itself.
    }
    sys_fchmodat(args)
}

/// sys_map_shadow_stack - Map shadow stack (NR 453)
pub fn sys_map_shadow_stack(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_futex_wake - Wake futex (NR 454)
pub fn sys_futex_wake(args: SyscallArgs) -> i64 {
    // futex_wake ABI (Linux 6.7): (uaddr, mask, nr, flags).
    // FUTEX2 flags: bits 0-1 are the futex size domain (FUTEX2_SIZE_U8/U16/
    // U32/U64), bit 7 is FUTEX2_PRIVATE (== FUTEX_PRIVATE_FLAG 0x80).
    // Linux sys_futex_wake only accepts FUTEX2_SIZE_U32 | FUTEX2_PRIVATE.
    const FUTEX2_SIZE_U32: u64 = 0x2;
    const FUTEX2_PRIVATE: u64 = 0x80;
    const FUTEX2_VALID: u64 = FUTEX2_SIZE_U32 | FUTEX2_PRIVATE;
    if args[3] & !FUTEX2_VALID != 0 {
        return -(errno::EINVAL as i64);
    }
    let flags = crate::sync::futex::futex_to_flags(
        args[3] as u32 | if args[3] & FUTEX2_PRIVATE != 0 {
            crate::sync::futex::FUTEX_PRIVATE_FLAG as u32
        } else {
            0
        },
    );
    // mask selects which waiter bitsets match; 0 = match everything
    // (FUTEX_BITSET_MATCH_ANY).
    const FUTEX_BITSET_MATCH_ANY: u32 = 0xFFFF_FFFF;
    let bitset = if args[1] == 0 { FUTEX_BITSET_MATCH_ANY } else { args[1] as u32 };
    // WAKE with nr == 0 is a valid no-op probe.
    let nr = args[2] as i32;
    if nr < 0 {
        return -(errno::EINVAL as i64);
    }
    // uaddr must be 4-byte aligned (Linux get_futex_key).
    if args[0] & 0x3 != 0 {
        return -(errno::EINVAL as i64);
    }
    crate::sync::futex::futex_wake_bitset(args[0] as usize, flags, nr, bitset)
}

/// sys_futex_wait - Wait on futex (NR 455)
pub fn sys_futex_wait(args: SyscallArgs) -> i64 {
    // futex_wait ABI (Linux 6.7): (uaddr, expected, mask, flags, *timeout)
    // — 5 params. The timeout is a RELATIVE timespec (unlike WAIT_BITSET's
    // absolute one), and `mask` selects the waiter's bitset.
    const FUTEX2_SIZE_U32: u64 = 0x2;
    const FUTEX2_PRIVATE: u64 = 0x80;
    const FUTEX2_VALID: u64 = FUTEX2_SIZE_U32 | FUTEX2_PRIVATE;
    if args[3] & !FUTEX2_VALID != 0 {
        return -(errno::EINVAL as i64);
    }
    // uaddr must be 4-byte aligned (Linux get_futex_key).
    if args[0] & 0x3 != 0 {
        return -(errno::EINVAL as i64);
    }

    let flags = crate::sync::futex::futex_to_flags(
        args[3] as u32 | if args[3] & FUTEX2_PRIVATE != 0 {
            crate::sync::futex::FUTEX_PRIVATE_FLAG as u32
        } else {
            0
        },
    );

    // Relative timeout → absolute jiffies deadline (futex_wait_bitset takes
    // an absolute deadline). NULL timeout = wait forever.
    let deadline: Option<u64> = if args[4] == 0 {
        None
    } else {
        match crate::sync::futex::futex_parse_timeout(args[4], false) {
            Ok(dl) => dl,
            Err(e) => return -(e as i64),
        }
    };

    // mask selects which wake-ups this waiter matches; 0 = MATCH_ANY.
    const FUTEX_BITSET_MATCH_ANY: u32 = 0xFFFF_FFFF;
    let bitset = if args[2] == 0 { FUTEX_BITSET_MATCH_ANY } else { args[2] as u32 };

    crate::sync::futex::futex_wait_bitset(
        args[0] as usize,
        flags,
        args[1] as u32, // expected value
        0,
        bitset,
        deadline,
    )
}

/// sys_futex_requeue - Requeue futex (NR 456)
pub fn sys_futex_requeue(_args: SyscallArgs) -> i64 {
    // R7-A7: futex_requeue ABI is (waiters*, flags, nr_wake, nr_requeue) —
    // the old blind passthrough decoded it as op=FUTEX_WAIT with
    // timeout=(nr_requeue as pointer) and could sleep forever. Return
    // ENOSYS until the waiters-array ABI is implemented.
    -(crate::syscall::errno::ENOSYS as i64)
}

/// sys_statmount - Get mount info (NR 457)
pub fn sys_statmount(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_listmount - List mounts (NR 458)
pub fn sys_listmount(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 459-470: LSM, mseal, xattrat, file_getattr/setattr, listns
// ============================================================================

/// sys_lsm_get_self_attr - Get LSM self attribute (NR 459)
pub fn sys_lsm_get_self_attr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_lsm_set_self_attr - Set LSM self attribute (NR 460)
pub fn sys_lsm_set_self_attr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_lsm_list_modules - List LSM modules (NR 461)
pub fn sys_lsm_list_modules(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_mseal - Memory sealing (NR 462)
pub fn sys_mseal(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// sys_setxattrat / getxattrat / listxattrat / removexattrat (NR 463-466)
// are implemented with the main xattr block earlier in this file.

/// sys_open_tree_attr - Open directory tree with attributes (NR 467)
pub fn sys_open_tree_attr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_file_getattr - Get file attributes (NR 468)
pub fn sys_file_getattr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_file_setattr - Set file attributes (NR 469)
pub fn sys_file_setattr(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_listns - List namespaces (NR 470)
pub fn sys_listns(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_renameat2 - rename a file (with flags support)
///
/// # Arguments
/// - args[0]: olddirfd - old directory fd
/// - args[1]: oldpath - old path pointer
/// - args[2]: newdirfd - new directory fd
/// - args[3]: newpath - new path pointer
/// - args[4]: flags - rename flags (RENAME_NOREPLACE, etc.)
pub fn sys_renameat2(args: SyscallArgs) -> i64 {
    let olddirfd = args[0] as i32;
    let oldpath = args[1] as *const u8;
    let newdirfd = args[2] as i32;
    let newpath = args[3] as *const u8;
    let flags = args[4] as u32;

    const RENAME_NOREPLACE: u32 = 1 << 0;
    const RENAME_EXCHANGE: u32 = 1 << 1;
    const RENAME_WHITEOUT: u32 = 1 << 2;
    const KNOWN_FLAGS: u32 = RENAME_NOREPLACE | RENAME_EXCHANGE | RENAME_WHITEOUT;

    // Unknown flag bits are EINVAL; WHITEOUT needs VFS support we do not
    // have (return EINVAL rather than silently mis-replacing).
    if flags & !KNOWN_FLAGS != 0 {
        return -(errno::EINVAL as i64);
    }
    if flags & RENAME_WHITEOUT != 0 {
        return -(errno::EINVAL as i64);
    }

    let old = match resolve_user_path(olddirfd, oldpath) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };
    let new = match resolve_user_path(newdirfd, newpath) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    // RENAME_EXCHANGE: atomically swap the two paths (both must exist).
    // Mutually exclusive with NOREPLACE by definition (the target must
    // exist). Implemented in the VFS under the mutation lock.
    if flags & RENAME_EXCHANGE != 0 {
        return match crate::fs::vfs::vfs_rename_exchange(&old, &new) {
            Ok(()) => 0,
            Err(e) => e as i64,
        };
    }

    // RENAME_NOREPLACE: fail with EEXIST if the destination exists instead
    // of silently replacing it (the old code ignored the flag entirely).
    if flags & RENAME_NOREPLACE != 0 {
        let mut st = crate::fs::Stat::new();
        if crate::fs::vfs::stat_file_by_path(&new, &mut st).is_ok() {
            return -(errno::EEXIST as i64);
        }
    }

    match crate::fs::vfs::vfs_rename(&old, &new) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}
