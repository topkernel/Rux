//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Time-related system calls
//!
//! Includes: gettimeofday, clock_gettime, nanosleep, clock_getres, clock_nanosleep

use super::*;

/// clock_gettime clock IDs
const CLOCK_REALTIME: u32 = 0;
const CLOCK_MONOTONIC: u32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
const CLOCK_THREAD_CPUTIME_ID: u32 = 3;
const CLOCK_MONOTONIC_RAW: u32 = 4;
const CLOCK_REALTIME_COARSE: u32 = 5;
const CLOCK_MONOTONIC_COARSE: u32 = 6;
const CLOCK_BOOTTIME: u32 = 7;
const CLOCK_TAI: u32 = 11;

/// Read the monotonic clock as (seconds, nanoseconds) from the CLINT.
fn monotonic_time() -> (u64, u64) {
    let cycles = crate::arch::cpu::read_time();
    let freq_hz: u64 = crate::config::TIMER_CLOCK_FREQ_HZ; // 10 MHz
    (cycles / freq_hz, (cycles % freq_hz) * 1_000_000_000 / freq_hz)
}

/// Current CLOCK_REALTIME (monotonic + wall epoch offset) as (secs, nanos).
/// FIX8: shared by the adjtimex GET path (tx.time) and ADJ_SETOFFSET.
fn realtime_secs_nanos() -> (i64, u64) {
    let (s, ns) = monotonic_time();
    let total_ns = (s as u128 * 1_000_000_000u128 + ns as u128)
        + wall_epoch_offset_ns() as u128;
    ((total_ns / 1_000_000_000u128) as i64, (total_ns % 1_000_000_000u128) as u64)
}

#[repr(C)]
struct TimespecForGettime {
    tv_sec: i64,
    tv_nsec: i64,
}

/// sys_gettimeofday - Get current time
///
/// # Arguments
/// - args[0]: tv - pointer to timeval structure
/// - args[1]: tz - pointer to timezone structure (deprecated, should be null)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_gettimeofday(args: SyscallArgs) -> i64 {
    let tv_ptr = args[0] as *mut TimeVal;
    let _tz_ptr = args[1] as *mut u8;  // timezone is deprecated

    if tv_ptr.is_null() {
        return 0;  // NULL is allowed, just return success
    }

    // Check if tv_ptr is in valid user space
    if !crate::arch::uaccess::access_ok(tv_ptr as usize, core::mem::size_of::<TimeVal>()) {
        return -(errno::EFAULT as i64);
    }

    // Get time from RISC-V timer + wall-clock epoch offset (ns-precise:
    // the sub-second part of a clock_settime must survive, LTP
    // clock_settime01 advances/recedes by 10 ms deltas).
    let cycles = crate::arch::cpu::read_time();
    let freq_hz: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;  // 10 MHz

    let total_ns = cycles
        .saturating_mul(1_000_000_000 / freq_hz)
        .saturating_add(wall_epoch_offset_ns());
    let sec = total_ns / 1_000_000_000;
    let usec = (total_ns % 1_000_000_000) / 1_000;

    // SAFETY: tv_ptr validated with access_ok; put_user is the
    // exception-table copy path (SUM=0 safe).
    // SAFETY: tv_ptr validated with access_ok; put_user is the
    // exception-table copy path. A faulting store is EFAULT (LTP
    // gettimeofday01's bad_addr case).
    unsafe {
        let ok1 = crate::arch::uaccess::put_user(&raw mut (*tv_ptr).tv_sec, sec as i64);
        let ok2 = crate::arch::uaccess::put_user(&raw mut (*tv_ptr).tv_usec, usec as i64);
        if !ok1 || !ok2 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_clock_gettime - Get time of specified clock
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: tp - pointer to timespec structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_gettime(args: SyscallArgs) -> i64 {
    let clk_id = args[0] as u32;
    let tp_ptr = args[1] as *mut TimespecForGettime;

    if tp_ptr.is_null() {
        return -(errno::EINVAL as i64);
    }

    // Check if tp_ptr is in valid user space
    if !crate::arch::uaccess::access_ok(tp_ptr as usize, core::mem::size_of::<TimespecForGettime>()) {
        return -(errno::EFAULT as i64);
    }

    match clk_id {
        CLOCK_REALTIME | CLOCK_TAI => {
            // Wall clock = monotonic + epoch offset (settimeofday-adjustable;
            // zero until set — no RTC on this platform). The offset is
            // ns-precise (LTP clock_settime01's 10 ms deltas must be
            // observable). CLOCK_TAI is REALTIME + TAI-UTC offset. No
            // clock_adjtime (ADJ_TAI) support, so use the current
            // real-world constant 37 s (2017+ offset) as the boot default.
            let tai_offset: u64 = 37;
            let (mono_sec, mono_nsec) = monotonic_time();
            let total_ns = (mono_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(mono_nsec as u64)
                .saturating_add(wall_epoch_offset_ns())
                .saturating_add(if clk_id == CLOCK_TAI {
                    tai_offset.saturating_mul(1_000_000_000)
                } else {
                    0
                });
            let sec = total_ns / 1_000_000_000;
            let mono_nsec = (total_ns % 1_000_000_000) as u64;
            // SAFETY: tp_ptr validated with access_ok; put_user is the
            // exception-table copy path (SUM=0 safe).
            // SAFETY: tp_ptr validated with access_ok; put_user is the
            // exception-table copy path (SUM=0 safe). A store that faults
            // (PROT_NONE / unmapped — LTP clock_gettime02's bad_addr)
            // must surface as EFAULT, not be swallowed into success.
            unsafe {
                let ok1 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_sec, sec as i64);
                let ok2 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_nsec, mono_nsec as i64);
                if !ok1 || !ok2 {
                    return -(errno::EFAULT as i64);
                }
            }
            0
        }
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME => {
            let (sec, nsec) = monotonic_time();
            // SAFETY: tp_ptr validated with access_ok; put_user is the
            // exception-table copy path (SUM=0 safe). EFAULT on fault.
            unsafe {
                let ok1 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_sec, sec as i64);
                let ok2 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_nsec, nsec as i64);
                if !ok1 || !ok2 {
                    return -(errno::EFAULT as i64);
                }
            }
            0
        }
        // Coarse clocks: tick-grid-granular timestamps (Linux CLOCK_*_COARSE
        // advances in 1/HZ steps — that IS the contract, and LTP
        // clock_gettime04 sizes its tolerance from clock_getres()).
        // Computed from the LIVE timebase snapped to the nominal grid —
        // the stored JIFFIES counter only advances in the tick handler,
        // which can lag the timebase by seconds under load, and this
        // clock must agree with rtc::wall_secs() (fs timestamps): LTP
        // utime01 brackets utime(NULL) stamps between coarse samples.
        CLOCK_REALTIME_COARSE | CLOCK_MONOTONIC_COARSE => {
            let mut total_ns = crate::drivers::timer::coarse_ns_since_boot();
            if clk_id == CLOCK_REALTIME_COARSE {
                total_ns = total_ns.saturating_add(wall_epoch_offset_ns());
            }
            let sec = total_ns / 1_000_000_000;
            let nsec = total_ns % 1_000_000_000;
            // SAFETY: tp_ptr validated with access_ok; put_user is the
            // exception-table copy path (SUM=0 safe).
            unsafe {
                let ok1 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_sec, sec as i64);
                let ok2 = crate::arch::uaccess::put_user(&raw mut (*tp_ptr).tv_nsec, nsec as i64);
                if !ok1 || !ok2 {
                    return -(errno::EFAULT as i64);
                }
            }
            0
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // Minimal implementation: the scheduling entity's cumulative
            // execution time (nanoseconds). PROCESS and THREAD collapse to
            // the same value until per-thread accounting exists.
            let cputime_ns = crate::process::current_task()
                .map(|t| t.sched_entity().sum_exec_runtime.load(core::sync::atomic::Ordering::Acquire))
                .unwrap_or(0);
            // SAFETY: tp_ptr validated with access_ok; put_user is the
            // exception-table copy path (SUM=0 safe).
            // SAFETY: tp_ptr validated with access_ok; EFAULT on a
            // faulting store (LTP clock_gettime02).
            unsafe {
                let ok1 = crate::arch::uaccess::put_user(
                    &raw mut (*tp_ptr).tv_sec, (cputime_ns / 1_000_000_000) as i64);
                let ok2 = crate::arch::uaccess::put_user(
                    &raw mut (*tp_ptr).tv_nsec, (cputime_ns % 1_000_000_000) as i64);
                if !ok1 || !ok2 {
                    return -(errno::EFAULT as i64);
                }
            }
            0
        }
        _ => {
            // Unsupported clock type
            -(errno::EINVAL as i64)
        }
    }
}

/// Timespec structure
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// sys_nanosleep - High-resolution sleep
///
/// # Arguments
/// - args[0]: req - requested sleep time
/// - args[1]: rem - remaining time (when interrupted by signal)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_nanosleep(args: SyscallArgs) -> i64 {
    use crate::drivers::timer;
    use crate::process;

    let req_ptr = args[0] as *const Timespec;
    let rem_ptr = args[1] as *mut Timespec;

    // Check request pointer validity
    if req_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Check if req_ptr is in valid user space
    if !crate::arch::uaccess::access_ok(req_ptr as usize, core::mem::size_of::<Timespec>()) {
        return -(errno::EFAULT as i64);
    }

    // Check rem_ptr if provided
    if !rem_ptr.is_null() && !crate::arch::uaccess::access_ok(rem_ptr as usize, core::mem::size_of::<Timespec>()) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: req_ptr validated with access_ok; copy_from_user is the
    // exception-table copy path (SUM=0 safe) and zero-fills on fault.
    let mut req = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe {
        crate::arch::uaccess::copy_from_user(
            &mut req as *mut Timespec as *mut u8,
            req_ptr as *const u8,
            core::mem::size_of::<Timespec>(),
        );
    }

    // POSIX: tv_nsec must be in [0, 999_999_999]; a negative tv_sec is also
    // EINVAL (previously a negative request slept ~1ms instead).
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec > 999_999_999 {
        return -(errno::EINVAL as i64);
    }

    nanosleep_impl(&req, rem_ptr)
}

/// Internal nanosleep implementation shared by sys_nanosleep and sys_clock_nanosleep
fn nanosleep_impl(req: &Timespec, rem_ptr: *mut Timespec) -> i64 {
    use crate::drivers::timer;
    use crate::process;

    let total_nanos = req.tv_sec.saturating_mul(1_000_000_000).saturating_add(req.tv_nsec);

    // If sleep time is 0, return immediately
    if total_nanos == 0 {
        return 0;
    }

    // ---- Precise (high-resolution) deadline ----
    // The time CSR runs at TIMER_CLOCK_FREQ_HZ (10 MHz, 100 ns/tick).
    // Round the request UP to whole ticks (+1 guard tick) so the sleeper
    // is NEVER woken early (LTP clock_nanosleep02: any early sample is a
    // failure). The jiffy wheel alone truncates and wakes up to one tick
    // (10 ms) short.
    let freq_hz = crate::config::TIMER_CLOCK_FREQ_HZ;
    let now0 = timer::read_time();
    let ns_to_ticks = ((total_nanos as u64).saturating_mul(freq_hz) / 1_000_000_000).saturating_add(1);
    let hres_deadline = now0.saturating_add(ns_to_ticks);

    // Jiffy bookkeeping for the wheel entry: the first jiffy that FULLY
    // covers the precise deadline (ceil + 1 grid tick). The softirq's
    // expiry decision uses expires_time exclusively for hres entries (a
    // jiffy fallback firing these EARLY — jiffies lag the time grid under
    // load — consumed the one-shot wake and hung the sleeper), so this
    // value no longer arms anything by itself; it is kept for wheel
    // diagnostics/readback symmetry.
    let start_jiffies = timer::get_jiffies();
    let ticks_per_jiffy = freq_hz / timer::HZ;
    let sleep_jiffies = (ns_to_ticks + ticks_per_jiffy - 1) / ticks_per_jiffy + 1;
    let target_jiffies = start_jiffies.saturating_add(sleep_jiffies);

    // Get current task pointer + PID for timer wakeup
    let current = match crate::sched::current() {
        Some(c) => c as *mut process::task::Task,
        None => return -(errno::EFAULT as i64),
    };
    // SAFETY: current is the currently running task's pointer from
    // sched::current(), valid for the whole syscall (we are it).
    let my_pid = unsafe { (*current).pid() };

    // Register a one-shot HIGH-RESOLUTION timer to wake us up at the
    // precise deadline (jiffy fallback inside the wheel).
    // Without this, the sleep below would have no mechanism to wake us up
    // — timer softirq would fire but nobody would call wake_up_process.
    let mut timer_id = crate::timer::add_timer_wakeup_hres(target_jiffies, hres_deadline, my_pid);
    // The hart timer may already be armed for the NEXT GRID TICK, which can
    // be LATER than our precise deadline — re-arm it now (we are running on
    // the hart that will sleep).
    timer::rearm_for_hres();

    // R33 (B-family wedge — lost wakeup): state-first + re-check protocol.
    // The timer softirq's wake is ONE-SHOT: it fires wake_up_process exactly
    // once and deletes the timer. Task::wake_up drops a wake that finds the
    // target not is_sleeping(), so a timer expiry landing between the
    // jiffies/signal check and the (old) Task::sleep's set_state was
    // silently discarded — the only waker was consumed and the task slept
    // forever (the `echo PP | cat` silent-hang signature: all CPUs idle,
    // the sleeper never re-checks). Marking INTERRUPTIBLE BEFORE the final
    // re-check closes the window: the racing wake either lands (flips us
    // RUNNING + enqueues while we still execute — undone by the dequeue
    // below, NEW-C2 discipline) or the re-check observes the advanced
    // jiffies / pending signal and we exit the loop instead of sleeping.
    // Mirrors sys_rt_sigtimedwait's established pattern.
    loop {
        // Precise wake condition: the time CSR has reached the deadline.
        let now = timer::read_time();

        // Check if target time has been reached
        if now >= hres_deadline {
            crate::timer::del_timer(timer_id);
            return 0;  // Success
        }

        // Calculate remaining time (ns, from the precise deadline)
        let remaining_ns = (hres_deadline - now) * (1_000_000_000 / crate::config::TIMER_CLOCK_FREQ_HZ);
        let remaining_msecs = (remaining_ns + 999_999) / 1_000_000;

        // Check for pending signals
        use crate::signal;
        if signal::signal_pending() {
            crate::timer::del_timer(timer_id);
            // Write remaining time to rem (if rem_ptr is provided).
            // A faulting rem write is EFAULT — Linux reports the copy
            // failure over the EINTR (LTP clock_nanosleep01's
            // bad-rmtp-with-signal case).
            if !rem_ptr.is_null() {
                // SAFETY: rem_ptr validated with access_ok in caller;
                // copy_to_user is the exception-table copy path (SUM=0 safe).
                unsafe {
                    // Convert milliseconds to timespec
                    let rem_sec = (remaining_msecs / 1000) as i64;
                    let rem_nsec = ((remaining_msecs % 1000) * 1_000_000) as i64;
                    if crate::arch::uaccess::copy_to_user(
                        rem_ptr as *mut u8,
                        &Timespec { tv_sec: rem_sec, tv_nsec: rem_nsec }
                            as *const Timespec as *const u8,
                        core::mem::size_of::<Timespec>(),
                    ) != 0
                    {
                        return -(errno::EFAULT as i64);
                    }
                }
            }

            return -(errno::EINTR as i64);
        }

        // Timer-fix (lost wakeup, belt-and-braces): the one-shot token may
        // already be spent while the deadline is still in the future — any
        // early expiry consumed the entry, and a refused/captured early
        // wake is absorbed by the re-checks below while the SLEEP ITSELF
        // would have no waker left. Re-arm a fresh timer in that case
        // (mirrors the wheel-side fix that stops hres entries firing on
        // the lagging jiffy clock; this guards against any other
        // early-consumption source). A failed re-arm (pool full) falls
        // through to the runnable-yield branch — never sleep waker-less.
        if timer_id != 0 && !crate::timer::timer_pending(timer_id) {
            timer_id = crate::timer::add_timer_wakeup_hres(
                target_jiffies,
                hres_deadline,
                my_pid,
            );
            if timer_id != 0 {
                timer::rearm_for_hres();
            }
        }

        if timer_id != 0 {
            // Mark INTERRUPTIBLE BEFORE the final re-check (state-first).
            // SAFETY: current is the running task's pointer (see above).
            unsafe {
                (*current).set_state(process::task::TaskState::new(
                    process::task::TaskState::INTERRUPTIBLE
                ));
            }

            // Re-check AFTER marking sleeping: the one-shot timer (or a
            // signal) may have fired between the checks above and the
            // set_state. Either its wake was captured by the INTERRUPTIBLE
            // state (we may already be RUNNING + enqueued again), or the
            // condition is now observable — either way we must not sleep.
            if timer::read_time() >= hres_deadline || signal::signal_pending() {
                // SAFETY: current is the running task's pointer.
                unsafe {
                    (*current).set_state(process::task::TaskState::new(
                        process::task::TaskState::RUNNING
                    ));
                }
                // NEW-C2: a wake in the window above may have enqueued us
                // while we are in fact still executing on this CPU — take
                // ourselves back off before looping, or a second CPU could
                // pick and run this very task.
                // SAFETY: current is the running task's pointer (see above).
                unsafe {
                    crate::sched::dequeue_task(&*current);
                }
                continue;
            }

            // Enable interrupts before schedule() — syscall context runs
            // with SIE=0; without this the local timer tick cannot fire and
            // lock_irqsave in __schedule would save SIE=0 for our wake path
            // (same discipline as do_wait / wait_event).
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
        } else {
            // Timer registration failed (timer table full): an
            // INTERRUPTIBLE sleep would have NO waker — the task would
            // block forever (until a signal). Stay runnable and yield so
            // the loop keeps re-checking jiffies.
            crate::sched::schedule();
        }
    }
}

/// sys_clock_settime - Set time of specified clock
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: tp - pointer to timespec structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_settime(args: SyscallArgs) -> i64 {
    let clk_id = args[0] as u32;
    let tp_ptr = args[1] as *const u8;

    // CAP_SYS_TIME required to set time
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -(errno::EPERM as i64);
    }

    // Only CLOCK_REALTIME can be set
    if clk_id != CLOCK_REALTIME {
        return -(errno::EINVAL as i64);
    }

    if tp_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(tp_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // struct timespec (64-bit): { i64 tv_sec; i64 tv_nsec; }
    let mut ts = [0i64; 2];
    // SAFETY: tp_ptr validated with access_ok; copy_from_user is the
    // exception-table copy path (SUM=0 safe) and zero-fills on fault.
    let residual = unsafe {
        crate::arch::uaccess::copy_from_user(
            ts.as_mut_ptr() as *mut u8,
            tp_ptr,
            16,
        )
    };
    if residual != 0 {
        return -(errno::EFAULT as i64);
    }

    // Linux (posix_clock_settime): tv_nsec must be normalized.
    if ts[1] < 0 || ts[1] > 999_999_999 {
        return -(errno::EINVAL as i64);
    }

    set_realtime_from_secs_nanos(ts[0], ts[1])
}

/// sys_clock_getres - Get clock resolution
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: res - pointer to timespec structure (for storing result)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_getres(args: SyscallArgs) -> i64 {
    let clk_id = args[0] as i32;
    let res = args[1] as *mut u64;

    // Validate clock ID
    match clk_id as u32 {
        CLOCK_REALTIME | CLOCK_TAI | CLOCK_REALTIME_COARSE | CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW
        | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME
        | CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {}
        _ => return -(errno::EINVAL as i64),
    }

    // Return actual timer resolution: 100ns for the 10 MHz timer, but the
    // COARSE clocks are jiffies-granular (1/HZ steps) — LTP
    // clock_gettime04 sizes its tolerance from clock_getres(), so a coarse
    // clock must not claim 100 ns.
    let res_ns: u64 = if clk_id as u32 == CLOCK_REALTIME_COARSE
        || clk_id as u32 == CLOCK_MONOTONIC_COARSE
    {
        1_000_000_000 / crate::drivers::timer::HZ
    } else {
        100
    };
    if !res.is_null() {
        // Check if res is in valid user space
        if !crate::arch::uaccess::access_ok(res as usize, 16) {  // 2 * sizeof(u64)
            return -(errno::EFAULT as i64);
        }
        // SAFETY: res validated with access_ok; put_user is the
        // exception-table copy path with SUM managed by uaccess.S. A raw
        // `*res = ...` store faults on the U-bit user page with SUM=0 and,
        // having no exception-table entry, panicked the kernel (Xorg's
        // clock_getres on its main stack, badaddr=0x3fffffe5a8).
        unsafe {
            // timespec structure: tv_sec (8 bytes) + tv_nsec (8 bytes)
            let ok_sec = crate::arch::uaccess::put_user(res, 0u64);          // tv_sec = 0
            let ok_nsec = crate::arch::uaccess::put_user(res.offset(1), res_ns);  // tv_nsec
            if !ok_sec || !ok_nsec {
                return -(errno::EFAULT as i64);
            }
        }
    }

    0
}

/// sys_getitimer - Get interval timer value
///
/// # Arguments
/// - args[0]: which - timer type (ITIMER_REAL=0, ITIMER_VIRTUAL=1, ITIMER_PROF=2)
/// - args[1]: curr_value - pointer to struct itimerval (output)
pub fn sys_getitimer(args: SyscallArgs) -> i64 {
    let which = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    if curr_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(curr_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    if which < 0 || which > 2 {
        return -(errno::EINVAL as i64);
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // struct itimerval { struct timeval it_interval, it_value }
    // struct timeval { time_t tv_sec, suseconds_t tv_usec }
    let (interval_sec, interval_usec, value_sec, value_usec) = if which == 0 {
        // ITIMER_REAL — remaining against the monotonic mirror recorded
        // at arm time (set_itimer_real).
        let deadline_ns = task.itimer_real[0].load(core::sync::atomic::Ordering::Acquire);
        if deadline_ns == 0 {
            // Disarmed
            (0i64, 0i64, 0i64, 0i64)
        } else {
            let interval_ns = task.itimer_real[1].load(core::sync::atomic::Ordering::Acquire);
            let (mono_sec, mono_nsec) = monotonic_time();
            let now_ns = (mono_sec as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(mono_nsec as u64);
            let remaining_ns = deadline_ns.saturating_sub(now_ns);
            (
                (interval_ns / 1_000_000_000) as i64,
                ((interval_ns % 1_000_000_000) / 1000) as i64,
                (remaining_ns / 1_000_000_000) as i64,
                ((remaining_ns % 1_000_000_000) / 1000) as i64,
            )
        }
    } else {
        // ITIMER_VIRTUAL / ITIMER_PROF — CPU-time timers tracked against
        // the sched entity's sum_exec_runtime. Remaining = deadline - now;
        // interval is stored verbatim at setitimer time.
        let state = if which == 1 { &task.itimer_virt } else { &task.itimer_prof };
        let deadline_ns = state[0].load(core::sync::atomic::Ordering::Acquire);
        if deadline_ns == 0 {
            (0i64, 0i64, 0i64, 0i64)
        } else {
            let interval_ns = state[1].load(core::sync::atomic::Ordering::Acquire);
            let now_ns = task
                .sched_entity()
                .sum_exec_runtime
                .load(core::sync::atomic::Ordering::Acquire);
            let remaining_ns = deadline_ns.saturating_sub(now_ns);
            (
                (interval_ns / 1_000_000_000) as i64,
                ((interval_ns % 1_000_000_000) / 1_000) as i64,
                (remaining_ns / 1_000_000_000) as i64,
                ((remaining_ns % 1_000_000_000) / 1_000) as i64,
            )
        }
    };

    // Write struct itimerval
    // SAFETY: curr_value validated with access_ok(32); put_user goes through
    // the exception-table copy path (SUM=0 safe).
    unsafe {
        // it_interval (offset 0)
        let p = curr_value as *mut i64;
        let _ = crate::arch::uaccess::put_user(p, interval_sec);
        let _ = crate::arch::uaccess::put_user(p.add(1), interval_usec);
        // it_value (offset 16)
        let _ = crate::arch::uaccess::put_user(p.add(2), value_sec);
        let _ = crate::arch::uaccess::put_user(p.add(3), value_usec);
    }

    0
}

/// sys_setitimer - Set interval timer
///
/// # Arguments
/// - args[0]: which - timer type (ITIMER_REAL=0, ITIMER_VIRTUAL=1, ITIMER_PROF=2)
/// - args[1]: new_value - pointer to struct itimerval
/// - args[2]: old_value - pointer to struct itimerval (output, may be NULL)
pub fn sys_setitimer(args: SyscallArgs) -> i64 {
    let which = args[0] as i32;
    let new_value = args[1] as *const u64;
    let old_value = args[2] as *mut u64;

    if which < 0 || which > 2 {
        return -(errno::EINVAL as i64);
    }

    // Write old_value: the timer's CURRENT state (remaining + interval)
    // BEFORE re-arming — musl's alarm() derives its return value from
    // this, and getitimer/setitimer old-value reporting is POSIX
    // observable (LTP alarm02: alarm(0) after alarm(N) returns ~N).
    if !old_value.is_null() {
        if !crate::arch::uaccess::access_ok(old_value as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        let (i_sec, i_usec, v_sec, v_usec) = match crate::process::current_task() {
            Some(t) => {
                if which == 0 {
                    let deadline_ns = t.itimer_real[0].load(core::sync::atomic::Ordering::Acquire);
                    if deadline_ns == 0 {
                        (0i64, 0i64, 0i64, 0i64)
                    } else {
                        let interval_ns = t.itimer_real[1].load(core::sync::atomic::Ordering::Acquire);
                        let (mono_sec, mono_nsec) = monotonic_time();
                        let now_ns = (mono_sec as u64)
                            .saturating_mul(1_000_000_000)
                            .saturating_add(mono_nsec as u64);
                        let remaining_ns = deadline_ns.saturating_sub(now_ns);
                        (
                            (interval_ns / 1_000_000_000) as i64,
                            ((interval_ns % 1_000_000_000) / 1000) as i64,
                            (remaining_ns / 1_000_000_000) as i64,
                            ((remaining_ns % 1_000_000_000) / 1000) as i64,
                        )
                    }
                } else {
                    let state = if which == 1 { &t.itimer_virt } else { &t.itimer_prof };
                    let deadline_ns = state[0].load(core::sync::atomic::Ordering::Acquire);
                    if deadline_ns == 0 {
                        (0i64, 0i64, 0i64, 0i64)
                    } else {
                        let interval_ns = state[1].load(core::sync::atomic::Ordering::Acquire);
                        let now_ns = t
                            .sched_entity()
                            .sum_exec_runtime
                            .load(core::sync::atomic::Ordering::Acquire);
                        let remaining_ns = deadline_ns.saturating_sub(now_ns);
                        (
                            (interval_ns / 1_000_000_000) as i64,
                            ((interval_ns % 1_000_000_000) / 1000) as i64,
                            (remaining_ns / 1_000_000_000) as i64,
                            ((remaining_ns % 1_000_000_000) / 1000) as i64,
                        )
                    }
                }
            }
            None => (0i64, 0i64, 0i64, 0i64),
        };
        // SAFETY: old_value validated with access_ok(32); put_user is the
        // exception-table copy path (SUM=0 safe).
        unsafe {
            let p = old_value as *mut i64;
            let _ = crate::arch::uaccess::put_user(p, i_sec);
            let _ = crate::arch::uaccess::put_user(p.add(1), i_usec);
            let _ = crate::arch::uaccess::put_user(p.add(2), v_sec);
            let _ = crate::arch::uaccess::put_user(p.add(3), v_usec);
        }
    }

    if new_value.is_null() {
        // Disarm the timer
        if which == 0 {
            disarm_itimer_real();
        }
        return 0;
    }

    if !crate::arch::uaccess::access_ok(new_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    // Read struct itimerval via the exception-table copy path so an
    // unmapped user page yields EFAULT instead of a kernel page fault.
    let mut itimer_bits = [0u8; 32];
    let uncopied = unsafe {
        crate::arch::uaccess::copy_from_user(
            itimer_bits.as_mut_ptr(),
            new_value as *const u8,
            32,
        )
    };
    if uncopied > 0 {
        return -(errno::EFAULT as i64);
    }
    let rd = |i: usize| i64::from_le_bytes(itimer_bits[i * 8..i * 8 + 8].try_into().unwrap());
    let (interval_sec, interval_usec, value_sec, value_usec) = (rd(0), rd(1), rd(2), rd(3));

    if which == 0 {
        // ITIMER_REAL — arm using kernel timer wheel
        set_itimer_real(interval_sec, interval_usec, value_sec, value_usec);
    } else if which == 1 || which == 2 {
        // ITIMER_VIRTUAL / ITIMER_PROF — CPU-time timers checked in
        // scheduler_tick against sum_exec_runtime. value_ns is counted
        // from the CURRENT cumulative CPU time (Linux arms from now).
        let task = match crate::process::current_task() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        };
        let state = if which == 1 { &task.itimer_virt } else { &task.itimer_prof };
        let value_ns = (value_sec.saturating_mul(1_000_000_000)
            .saturating_add(value_usec.saturating_mul(1_000)))
            .max(0) as u64;
        let interval_ns = (interval_sec.saturating_mul(1_000_000_000)
            .saturating_add(interval_usec.saturating_mul(1_000)))
            .max(0) as u64;
        let deadline = if value_ns != 0 {
            task.sched_entity()
                .sum_exec_runtime
                .load(core::sync::atomic::Ordering::Acquire)
                .saturating_add(value_ns)
        } else {
            0 // disarm
        };
        state[0].store(deadline, core::sync::atomic::Ordering::Release);
        state[1].store(interval_ns, core::sync::atomic::Ordering::Release);
    }

    0
}

/// Disarm ITIMER_REAL timer for the current process.
fn disarm_itimer_real() {
    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return,
    };

    let old_timer_id = task.itimer_ids[0].swap(0, core::sync::atomic::Ordering::AcqRel);
    if old_timer_id != 0 {
        crate::timer::del_timer(old_timer_id);
    }
    task.itimer_real[0].store(0, core::sync::atomic::Ordering::Release);
    task.itimer_real[1].store(0, core::sync::atomic::Ordering::Release);
}

/// Set ITIMER_REAL timer for the current process.
fn set_itimer_real(interval_sec: i64, interval_usec: i64, value_sec: i64, value_usec: i64) {
    use crate::drivers::timer;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return,
    };

    let pid = task.pid();

    // Disarm existing timer
    let old_timer_id = task.itimer_ids[0].swap(0, core::sync::atomic::Ordering::AcqRel);
    if old_timer_id != 0 {
        crate::timer::del_timer(old_timer_id);
    }

    // If value is zero, just disarm (already done above) — and CLEAR the
    // mirrored deadline so the next setitimer's old-value read reports a
    // DISARMED timer (musl alarm() derives its return value from it; the
    // stale deadline made alarm(N) after alarm(0) return the cancelled
    // timer's remaining time — LTP alarm02).
    let total_usec = value_sec.saturating_mul(1_000_000).saturating_add(value_usec);
    if total_usec <= 0 {
        task.itimer_real[0].store(0, core::sync::atomic::Ordering::Release);
        task.itimer_real[1].store(0, core::sync::atomic::Ordering::Release);
        return;
    }

    // Convert to jiffies (minimum 1)
    let value_msecs = (total_usec / 1000) as u64;
    let value_jiffies = timer::msecs_to_jiffies(value_msecs).max(1);
    let expires = timer::get_jiffies() + value_jiffies;

    // Compute interval in jiffies
    let interval_usec_total = interval_sec.saturating_mul(1_000_000).saturating_add(interval_usec);
    let interval_jiffies = if interval_usec_total > 0 {
        let interval_msecs = (interval_usec_total / 1000) as u64;
        timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0 // one-shot
    };

    let new_timer_id = crate::timer::add_timer_with_action(
        expires,
        pid,
        crate::signal::Signal::SIGALRM as i32,
        interval_jiffies,
        0,
    );

    task.itimer_ids[0].store(new_timer_id, core::sync::atomic::Ordering::Release);

    // Mirror the armed state for getitimer/setitimer-old-value (and so
    // musl's alarm() can report the previous remaining time).
    let (mono_sec, mono_nsec) = monotonic_time();
    let now_ns = mono_sec.saturating_mul(1_000_000_000).saturating_add(mono_nsec as u64);
    let deadline_ns = now_ns.saturating_add((total_usec as u64).saturating_mul(1000));
    task.itimer_real[0].store(deadline_ns, core::sync::atomic::Ordering::Release);
    task.itimer_real[1].store(
        (interval_usec_total.max(0) as u64).saturating_mul(1000),
        core::sync::atomic::Ordering::Release,
    );
}

/// sys_clock_nanosleep - High-resolution sleep (with specified clock)
///
/// # Arguments
/// - args[0]: clk_id - clock ID
/// - args[1]: flags - flags (TIMER_ABSTIME = 1)
/// - args[2]: rqtp - requested sleep time
/// - args[3]: rmtp - remaining time (when interrupted by signal)
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_clock_nanosleep(args: SyscallArgs) -> i64 {
    let clk_id = args[0] as u32;
    let flags = args[1] as i32; // bit0 = TIMER_ABSTIME
    let rqtp = args[2] as *const Timespec;
    let rmtp = args[3] as *mut Timespec;

    // ABI correction (LTP clock_nanosleep01): the KERNEL syscall returns
    // -errno like every other syscall — musl's __clock_nanosleep NEGATES it
    // (`return -__syscall_cp(...)`) to produce the POSIX-style positive
    // error number. Returning the positive errno here leaked through musl
    // as a NEGATIVE libc return and through syscall(2) as a bogus success
    // value.
    let fail = |e: i32| -> i64 { -(e as i64) };

    // Only the sleepable clocks are valid here. THREAD-cputime has no
    // nsleep op in Linux and fails with ENOTSUP (LTP clock_nanosleep01
    // expects EOPNOTSUPP for the raw-syscall variant).
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC => {}
        CLOCK_THREAD_CPUTIME_ID => return fail(errno::EOPNOTSUPP),
        _ => return fail(errno::EINVAL),
    }

    // Only TIMER_ABSTIME (bit 0) is a defined flag; anything else is EINVAL.
    if flags & !1 != 0 {
        return fail(errno::EINVAL);
    }

    // Validate request pointer
    if rqtp.is_null() {
        return fail(errno::EFAULT);
    }

    // Check if rqtp is in valid user space
    if !crate::arch::uaccess::access_ok(rqtp as usize, core::mem::size_of::<Timespec>()) {
        return fail(errno::EFAULT);
    }

    // Check rmtp if provided (only meaningful without TIMER_ABSTIME, but
    // validate whatever the caller passed)
    if !rmtp.is_null() && !crate::arch::uaccess::access_ok(rmtp as usize, core::mem::size_of::<Timespec>()) {
        return fail(errno::EFAULT);
    }

    // Read requested sleep time. copy_from_user (exception-table): a raw
    // dereference faults in S-mode on every U page (SUM=0) and panics the
    // kernel — glibc sleep() hits this on its first call.
    let mut req = Timespec { tv_sec: 0, tv_nsec: 0 };
    {
        let uncopied = unsafe {
            crate::arch::uaccess::copy_from_user(
                &mut req as *mut Timespec as *mut u8,
                rqtp as *const u8,
                core::mem::size_of::<Timespec>(),
            )
        };
        if uncopied != 0 {
            return fail(errno::EFAULT);
        }
    }

    if req.tv_nsec < 0 || req.tv_nsec > 999_999_999 {
        return fail(errno::EINVAL);
    }

    // TIMER_ABSTIME: rqtp is an absolute timestamp on the selected clock —
    // sleep until then, not for that duration (pthread_cond_timedwait depends
    // on this; without it every absolute wait slept for decades — review M-16).
    if flags & 1 != 0 {
        // Current monotonic time in ns (same source as clock_gettime).
        let (mono_sec, mono_nsec) = monotonic_time();
        let now_nanos = mono_sec.saturating_mul(1_000_000_000).saturating_add(mono_nsec);
        // For CLOCK_REALTIME the deadline is expressed against the wall
        // epoch: shift it back into the monotonic domain.
        let epoch_nanos = wall_epoch_offset_ns();
        let target_nanos = (req.tv_sec.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(req.tv_nsec as u64);
        let target_monotonic = if clk_id == CLOCK_REALTIME {
            target_nanos.saturating_sub(epoch_nanos)
        } else {
            target_nanos
        };
        if target_monotonic <= now_nanos {
            return 0; // deadline already passed
        }
        let rel_nanos = target_monotonic - now_nanos;
        let rel = Timespec {
            tv_sec: (rel_nanos / 1_000_000_000) as i64,
            tv_nsec: ((rel_nanos % 1_000_000_000)) as i64,
        };
        let r = nanosleep_impl(&rel, core::ptr::null_mut()); // rmtp ignored with ABSTIME
        return if r < 0 { fail((-r) as i32) } else { r };
    }

    if req.tv_sec < 0 {
        return fail(errno::EINVAL);
    }

    let r = nanosleep_impl(&req, rmtp);
    if r < 0 { fail((-r) as i32) } else { r }
}

/// sys_timer_create - Create POSIX interval timer (NR 107)
///
/// Creates a per-process POSIX timer. The timer ID is returned via timerid_ptr.
pub fn sys_timer_create(args: SyscallArgs) -> i64 {
    let clockid = args[0] as i32;
    let sigevent_ptr = args[1] as *const u8;
    let timerid_ptr = args[2] as *mut i32;

    if timerid_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(timerid_ptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    // CLOCK_REALTIME (0), CLOCK_MONOTONIC (1) arm on the wall timer
    // wheel; the CPU-time clocks (2/3) arm against sum_exec_runtime and
    // are checked in scheduler_tick (LTP timer_settime01/timer_delete01
    // create timers on all four).
    if !(0..=3).contains(&clockid) {
        return -(errno::EINVAL as i64);
    }

    // Parse sigevent for signal notification
    let mut sigev_signo = crate::signal::Signal::SIGALRM as i32;
    let mut sigev_notify = 0; // SIGEV_SIGNAL

    if !sigevent_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(sigevent_ptr as usize, 64) {
            return -(errno::EFAULT as i64);
        }
        // struct sigevent { sigval sigev_value, int sigev_signo, int sigev_notify, ... }
        // SAFETY: sigevent_ptr validated with access_ok(64); get_user goes
        // through the exception-table copy path (SUM=0 safe).
        unsafe {
            let p = sigevent_ptr as *const i32;
            // sigev_value is 8 bytes (union), then sigev_signo at offset 8
            let signo = crate::arch::uaccess::get_user(p.add(2)).unwrap_or(0);
            let notify = crate::arch::uaccess::get_user(p.add(3)).unwrap_or(0);
            if signo > 0 && signo <= 64 {
                sigev_signo = signo;
            }
            sigev_notify = notify;
        }
    }

    // Allocate a stable, never-reused timer handle. Previously the id was
    // `timers.len() + 1` and lookups were by Vec index: deleting a middle
    // timer shifted every later entry and silently redirected operations on
    // surviving timers to the WRONG timer (review批次1). Ids only need to be
    // unique within the process; a global monotonic source guarantees it
    // across create/delete cycles for the process' lifetime.
    static NEXT_POSIX_TIMER_ID: core::sync::atomic::AtomicI32 =
        core::sync::atomic::AtomicI32::new(1);
    let user_timer_id = NEXT_POSIX_TIMER_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    let state = crate::process::task::PosixTimerState {
        kernel_timer_id: 0,
        clock_id: clockid,
        interval_jiffies: 0,
        sigev_signo,
        sigev_notify,
        overrun_count: 0,
        user_timer_id: user_timer_id,
        cputime_deadline_ns: core::sync::atomic::AtomicU64::new(0),
        cputime_interval_ns: core::sync::atomic::AtomicU64::new(0),
        wall_deadline_ticks: core::sync::atomic::AtomicU64::new(0),
        interval_ns: 0,
    };

    let mut timers = task.posix_timers.lock();
    timers.push(state);
    // SAFETY: timerid_ptr validated with access_ok(4); put_user is the
    // exception-table copy path.
    unsafe {
        let _ = crate::arch::uaccess::put_user(timerid_ptr, user_timer_id);
    }

    0
}

/// sys_timer_settime - Set timer value (NR 110)
///
/// # Arguments
/// - args[0]: timerid - timer ID (returned by timer_create)
/// - args[1]: flags - TIMER_ABSTIME (1) for absolute time
/// - args[2]: new_value - new timer settings (struct itimerspec, 32 bytes)
/// - args[3]: old_value - old timer settings (output)
pub fn sys_timer_settime(args: SyscallArgs) -> i64 {
    let timerid = args[0] as i32;
    let flags = args[1] as i32;
    let new_value = args[2] as *const u64;
    let old_value = args[3] as *mut u64;

    // Linux (common_timer_set): a NULL new_value pointer is EINVAL, and
    // only TIMER_ABSTIME (bit 0) is a legal flag (LTP timer_settime02
    // cases 1 and the flags checks).
    if new_value.is_null() {
        return -(errno::EINVAL as i64);
    }
    if flags & !1 != 0 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(new_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Read struct itimerspec { struct timespec it_interval, struct timespec it_value }
    // SAFETY: new_value validated with access_ok(32); get_user is the
    // exception-table copy path (SUM=0 safe). Unreadable fields read as 0.
    let (int_sec, int_nsec, val_sec, val_nsec) = unsafe {
        let p = new_value as *const i64;
        let get = crate::arch::uaccess::get_user::<i64>;
        match (get(p), get(p.add(1)), get(p.add(2)), get(p.add(3))) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return -(errno::EFAULT as i64),
        }
    };

    // Linux: tv_nsec of BOTH members must be normalized [0, 1e9) and
    // tv_sec non-negative (LTP timer_settime02: -1 and NSEC_PER_SEC+1
    // must fail with EINVAL, not be silently accepted).
    if int_sec < 0 || int_nsec < 0 || int_nsec > 999_999_999
        || val_sec < 0 || val_nsec < 0 || val_nsec > 999_999_999
    {
        return -(errno::EINVAL as i64);
    }

    // Find timer by stable user handle (review批次1: index arithmetic
    // misdirected operations after a middle timer was deleted).
    let mut timers = task.posix_timers.lock();
    let timer = match timers.iter_mut().find(|t| t.user_timer_id == timerid) {
        Some(t) => t,
        None => return -(errno::EINVAL as i64),
    };

    // Write old_value (the CURRENT settings — real values, not zeros)
    if !old_value.is_null() {
        if !crate::arch::uaccess::access_ok(old_value as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        let (oi, on, ov, vn) = posix_timer_current(timer);
        // SAFETY: old_value validated with access_ok(32); put_user is the
        // exception-table copy path (SUM=0 safe).
        unsafe {
            let p = old_value as *mut i64;
            let put = crate::arch::uaccess::put_user;
            let _ = put(p, oi);
            let _ = put(p.add(1), on);
            let _ = put(p.add(2), ov);
            let _ = put(p.add(3), vn);
        }
    }

    let pid = task.pid();

    // Disarm existing kernel timer / cpu-time deadline
    if timer.kernel_timer_id != 0 {
        crate::timer::del_timer(timer.kernel_timer_id);
        timer.kernel_timer_id = 0;
    }
    timer.wall_deadline_ticks.store(0, core::sync::atomic::Ordering::Release);
    timer.cputime_deadline_ns.store(0, core::sync::atomic::Ordering::Release);

    // If value is zero, timer is disarmed
    let total_nsec = val_sec.saturating_mul(1_000_000_000).saturating_add(val_nsec);
    if total_nsec <= 0 {
        timer.interval_ns = 0;
        timer.cputime_interval_ns.store(0, core::sync::atomic::Ordering::Release);
        return 0;
    }

    let interval_nsec = int_sec.saturating_mul(1_000_000_000).saturating_add(int_nsec);
    let interval_jiffies = if interval_nsec > 0 {
        let interval_msecs = (interval_nsec / 1_000_000) as u64;
        crate::drivers::timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0
    };
    timer.interval_ns = interval_nsec as u64;
    timer.interval_jiffies = interval_jiffies;
    timer.cputime_interval_ns.store(interval_nsec as u64, core::sync::atomic::Ordering::Release);

    // ---- CPU-time clocks: arm against sum_exec_runtime ----
    if timer.clock_id == CLOCK_PROCESS_CPUTIME_ID as i32 || timer.clock_id == CLOCK_THREAD_CPUTIME_ID as i32 {
        let now_cpu_ns = task
            .sched_entity()
            .sum_exec_runtime
            .load(core::sync::atomic::Ordering::Acquire);
        timer.cputime_deadline_ns.store(
            now_cpu_ns.saturating_add(total_nsec as u64),
            core::sync::atomic::Ordering::Release,
        );
        timer.overrun_count = 0;
        return 0;
    }

    // ---- Wall clocks (REALTIME / MONOTONIC): precise deadline ----
    let freq_hz: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;
    let now_ticks = crate::drivers::timer::read_time();
    // Relative or absolute target, in MONOTONIC ns.
    // Signed monotonic target: TIMER_ABSTIME values BEFORE the clock's
    // zero point are legal (LTP timer_settime03 arms ~300 s in the past)
    // and count fully as missed periods. TIMER_ABSTIME on CLOCK_REALTIME
    // addresses the WALL clock — shift back to the monotonic domain with
    // the epoch offset (the old code always read it as monotonic, so an
    // absolute wall deadline after a clock_settime never fired — LTP
    // clock_settime03 hung waiting for SIGABRT).
    let now_ns_i = (now_ticks as u64)
        .saturating_mul(1_000_000_000 / freq_hz) as i64;
    let target_mono_ns: i64 = if flags & 1 != 0 {
        let abs_ns = val_sec
            .saturating_mul(1_000_000_000)
            .saturating_add(val_nsec);
        if timer.clock_id == CLOCK_REALTIME as i32 {
            abs_ns.saturating_sub(wall_epoch_offset_ns() as i64)
        } else {
            abs_ns
        }
    } else {
        now_ns_i.saturating_add(total_nsec)
    };

    // Precise absolute deadline in time CSR ticks (round UP, never early).
    // A non-positive target is already expired: arm at "now".
    let deadline_ticks: u64 = if target_mono_ns <= 0 {
        0
    } else {
        ((target_mono_ns as u64)
            .saturating_mul(freq_hz)
            .div_ceil(1_000_000_000))
        .saturating_add(1)
    };
    timer.wall_deadline_ticks.store(deadline_ticks, core::sync::atomic::Ordering::Release);

    // Jiffy fallback (>= the jiffy covering the precise deadline + 1 grid
    // tick); the hres deadline normally fires first.
    let ticks_per_jiffy = freq_hz / crate::drivers::timer::HZ;
    let now_j = crate::drivers::timer::get_jiffies();
    let expires = if deadline_ticks <= now_ticks {
        // Already expired. With an interval, Linux reports the number of
        // MISSED periods as the overrun (capped at INT_MAX — LTP
        // timer_settime03 arms an absolute deadline ~INT_MAX periods in
        // the past and reads timer_getoverrun).
        if interval_nsec > 0 {
            let late_ns = now_ns_i.saturating_sub(target_mono_ns);
            let missed = if late_ns <= 0 {
                0u64
            } else {
                (late_ns as u64) / interval_nsec as u64
            };
            timer.overrun_count = missed.min(i32::MAX as u64) as i32;
        }
        // Fire at the next tick.
        now_j + 1
    } else {
        let rel_ticks = deadline_ticks - now_ticks;
        now_j.saturating_add(rel_ticks.div_ceil(ticks_per_jiffy)).saturating_add(1)
    };

    // Arm with the precise deadline (add_timer_with_action_hres).
    let new_kernel_id = crate::timer::add_timer_with_action_hres(
        expires,
        deadline_ticks,
        pid,
        timer.sigev_signo,
        interval_jiffies,
        0,
    );

    timer.kernel_timer_id = new_kernel_id;
    // NOTE: overrun_count was already seeded by the late-ABSTIME accounting
    // above (missed periods); a fresh on-time arm leaves it at 0.

    0
}

/// Current (it_interval sec/nsec, it_value sec/nsec) of a POSIX timer —
/// precise readback for timer_gettime and settime old_value.
fn posix_timer_current(timer: &crate::process::task::PosixTimerState) -> (i64, i64, i64, i64) {
    let interval_ns = timer.interval_ns;
    let oi = (interval_ns / 1_000_000_000) as i64;
    let on = (interval_ns % 1_000_000_000) as i64;

    if timer.clock_id == CLOCK_PROCESS_CPUTIME_ID as i32 || timer.clock_id == CLOCK_THREAD_CPUTIME_ID as i32 {
        let deadline = timer.cputime_deadline_ns.load(core::sync::atomic::Ordering::Acquire);
        if deadline == 0 {
            return (oi, on, 0, 0);
        }
        match crate::process::current_task() {
            Some(t) => {
                let now = t
                    .sched_entity()
                    .sum_exec_runtime
                    .load(core::sync::atomic::Ordering::Acquire);
                let rem = deadline.saturating_sub(now);
                (oi, on, (rem / 1_000_000_000) as i64, (rem % 1_000_000_000) as i64)
            }
            None => (oi, on, 0, 0),
        }
    } else {
        let deadline_ticks = timer.wall_deadline_ticks.load(core::sync::atomic::Ordering::Acquire);
        if deadline_ticks == 0 {
            return (oi, on, 0, 0);
        }
        let now = crate::drivers::timer::read_time();
        if deadline_ticks <= now {
            return (oi, on, 0, 0);
        }
        let rem_ns = (deadline_ticks - now).saturating_mul(1_000_000_000 / crate::config::TIMER_CLOCK_FREQ_HZ);
        (oi, on, (rem_ns / 1_000_000_000) as i64, (rem_ns % 1_000_000_000) as i64)
    }
}

/// sys_timer_gettime - Get timer value (NR 108)
pub fn sys_timer_gettime(args: SyscallArgs) -> i64 {
    let timerid = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    if curr_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(curr_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Lookup by stable user handle, not Vec index (review批次1).
    let timers = task.posix_timers.lock();
    let timer = match timers.iter().find(|t| t.user_timer_id == timerid) {
        Some(t) => t,
        None => return -(errno::EINVAL as i64),
    };

    // Precise readback: it_interval as programmed (ns) and it_value from
    // the stored absolute deadline (LTP timer_settime01 checks
    // interval == 50 ms and value <= max(value, interval)).
    let (int_sec, int_nsec, val_sec, val_nsec) = posix_timer_current(timer);

    // Write struct itimerspec { struct timespec it_interval, struct timespec it_value }
    // SAFETY: curr_value validated with access_ok(32); put_user is the
    // exception-table copy path (SUM=0 safe).
    unsafe {
        let p = curr_value as *mut i64;
        let put = crate::arch::uaccess::put_user;
        let ok1 = put(p, int_sec);
        let ok2 = put(p.add(1), int_nsec);
        let ok3 = put(p.add(2), val_sec);
        let ok4 = put(p.add(3), val_nsec);
        if !ok1 || !ok2 || !ok3 || !ok4 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_timer_getoverrun - Get timer overrun count (NR 109)
pub fn sys_timer_getoverrun(args: SyscallArgs) -> i64 {
    let timerid = args[0] as i32;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Lookup by stable user handle, not Vec index (review批次1).
    let timers = task.posix_timers.lock();
    match timers.iter().find(|t| t.user_timer_id == timerid) {
        Some(t) => t.overrun_count as i64,
        None => -(errno::EINVAL as i64),
    }
}

/// sys_timer_delete - Delete POSIX timer (NR 111)
pub fn sys_timer_delete(args: SyscallArgs) -> i64 {
    let timerid = args[0] as i32;

    let task = match crate::process::current_task() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Lookup by stable user handle. Removing the entry is safe now that
    // every other operation resolves handles by value (no index shift).
    let mut timers = task.posix_timers.lock();
    let idx = match timers.iter().position(|t| t.user_timer_id == timerid) {
        Some(i) => i,
        None => return -(errno::EINVAL as i64),
    };

    // Disarm kernel timer / cpu-time deadline
    if let Some(timer) = timers.get_mut(idx) {
        if timer.kernel_timer_id != 0 {
            crate::timer::del_timer(timer.kernel_timer_id);
        }
        timer.cputime_deadline_ns.store(0, core::sync::atomic::Ordering::Release);
        timer.wall_deadline_ticks.store(0, core::sync::atomic::Ordering::Release);
    }

    timers.remove(idx);
    0
}

/// Set CLOCK_REALTIME to `tv_sec` seconds + `tv_nsec` nanoseconds.
///
/// Both settimeofday and clock_settime(CLOCK_REALTIME) funnel here. The
/// monotonic CLINT clock is never touched — only WALL_EPOCH_OFFSET_NS
/// (and the vDSO data-page snapshot) moves, so CLOCK_MONOTONIC, timers
/// and /proc/uptime are unaffected, matching Linux semantics. The offset
/// is kept with NANOSECOND precision — clock_settime01 advances/recades
/// the clock by 10 ms deltas and reads it back through both the syscall
/// and the vDSO fast path.
fn set_realtime_from_secs_nanos(tv_sec: i64, tv_nsec: i64) -> i64 {
    // Negative absolute times are not representable (unsigned offset).
    if tv_sec < 0 || tv_nsec < 0 {
        return -(errno::EINVAL as i64);
    }

    let (mono_s, mono_ns) = monotonic_time();
    // wall = mono + offset  ⇒  offset = wall - mono.
    // Saturating: a target before boot clamps the offset to 0 (epoch),
    // which is the closest representable time.
    let target_ns = (tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(tv_nsec as u64);
    let mono_total_ns = mono_s
        .saturating_mul(1_000_000_000)
        .saturating_add(mono_ns);
    let offset_ns = target_ns.saturating_sub(mono_total_ns);
    set_wall_epoch_offset_ns(offset_ns);
    0
}

/// sys_settimeofday - Set wall-clock time (NR 170)
pub fn sys_settimeofday(args: SyscallArgs) -> i64 {
    let tv_ptr = args[0] as *const u8;
    let _tz_ptr = args[1] as *const u8; // timezone is deprecated and ignored

    // CAP_SYS_TIME required to set time
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -(errno::EPERM as i64);
    }
    if tv_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(tv_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // struct timeval (64-bit): { i64 tv_sec; i64 tv_usec; }
    let mut tv = [0i64; 2];
    // SAFETY: tv_ptr validated with access_ok; copy_from_user is the
    // exception-table copy path (SUM=0 safe) and zero-fills on fault.
    let residual = unsafe {
        crate::arch::uaccess::copy_from_user(
            tv.as_mut_ptr() as *mut u8,
            tv_ptr,
            16,
        )
    };
    if residual != 0 {
        return -(errno::EFAULT as i64);
    }

    // Linux (do_settimeofday64): tv_usec must be normalized.
    if tv[1] < 0 || tv[1] > 999_999 {
        return -(errno::EINVAL as i64);
    }

    set_realtime_from_secs_nanos(tv[0], tv[1].saturating_mul(1_000))
}

/// sys_adjtimex - Adjust system clock (NR 171)
///
/// struct timex is 128 bytes on 64-bit. We fill it as "clock synchronized".
/// Kernel NTP state actually modeled: the settable timex fields are
/// stored and read back (LTP clock_adjtime01 round-trips every mode's
/// value through a verify GET).
///
/// struct timex (LP64 — offsets verified against glibc sys/timex.h):
/// modes i32 @0; offset @8, freq @16, maxerror @24, esterror @32 (i64);
/// status i32 @40; constant @48, precision @56, tolerance @64 (i64);
/// time.tv_sec @72, time.tv_usec @80 (i64); tick @88; ppsfreq @96;
/// jitter @104; shift i32 @112.
static TIMEX_STATE: crate::sync::spinlock::Spinlock<[u8; 96]> =
    crate::sync::spinlock::Spinlock::new([0u8; 96]);

/// Shared adjtimex/clock_adjtime core: copy the timex in, validate it,
/// apply the settable fields, report TIME_OK.
fn adjtimex_common(buf_ptr: *mut u8) -> i64 {
    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(buf_ptr as usize, 128) {
        return -(errno::EFAULT as i64);
    }

    // Read the request (a bad pointer is EFAULT — LTP clock_adjtime02's
    // bad_addr case; the old stub never read the buffer at all).
    let mut buf = [0u8; 128];
    // SAFETY: buf_ptr validated with access_ok(128); copy_from_user is the
    // exception-table copy path (SUM=0 safe).
    if unsafe {
        crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), buf_ptr, 128)
    } != 0
    {
        return -(errno::EFAULT as i64);
    }
    let modes = u32::from_le_bytes(buf[0..4].try_into().unwrap());

    // Non-zero modes require CAP_SYS_TIME.
    if modes != 0 && !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -(errno::EPERM as i64);
    }

    // Mode validation (LTP adjtimex03): only the bits Linux actually
    // implements are accepted; ADJ_MICRO (0x1000) is nano-kernel-only
    // legacy (rejected with EINVAL), and so is any undefined bit (the
    // test probes 0x8000).
    const ADJ_KNOWN_MODES: u32 = 0x007F      // offset|freq|maxerror|esterror|status|constant
        | 0x0080                              // ADJ_TAI
        | 0x0100                              // ADJ_SETOFFSET
        | 0x1000                              // ADJ_MICRO (unit flag, accepted)
        | 0x2000                              // ADJ_NANO / ADJ_OFFSET_READONLY
        | 0x4000                              // ADJ_TICK
        | 0x8000;                             // ADJ_ADJTIME
    if modes & !ADJ_KNOWN_MODES != 0 {
        return -(errno::EINVAL as i64);
    }
    // ADJ_ADJTIME alone (without ADJ_OFFSET / ADJ_OFFSET_READONLY) is the
    // classic invalid combination (LTP adjtimex03 probes exactly 0x8000).
    const ADJ_ADJTIME: u32 = 0x8000;
    if modes & ADJ_ADJTIME != 0
        && modes & (0x001 | 0x2000) == 0
    {
        return -(errno::EINVAL as i64);
    }

    // ADJ_TICK bounds: user tick (usec/s per HZ-scaled units) must stay in
    // [900000/HZ, 1100000/HZ] (LTP clock_adjtime02's low/high cases);
    // tick == 0 means "leave unchanged".
    const ADJ_TICK: u32 = 0x4000;
    if modes & ADJ_TICK != 0 {
        let tick = i32::from_le_bytes(buf[88..92].try_into().unwrap());
        if tick != 0 {
            let hz = crate::drivers::timer::HZ as i32;
            let low = 900_000 / hz;
            let high = 1_100_000 / hz;
            if !(low..=high).contains(&tick) {
                return -(errno::EINVAL as i64);
            }
        }
    }

    // Apply the settable fields to the kernel state and echo the FULL
    // state back (clock_adjtime01 verifies a round-trip of every field).
    let mut state = TIMEX_STATE.lock();
    // Seed the Linux default tick (1e6/HZ usec) on first use so an
    // ADJ_TICK delta from the GET value stays inside the valid window
    // (clock_adjtime01 reads tick then adds delta).
    if state[88..96] == [0u8; 8] {
        let def_tick: u64 = 1_000_000 / crate::drivers::timer::HZ;
        state[88..96].copy_from_slice(&def_tick.to_le_bytes());
    }
    const ADJ_OFFSET: u32 = 0x001;
    const ADJ_FREQUENCY: u32 = 0x002;
    const ADJ_MAXERROR: u32 = 0x004;
    const ADJ_ESTERROR: u32 = 0x008;
    const ADJ_STATUS: u32 = 0x010;
    const ADJ_TIMECONST: u32 = 0x020;
    let copy_field = |state: &mut [u8; 96], off: usize, w: usize| {
        state[off..off + w].copy_from_slice(&buf[off..off + w]);
    };
    if modes & ADJ_OFFSET != 0 { copy_field(&mut state, 8, 8); }
    if modes & ADJ_FREQUENCY != 0 { copy_field(&mut state, 16, 8); }
    if modes & ADJ_MAXERROR != 0 { copy_field(&mut state, 24, 8); }
    if modes & ADJ_ESTERROR != 0 { copy_field(&mut state, 32, 8); }
    if modes & ADJ_STATUS != 0 { copy_field(&mut state, 40, 4); }
    if modes & ADJ_TIMECONST != 0 { copy_field(&mut state, 48, 8); }
    if modes & ADJ_TICK != 0 { copy_field(&mut state, 88, 8); }

    // FIX8 (leapsec01): ADJ_SETOFFSET — step the realtime clock by the
    // caller's tv (Linux do_adjtimex: a positive/negative offset is added
    // to CLOCK_REALTIME immediately). tv must be normalized; the nanosecond
    // flavour (ADJ_NANO) reads tv_usec as tv_nsec. A zero step is a no-op.
    const ADJ_SETOFFSET: u32 = 0x0100;
    const ADJ_NANO: u32 = 0x2000;
    if modes & ADJ_SETOFFSET != 0 {
        let mut sec = i64::from_le_bytes(buf[72..80].try_into().unwrap());
        let mut usec = i64::from_le_bytes(buf[80..88].try_into().unwrap());
        if modes & ADJ_NANO != 0 {
            if usec < 0 || usec > 999_999_999 {
                return -(errno::EINVAL as i64);
            }
        } else if usec < 0 || usec > 999_999 {
            return -(errno::EINVAL as i64);
        }
        // Normalize (tv_sec negative + positive frac is legal for setoffset).
        if sec < 0 && usec > 0 {
            sec += 1;
            if modes & ADJ_NANO != 0 {
                usec -= 1_000_000_000;
            } else {
                usec -= 1_000_000;
            }
        }
        let step_ns = sec
            .saturating_mul(1_000_000_000)
            .saturating_add(usec.saturating_mul(if modes & ADJ_NANO != 0 { 1 } else { 1_000 }));
        if step_ns != 0 {
            let (cur_s, cur_ns) = realtime_secs_nanos();
            let _ = set_realtime_from_secs_nanos(
                cur_s.saturating_add(step_ns / 1_000_000_000),
                (cur_ns as i64 + step_ns % 1_000_000_000).clamp(0, 999_999_999),
            );
        }
    }

    // Write the state back: all modeled fields, modes = 0 (request
    // consumed), status kept, return value TIME_OK.
    // SAFETY: buf_ptr validated with access_ok(128); copy_to_user is the
    // exception-table copy path.
    let mut out = [0u8; 128];
    // Preserve the caller's read-only fields (precision/tolerance/ppsfreq/
    // jitter/shift) where we do not model them — a GET should not zero
    // what it cannot know; the tests compare the settable fields.
    out[8..128].copy_from_slice(&buf[8..128]);
    // Overlay the modeled state (settable fields + seeded tick).
    out[8..48].copy_from_slice(&state[8..48]);
    out[40..44].copy_from_slice(&state[40..44]); // status
    out[48..56].copy_from_slice(&state[48..56]); // constant
    out[88..96].copy_from_slice(&state[88..96]); // tick
    // FIX8 (leapsec01): tx.time is the kernel's CURRENT CLOCK_REALTIME —
    // the NTP readers (and LTP leapsec01's wait loop, whose exit condition
    // is tx.time.tv_sec) poll it on every adjtimex GET. Echoing the
    // caller's zeros back made the loop spin forever (TIMEOUT family).
    {
        let (s, ns) = realtime_secs_nanos();
        out[72..80].copy_from_slice(&s.to_le_bytes());
        let usec = (ns / 1_000) as u64;
        out[80..88].copy_from_slice(&usec.to_le_bytes());
    }
    unsafe {
        if crate::arch::uaccess::copy_to_user(buf_ptr, out.as_ptr(), 128) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    0
}

pub fn sys_adjtimex(args: SyscallArgs) -> i64 {
    adjtimex_common(args[0] as *mut u8)
}

/// sys_clock_adjtime - Adjust per-ClockID (NR 266)
pub fn sys_clock_adjtime(args: SyscallArgs) -> i64 {
    let clk_id = args[0] as i32;

    // Only CLOCK_REALTIME is adjustable; every other clock id (including
    // ids >= MAX_CLOCKS) is EINVAL before anything else (LTP
    // clock_adjtime02 cases 1-2 pass MAX_CLOCKS / MAX_CLOCKS+1).
    if clk_id != 0 {
        return -(errno::EINVAL as i64);
    }

    // Permission check: require CAP_SYS_TIME
    if !crate::security::capable(crate::security::CAP_SYS_TIME) {
        return -(errno::EPERM as i64);
    }

    adjtimex_common(args[1] as *mut u8)
}

/// sys_fanotify_init - Initialize fanotify (NR 262)
pub fn sys_fanotify_init(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_fanotify_mark - Add/remove fanotify mark (NR 263)
pub fn sys_fanotify_mark(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_lookup_dcookie - Lookup directory cookie (NR 18)
pub fn sys_lookup_dcookie(_args: SyscallArgs) -> i64 {
    // No dcookie support — return -EINVAL per convention
    -(errno::EINVAL as i64)
}

/// sys_nfsservctl - NFS service control (NR 42, deprecated)
pub fn sys_nfsservctl(_args: SyscallArgs) -> i64 {
    // Deprecated syscall, removed from kernel
    -(errno::ENOSYS as i64)
}

/// sys_get_robust_list - Get robust futex list (NR 100)
pub fn sys_get_robust_list(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;
    let head_ptr = args[1] as *mut u64;
    let len_ptr = args[2] as *mut u32;

    if pid != 0 && pid as u32 != crate::process::current_pid() {
        return -(errno::EPERM as i64);
    }

    if !head_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(head_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: head_ptr validated with access_ok(8); put_user is the
        // exception-table copy path.
        unsafe { let _ = crate::arch::uaccess::put_user(head_ptr, 0u64); }
    }
    if !len_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(len_ptr as usize, 4) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: len_ptr validated with access_ok(4); writes sizeof(struct robust_list_head).
        unsafe { let _ = crate::arch::uaccess::put_user(len_ptr, 24u32); } // sizeof(struct robust_list_head) on 64-bit
    }
    0
}

/// sys_rseq - Register restartable sequence (NR 293)
pub fn sys_rseq(args: SyscallArgs) -> i64 {
    let rseq_ptr = args[0] as *const u32;
    let rseq_len = args[1] as u32;
    let flags = args[2] as i32;
    let _sig = args[3] as u32;

    const RSEQ_FLAG_UNREGISTER: i32 = 1;

    if rseq_len != 32 && rseq_len != 0 {
        return -(errno::EINVAL as i64);
    }
    if rseq_ptr.is_null() && (flags & RSEQ_FLAG_UNREGISTER) == 0 {
        return -(errno::EINVAL as i64);
    }
    if !rseq_ptr.is_null() {
        if rseq_ptr.align_offset(32) != 0 {
            return -(errno::EINVAL as i64);
        }
        if !crate::arch::uaccess::access_ok(rseq_ptr as usize, rseq_len as usize) {
            return -(errno::EFAULT as i64);
        }
    }

    // Store rseq pointer in current task (simplified: no per-task storage yet)
    // Accept the registration silently
    0
}

// ============================================================================
// NR 403-423: _time64 variants (Y2038-safe syscalls)
// These are Y2038-safe versions of existing syscalls that use 64-bit
// time values directly instead of struct timespec.
// On 64-bit RISC-V, these can delegate to the existing implementations.
// ============================================================================

/// sys_clock_gettime64 - 64-bit clock_gettime (NR 403)
pub fn sys_clock_gettime64(args: SyscallArgs) -> i64 {
    // On 64-bit, delegate to clock_gettime
    sys_clock_gettime(args)
}

/// sys_clock_settime64 - 64-bit clock_settime (NR 404)
pub fn sys_clock_settime64(args: SyscallArgs) -> i64 {
    sys_clock_settime(args)
}

/// sys_clock_adjtime64 - 64-bit clock_adjtime (NR 405)
pub fn sys_clock_adjtime64(args: SyscallArgs) -> i64 {
    sys_clock_adjtime(args)
}

/// sys_clock_getres_time64 - 64-bit clock_getres (NR 406)
pub fn sys_clock_getres_time64(args: SyscallArgs) -> i64 {
    sys_clock_getres(args)
}

/// sys_clock_nanosleep_time64 - 64-bit clock_nanosleep (NR 407)
pub fn sys_clock_nanosleep_time64(args: SyscallArgs) -> i64 {
    sys_clock_nanosleep(args)
}

/// sys_timer_gettime64 - 64-bit timer_gettime (NR 408)
pub fn sys_timer_gettime64(args: SyscallArgs) -> i64 {
    sys_timer_gettime(args)
}

/// sys_timer_settime64 - 64-bit timer_settime (NR 409)
pub fn sys_timer_settime64(args: SyscallArgs) -> i64 {
    sys_timer_settime(args)
}

/// sys_timerfd_gettime64 - 64-bit timerfd_gettime (NR 410)
pub fn sys_timerfd_gettime64(args: SyscallArgs) -> i64 {
    crate::syscall::misc::sys_timerfd_gettime(args)
}

/// sys_timerfd_settime64 - 64-bit timerfd_settime (NR 411)
pub fn sys_timerfd_settime64(args: SyscallArgs) -> i64 {
    crate::syscall::misc::sys_timerfd_settime(args)
}

/// sys_utimensat_time64 - 64-bit utimensat (NR 412)
pub fn sys_utimensat_time64(args: SyscallArgs) -> i64 {
    crate::syscall::file::sys_futimesat(args)
}

/// sys_pselect6_time64 - 64-bit pselect6 (NR 413)
pub fn sys_pselect6_time64(args: SyscallArgs) -> i64 {
    crate::syscall::misc::sys_pselect6(args)
}

/// sys_ppoll_time64 - 64-bit ppoll (NR 414)
pub fn sys_ppoll_time64(args: SyscallArgs) -> i64 {
    crate::syscall::misc::sys_ppoll(args)
}

/// sys_io_pgetevents_time64 - 64-bit io_pgetevents (NR 416)
pub fn sys_io_pgetevents_time64(args: SyscallArgs) -> i64 {
    crate::syscall::memory::sys_io_pgetevents(args)
}

/// sys_recvmmsg_time64 - 64-bit recvmmsg (NR 417)
pub fn sys_recvmmsg_time64(args: SyscallArgs) -> i64 {
    crate::syscall::network::sys_recvmmsg(args)
}

/// sys_mq_timedsend_time64 - 64-bit mq_timedsend (NR 418)
pub fn sys_mq_timedsend_time64(args: SyscallArgs) -> i64 {
    crate::ipc::posix_mq::sys_mq_timedsend(args)
}

/// sys_mq_timedreceive_time64 - 64-bit mq_timedreceive (NR 419)
pub fn sys_mq_timedreceive_time64(args: SyscallArgs) -> i64 {
    crate::ipc::posix_mq::sys_mq_timedreceive(args)
}

/// sys_semtimedop_time64 - 64-bit semtimedop (NR 420)
pub fn sys_semtimedop_time64(args: SyscallArgs) -> i64 {
    crate::ipc::sysv_sem::sys_semtimedop(args)
}

/// sys_rt_sigtimedwait_time64 - 64-bit rt_sigtimedwait (NR 421)
pub fn sys_rt_sigtimedwait_time64(args: SyscallArgs) -> i64 {
    crate::syscall::process::sys_rt_sigtimedwait(args)
}

/// sys_futex_time64 - 64-bit futex (NR 422)
pub fn sys_futex_time64(args: SyscallArgs) -> i64 {
    crate::syscall::sched::sys_futex(args)
}

/// sys_sched_rr_get_interval_time64 - 64-bit sched_rr_get_interval (NR 423)
pub fn sys_sched_rr_get_interval_time64(args: SyscallArgs) -> i64 {
    crate::syscall::sched::sys_sched_rr_get_interval(args)
}


/// Wall-clock epoch offset in NANOSECONDS (whole-second view kept for the
/// RTC / legacy readers).
/// REALTIME = monotonic + this offset. Armed once at boot from the
/// goldfish RTC (drivers/rtc::rtc_init_wall_clock) and re-derived by
/// settimeofday / clock_settime(CLOCK_REALTIME).
static WALL_EPOCH_OFFSET_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Current wall-clock epoch offset (seconds, floored).
pub fn wall_epoch_offset_secs() -> u64 {
    wall_epoch_offset_ns() / 1_000_000_000
}

/// Current wall-clock epoch offset (nanoseconds).
pub fn wall_epoch_offset_ns() -> u64 {
    WALL_EPOCH_OFFSET_NS.load(core::sync::atomic::Ordering::Acquire)
}

/// Set the wall-clock epoch offset, seconds variant (RTC boot path).
pub fn set_wall_epoch_offset_secs(secs: u64) {
    set_wall_epoch_offset_ns(secs.saturating_mul(1_000_000_000));
}

/// Set the wall-clock epoch offset (settimeofday / clock_settime path).
pub fn set_wall_epoch_offset_ns(ns: u64) {
    WALL_EPOCH_OFFSET_NS.store(ns, core::sync::atomic::Ordering::Release);
    // P2 vDSO: refresh the shared data page immediately so REALTIME
    // readers do not wait for the next timer tick.
    crate::mm::vdso::vdso_data_tick();
}
