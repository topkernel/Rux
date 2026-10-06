//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tests for the readlinkat proc-path resolver join semantics. Copied from:
//! kernel/src/syscall/file.rs (resolve_proc_readlink_path)
//!
//! The resolver walks /proc magic-link special cases and otherwise joins a
//! possibly-relative path onto the caller's cwd. An ABSOLUTE path must pass
//! through UNTOUCHED: sys_readlinkat feeds it the already-absolutized result
//! of resolve_user_path, and prepending the cwd again made every readlink of
//! an existing non-/proc absolute path return ENOENT whenever cwd != "/"
//! (the EINVAL-for-non-symlink branch was unreachable, so libc realpath()
//! — which probes components with readlink — aborted with ENOENT; seen as
//! LTP acct01's "Cannot resolve the absolute path of ro_mntpoint" TINFO).

use proptest::prelude::*;
use proptest::sample::select;

const AT_FDCWD: i32 = -100;

/// Copied resolver, POST-fix — must match
/// kernel/src/syscall/file.rs resolve_proc_readlink_path exactly in
/// structure (the /proc/{self,pid} PID substitution is exercised through
/// `pid`).
fn resolve_proc_readlink_path(dirfd: i32, pathname: &str, cwd: &str, pid: u32) -> String {
    let sub = |p: String| -> String {
        if p.contains("/self/") {
            p.replace("/self/", &format!("/{}/", pid))
        } else {
            p
        }
    };

    if pathname.starts_with("/proc/") {
        return sub(String::from(pathname));
    }

    if !pathname.starts_with('/') && dirfd == AT_FDCWD {
        let full = if cwd.ends_with('/') {
            format!("{}{}", cwd, pathname)
        } else {
            format!("{}/{}", cwd, pathname)
        };
        return sub(full);
    }

    String::from(pathname)
}

fn any_of(consts: Vec<&'static str>) -> BoxedStrategy<String> {
    select(consts)
        .prop_map(|s: &str| String::from(s))
        .boxed()
}

fn arb_abs_path() -> BoxedStrategy<String> {
    "[a-z][a-z0-9/]{0,10}"
        .prop_map(move |rel| format!("/{}", rel))
        .boxed()
}

proptest! {
    /// CONTRACT: an absolute input is returned verbatim, whatever the cwd is.
    #[test]
    fn absolute_path_passes_through((path, cwd) in (arb_abs_path(), any_of(vec!["/", "/tmp", "/tmp/LTP_x"]))) {
        let out = resolve_proc_readlink_path(AT_FDCWD, &path, &cwd, 7);
        prop_assert_eq!(out, path);
    }

    /// A relative path joins onto the cwd with exactly one '/' between them.
    #[test]
    fn relative_path_joins_cleanly((rel, cwd) in ("[a-z][a-z0-9/]{0,12}", any_of(vec!["/", "/tmp", "/tmp/dir"]))) {
        let out = resolve_proc_readlink_path(AT_FDCWD, &rel, &cwd, 7);
        let joined = cwd.trim_end_matches('/');
        prop_assert_eq!(out, format!("{}/{}", joined, rel));
    }

    /// dirfd-relative (not AT_FDCWD) inputs are returned as-is by this
    /// resolver (the *at() layer resolved them already).
    #[test]
    fn dirfd_paths_untouched(rel in "[a-z][a-z0-9/]{0,12}") {
        let out = resolve_proc_readlink_path(3, &rel, "/tmp", 7);
        prop_assert_eq!(out, rel);
    }

    /// /proc/self/... resolves self to the pid, absolute or relative.
    #[test]
    fn proc_self_pid_substitution((p, cwd) in (
        any_of(vec!["/proc/self/exe", "/proc/self/fd/3", "/proc/self/ns/ipc"]),
        any_of(vec!["/", "/tmp"]),
    )) {
        let out = resolve_proc_readlink_path(AT_FDCWD, &p, &cwd, 42);
        prop_assert!(!out.contains("/self/"));
        prop_assert!(out.contains("/42/"));
    }
}

/// Deterministic regressions from the bug family.
#[test]
fn readlink_path_regressions() {
    // THE bug: readlink("/tmp") with cwd=/tmp became "/tmp/tmp" -> ENOENT.
    assert_eq!(resolve_proc_readlink_path(AT_FDCWD, "/tmp", "/tmp", 1), "/tmp");
    assert_eq!(resolve_proc_readlink_path(AT_FDCWD, "/etc", "/tmp", 1), "/etc");
    // Root cwd used to hide the bug ("//tmp" parsed fine).
    assert_eq!(resolve_proc_readlink_path(AT_FDCWD, "/tmp", "/", 1), "/tmp");
    // Relative join picks up the missing separator (cwd without '/').
    assert_eq!(resolve_proc_readlink_path(AT_FDCWD, "file", "/tmp", 1), "/tmp/file");
    assert_eq!(resolve_proc_readlink_path(AT_FDCWD, "file", "/", 1), "/file");
    // /proc paths: self -> pid.
    assert_eq!(
        resolve_proc_readlink_path(AT_FDCWD, "/proc/self/exe", "/tmp", 99),
        "/proc/99/exe"
    );
}
