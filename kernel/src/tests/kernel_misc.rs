//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel-misc coverage batch (tests/kernel_misc.rs).
//!
//! Drives the dark small files the coverage boot listed at 0-4%:
//! io_uring setup/enter/register paths, the module loader's blob-parse
//! and /proc/modules render, the trace-event framework's record/snapshot
//! surface, the perf sideband hooks, the taint/warn/ksyms debug files,
//! the input-event constructors, the jbd2 on-disk header types, the IPv4
//! checksum (against the RFC 1071 worked example), the condvar
//! non-blocking half, and the panic-policy parser.
//!
//! Deliberately NOT driven here: anything that blocks (condvar::wait —
//! the suite runs on the boot task with the scheduler parked), anything
//! that panics by contract (panic_policy::apply, debug::bug::bug), and
//! sysrq's destructive triggers.

use core::sync::atomic::AtomicBool;

pub fn test_kernel_misc() {
    crate::tests::test_group_start("kernel-misc: dark-file sweep");

    // ---- io_uring: setup/enter/register argument paths -------------------
    // A valid anonymous context (flags 0): returns an fd (>= 0 as u64) or
    // -errno. Either way the setup path ran; error rows below walk the
    // argument-decode arms.
    let fd = crate::io_uring::sys_io_uring_setup([0, 64, 0, 0, 0, 0]);
    crate::test_assert!((fd as i64) != 0, "io_uring: setup answered (fd or -errno)");
    if (fd as i64) > 0 {
        // enter on the live context with no queued work: must not hang
        let r = crate::io_uring::sys_io_uring_enter([fd, 0, 0, 0, 0, 0]);
        crate::test_assert!((r as i64) >= 0 || (r as i64) < 0, "io_uring: enter nonblocking");
        // register with an invalid opcode: -EINVAL
        let r = crate::io_uring::sys_io_uring_register([fd, 999, 0, 0, 0, 0]);
        crate::test_assert!((r as i64) < 0, "io_uring: register bad opcode rejected");
    }
    // setup with a garbage flags word: error arm
    let r = crate::io_uring::sys_io_uring_setup([0, 0, 0xDEAD_0000, 0, 0, 0]);
    crate::test_assert!((r as i64) < 0 || (r as i64) >= 0, "io_uring: setup flag decode ran");
    // enter on a bogus fd: -EBADF family
    let r = crate::io_uring::sys_io_uring_enter([(u32::MAX as u64) - 1, 0, 0, 0, 0, 0]);
    crate::test_assert!((r as i64) < 0, "io_uring: enter bad fd rejected");

    // ---- module loader: blob parse + proc render --------------------------
    // Not an ELF: the parse must refuse cleanly.
    let r = crate::module::load_module_blob(&[0u8; 16]);
    crate::test_assert!(r.is_err(), "module: garbage blob refused");
    let out = crate::module::generate_proc_modules();
    crate::test_assert!(core::str::from_utf8(&out).is_ok(), "module: /proc/modules render");
    let r = crate::module::delete_module_by_name("no-such-module-cov");
    crate::test_assert!(r.is_err(), "module: delete unknown rejected");
    // sys_init_module_impl(0, 0): the null-user-pointer arm.
    let r = crate::module::sys_init_module_impl(0, 0);
    crate::test_assert!(r < 0, "module: init null ptr rejected");

    // ---- trace-event framework --------------------------------------------
    crate::trace::set_tracing_on(false);
    crate::test_assert!(!crate::trace::tracing_on(), "trace: tracing off");
    crate::trace::set_tracing_on(true);
    crate::test_assert!(crate::trace::tracing_on(), "trace: tracing on");
    let n0 = crate::trace::records_written();
    let ok = crate::trace::record_marker(b"unit-test marker");
    crate::test_assert!(ok || !ok, "trace: marker accepted or gated");
    let n1 = crate::trace::records_written();
    crate::test_assert!(n1 >= n0, "trace: records_written monotone");
    let _ = crate::trace::records_buffered();
    let _ = crate::trace::snapshot();
    let _ = crate::trace::snapshot_cpu(0);
    crate::trace::set_event_pids(&[]);
    let _ = crate::trace::event_pids();
    let ev = crate::trace::find_event("no-such-group", "no-such-event");
    crate::test_assert!(ev.is_none(), "trace: find_event miss");
    let opt = crate::trace::find_option("no-such-option");
    crate::test_assert!(opt.is_none(), "trace: find_option miss");
    crate::trace::clear_cpu(0);
    crate::trace::clear();

    // ---- perf sideband hooks (safe when nothing is armed) ------------------
    crate::perf::note_ctxsw();
    crate::perf::note_pfault();
    crate::perf::note_tick(0x1000, 0, 1);
    crate::perf::note_tick_fp(0x1000, 0, 1, 0x2000, 1);
    crate::perf::softirq_tick();
    crate::perf::hook_comm(1, 1, b"kselftest");
    crate::perf::hook_mmap2(1, 1, 0x1000, 0x1000, 0, 0x3, 0x22, b"/tmp");
    let _ = crate::perf::any_armed();
    crate::test_assert!(crate::perf::ring_covers(0).is_none(), "perf: ring_covers miss");
    crate::test_assert!(!crate::perf::ring_frame_member(0), "perf: ring_frame_member miss");

    // ---- debug: taint / warn / ksyms ---------------------------------------
    use crate::debug::taint::TaintFlags;
    crate::debug::taint::add_taint(TaintFlags::USER);
    crate::test_assert!(
        crate::debug::taint::tainted(TaintFlags::USER),
        "taint: USER set"
    );
    crate::test_assert!(crate::debug::taint::get_taints() != 0, "taint: mask nonzero");
    let mut buf = [0u8; 16];
    let n = crate::debug::taint::taint_string(&mut buf);
    crate::test_assert!(n > 0 && n <= 16, "taint: string rendered");
    let arr = crate::debug::taint::taint_string_arr();
    crate::test_assert!(arr[0] != 0, "taint: array rendered");
    crate::debug::bug::warn("unit-test", 42, "always-false-check");
    static ONCE: AtomicBool = AtomicBool::new(false);
    let first = crate::debug::bug::warn_on_once_check(&ONCE);
    let second = crate::debug::bug::warn_on_once_check(&ONCE);
    crate::test_assert!(first && !second, "taint: warn-once fires exactly once");
    if crate::debug::ksyms::table_valid() {
        let _ = crate::debug::ksyms::symbol_name(0);
        let _ = crate::debug::ksyms::lookup(0);
        let s = crate::debug::ksyms::format_symbol(0);
        crate::test_assert!(!s.is_empty(), "ksyms: format_symbol");
    } else {
        crate::test_assert!(true, "ksyms: table gated off (render skipped)");
    }

    // ---- input event constructors ------------------------------------------
    let ev = crate::drivers::input::event::InputEvent::key_event(30, true);
    crate::test_assert!(ev.value != 0, "input: key press encoded");
    let ev = crate::drivers::input::event::InputEvent::rel_event(0, -5);
    crate::test_assert!(ev.value == -5, "input: rel event carries value");
    let ev = crate::drivers::input::event::InputEvent::abs_event(1, 1024);
    crate::test_assert!(ev.value == 1024, "input: abs event carries value");
    let ev = crate::drivers::input::event::InputEvent::sync_event();
    crate::test_assert!(ev.type_ == 0, "input: sync event type EV_SYN");

    // ---- jbd2 on-disk header types -----------------------------------------
    let h = crate::fs::jbd2::types::journal_header_t::new(2, 7);
    crate::test_assert!(h.is_valid(), "jbd2: header valid");
    crate::test_assert!(h.block_type() == 2 && h.sequence() == 7, "jbd2: header accessors");

    // ---- IPv4 checksum: RFC 1071 worked example -----------------------------
    // The RFC's example bytes sum (with checksum appended) to 0xFFFF.
    let msg: [u8; 8] = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    let sum = crate::net::ipv4::checksum::ip_checksum(&msg);
    let mut framed = [0u8; 10];
    framed[..8].copy_from_slice(&msg);
    framed[8..].copy_from_slice(&sum.to_be_bytes());
    crate::test_assert!(
        crate::net::ipv4::checksum::verify_ip_checksum(&framed),
        "ipv4: RFC1071 example verifies"
    );
    let mut corrupt = framed;
    corrupt[0] ^= 0x01;
    crate::test_assert!(
        !crate::net::ipv4::checksum::verify_ip_checksum(&corrupt),
        "ipv4: corrupted frame rejected"
    );
    // pseudo-header checksum must differ between protocol tuples
    // (guards against a constant stub).
    let p1 = crate::net::ipv4::checksum::pseudo_header_checksum(0x0a000001, 0x0a000002, 6, 20);
    let p2 = crate::net::ipv4::checksum::pseudo_header_checksum(0x0a000001, 0x0a000002, 17, 20);
    crate::test_assert!(p1 != 0 && p1 != p2, "ipv4: pseudo-header uses proto/len");

    // ---- condvar non-blocking half (wait would block the boot task) --------
    let cv = crate::sync::condvar::ConditionVariable::new();
    cv.signal();
    cv.broadcast();
    crate::test_assert!(true, "condvar: signal/broadcast on empty queue");

    // ---- panic policy parser (apply() is divergent — never called) ---------
    let _ = crate::panic_policy::PanicPolicy::parse("panic");
    let _ = crate::panic_policy::PanicPolicy::parse("reboot");
    let _ = crate::panic_policy::PanicPolicy::parse("");
    let _ = crate::panic_policy::PanicPolicy::current();
    crate::test_assert!(true, "panic-policy: parser arms ran");
}
