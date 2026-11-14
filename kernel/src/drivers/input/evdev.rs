//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! evdev character device interface
//!
//! Provides compatible /dev/input/eventX device
//!
//! Linux-parity checklist (xf86-input-evdev / libevdev / evtest probe path):
//! - EVIOCGVERSION / EVIOCGID / EVIOCGNAME(len) / EVIOCGPROP(len)
//! - EVIOCGBIT(ev, len) from the virtio device's own capability bitmaps
//! - EVIOCGABS(abs) from the virtio device's ABS_INFO config
//! - EVIOCGRAB (exclusive access) / EVIOCGKEY / EVIOCGLED / EVIOCGSW
//! - blocking read (sleep until an event arrives) and poll reporting POLLIN
//! - 24-byte input_event passthrough including EV_SYN/SYN_REPORT packets

use super::event::*;
use alloc::collections::vec_deque::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;
use crate::fs::file::{File, FileOps};
use crate::fs::dev_t::{DevNo, DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER};
use crate::fs::devfs;

// ============================================================================
// evdev ioctl command decoding
// ============================================================================

/// ioctl NR (command byte) base values from <uapi/linux/input.h>. The size
/// field of EVIOCGNAME/EVIOCGBIT-style commands carries the user buffer
/// length and must be honored, so commands are decoded by NR + DIR instead
/// of compared as whole u32s.
const NR_VERSION: u32 = 0x01; // EVIOCGVERSION
const NR_ID: u32 = 0x02; // EVIOCGID
const NR_REP_GET: u32 = 0x03; // EVIOCGREP
const NR_REP_SET: u32 = 0x04; // EVIOCSREP
const NR_NAME: u32 = 0x06; // EVIOCGNAME(len)
const NR_PHYS: u32 = 0x07; // EVIOCGPHYS(len)
const NR_UNIQ: u32 = 0x08; // EVIOCGUNIQ(len)
const NR_PROP: u32 = 0x09; // EVIOCGPROP(len)
const NR_KEY_STATE: u32 = 0x18; // EVIOCGKEY(len)
const NR_LED_STATE: u32 = 0x19; // EVIOCGLED(len)
const NR_SND_STATE: u32 = 0x1a; // EVIOCGSND(len)
const NR_SW_STATE: u32 = 0x1b; // EVIOCGSW(len)
const NR_BIT_BASE: u32 = 0x20; // EVIOCGBIT(ev, len): 0x20 + ev
const NR_ABS_BASE: u32 = 0x40; // EVIOCGABS(abs): 0x40 + abs
const NR_GRAB: u32 = 0x90; // EVIOCGRAB
const NR_SCLOCKID: u32 = 0xa0; // EVIOCSCLOCKID

const IOC_READ: u32 = 2; // _IOC_WRITE/_IOC_READ from asm-generic/ioctl.h
const IOC_WRITE: u32 = 1;

#[inline]
fn ioc_nr(cmd: u32) -> u32 {
    cmd & 0xff
}

#[inline]
fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

#[inline]
fn ioc_dir(cmd: u32) -> u32 {
    cmd >> 30
}

// ============================================================================
// Input device ID structure
// ============================================================================

/// Input device ID (input_id)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputId {
    /// Bus type
    pub bustype: u16,
    /// Vendor ID
    pub vendor: u16,
    /// Product ID
    pub product: u16,
    /// Version
    pub version: u16,
}

/// input_absinfo (uapi layout): current value + the five virtio_absinfo
/// fields.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputAbsinfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

// ============================================================================
// Capability bitmaps
// ============================================================================

/// KEY_CNT = 768 bits = 96 bytes; uniform storage for every event type's
/// code bitmap (SYN/REL/ABS use far less).
const BITS_LEN: usize = 96;
/// Number of event types we carry code bitmaps for: EV_SYN..EV_ABS.
const BIT_TYPES: usize = 4;

/// Fill `out` with the derived "supported event types" bitmap (what Linux
/// reports for EVIOCGBIT(0, len)): a type is supported when its code bitmap
/// is non-empty; EV_SYN is always present (input core guarantees it).
fn evtype_bitmap(bits: &[[u8; BITS_LEN]; BIT_TYPES]) -> [u8; BITS_LEN] {
    let mut out = [0u8; BITS_LEN];
    out[0] |= 1 << (EV_SYN & 7); // bit 0
    if bits[1].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_KEY & 7); // bit 1
    }
    if bits[2].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_REL & 7); // bit 2
    }
    if bits[3].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_ABS & 7); // bit 3
    }
    out
}

// ============================================================================
// evdev device
// ============================================================================

/// evdev event queue maximum capacity - from config
const EVENT_QUEUE_SIZE: usize = crate::config::EVDEV_EVENT_QUEUE_SIZE;

/// evdev device structure
pub struct EvdevDevice {
    /// Device name
    pub name: [u8; 32],
    /// The file ids of the open file descriptions for this device.
    /// epoll notification targets a FILE (File.file_id — a per-open
    /// generation counter, NOT the device number), so the device has to
    /// remember which files are open on it: an epoll-based reader that
    /// opened this node is only re-armed when the event notification
    /// carries ITS file id.
    pub open_file_ids: Spinlock<[u64; 4]>,
    /// Number of valid entries in `open_file_ids`.
    pub open_file_count: core::sync::atomic::AtomicU32,
    /// Events pushed since boot (dfx=inputtrace counter).
    pub events_pushed: core::sync::atomic::AtomicU32,
    /// Device ID
    pub id: InputId,
    /// Whether it is a pointer device
    pub is_pointer: bool,
    /// Code bitmaps for EV_SYN(0)/EV_KEY(1)/EV_REL(2)/EV_ABS(3), read from
    /// the virtio device config (VIRTIO_INPUT_CFG_EV_BITS) at init.
    ev_bits: [[u8; BITS_LEN]; BIT_TYPES],
    /// ABS axis info (min/max/fuzz/flat/res from VIRTIO_INPUT_CFG_ABS_INFO)
    /// — reported through EVIOCGABS. Slots: 0=ABS_X, 1=ABS_Y,
    /// 2=ABS_MT_POSITION_X(0x35), 3=ABS_MT_POSITION_Y(0x36). A device that
    /// ADVERTISES an MT axis must also answer a real range for it —
    /// libinput rejects whole devices whose MT axes report minimum==
    /// maximum (the silent device-drop face: probed, never read).
    absinfo: [InputAbsinfo; 4],
    /// EVIOCGRAB state (device-level; we have a single client in practice).
    grabbed: AtomicBool,
    /// LED state bitmap (bit N = LED_N), maintained from EV_LED writes the
    /// way Linux's evdev_write → input_inject_event does; readable back via
    /// EVIOCGLED(len).
    led_state: AtomicU32,
    /// Event queue
    pub event_queue: Spinlock<VecDeque<InputEvent>>,
    /// Poll/select waiters (P3: event-driven poll). Woken in push_event so
    /// a waiter blocked in poll() on this device sleeps until a real input
    /// event arrives instead of waking at tick rate to re-check.
    pub poll_waitq: crate::process::wait::WaitQueueHead,
}

impl EvdevDevice {
    /// Create new evdev device
    pub fn new(name: &[u8], is_pointer: bool) -> Self {
        let mut name_arr = [0u8; 32];
        let len = name.len().min(31);
        name_arr[..len].copy_from_slice(&name[..len]);

        Self {
            name: name_arr,
            open_file_ids: Spinlock::new([0u64; 4]),
            open_file_count: core::sync::atomic::AtomicU32::new(0),
            events_pushed: core::sync::atomic::AtomicU32::new(0),
            id: InputId {
                bustype: 0x0019, // BUS_VIRTIO
                vendor: 0x1AF4,  // Red Hat
                product: if is_pointer { 0x1052 } else { 0x1052 },
                version: 0x0001,
            },
            is_pointer,
            ev_bits: [[0u8; BITS_LEN]; BIT_TYPES],
            absinfo: [InputAbsinfo::default(); 4],
            grabbed: AtomicBool::new(false),
            led_state: AtomicU32::new(0),
            event_queue: Spinlock::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE)),
            poll_waitq: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// Install a capability bitmap for event type `ev` (0..=3).
    pub fn set_ev_bits(&mut self, ev: usize, bits: &[u8; BITS_LEN]) {
        if ev < BIT_TYPES {
            self.ev_bits[ev] = *bits;
        }
    }

    /// Install ABS axis info. `axis` is the evdev ABS code: ABS_X(0),
    /// ABS_Y(1) map to slots 0/1; ABS_MT_POSITION_X(0x35)/Y(0x36) map to
    /// 2/3. Any other axis is ignored (no slot for it).
    pub fn set_absinfo(&mut self, axis: usize, min: i32, max: i32, fuzz: i32, flat: i32, res: i32) {
        let slot = match axis {
            0 => 0,
            1 => 1,
            0x35 => 2,
            0x36 => 3,
            _ => return,
        };
        self.absinfo[slot] = InputAbsinfo {
            value: 0,
            minimum: min,
            maximum: max,
            fuzz,
            flat,
            resolution: res,
        };
    }

    /// Register an open file description on this device so event
    /// arrivals can re-arm its epoll watchers.
    pub fn register_open_file(&self, file_id: u64) {
        if file_id == 0 {
            return;
        }
        let mut ids = self.open_file_ids.lock_irqsave();
        let n = self.open_file_count.load(Ordering::Acquire) as usize;
        for slot in ids.iter_mut() {
            if *slot == 0 {
                *slot = file_id;
                self.open_file_count.store((n + 1) as u32, Ordering::Release);
                return;
            }
        }
    }

    /// Forget a closed file description (its epoll watchers are gone).
    pub fn unregister_open_file(&self, file_id: u64) {
        let mut ids = self.open_file_ids.lock_irqsave();
        let mut n = self.open_file_count.load(Ordering::Acquire) as usize;
        let mut i = 0;
        while i < n && i < ids.len() {
            if ids[i] == file_id {
                for j in i + 1..n {
                    ids[j - 1] = ids[j];
                }
                ids[n - 1] = 0;
                n -= 1;
                i = 0;
                continue;
            }
            i += 1;
        }
        self.open_file_count.store(n as u32, Ordering::Release);
    }

/// Push event (called from softirq context)
pub fn push_event(&self, event: InputEvent) {
        {
            // dfx=inputtrace: the input arrival trace (release-visible via
            // the raw UART path — this is the datum that says whether the
            // guest's event source reaches the device at all). Line
            // rides the taskdump funnel (obs-hygiene): one atomic write,
            // not per-byte uart_putc_raw the kmsg flood could splice.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
                let n = self.events_pushed.fetch_add(1, Ordering::Relaxed);
                if n < 24 {
                    let nm: alloc::vec::Vec<u8> = self
                        .name
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect();
                    let line = alloc::format!(
                        "IN-EV n={} name={} type={} code={} val={}\n",
                        n,
                        String::from_utf8_lossy(&nm),
                        event.type_,
                        event.code,
                        event.value
                    );
                    crate::dfx::taskdump::taskdump_raw_line(line.as_bytes());
                }
            }
        }
        {
            let mut queue = self.event_queue.lock_irqsave();
            if queue.len() >= EVENT_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(event);
        }
        // Wake poll/select waiters OUTSIDE the event_queue lock (INV-LOCK-6
        // discipline: never nest a wait-queue wake inside a held spinlock).
        // input:evdev_wake (wake-drop hunt): when armed, wrap the wake with
        // a per-entry verdict — pre/post entry snapshots around
        // wake_up_all() split EMPTY / DELIVERED / REFUSED / STICKY / GONE
        // for every parked poller, with the GRQ linkage snapshot after.
        // Off by default like every tracepoint; the serial echo rides the
        // same armed() gate with the INEV budget discipline.
        if crate::trace::EVENT_EVDEV_WAKE.armed() {
            self.wake_pollq_probed();
        } else {
            self.poll_waitq.wake_up_all();
        }
        // …and wake the EPOLL waiters: an epoll-based reader (OH's
        // multimodalinput reads /dev/input/eventN through epoll) that is
        // blocked in epoll_wait() is parked on the EPOLL wait queue, not
        // on this device's poll queue — the device wake above never
        // reaches it. Without this notification the event sits in the
        // queue and the UI never sees the gesture (the launcher swipe
        // stays dead while the screen renders fine).
        //
        // Notify each open file description on this device: the epoll
        // wake registry is keyed by File.file_id.
        let n = self.open_file_count.load(Ordering::Acquire) as usize;
        if n > 0 {
            let ids = self.open_file_ids.lock_irqsave();
            for &fid in ids.iter().take(n.min(ids.len())) {
                if fid != 0 {
                    crate::syscall::misc::epoll_notify_file(fid);
                }
            }
        }
    }

    /// input:evdev_wake probe: wake the poll wait queue with a per-entry
    /// verdict (the wake-drop hunt's per-wake answer). Runs only when the
    /// tracepoint is armed; the serial echo carries the same verdicts so
    /// the OH rig (no in-guest tracefs) reads them off serial.
    ///
    /// Verdicts per pre-wake entry:
    ///   0 EMPTY     — queue had no entries at push time
    ///   1 DELIVERED — wake consumed the token AND the target left the
    ///                 sleeping state (linked for a pick, or already
    ///                 running through its poll re-scan)
    ///   2 REFUSED   — token returned (wake_up_process refused: the W1
    ///                 path — the entry stays eligible for the next push)
    ///   3 STICKY    — pre-woken token on a still-sleeping target at post
    ///                 time (the sc100 re-issue attempted this push)
    ///   4 GONE      — entry vanished inside the window (the sleeper's
    ///                 unwait raced the push — its own re-check owns the
    ///                 event)
    ///   5 RERAN     — token consumed and the target is parked AGAIN at
    ///                 post time: the wake ran it through the poll loop
    ///                 within the window (fast SMP round trip)
    fn wake_pollq_probed(&self) {
        use core::sync::atomic::{AtomicU64, Ordering};
        let (pre_waiters, pre) = self.poll_waitq.debug_entries_ptr();
        if pre.is_empty() {
            crate::trace::trace_evdev_wake(0, 0, 0, 0, 2, pre_waiters as u32);
            static EMPTY_N: AtomicU64 = AtomicU64::new(0);
            let n = EMPTY_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let q = self.event_queue.lock_irqsave().len();
                let mut l = HuntLine::new();
                l.push(b"INWK[");
                l.u64(n);
                l.push(b"]E ");
                for &c in self.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" q=");
                l.u64(q as u64);
                // w= names the waiters COUNTER (not just list emptiness):
                // w>0 with an empty list is the counter/list divergence
                // signature; w=0 confirms nobody is registered at all.
                l.push(b" w=");
                l.u64(pre_waiters as u64);
                // stE= the R127 wake-time stale-eviction total: stE>0 on
                // this boot means the screen has already self-healed
                // leaked entries from dead parkers upstream of this push.
                l.push(b" stE=");
                l.u64(crate::process::wait::WQ_STALE_EVICTED.load(Ordering::Relaxed));
                l.push(b"\n");
                l.flush();
            }
            return;
        }
        self.poll_waitq.wake_up_all();
        let (post_waiters, post) = self.poll_waitq.debug_entries_ptr();
        static WK_ECHO_N: AtomicU64 = AtomicU64::new(0);
        for (pid, pre_state, pre_woken, task) in pre.iter() {
            if *pid == 0 {
                continue;
            }
            let post_e = post.iter().find(|(p, _, _, _)| p == pid);
            let (verdict, post_state) = match post_e {
                None => (4u32, 0u32),
                Some((_, st, woken, _)) => {
                    let st = *st;
                    let sleeping = st & 0x3 != 0; // INTERRUPTIBLE|UNINTERRUPTIBLE bits
                    if !*pre_woken {
                        if !*woken {
                            (2, st) // token returned — refused
                        } else if sleeping {
                            (5, st) // delivered, ran, re-parked inside window
                        } else {
                            (1, st) // delivered
                        }
                    } else if sleeping {
                        (3, st) // sticky token on sleeping target
                    } else {
                        (1, st) // re-issue delivered
                    }
                }
            };
            let linked = match crate::sched::sched::grq_diag_cfs_linked(*task) {
                Some(true) => 1,
                Some(false) => 0,
                None => 2,
            };
            crate::trace::trace_evdev_wake(
                *pid, verdict, *pre_state, post_state, linked, post_waiters as u32,
            );
            let n = WK_ECHO_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let mut l = HuntLine::new();
                l.push(b"INWK[");
                l.u64(n);
                l.push(b"] pid=");
                l.u64(*pid as u64);
                l.push(b" v=");
                l.u64(verdict as u64);
                l.push(b" pre=");
                l.u64(*pre_state as u64);
                l.push(b" post=");
                l.u64(post_state as u64);
                l.push(b" lnk=");
                l.u64(linked as u64);
                l.push(b" w=");
                l.u64(post_waiters as u64);
                l.push(b"\n");
                l.flush();
            }
        }
    }

    /// Read event
    pub fn pop_event(&self) -> Option<InputEvent> {
        let ev = self.event_queue.lock_irqsave().pop_front();
        // input:evdev_wake hunt: name the consumer even when the INRD
        // echo tears on the raw UART — one short line per pop (budgeted),
        // first-8-chars device name + remaining depth.
        if ev.is_some() && crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU64, Ordering};
            static POP_N: AtomicU64 = AtomicU64::new(0);
            let n = POP_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let remain = self.event_queue.lock_irqsave().len();
                let mut l = HuntLine::new();
                l.push(b"INPOP[");
                l.u64(n);
                l.push(b"] ");
                l.u64(crate::process::current_pid() as u64);
                l.push(b" ");
                for &c in self.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" r=");
                l.u64(remain as u64);
                l.push(b"\n");
                l.flush();
            }
        }
        ev
    }

    /// Check if there are events
    pub fn has_event(&self) -> bool {
        !self.event_queue.lock_irqsave().is_empty()
    }
}

// ============================================================================
// Global evdev devices
// ============================================================================

/// Keyboard evdev device
pub static mut EVDEV_KEYBOARD: Option<EvdevDevice> = None;

/// Pointer evdev device
pub static mut EVDEV_POINTER: Option<EvdevDevice> = None;

/// Touchscreen evdev device (event2): mirrors pointer coordinates with
/// touch protocol for OH multimodalinput.
pub static mut EVDEV_TOUCHSCREEN: Option<EvdevDevice> = None;

/// Whether a file id belongs to one of the three open evdev file
/// descriptions (the epoll_ctl probe uses this to filter).
pub fn file_id_is_input(fid: u64) -> bool {
    unsafe {
        [EVDEV_KEYBOARD.as_ref(), EVDEV_POINTER.as_ref(), EVDEV_TOUCHSCREEN.as_ref()]
            .iter()
            .flatten()
            .any(|d| {
                let ids = d.open_file_ids.lock_irqsave();
                let n = d.open_file_count.load(Ordering::Acquire) as usize;
                ids.iter().take(n.min(ids.len())).any(|&x| x == fid)
            })
    }
}

/// Pids that opened an evdev node this boot (wake-drop hunt, max 8):
/// the PPARK/PRET echoes filter on this set so the input openers' poll
/// rounds are all visible without flooding the console with every
/// GLib main-loop park on the machine.
static EVDEV_OPENER_PIDS: [core::sync::atomic::AtomicU32; 8] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 8];

fn latch_opener_pid(pid: u32) {
    for slot in EVDEV_OPENER_PIDS.iter() {
        match slot.compare_exchange(
            0,
            pid,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(v) if v == pid => return,
            Err(_) => continue,
        }
    }
}

/// True when `pid` opened an evdev node this boot (hunt filter).
pub fn evdev_opener_pid(pid: u32) -> bool {
    EVDEV_OPENER_PIDS
        .iter()
        .any(|s| s.load(Ordering::Acquire) == pid)
}

/// Resolve an EvdevDevice by its devfs device number (the devfs open
/// path uses this to stamp the device with the new file description).
pub fn device_of_devno(devno: DevNo) -> Option<&'static EvdevDevice> {
    if devno == DEV_EVDEV_KEYBOARD {
        unsafe { EVDEV_KEYBOARD.as_ref() }
    } else if devno == DEV_EVDEV_POINTER {
        unsafe { EVDEV_POINTER.as_ref() }
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        unsafe { EVDEV_TOUCHSCREEN.as_ref() }
    } else {
        None
    }
}

/// Resolve the EvdevDevice a file was opened against. Returns None for
/// non-evdev files (caller falls through to other ioctl handlers).
// SAFETY: EVDEV_KEYBOARD / EVDEV_POINTER are initialized by init_evdev()
// before any file operations can occur; private_data holds a valid DevNo
// boxed by devfs_open for as long as the file lives.
unsafe fn device_of_file(file: &File) -> Option<&'static EvdevDevice> {
    let ptr = match *(file.private_data.get()) {
        Some(p) => p,
        None => return None,
    };
    let devno = *(ptr as *const DevNo);
    if devno == DEV_EVDEV_KEYBOARD {
        EVDEV_KEYBOARD.as_ref()
    } else if devno == DEV_EVDEV_POINTER {
        EVDEV_POINTER.as_ref()
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        EVDEV_TOUCHSCREEN.as_ref()
    } else {
        None
    }
}

// ============================================================================
// FileOps implementation
// ============================================================================

/// Interruptible timed sleep (milliseconds) used by the blocking read —
/// the task actually sleeps (yields the CPU) instead of busy-polling and
/// wakes early on signals.
///
/// Returns false if no task context / timer was available (caller should
/// fall back to a yield-style retry).
fn sleep_ms_interruptible(ms: u64) -> bool {
    use crate::process::task::{Task, TaskState};

    let target =
        crate::drivers::timer::get_jiffies() + crate::drivers::timer::msecs_to_jiffies(ms);

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return false,
    };
    // SAFETY: current is the running task (we are it); pid() is a const read.
    let pid = unsafe { (*current).pid() };

    // The input tasklet BH (IRQ-driven delivery) takes the INPUT_* locks
    // with IRQs disabled, so every task-context taker must too — a plain
    // holder interrupted by the device IRQ would deadlock against the BH
    // at irq_exit on this CPU. Registration failure means we must NOT
    // sleep.
    let timer_id = crate::timer::add_timer_wakeup(target, pid);
    if timer_id == 0 {
        return false;
    }

    loop {
        let jiffies_now = crate::drivers::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first lost-
        // wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        let jiffies_now = crate::drivers::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            }
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            unsafe {
                crate::sched::dequeue_task(&*current);
            }
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Syscall context runs with SIE=0 — re-arm so the timer tick can
        // reach this CPU and wake us.
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        // Woke up (timer expiry or signal): loop tail re-checks and exits.
    }
}

/// Resolve the EvdevDevice AND its devfs minor a file was opened
/// against (the minor is the input:evdev_read/evdev_open `node` field).
// SAFETY: same contract as device_of_file.
unsafe fn node_of_file(file: &File) -> Option<(&'static EvdevDevice, u32)> {
    let ptr = match *(file.private_data.get()) {
        Some(p) => p,
        None => return None,
    };
    let devno = *(ptr as *const DevNo);
    if devno == DEV_EVDEV_KEYBOARD {
        unsafe { EVDEV_KEYBOARD.as_ref() }.map(|d| (d, devno.minor))
    } else if devno == DEV_EVDEV_POINTER {
        unsafe { EVDEV_POINTER.as_ref() }.map(|d| (d, devno.minor))
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        unsafe { EVDEV_TOUCHSCREEN.as_ref() }.map(|d| (d, devno.minor))
    } else {
        None
    }
}

/// evdev read function
///
/// Linux semantics: a blocking fd sleeps until at least one event is
/// available (returning exactly one input_event per read); O_NONBLOCK
/// returns -EAGAIN when the queue is empty. The IRQ-driven tasklet BH
/// pushes events and wakes the poll_waitq; the drain here is the backstop
/// for events that raced the BH (the reader-side re-check is free).
fn evdev_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let (device, node) = match unsafe { node_of_file(file) } {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.len() < event_size {
        return -22; // EINVAL
    }

    let nonblock = file.flags_bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0;

    loop {
        // Poll for new events
        poll_virtio_events();

        match device.pop_event() {
            Some(event) => {
                // input:evdev_read — hop 2 of the input chain: a
                // userspace reader took this event off the node. The
                // reader's pid/comm ride the trace record's TASK-PID
                // column; the serial echo spells the pid out (the OH
                // rig reads its verdict off serial).
                if crate::trace::EVENT_EVDEV_READ.armed() {
                    crate::trace::trace_evdev_read(node, event.type_, event.code, event.value);
                    use core::sync::atomic::{AtomicU64, Ordering};
                    static RD_ECHO_N: AtomicU64 = AtomicU64::new(0);
                    let n = RD_ECHO_N.fetch_add(1, Ordering::Relaxed);
                    if input_echo_budget(n) {
                        let (pid, tgid) = current_pid_tgid();
                        itt_msg(b"INRD[");
                        itt_u64(n);
                        itt_msg(b"] pid=");
                        itt_u64(pid);
                        itt_msg(b"(");
                        itt_u64(tgid);
                        itt_msg(b") node=event");
                        itt_u64(node.saturating_sub(64) as u64);
                        itt_msg(b" type=");
                        itt_u64(event.type_ as u64);
                        itt_msg(b" code=");
                        itt_u64(event.code as u64);
                        itt_msg(b" value=");
                        itt_i64(event.value as i64);
                        itt_msg(b"\n");
                    }
                }
                // dfx=inputtrace: the userspace READ hop — pid, which node,
                // how many events remained after this one. If this prints
                // during a gesture the delivery chain is complete and any
                // remaining failure is consumer-side semantics.
                if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
                    static RD_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let rn = RD_N.fetch_add(1, Ordering::Relaxed);
                    if rn < 40 {
                        let nm: alloc::vec::Vec<u8> = device
                            .name
                            .iter()
                            .take_while(|&&c| c != 0)
                            .copied()
                            .collect();
                        crate::pr_warn!(
                            "IN-READ #{} pid={} name={} type={} code={} val={} remain={}",
                            rn,
                            crate::process::current_pid(),
                            alloc::string::String::from_utf8_lossy(&nm),
                            event.type_,
                            event.code,
                            event.value,
                            device.has_event() as u8
                        );
                    }
                }
                // Copy event to buffer
                let src = &event as *const InputEvent as *const u8;
                // SAFETY: src points to a valid InputEvent on the stack; buf is
                // guaranteed to be at least event_size bytes by the check above.
                unsafe {
                    core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), event_size);
                }
                return event_size as isize;
            }
            None => {
                if nonblock {
                    return -11; // EAGAIN
                }
                if crate::signal::signal_pending() {
                    return -4; // EINTR
                }
                if !sleep_ms_interruptible(EVDEV_READ_POLL_MS) {
                    // No timer slot: yield rather than burn the CPU.
                    crate::sched::yield_cpu();
                }
            }
        }
    }
}

/// Backstop poll interval for the blocking read sleep.
const EVDEV_READ_POLL_MS: u64 = 10;

/// dfx=inputtrace: announce which node the guest opened. Also records
/// input:evdev_open (hop 2a of the input chain — a consumer opened the
/// node; pid/comm name the opener in the trace record header and the
/// serial echo). `node` is the devfs minor (64/65/66).
pub fn trace_open(dev: &EvdevDevice, file: &File, node: u32) {
    // The wake-drop hunt's opener latch (PPARK/PRET filter) — armed
    // with the same gate as the rest of the hunt echoes.
    if crate::trace::EVENT_EVDEV_OPEN.armed() {
        latch_opener_pid(crate::process::current_pid() as u32);
    }
    // The tracepoint path first (it's the policy instrument; the dfx
    // prints below are the frozen legacy view of the same event).
    if crate::trace::EVENT_EVDEV_OPEN.armed() {
        crate::trace::trace_evdev_open(node);
        use core::sync::atomic::{AtomicU64, Ordering};
        static OP_ECHO_N: AtomicU64 = AtomicU64::new(0);
        let n = OP_ECHO_N.fetch_add(1, Ordering::Relaxed);
        if n < 64 {
            let (pid, tgid) = current_pid_tgid();
            itt_msg(b"INOP[");
            itt_u64(n);
            itt_msg(b"] pid=");
            itt_u64(pid);
            itt_msg(b"(");
            itt_u64(tgid);
            itt_msg(b") node=event");
            itt_u64(node.saturating_sub(64) as u64);
            itt_msg(b" file_id=");
            itt_u64(file.file_id);
            itt_msg(b"\n");
        }
    }
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
        return;
    }
    let nm: alloc::vec::Vec<u8> = dev
        .name
        .iter()
        .take_while(|&&c| c != 0)
        .copied()
        .collect();
    let s = alloc::format!("IN-OPEN pid={} name={} file_id={}\n", crate::process::current_pid(), String::from_utf8_lossy(&nm), file.file_id);
    // obs-hygiene: line-atomic funnel instead of per-byte raw writes.
    crate::dfx::taskdump::taskdump_raw_line(s.as_bytes());
}

/// evdev close function
fn evdev_file_close(file: &File) -> i32 {
    // Release a grab held through this fd and free the DevNo boxed by
    // devfs_open.
    // SAFETY: private_data contains a valid DevNo pointer set during open.
    unsafe {
        if let Some(ptr) = *file.private_data.get() {
            let devno = *(ptr as *const DevNo);
            // The file description is going away — drop its epoll
            // notification registration.
            if let Some(dev) = device_of_devno(devno) {
                dev.unregister_open_file(file.file_id);
            }
            if devno == DEV_EVDEV_KEYBOARD {
                if let Some(ref d) = EVDEV_KEYBOARD {
                    d.grabbed.store(false, Ordering::Release);
                }
            } else if devno == DEV_EVDEV_POINTER {
                if let Some(ref d) = EVDEV_POINTER {
                    d.grabbed.store(false, Ordering::Release);
                }
            }
            drop(Box::from_raw(ptr as *mut DevNo));
        }
    }
    0
}

/// evdev poll function
///
/// Report readable whenever the queue holds an event. The virtqueues are
/// drained first so events that raced the tasklet BH still count (and so
/// a poll-only client — no blocking read — keeps receiving).
fn evdev_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    // SAFETY: private_data contains a valid DevNo pointer set during open;
    // the EVDEV_* statics are initialized by init_evdev() before any file
    // operations can occur.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return POLLERR,
    };

    let mut ready = 0u16;
    if events & POLLIN != 0 {
        // Drain the virtio queues so recently-arrived input counts.
        poll_virtio_events();
        if device.has_event() {
            ready |= POLLIN | POLLRDNORM;
        }
        // dfx=inputtrace (leg-9 epoll hunt): one line per POLL dispatch,
        // raw-console (loglevel-proof; pr_info is dropped at loglevel=3).
        // The r13ep2/r13ep3 A/B proved the virtqueues ARE drained during
        // an epoll-only boot (14 INPTRC) yet epoll_wait returned idle —
        // this line ties every dispatch to its caller's file and result,
        // splitting "dispatch never ran" from "dispatch ran, reported
        // ready, but the waiter still idled". Rate-limited: first 600
        // then 1-in-50 (a spinning poller must not flood the console).
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
            use core::sync::atomic::{AtomicU64, Ordering};
            static PL_N: AtomicU64 = AtomicU64::new(0);
            let n = PL_N.fetch_add(1, Ordering::Relaxed);
            if n < 600 || n % 50 == 0 {
                it_msg(b"INPPL[");
                it_u64(n);
                it_msg(b"] file=");
                it_u64(file.file_id);
                it_msg(b" mask=");
                it_u64(events as u64);
                it_msg(b" ready=");
                it_u64(ready as u64);
                it_msg(b" has=");
                it_u64(if device.has_event() { 1 } else { 0 });
                it_msg(b"\n");
            }
        }
    }
    if events & POLLOUT != 0 {
        // evdev is never writable.
    }
    ready
}

// ============================================================================
// Poll-wait registration (P3: event-driven poll/select)
// ============================================================================

/// Register `task` on the device's poll wait queue; push_event wakes it.
pub fn evdev_poll_wait(file: &File, task: *mut crate::process::task::Task) -> bool {
    // SAFETY: dispatcher identity-checked the File against EVDEV_OPS;
    // private_data holds a DevNo installed at open and the EVDEV_* statics
    // outlive every file.
    match unsafe { device_of_file(file) } {
        Some(device) => {
            device
                .poll_waitq
                .add(crate::process::wait::WaitQueueEntry::new(task, false));
            // input:evdev_wake hunt: echo the park registration (names
            // WHO parks on the input nodes — the OH rig reads it off
            // serial). Same armed() gate as the verdict echo.
            if crate::trace::EVENT_EVDEV_WAKE.armed() {
                use core::sync::atomic::{AtomicU64, Ordering};
                static REG_N: AtomicU64 = AtomicU64::new(0);
                let n = REG_N.fetch_add(1, Ordering::Relaxed);
                if input_echo_budget(n) {
                    // SAFETY: task is the current task parking here.
                    let pid = unsafe { (*task).pid() };
                    let mut l = HuntLine::new();
                    l.push(b"INPW[");
                    l.u64(n);
                    l.push(b"] reg pid=");
                    l.u64(pid as u64);
                    l.push(b" dev=");
                    for &c in device.name.iter().take_while(|&&c| c != 0).take(10) {
                        l.push(&[c]);
                    }
                    // f= correlates the registration with the INOP open
                    // line (which node this file really is).
                    l.push(b" f=");
                    l.u64(file.file_id);
                    l.push(b"\n");
                    l.flush();
                }
            }
            true
        }
        None => false,
    }
}

/// Remove a registration added by evdev_poll_wait.
pub fn evdev_poll_unwait(file: &File, task: *mut crate::process::task::Task) {
    // SAFETY: same contract as evdev_poll_wait.
    if let Some(device) = unsafe { device_of_file(file) } {
        let dropped = device.poll_waitq.remove_count(task);
        if crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU64, Ordering};
            static UNREG_N: AtomicU64 = AtomicU64::new(0);
            let n = UNREG_N.fetch_add(1, Ordering::Relaxed);
            // d>1 is the pointer-recycling theft signature: this task
            // registered at most ONE entry on this queue per poll round,
            // so a remove dropping more says a stale entry from a DEAD
            // previous owner of the same Task address was stripped too
            // (exit leak + page recycle). With the R127 (ptr, pid) key a
            // live task's remove can no longer strip a predecessor's
            // entries, so d>1 now names either a dup-fd double
            // registration (legitimate) or a keying bug — print those
            // unconditionally (not just inside the budget).
            let steal = dropped > 1;
            if steal || input_echo_budget(n) {
                // SAFETY: task is the current task leaving the wait.
                let pid = unsafe { (*task).pid() };
                let mut l = HuntLine::new();
                l.push(b"INPW[");
                l.u64(n);
                l.push(b"] unreg pid=");
                l.u64(pid as u64);
                l.push(b" dev=");
                for &c in device.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" d=");
                l.u64(dropped as u64);
                l.push(b" f=");
                l.u64(file.file_id);
                if steal {
                    l.push(b" MULTI");
                }
                l.push(b"\n");
                l.flush();
            }
        }
    }
}

/// Event type constants from <uapi/linux/input.h> needed by the write path.
const EV_SYN: u16 = 0x00;
const EV_LED: u16 = 0x11;

/// evdev write function
///
/// Linux semantics (drivers/input/evdev.c evdev_write): the buffer must be
/// a positive multiple of sizeof(struct input_event); each record is handed
/// to the input core, which silently drops events the device cannot take
/// and keeps LED/SND state for EVIOCGLED/EVIOCGSND. X servers write EV_LED
/// + SYN_REPORT packets here whenever core keyboard LED state changes —
/// returning an error (the old no-write-op EBADF) made every
/// xf86-input-evdev LED update log "(EE) Failed to set keyboard controls:
/// Bad file descriptor".
fn evdev_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.is_empty() || buf.len() % event_size != 0 {
        return -22; // EINVAL
    }

    for chunk in buf.chunks(event_size) {
        // InputEvent is repr(C) with no padding: the byte view parses
        // in place.
        let (sec, rest) = chunk.split_at(8);
        let (usec, rest) = rest.split_at(8);
        let (tycode, val) = rest.split_at(4);
        let type_ = u16::from_ne_bytes([tycode[0], tycode[1]]);
        let code = u16::from_ne_bytes([tycode[2], tycode[3]]);
        let value = i32::from_ne_bytes([val[0], val[1], val[2], val[3]]);
        let _ = (sec, usec); // client-supplied timestamps are advisory

        if type_ == EV_LED && code < 32 {
            if value != 0 {
                device.led_state.fetch_or(1 << code, Ordering::AcqRel);
            } else {
                device.led_state.fetch_and(!(1 << code), Ordering::AcqRel);
            }
        }
        // Everything else (including the trailing EV_SYN/SYN_REPORT) needs
        // no action: there is no hardware to inject into, and Linux's input
        // core equally ignores events without a capable handler.
    }
    buf.len() as isize
}

/// evdev FileOps
pub static EVDEV_OPS: FileOps = FileOps::builder()
    .read(evdev_file_read)
    .write(evdev_file_write)
    .close(evdev_file_close)
    .poll(evdev_file_poll)
    .build();

// ============================================================================
// ioctl (per-file, dispatched from sys_ioctl via ops identity)
// ============================================================================

/// evdev ioctl entry — returns None when `file` is not an evdev node so the
/// generic ioctl path can continue.
///
/// Mirrors drivers/input/evdev.c: data-returning ioctls succeed with 0
/// (put_user/copy_to_user style), unknown requests get ENOTTY.
pub fn evdev_file_ioctl(file: &File, cmd: u32, arg: usize) -> Option<i64> {
    // Dispatch on the file's ops identity (pty_ioctl / is_fb_file pattern).
    if !file.get_ops().map_or(false, |ops| {
        core::ptr::eq(ops as *const _, &EVDEV_OPS as *const _)
    }) {
        return None;
    }

    // dfx=inputtrace: the capability probe — what we hand to the input
    // framework for classification. A device whose EVIOCGBIT(0) misses
    // EV_SYN/EV_KEY/EV_ABS, or whose name/id ioctls return garbage, is
    // rejected by consumers after open (opened-but-never-read is the
    // signature). Budgeted, first 30 ioctls per boot.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
        static IO_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = IO_N.fetch_add(1, Ordering::Relaxed);
        if n < 400 && (cmd >> 30) == 2 {
            // _IOR reads: EVIOCG* family
            let nm: alloc::vec::Vec<u8> = unsafe { device_of_file(file) }
                .map(|d| {
                    d.name
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect()
                })
                .unwrap_or_default();
            crate::pr_warn!(
                "IN-IOCTL #{} pid={} name={} cmd={:#x}",
                n,
                crate::process::current_pid(),
                alloc::string::String::from_utf8_lossy(&nm),
                cmd
            );
        }
    }

    // SAFETY: the EVDEV_* statics are initialized by init_evdev() before
    // any file operations can occur; device_of_file validates the DevNo.
    let device = match unsafe { device_of_file(file) } {
        Some(d) => d,
        None => return Some(-9), // EBADF
    };

    /// Copy `data` out honoring the length encoded in the ioctl command.
    /// Returns the ioctl return value.
    fn copy_out(arg: usize, cmd: u32, data: &[u8]) -> i64 {
        use crate::arch::uaccess::{access_ok, copy_to_user};
        let len = ioc_size(cmd).min(data.len());
        if len == 0 {
            return 0;
        }
        if !access_ok(arg, len) {
            return -14; // EFAULT
        }
        // SAFETY: arg validated with access_ok(len); data is a kernel buffer.
        if unsafe { copy_to_user(arg as *mut u8, data.as_ptr(), len) } > 0 {
            return -14; // EFAULT
        }
        0
    }

    let nr = ioc_nr(cmd);
    let dir = ioc_dir(cmd);

    let ret: i64 = match (nr, dir) {
        (NR_VERSION, IOC_READ) => copy_out(arg, cmd, &0x010001u32.to_ne_bytes()), // EV_VERSION
        (NR_ID, IOC_READ) => copy_out(
            arg,
            cmd,
            &{
                // SAFETY: InputId is repr(C), Copy; transmute-free byte view.
                let id = device.id;
                let mut b = [0u8; 8];
                b[0..2].copy_from_slice(&id.bustype.to_ne_bytes());
                b[2..4].copy_from_slice(&id.vendor.to_ne_bytes());
                b[4..6].copy_from_slice(&id.product.to_ne_bytes());
                b[6..8].copy_from_slice(&id.version.to_ne_bytes());
                b
            },
        ),
        (NR_NAME, IOC_READ) => {
            // NUL-terminated device name, truncated to the caller's buffer.
            let len = ioc_size(cmd);
            let name_len = device.name.iter().position(|&c| c == 0).unwrap_or(31) + 1;
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &device.name[..name_len.min(32)])
            }
        }
        (NR_PHYS, IOC_READ) | (NR_UNIQ, IOC_READ) => {
            // No physical path / unique id on virtio-input: empty string.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8])
            }
        }
        (NR_PROP, IOC_READ) => {
            // No INPUT_PROP_* flags.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
            }
        }
        (NR_REP_GET, IOC_READ) => {
            // Repeat settings {delay, period} in ms (kernel defaults).
            let rep = [250u32, 33u32];
            let mut b = [0u8; 8];
            b[0..4].copy_from_slice(&rep[0].to_ne_bytes());
            b[4..8].copy_from_slice(&rep[1].to_ne_bytes());
            copy_out(arg, cmd, &b)
        }
        (NR_REP_SET, IOC_WRITE) => 0,
        (NR_KEY_STATE, IOC_READ) | (NR_LED_STATE, IOC_READ) | (NR_SND_STATE, IOC_READ)
        | (NR_SW_STATE, IOC_READ) => {
            // Live state bitmap. LED bits track EV_LED writes (Linux
            // input-core semantics); key/sound/switch are not tracked and
            // report "all clear" — advisory, as on any kernel without
            // stateful drivers.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                let state = if nr == NR_LED_STATE {
                    device.led_state.load(Ordering::Acquire)
                } else {
                    0
                };
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&state.to_ne_bytes());
                copy_out(arg, cmd, &b[..len.min(32)])
            }
        }
        (NR_GRAB, IOC_WRITE) => {
            // arg is the grab flag passed by value (not a pointer).
            if arg != 0 {
                if device.grabbed.swap(true, Ordering::AcqRel) {
                    // Already grabbed by another client.
                    -16 // EBUSY
                } else {
                    0
                }
            } else {
                device.grabbed.store(false, Ordering::Release);
                0
            }
        }
        (NR_SCLOCKID, IOC_WRITE) => {
            // Accept the clock switch request; timestamps already come from
            // the monotonic CLINT timer.
            0
        }
        _ => {
            if (NR_BIT_BASE..NR_BIT_BASE + 0x20).contains(&nr) && dir == IOC_READ {
                // EVIOCGBIT(ev, len) for ev in 0..=0x1f (EV_MAX). Types we
                // carry bitmaps for answer from the virtio config; the rest
                // return an all-zero bitmap (success) like Linux — libevdev
                // probes every type and treats ENOTTY as an error.
                let ev = (nr - NR_BIT_BASE) as usize;
                if ev == 0 {
                    let types = evtype_bitmap(&device.ev_bits);
                    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
                        static TY_N: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        let tn = TY_N.fetch_add(1, Ordering::Relaxed);
                        if tn < 12 {
                            let nm: alloc::vec::Vec<u8> = device
                                .name
                                .iter()
                                .take_while(|&&c| c != 0)
                                .copied()
                                .collect();
                            crate::pr_warn!(
                                "IN-TYPEBITS #{} pid={} name={} types={:#04x}",
                                tn,
                                crate::process::current_pid(),
                                alloc::string::String::from_utf8_lossy(&nm),
                                types[0]
                            );
                        }
                    }
                    copy_out(arg, cmd, &types)
                } else if ev < BIT_TYPES {
                    copy_out(arg, cmd, &device.ev_bits[ev])
                } else {
                    let len = ioc_size(cmd);
                    if len == 0 {
                        0
                    } else {
                        copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
                    }
                }
            } else if (NR_ABS_BASE..NR_ABS_BASE + 8).contains(&nr) && dir == IOC_READ {
                // EVIOCGABS(abs)
                let abs = (nr - NR_ABS_BASE) as usize;
                let slot = match abs {
                    0 | 1 => Some(abs),
                    0x35 => Some(2),
                    0x36 => Some(3),
                    _ => None,
                };
                let info = match slot {
                    Some(k) => device.absinfo[k],
                    None => InputAbsinfo::default(),
                };
                let mut b = [0u8; 24];
                b[0..4].copy_from_slice(&info.value.to_ne_bytes());
                b[4..8].copy_from_slice(&info.minimum.to_ne_bytes());
                b[8..12].copy_from_slice(&info.maximum.to_ne_bytes());
                b[12..16].copy_from_slice(&info.fuzz.to_ne_bytes());
                b[16..20].copy_from_slice(&info.flat.to_ne_bytes());
                b[20..24].copy_from_slice(&info.resolution.to_ne_bytes());
                copy_out(arg, cmd, &b)
            } else {
                -25 // ENOTTY
            }
        }
    };
    Some(ret)
}

// ============================================================================
// Initialization and registration
// ============================================================================

/// Format a capability bitmap the way Linux `input_print_bitmap()` prints
/// it into uevent env vars and /sys capabilities attrs: space-separated
/// lowercase-hex 64-bit words, most-significant word FIRST, leading zero
/// words skipped, always at least one word.
///
/// OH's libudev shim parses exactly this format (BitVector reads hex
/// u64 words high-word-first and reverses them), and libinput derives the
/// ID_INPUT_KEYBOARD / ID_INPUT_MOUSE / ... tags from those lines to
/// decide whether a device is usable at all.
pub fn format_input_bitmap(bits: &[u8]) -> String {
    fn word_at(bits: &[u8], w: usize) -> u64 {
        let mut word: u64 = 0;
        for i in 0..8 {
            let idx = w * 8 + i;
            if idx < bits.len() {
                word |= (bits[idx] as u64) << (8 * i);
            }
        }
        word
    }

    let words = (bits.len() + 7) / 8;
    let mut hi = words;
    while hi > 1 && word_at(bits, hi - 1) == 0 {
        hi -= 1;
    }

    use core::fmt::Write as _;
    let mut out = String::new();
    for w in (0..hi).rev() {
        if !out.is_empty() {
            out.push(' ');
        }
        let _ = write!(out, "{:x}", word_at(bits, w));
    }
    if out.is_empty() {
        out.push('0');
    }
    out
}

/// Build the sysfs registration (input_dev + event node pair) for one
/// evdev device from the same capability bitmaps EVIOCGBIT serves, so the
/// sysfs view and the ioctl view can never disagree.
fn sysfs_registration(
    input_name: &str,
    event_name: &str,
    devno: DevNo,
    device: &EvdevDevice,
) -> crate::fs::sysfs::InputEventReg {
    let name_len = device.name.iter().position(|&c| c == 0).unwrap_or(31);
    let dev_name = String::from(core::str::from_utf8(&device.name[..name_len]).unwrap_or("virtio input"));

    // EV word: which event types exist (bits 0..EV_MAX live in one word).
    let types = evtype_bitmap(&device.ev_bits);
    let ev = format_input_bitmap(&types[..8.min(types.len())]);

    // KEY/REL/ABS words only when the type is supported (Linux
    // input_dev_uevent emits the bitmap line only for set types).
    let type_bit = |t: usize| types[0] & (1u8 << (t & 7)) != 0;
    let key = if type_bit(EV_KEY as usize) {
        format_input_bitmap(&device.ev_bits[1])
    } else {
        String::new()
    };
    let rel = if type_bit(EV_REL as usize) {
        format_input_bitmap(&device.ev_bits[2])
    } else {
        String::new()
    };
    let abs = if type_bit(EV_ABS as usize) {
        format_input_bitmap(&device.ev_bits[3])
    } else {
        String::new()
    };

    crate::fs::sysfs::InputEventReg {
        input_name: String::from(input_name),
        event_name: String::from(event_name),
        major: devno.major,
        minor: devno.minor,
        dev_name,
        bustype: device.id.bustype as u32,
        vendor: device.id.vendor as u32,
        product: device.id.product as u32,
        version: device.id.version as u32,
        ev,
        key,
        rel,
        abs,
    }
}

/// Initialize evdev devices and register to devfs
pub fn init_evdev() {
    // SAFETY: Called once during kernel init; no concurrent access is possible.
    unsafe {
        // Create keyboard device
        EVDEV_KEYBOARD = Some(EvdevDevice::new(b"VirtIO Keyboard", false));

        // Create pointer device
        EVDEV_POINTER = Some(EvdevDevice::new(b"VirtIO Tablet", true));

        // Pull the capability bitmaps straight from the virtio devices'
        // config space (what a Linux guest would register), with the
        // previously-hardcoded bitmaps as fallback when the config read
        // yields nothing.
        if let Some(ref mut dev) = EVDEV_KEYBOARD {
            fill_capabilities(dev, &super::INPUT_KEYBOARD, false);
        }
        if let Some(ref mut dev) = EVDEV_POINTER {
            fill_capabilities(dev, &super::INPUT_POINTER, true);
        }
    }

    // Touchscreen device: same coordinates as the pointer, but with
    // touch protocol (BTN_TOUCH + ABS_MT_POSITION_X/Y). OH's
    // multimodalinput framework only recognizes touch input from
    // devices carrying these capabilities — the launcher's swipe
    // gesture needs it.
    unsafe {
        let ts = EvdevDevice::new(b"VirtIO Touchscreen", true);
        EVDEV_TOUCHSCREEN = Some(ts);
        if let Some(ref mut dev) = EVDEV_TOUCHSCREEN {
            // KEY bitmap: BTN_TOUCH = 0x14a, BTN_LEFT = 0x110 (keep both
            // so single-touch and multi-touch consumers both bind).
            let mut key_bits = [0u8; 96];
            key_bits[0x14a / 8] |= 1 << (0x14a % 8); // BTN_TOUCH
            key_bits[0x110 / 8] |= 1 << (0x110 % 8); // BTN_LEFT
            dev.set_ev_bits(1, &key_bits); // EV_KEY
            // ABS bitmap: ABS_MT_POSITION_X=0x35, ABS_MT_POSITION_Y=0x36
            let mut abs_bits = [0u8; 96];
            abs_bits[0x35 / 8] |= 1 << (0x35 % 8); // ABS_MT_POSITION_X
            abs_bits[0x36 / 8] |= 1 << (0x36 % 8); // ABS_MT_POSITION_Y
            abs_bits[0x00 / 8] |= 1 << (0x00 % 8); // ABS_X
            abs_bits[0x01 / 8] |= 1 << (0x01 % 8); // ABS_Y
            dev.set_ev_bits(3, &abs_bits); // EV_ABS
            // Copy axis ranges from the pointer device. The MT axes
            // (0x35/0x36) MUST carry the same range as ABS_X/Y: consumers
            // (OH multimodalinput) read each axis's min/max via EVIOCGABS
            // to map events into screen space — an MT axis with no absinfo
            // reads as min=max=0 and every MT position clamps to (0,0),
            // killing the swipe gesture even though the events flow.
            dev.set_absinfo(0, 0, 32767, 0, 0, 0);  // ABS_X
            dev.set_absinfo(1, 0, 32767, 0, 0, 0);  // ABS_Y
            dev.set_absinfo(0x35, 0, 32767, 0, 0, 0); // ABS_MT_POSITION_X
            dev.set_absinfo(0x36, 0, 32767, 0, 0, 0); // ABS_MT_POSITION_Y
        }
    }

    // Register device operations
    devfs::registry::register_char_device(DEV_EVDEV_KEYBOARD, &EVDEV_OPS)
        .expect("Failed to register keyboard evdev");
    devfs::registry::register_char_device(DEV_EVDEV_POINTER, &EVDEV_OPS)
        .expect("Failed to register pointer evdev");
    devfs::registry::register_char_device(
        crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN,
        &EVDEV_OPS,
    )
    .expect("Failed to register touchscreen evdev");

    // Create device nodes
    devfs::mknod("/input/event0", DEV_EVDEV_KEYBOARD, 0o666)
        .expect("Failed to create /dev/input/event0");
    devfs::mknod("/input/event1", DEV_EVDEV_POINTER, 0o666)
        .expect("Failed to create /dev/input/event1");
    devfs::mknod("/input/event2", crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN, 0o666)
        .expect("Failed to create /dev/input/event2");

    // U3: registration-time sysfs entries + "add" uevents. The
    // two-kobject layout (inputN/eventM) mirrors Linux input_register_dev:
    // the inputN uevent carries PRODUCT/NAME/EV/KEY/... which OH's libudev
    // shim and libinput need to tag the device ID_INPUT_KEYBOARD etc.;
    // DEVNAME is "input/event0" — the /dev-relative node path.
    // SAFETY: EVDEV_KEYBOARD/EVDEV_POINTER were just initialized above and
    // are only read here.
    let kbd_reg = unsafe {
        EVDEV_KEYBOARD
            .as_ref()
            .map(|d| sysfs_registration("input0", "event0", DEV_EVDEV_KEYBOARD, d))
    };
    let ptr_reg = unsafe {
        EVDEV_POINTER
            .as_ref()
            .map(|d| sysfs_registration("input1", "event1", DEV_EVDEV_POINTER, d))
    };
    if let Some(reg) = kbd_reg {
        crate::fs::sysfs::register_input_event(&reg);
    }
    if let Some(reg) = ptr_reg {
        crate::fs::sysfs::register_input_event(&reg);
    }
    // The touchscreen needs the same sysfs presence: OH's input stack
    // enumerates devices through /sys/class/input (ID_INPUT_TOUCHSCREEN +
    // the uevent), so a touchscreen with a /dev node but no sysfs entry
    // is invisible to multimodalinput — the swipe gesture never starts.
    let ts_reg = unsafe {
        EVDEV_TOUCHSCREEN
            .as_ref()
            .map(|d| sysfs_registration("input2", "event2", crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN, d))
    };
    if let Some(reg) = ts_reg {
        crate::fs::sysfs::register_input_event(&reg);
    }
}

/// Fill an evdev device's capability bitmaps from the backing virtio
/// device (locks the INPUT_* static and reads its config space).
fn fill_capabilities(
    dev: &mut EvdevDevice,
    virt_dev: &Spinlock<Option<super::VirtioInputDevice>>,
    is_pointer: bool,
) {
    let mut bits = [[0u8; BITS_LEN]; BIT_TYPES];
    let mut have_any = false;
    {
        let mut guard = virt_dev.lock();
        if let Some(ref mut vd) = *guard {
            for ev in 0..BIT_TYPES {
                if vd.read_ev_bitmap(ev as u8, &mut bits[ev]) > 0 {
                    have_any = true;
                }
            }
            // Absolute axes: read ABS_INFO for X/Y.
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_X as u8) {
                dev.set_absinfo(0, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_Y as u8) {
                dev.set_absinfo(1, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
        }
    }

    if !have_any {
        // Config space unreadable: report what the driver actually emits.
        bits[0][0] = 1; // EV_SYN/SYN_REPORT
        if is_pointer {
            // BTN_LEFT/RIGHT/MIDDLE = 0x110..0x112
            bits[1][0x110 / 8] = 0b0000_0111;
            bits[2][0] = 0x03; // REL_X | REL_Y
            bits[2][1] = 0x01; // REL_WHEEL (0x08)
            bits[3][0] = 0x03; // ABS_X | ABS_Y
            dev.set_absinfo(0, 0, 32767, 0, 0, 0);
            dev.set_absinfo(1, 0, 32767, 0, 0, 0);
        } else {
            // Standard keyboard keys 0x01..=0x58.
            for code in 1u16..=0x58 {
                bits[1][(code / 8) as usize] |= 1 << (code % 8);
            }
        }
    }

    for (ev, b) in bits.iter().enumerate() {
        dev.set_ev_bits(ev, b);
    }
}

/// Push event to evdev device
pub fn push_input_event(is_pointer: bool, event: InputEvent) {
    // input:input_event — hop 1 of the input chain: the event crossed
    // from the virtio virtqueue into the kernel input core. The armed()
    // check inside the generated call is the one relaxed load this adds
    // when the event is off; the serial echo re-checks armed() so it
    // rides the same enable path (trace_event=).
    if crate::trace::EVENT_INPUT_EVENT.armed() {
        crate::trace::trace_input_event(
            if is_pointer { "ptr" } else { "kbd" },
            event.type_,
            event.code,
            event.value,
        );
        use core::sync::atomic::{AtomicU64, Ordering};
        static ECHO_N: AtomicU64 = AtomicU64::new(0);
        let n = ECHO_N.fetch_add(1, Ordering::Relaxed);
        if input_echo_budget(n) {
            itt_msg(b"INEV[");
            itt_u64(n);
            itt_msg(b"] dev=");
            itt_msg(if is_pointer { b"ptr" } else { b"kbd" });
            itt_msg(b" type=");
            itt_u64(event.type_ as u64);
            itt_msg(b" code=");
            itt_u64(event.code as u64);
            itt_msg(b" value=");
            itt_i64(event.value as i64);
            itt_msg(b"\n");
        }
    }

    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
        static PN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let pn = PN.fetch_add(1, Ordering::Relaxed);
        if pn < 10 {
            crate::pr_warn!("IN-PUSH enter #{} is_ptr={}", pn, is_pointer);
        }
    }

    // dfx=inputtrace: payload trace to hunt phantom event streams. Prints
    // every event with a monotonically increasing ordinal so serial-log
    // frequency analysis works; rate-limited so a flooding stream cannot
    // wedge the serial console; gated by a runtime switch (default OFF).
    // Writes go through the raw console path (putchar_no_lock), NOT
    // pr_info!: session boots run loglevel=3, and printk's pre-ring
    // filter drops KERN_INFO whole — the first deployment of this trace
    // (leg-6 r6win/r8win runs, kernel with b3af8cfc) silently produced
    // zero INPTRC lines for exactly that reason. Raw bytes are the same
    // convention as sched/wakewatch's ww_msg. EV_SYN (type 0) is the
    // high-rate filler: first 200 then 1-in-1000. Payload events
    // (type != 0) carry the actual signal: first 500 then 1-in-100,
    // enough to survive a phantom flood while keeping full detail in
    // the common case (a quiet -display none boot has no events at
    // all, so anything printed here IS the anomaly).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
        use core::sync::atomic::{AtomicU64, Ordering};
        static TRACE_SEQ: AtomicU64 = AtomicU64::new(0);
        static TRACE_SYN_SEQ: AtomicU64 = AtomicU64::new(0);
        let is_syn = event.type_ == 0;
        let print = if is_syn {
            let n = TRACE_SYN_SEQ.fetch_add(1, Ordering::Relaxed);
            n < 200 || n % 1000 == 0
        } else {
            let n = TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
            n < 500 || n % 100 == 0
        };
        if print {
            let tag = if is_syn { "syn" } else if is_pointer { "ptr" } else { "kbd" };
            let n = if is_syn {
                TRACE_SYN_SEQ.load(Ordering::Relaxed)
            } else {
                TRACE_SEQ.load(Ordering::Relaxed)
            };
            // Hand-rolled formatting on the raw console path (same shape
            // as sched/wakewatch's ww_msg/ww_u64): this can fire from the
            // virtio-input IRQ context and must not touch printk locks.
            it_msg(b"INPTRC[");
            it_u64(n);
            it_msg(b"] dev=");
            it_msg(match tag {
                "syn" => b"syn",
                "ptr" => b"ptr",
                _ => b"kbd",
            });
            it_msg(b" type=");
            it_u64(event.type_ as u64);
            it_msg(b" code=");
            it_u64(event.code as u64);
            it_msg(b" value=");
            it_i64(event.value as i64);
            it_msg(b"\n");
        }
    }
    // Keyboard events feed the Ctrl-Alt-Del detector (SA_CAD semantics):
    // track Ctrl/Alt state and latch on a Delete press with both held.
    // Called from the evdev read/poll path — task context — but latching
    // only (atomic stores) keeps it safe from any context; execution
    // happens in cad_deliver_pending() on the next return to user.
    if !is_pointer {
        cad_detect(event);
    }
    // SAFETY: EVDEV_KEYBOARD and EVDEV_POINTER are initialized by init_evdev()
    // before any events can be pushed.
    unsafe {
        if is_pointer {
            if let Some(ref dev) = EVDEV_POINTER {
                dev.push_event(event);
            }
            // Mirror to the touchscreen: translate pointer events to
            // touch protocol (OH multimodalinput). BTN_LEFT(0x110) ->
            // BTN_TOUCH(0x14a); ABS_X/Y -> also ABS_MT_POSITION_X/Y.
            if let Some(ref ts) = EVDEV_TOUCHSCREEN {
                let ev_type = event.type_;
                let ev_code = event.code;
                let mut ts_event = event;
                if ev_type == 1 && ev_code == 0x110 {
                    // EV_KEY / BTN_LEFT -> BTN_TOUCH
                    ts_event.code = 0x14a;
                } else if ev_type == 3 && (ev_code == 0x00 || ev_code == 0x01) {
                    // EV_ABS / ABS_X(0) or ABS_Y(1) -> also emit
                    // ABS_MT_POSITION_X(0x35) or ABS_MT_POSITION_Y(0x36)
                    ts_event.code = if ev_code == 0x00 { 0x35 } else { 0x36 };
                }
                ts.push_event(ts_event);
            }
        } else {
            if let Some(ref dev) = EVDEV_KEYBOARD {
                dev.push_event(event);
            }
        }
    }
}

// ============================================================================
// input:* tracepoint serial echo (OH input-chain hunts)
// ============================================================================
//
// The OH image gives the hunts no in-guest way to read tracefs (no
// shell on the serial console, debugfs not mounted), so the per-hop
// verdict is read off the serial log. When the input tracepoints are
// armed — through the reference's `trace_event=` boot parameter, the
// unified-framework enable path (NOT a dfx= switch) — each traced hop
// ALSO echoes one budgeted line through uart_putc_raw (the raw UART
// path that is deliberately not compiled out of release builds; safe
// from the softirq tasklet context push_input_event runs in). Off by
// default exactly like the tracepoints themselves. Hand-rolled number
// formatting, no allocation: these fire from the input tasklet BH.

/// Serial-echo budget per class: first 300 lines in full, then every
/// 25th (a phantom event flood must not wedge the console; a quiet
/// injection window prints every line).
fn input_echo_budget(n: u64) -> bool {
    n < 300 || n % 25 == 0
}

/// Line-staging buffer for the hunt echoes (R127 wake-drop round): the
/// per-byte uart_putc_raw writes interleaved CHAR-BY-CHAR when two CPUs
/// echoed concurrently (r3's "IINPWCK_[11_7665]" mangling — two whole
/// lines lost), so the verdict lines are now staged into a stack buffer
/// and flushed as ONE critical section under a dedicated echo lock.
/// irqsave: the wake-side echoes fire from the input tasklet (BH).
static HUNT_ECHO_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

pub(crate) struct HuntLine {
    buf: [u8; 192],
    n: usize,
}

impl HuntLine {
    pub(crate) const fn new() -> Self {
        Self { buf: [0; 192], n: 0 }
    }
    pub(crate) fn push(&mut self, s: &[u8]) {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
    }
    pub(crate) fn u64(&mut self, v: u64) {
        let mut tmp = [0u8; 20];
        let mut k = 0usize;
        let mut x = v;
        if x == 0 {
            tmp[k] = b'0';
            k = 1;
        } else {
            while x > 0 && k < tmp.len() {
                tmp[k] = b'0' + (x % 10) as u8;
                x /= 10;
                k += 1;
            }
        }
        for i in (0..k).rev() {
            self.push(&[tmp[i]]);
        }
    }
    pub(crate) fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let _g = HUNT_ECHO_LOCK.lock_irqsave();
        // obs-hygiene: the burst rides the taskdump funnel's emit-lock
        // coordination (one atomic write vs printk records too — the
        // kmsg-flood splice class), not the bare per-byte raw writes.
        crate::dfx::taskdump::line_atomic_write(&self.buf[..self.n]);
        self.n = 0;
    }
}

// obs-hygiene: the INRD/INOP echo lines were the remaining per-byte
// writers of the input hunt family — stage them under the hunt echo
// lock and flush whole lines through the same emit-lock-coordinated
// write the HuntLine bursts take, so a kmsg-flood record can no longer
// splice an INRD line mid-flight (the dhardware "missing lines" class).
struct IttStage {
    buf: [u8; 128],
    n: usize,
}

static ITT_STAGE: crate::sync::spinlock::Spinlock<IttStage> =
    crate::sync::spinlock::Spinlock::new(IttStage { buf: [0; 128], n: 0 });

fn itt_msg(msg: &[u8]) {
    for &b in msg {
        let mut st = ITT_STAGE.lock_irqsave();
        let at = st.n;
        st.buf[at] = b;
        st.n = at + 1;
        if b == b'\n' || st.n == st.buf.len() {
            crate::dfx::taskdump::line_atomic_write(&st.buf[..st.n]);
            st.n = 0;
        }
    }
}

fn itt_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        itt_msg(&[buf[i]]);
    }
}

fn itt_i64(v: i64) {
    if v < 0 {
        itt_msg(b"-");
        itt_u64(v.unsigned_abs());
    } else {
        itt_u64(v as u64);
    }
}

/// The current task's (tid, tgid) for the echo lines — the boot-5 hunt
/// spent a whole taskdump roundtrip to learn that the evdev opener
/// (pid 618) was audio_server's thread inside tgid 408; carrying the
/// tgid in the echo names the process at read/open time directly.
fn current_pid_tgid() -> (u64, u64) {
    match crate::sched::current() {
        // SAFETY: cur is the running task; pid/tgid are const reads.
        Some(cur) => unsafe { ((*cur).pid() as u64, (*cur).tgid() as u64) },
        None => (0, 0),
    }
}

// Raw-console writers for the dfx=inputtrace path (same shape as
// sched/wakewatch's ww_msg/ww_u64): printk is unusable here because the
// trace must survive loglevel=3 boots, and the callers can be in IRQ
// context, so no printk locks.
fn it_msg(msg: &[u8]) {
    if !cfg!(debug_assertions) { return; } // release: diag output off

    for &b in msg {
        crate::console::putchar_no_lock(b);
    }
}

fn it_u64(mut v: u64) {
    if !cfg!(debug_assertions) { return; } // release: diag output off

    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        crate::console::putchar_no_lock(buf[i]);
    }
}

fn it_i64(v: i64) {
    if v < 0 {
        it_msg(b"-");
        it_u64(v.unsigned_abs());
    } else {
        it_u64(v as u64);
    }
}

// ============================================================================
// Ctrl-Alt-Del detection (virtio-keyboard)
// ============================================================================

/// Modifier state for the CAD detector (bit flags, IRQ-safe atomics not
/// needed: push_input_event callers hold no shared state and the worst
/// race is a missed/extra detection on a simultaneous key event).
static CAD_MOD_CTRL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static CAD_MOD_ALT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Track Ctrl/Alt and latch a Ctrl-Alt-Del: a Delete PRESS while at least
/// one Ctrl and one Alt are down (left or right either). Value semantics
/// per input.h: 0 = release, 1 = press, 2 = autorepeat (repeat counts —
/// a held Delete keeps telling the user wants out).
fn cad_detect(event: InputEvent) {
    if event.type_ != EV_KEY {
        return;
    }
    match event.code {
        super::event::KEY_LEFTCTRL | super::event::KEY_RIGHTCTRL => {
            CAD_MOD_CTRL.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        super::event::KEY_LEFTALT | super::event::KEY_RIGHTALT => {
            CAD_MOD_ALT.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        super::event::KEY_DELETE if event.value != 0 => {
            if CAD_MOD_CTRL.load(core::sync::atomic::Ordering::Acquire)
                && CAD_MOD_ALT.load(core::sync::atomic::Ordering::Acquire)
            {
                crate::syscall::process::ctrl_alt_del_latch();
            }
        }
        _ => {}
    }
}

/// Poll VirtIO input devices
///
/// irqsave on both device locks: the input tasklet BH drains the same
/// queues from softirq context, and a task-context holder interrupted by
/// the device IRQ must not deadlock against it.
fn poll_virtio_events() {
    use crate::drivers::input::{INPUT_KEYBOARD, INPUT_POINTER};

    // Poll keyboard
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                push_input_event(true, event);
            }
        }
    }
}
