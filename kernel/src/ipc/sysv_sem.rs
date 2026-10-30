//! System V Semaphores
//!
//! Implements semget, semctl, semop, semtimedop following the Linux kernel design.

use crate::arch::uaccess::{access_ok, copy_to_user, get_user, put_user};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use core::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicUsize, Ordering};

use super::util::*;

/// Maximum semaphore value (Linux SEMVMX).
const SEMVMX: i32 = 32767;

/// Sentinel returned by try_apply_semops when the operation set must block.
/// Deliberately outside the errno range so it can never be confused with a
/// real error (previously -EINVAL/-22 was used and the caller treated it as
/// a user-visible error — see review IPC-C1).
const SEMOP_NEED_BLOCK: i32 = -512;
/// Maximum semaphore adjustment value for SEM_UNDO (Linux SEMAEM).
const SEMAEM: i32 = 16384;

// Compile-time verification: IpcPermUapi mode field offset must be 20 bytes
const _: () = assert!(core::mem::offset_of!(IpcPermUapi, mode) == 20);

// ============================================================================
// UAPI Structures
// ============================================================================

/// struct seminfo — returned by IPC_INFO / SEM_INFO
/// Must match Linux's include/uapi/linux/sem.h. Total: 40 bytes.
#[repr(C)]
pub struct SemInfoUapi {
    pub semmap: i32,  // +0
    pub semmni: i32,  // +4
    pub semmns: i32,  // +8
    pub semmnu: i32,  // +12
    pub semmsl: i32,  // +16
    pub semopm: i32,  // +20
    pub semume: i32,  // +24
    pub semusz: i32,  // +28
    pub semvmx: i32,  // +32
    pub semaem: i32,  // +36
}

/// struct semid64_ds — returned by IPC_STAT, IPC_SET
///
/// riscv64: asm-generic/sembuf.h — perm(48) + otime + ctime + nsems + 2
/// unused = 88 bytes, sem_nsems at offset 64.
///
/// x86_64: the x86 uapi (arch/x86/include/uapi/asm/sembuf.h) pads EACH
/// time_t with a high word, so glibc/musl `struct semid_ds` on x86_64
/// reads sem_nsems at offset 80 in a 104-byte struct (verified against a
/// real x86_64 host: semctl(IPC_STAT) fills nsems@80). Writing the
/// asm-generic layout put nsems where x86_64 userspace sees
/// __sem_ctime_high — LTP semget01 read 0 where the kernel wrote 10.
/// The high words are pure padding on x86_64 (time_t is 64-bit), so they
/// stay zero.
#[repr(C)]
pub struct SemidDsUapi {
    pub sem_perm: IpcPermUapi,
    pub sem_otime: i64,
    #[cfg(feature = "x86_64")]
    pub sem_otime_high: u64,
    pub sem_ctime: i64,
    #[cfg(feature = "x86_64")]
    pub sem_ctime_high: u64,
    pub sem_nsems: u64,
    pub __unused3: u64,
    pub __unused4: u64,
}

/// struct sembuf — passed by userspace to semop/semtimedop
/// Total: 6 bytes, no padding
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SemBuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: u16,
}

// ============================================================================
// Kernel Structures
// ============================================================================

/// Single semaphore within a set.
struct SemEntry {
    /// Current semaphore value.
    value: AtomicI32,
    /// Number of processes waiting for value to increase (sem_op < 0).
    ncnt: AtomicUsize,
    /// Number of processes waiting for value to become zero (sem_op == 0).
    zcnt: AtomicUsize,
    /// PID of the last process that operated on THIS semaphore (semop or
    /// SETALL) — semctl(GETPID) returns it (LTP semctl07).
    pid: AtomicU32,
}

/// Semaphore set (the IPC object).
pub struct SemArray {
    pub perm: KernIpcPerm,
    /// Per-semaphore values and wait queues.
    sems: Spinlock<Option<alloc::vec::Vec<SemEntry>>>,
    /// Time of last semop.
    sem_otime: AtomicI64,
    /// Time of last semctl that changed the set.
    sem_ctime: AtomicI64,
    /// PID of last semop.
    sem_padid: AtomicU32,
    /// Lock for the entire semaphore set (protects sems vector existence).
    lock: Spinlock<()>,
    /// Wait queue for processes blocked on semop.
    wq: crate::process::wait::WaitQueueHead,
}

impl IpcObject for SemArray {
    fn get_perm(&self) -> &KernIpcPerm {
        &self.perm
    }
    fn get_perm_mut(&mut self) -> &mut KernIpcPerm {
        &mut self.perm
    }
}

impl SemArray {
    fn new(nsems: usize, key: i32, mode: u16) -> Self {
        let mut sems = alloc::vec::Vec::with_capacity(nsems);
        for _ in 0..nsems {
            sems.push(SemEntry {
                value: AtomicI32::new(0),
                ncnt: AtomicUsize::new(0),
                zcnt: AtomicUsize::new(0),
                pid: AtomicU32::new(0),
            });
        }
        Self {
            perm: KernIpcPerm::new(key, mode),
            sems: Spinlock::new(Some(sems)),
            sem_otime: AtomicI64::new(0),
            sem_ctime: AtomicI64::new(ipc_current_time()),
            sem_padid: AtomicU32::new(0),
            lock: Spinlock::new(()),
            wq: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// Get the number of semaphores in this set.
    fn nsems(&self) -> usize {
        self.sems.lock().as_ref().map(|v| v.len()).unwrap_or(0)
    }
}

/// Get current process PID.
fn get_current_pid() -> u32 {
    crate::sched::current().map(|t| t.pid() as u32).unwrap_or(0)
}

// ============================================================================
// Global semaphore registry
// ============================================================================

static SEM_IDS: IpcIds<SemArray> = IpcIds::new();

// ============================================================================
// Syscall Implementations
// ============================================================================

/// sys_semget — Create or find a semaphore set (NR 190)
pub fn sys_semget(args: [u64; 6]) -> i64 {
    let key = args[0] as i32;
    let nsems = args[1] as usize;
    let semflg = args[2] as i32;

    // Existing-set path (ipcget_public): IPC_EXCL → EEXIST takes precedence
    // over the nsems comparison, and a found key ignores nsems except the
    // "larger than the set" check. nsems == 0 on an existing key is valid —
    // semget(key, 0, 0) is the standard "just give me the id" idiom; the old
    // blanket EINVAL for nsems == 0 broke it (the EINVAL only applies when a
    // NEW set would be created, newary()'s !nsems check).
    if key != 0 {
        if let Some(existing_idx) = SEM_IDS.find_by_key(key) {
            if semflg & IPC_EXCL != 0 {
                return -(errno::EEXIST as i64);
            }
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[existing_idx] {
                let existing_nsems = entry.inner.nsems();
                if nsems > existing_nsems {
                    return -(errno::EINVAL as i64);
                }
            }
        } else if semflg & IPC_CREAT == 0 {
            // Bare lookup on a nonexistent key: ENOENT (the old nsems EINVAL
            // preempted this answer).
            return -(errno::ENOENT as i64);
        }
    }

    // New-set path (newary): nsems must be 1..=SEMMSL.
    if nsems == 0 || nsems > 256 {
        return -(errno::EINVAL as i64);
    }

    match SEM_IDS.alloc(SemArray::new(nsems, key, (semflg & 0o777) as u16), key, semflg) {
        Ok((id, _)) => id as i64,
        Err(e) => e as i64,
    }
}

/// sys_semctl — Semaphore control operations (NR 191)
pub fn sys_semctl(args: [u64; 6]) -> i64 {
    let semid = args[0] as i32;
    let semnum = args[1] as i32;
    let cmd = args[2] as i32;
    let arg = args[3];

    // Index-addressed / id-ignoring commands (IPC_INFO, SEM_STAT,
    // SEM_STAT_ANY, SEM_INFO) bypass the id→slot lookup — see msgctl.
    match cmd {
        3 | 18 | 19 | 20 => {}
        _ => {
            let _ = match SEM_IDS.find(semid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
        }
    }

    match cmd {
        IPC_RMID => {
            let idx = match SEM_IDS.find(semid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
            // Owner check (review IPC P2): Linux allows RMID when euid
            // matches uid OR cuid, or with CAP_SYS_ADMIN (the old code
            // missed the uid path and checked the wrong capability).
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    let cred = crate::sched::current().map(|t| t.cred());
                    let allowed = match cred {
                        Some(ref c) => {
                            c.euid == entry.inner.perm.uid
                                || c.euid == entry.inner.perm.cuid
                                || crate::security::capable(crate::security::CAP_SYS_ADMIN)
                        }
                        None => false,
                    };
                    if !allowed {
                        return -(errno::EPERM as i64);
                    }
                }
            }
            // Wake all blocked processes before destroying
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx] {
                    entry.inner.wq.wake_up_all();
                }
            }
            let _ = SEM_IDS.remove(semid);
            SEM_IDS.free_slot(semid);
            0
        }
        IPC_STAT => {
            // R32 (NEW-4): Linux requires S_IRUGO for IPC_STAT — the old
            // path used the initial perm-free find() and leaked the set's
            // metadata (perm/uid/gid/ctime) to any caller.
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let buf_ptr = arg as *mut SemidDsUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<SemidDsUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let mut ds = SemidDsUapi {
                sem_perm: IpcPermUapi::default(),
                sem_otime: 0,
                #[cfg(feature = "x86_64")]
                sem_otime_high: 0,
                sem_ctime: 0,
                #[cfg(feature = "x86_64")]
                sem_ctime_high: 0,
                sem_nsems: 0,
                __unused3: 0,
                __unused4: 0,
            };
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx2] {
                    ds.sem_perm = entry.inner.perm.to_uapi();
                    ds.sem_otime = entry.inner.sem_otime.load(Ordering::Relaxed);
                    ds.sem_ctime = entry.inner.sem_ctime.load(Ordering::Relaxed);
                    ds.sem_nsems = entry.inner.nsems() as u64;
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated for size_of::<SemidDsUapi>() above;
            // ds is a stack-local copy of the semaphore set metadata.
            unsafe {
                copy_to_user(buf_ptr as *mut u8, &ds as *const SemidDsUapi as *const u8, core::mem::size_of::<SemidDsUapi>());
            }
            0
        }
        IPC_SET => {
            let buf_ptr = arg as *const u8;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<SemidDsUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let idx2 = match SEM_IDS.find(semid) {
                Some(i) => i,
                None => return -(errno::EINVAL as i64),
            };
            // Owner check (review IPC P2: IPC_SET 无属主检查——任何有写权限者可夺
            // 所有权): euid must be uid or cuid, or CAP_SYS_ADMIN.
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[idx2] {
                    let cred = crate::sched::current().map(|t| t.cred());
                    let allowed = match cred {
                        Some(ref c) => {
                            c.euid == entry.inner.perm.uid
                                || c.euid == entry.inner.perm.cuid
                                || crate::security::capable(crate::security::CAP_SYS_ADMIN)
                        }
                        None => false,
                    };
                    if !allowed {
                        return -(errno::EPERM as i64);
                    }
                }
            }
            // Copy the whole struct from user memory through the
            // exception-table path (review IPC M: uaccess 裸访改 copy_from_user).
            let mut ds = SemidDsUapi {
                sem_perm: IpcPermUapi::default(),
                sem_otime: 0,
                #[cfg(feature = "x86_64")]
                sem_otime_high: 0,
                sem_ctime: 0,
                #[cfg(feature = "x86_64")]
                sem_ctime_high: 0,
                sem_nsems: 0,
                __unused3: 0,
                __unused4: 0,
            };
            // SAFETY: buf_ptr was access_ok-validated above; ds is a
            // stack-local repr(C) struct of exactly that size.
            if unsafe {
                crate::arch::uaccess::copy_from_user(
                    &mut ds as *mut SemidDsUapi as *mut u8,
                    buf_ptr,
                    core::mem::size_of::<SemidDsUapi>(),
                )
            } != 0
            {
                return -(errno::EFAULT as i64);
            }
            let mut slots = SEM_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                entry.inner.perm.update_from_set(ds.sem_perm.uid, ds.sem_perm.gid, ds.sem_perm.mode);
                entry.inner.sem_ctime.store(ipc_current_time(), Ordering::Relaxed);
            }
            0
        }
        GETVAL => {
            if semnum < 0 {
                return -(errno::EINVAL as i64);
            }
            // Permission check: requires read permission
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx2] {
                let snum = semnum as usize;
                if snum >= entry.inner.nsems() {
                    // Out-of-range sem_num in semctl is EINVAL (reference
                    // semctl_main; EFBIG belongs to semop's sops check —
                    // the old EFBIG here was a mis-cite).
                    return -(errno::EINVAL as i64);
                }
                if let Some(ref sems) = *entry.inner.sems.lock() {
                    return sems[snum].value.load(Ordering::Relaxed) as i64;
                }
            }
            -(errno::EINVAL as i64)
        }
        SETVAL => {
            // Reference order (semctl_setval): val range first (ERANGE),
            // then id (EINVAL), then sem_num range (EINVAL), then write
            // permission (S_IWUGO — not rw: a write-only mode must pass).
            let val = arg as i32;
            if val < 0 || val > SEMVMX {
                return -(errno::ERANGE as i64);
            }
            if semnum < 0 {
                return -(errno::EINVAL as i64);
            }
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o2) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let mut slots = SEM_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                let snum = semnum as usize;
                if snum >= entry.inner.nsems() {
                    // Out-of-range sem_num in semctl is EINVAL (reference
                    // semctl_setval; EFBIG is semop-only).
                    return -(errno::EINVAL as i64);
                }
                let prev;
                if let Some(ref mut sems) = *entry.inner.sems.lock() {
                    prev = sems[snum].value.swap(val, Ordering::Relaxed);
                    // SETVAL records the caller as the last operator of
                    // this semaphore (semctl GETPID; Linux do_semctl SETVAL
                    // sets sem->pid).
                    sems[snum].pid.store(get_current_pid(), Ordering::Relaxed);
                } else {
                    return -(errno::EINVAL as i64);
                }
                entry.inner.sem_ctime.store(ipc_current_time(), Ordering::Relaxed);
                // Wake waiters whose predicate may now hold (review IPC P2:
                // SETVAL 后不唤醒——"用 semctl 释放信号灯"惯用法挂死):
                // a transition to 0 releases Z-waiters; any increase from a
                // value below 1 releases P-waiters. wake_up_all is a safe
                // superset — every woken waiter re-checks its predicate.
                let _ = prev;
                entry.inner.wq.wake_up_all();
            }
            0
        }
        GETALL => {
            // R32 (NEW-4): Linux requires S_IRUGO for GETALL — was missing.
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            // union semun.array points at an unsigned short[] — the
            // element size AND stride are 2 bytes (Linux GETALL writes
            // ushort values; the old 4-byte writes corrupted every other
            // entry of the caller's array, LTP semop01/semctl01).
            let array_ptr = arg as *mut u16;
            if array_ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx2] {
                let nsems = entry.inner.nsems();
                if !access_ok(array_ptr as usize, nsems * 2) {
                    return -(errno::EFAULT as i64);
                }
                if let Some(ref sems) = *entry.inner.sems.lock() {
                    for i in 0..nsems {
                        let val = sems[i].value.load(Ordering::Relaxed);
                        // SAFETY: array_ptr was access_ok-validated for nsems*2 bytes above;
                        // put_user is the exception-table copy path (SUM=0 safe).
                        if !unsafe { put_user(array_ptr.add(i), val as u16) } {
                            // Unwritable user page (PROT_NONE bad_addr):
                            // Linux fails the whole semctl with EFAULT
                            // (LTP semctl03 raw-syscall variant).
                            return -(errno::EFAULT as i64);
                        }
                    }
                }
            }
            0
        }
        SETALL => {
            // unsigned short[] ABI — see GETALL.
            let array_ptr = arg as *const u16;
            if array_ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            // S_IWUGO: SETALL needs write permission only (reference
            // semctl_main — the old rw demand wrongly failed write-only modes).
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o2) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let mut slots = SEM_IDS.slots.lock();
            if let Some(ref mut entry) = slots[idx2] {
                let nsems = entry.inner.nsems();
                if !access_ok(array_ptr as usize, nsems * 2) {
                    return -(errno::EFAULT as i64);
                }
                // Copy the whole array and validate EVERY element before
                // storing any of them (reference SETALL: copy_from_user +
                // full ERANGE scan, then apply). The old per-element
                // apply-as-you-read left the set partially updated when a
                // later element faulted or was out of range.
                let mut vals = alloc::vec::Vec::<u16>::with_capacity(nsems);
                for i in 0..nsems {
                    // SAFETY: array_ptr was access_ok-validated for nsems*2
                    // bytes above; get_user is the exception-table copy path
                    // (SUM=0 safe). LTP semctl03 raw-syscall EFAULT variant.
                    match unsafe { get_user(array_ptr.add(i)) } {
                        Some(v) => vals.push(v),
                        None => return -(errno::EFAULT as i64),
                    }
                }
                for v in vals.iter() {
                    if (*v as i32) > SEMVMX {
                        return -(errno::ERANGE as i64);
                    }
                }
                if let Some(ref mut sems) = *entry.inner.sems.lock() {
                    for (i, v) in vals.iter().enumerate() {
                        sems[i].value.store(*v as i32, Ordering::Relaxed);
                        // SETALL records the caller as the last operator
                        // of every semaphore (semctl GETPID).
                        sems[i].pid.store(get_current_pid(), Ordering::Relaxed);
                    }
                }
                entry.inner.sem_ctime.store(ipc_current_time(), Ordering::Relaxed);
                // Wake all waiters: SETALL may zero or raise semaphores —
                // both directions can satisfy blocked P/Z operations
                // (review IPC P2).
                entry.inner.wq.wake_up_all();
            }
            0
        }
        GETPID => {
            // R32 (NEW-4): Linux requires S_IRUGO for GETPID — was missing.
            if semnum < 0 {
                return -(errno::EINVAL as i64);
            }
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx2] {
                let snum = semnum as usize;
                if snum >= entry.inner.nsems() {
                    return -(errno::EINVAL as i64);
                }
                // PID of the last process that operated on THIS semaphore
                // (Linux returns sems[semnum].pid; semctl07).
                if let Some(ref sems) = *entry.inner.sems.lock() {
                    return sems[snum].pid.load(Ordering::Relaxed) as i64;
                }
            }
            return -(errno::EINVAL as i64);
        }
        GETNCNT => {
            if semnum < 0 {
                return -(errno::EINVAL as i64);
            }
            // R32 (NEW-4): Linux requires S_IRUGO for GETNCNT — was missing.
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx2] {
                let snum = semnum as usize;
                if snum >= entry.inner.nsems() {
                    // Out-of-range sem_num in semctl is EINVAL (reference
                    // semctl_main).
                    return -(errno::EINVAL as i64);
                }
                if let Some(ref sems) = *entry.inner.sems.lock() {
                    return sems[snum].ncnt.load(Ordering::Relaxed) as i64;
                }
            }
            -(errno::EINVAL as i64)
        }
        GETZCNT => {
            if semnum < 0 {
                return -(errno::EINVAL as i64);
            }
            // R32 (NEW-4): Linux requires S_IRUGO for GETZCNT — was missing.
            let idx2 = match SEM_IDS.find_with_perms(semid, 0o4) {
                Ok(i) => i,
                Err(e) => return e as i64,
            };
            let slots = SEM_IDS.slots.lock();
            if let Some(ref entry) = slots[idx2] {
                let snum = semnum as usize;
                if snum >= entry.inner.nsems() {
                    // Out-of-range sem_num in semctl is EINVAL (reference
                    // semctl_main).
                    return -(errno::EINVAL as i64);
                }
                if let Some(ref sems) = *entry.inner.sems.lock() {
                    return sems[snum].zcnt.load(Ordering::Relaxed) as i64;
                }
            }
            -(errno::EINVAL as i64)
        }
        IPC_INFO => {
            // struct seminfo — 10 int fields = 40 bytes
            let buf_ptr = arg as *mut SemInfoUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<SemInfoUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let info = SemInfoUapi {
                semmap: 0,
                semmni: 256,
                semmns: 256 * 256,
                semmnu: 0,
                semmsl: 256,
                semopm: 500,
                semume: 0,
                semusz: 0,
                semvmx: 32767,
                semaem: 16384,
            };
            // SAFETY: buf_ptr was null-checked and access_ok-validated above.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &info as *const SemInfoUapi as *const u8,
                    core::mem::size_of::<SemInfoUapi>(),
                );
            }
            // Return: highest used slot index (ipc_get_maxidx), -1 when
            // empty — LTP feeds it into SEM_STAT as a raw index.
            sem_max_idx() as i64
        }
        18 => {
            // SEM_STAT — IPC_STAT addressed by raw kernel index; returns the
            // real semid on success (Linux uapi: SEM_STAT=18, SEM_INFO=19,
            // SEM_STAT_ANY=20 — the old arm at 18 was mislabeled SEM_INFO).
            let raw_idx = semid as usize;
            let buf_ptr = arg as *mut SemidDsUapi;
            if buf_ptr.is_null()
                || !access_ok(buf_ptr as usize, core::mem::size_of::<SemidDsUapi>())
            {
                return -(errno::EFAULT as i64);
            }
            if raw_idx >= IPC_IDS_MAX {
                return -(errno::EINVAL as i64);
            }
            let mut ds = SemidDsUapi {
                sem_perm: IpcPermUapi::default(),
                sem_otime: 0,
                #[cfg(feature = "x86_64")]
                sem_otime_high: 0,
                sem_ctime: 0,
                #[cfg(feature = "x86_64")]
                sem_ctime_high: 0,
                sem_nsems: 0,
                __unused3: 0,
                __unused4: 0,
            };
            let result_id: i32;
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[raw_idx] {
                    if entry.deleted {
                        return -(errno::EINVAL as i64);
                    }
                    // SEM_STAT requires S_IRUGO like IPC_STAT (R32 NEW-4).
                    if !ipc_check_permissions(entry.inner.get_perm(), 0o4) {
                        return -(errno::EACCES as i64);
                    }
                    ds.sem_perm = entry.inner.perm.to_uapi();
                    ds.sem_otime = entry.inner.sem_otime.load(Ordering::Relaxed);
                    ds.sem_ctime = entry.inner.sem_ctime.load(Ordering::Relaxed);
                    ds.sem_nsems = entry.inner.nsems() as u64;
                    result_id = ipc_build_id(raw_idx, entry.inner.perm.seq);
                } else {
                    return -(errno::EINVAL as i64);
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated above;
            // ds is a stack-local repr(C) struct of exactly that size.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const SemidDsUapi as *const u8,
                    core::mem::size_of::<SemidDsUapi>(),
                );
            }
            result_id as i64
        }
        20 => {
            // SEM_STAT_ANY — SEM_STAT without the permission check. The
            // reference (semctl_stat) performs NO capability check either:
            // STAT_ANY is meant to mirror the /proc/sysvipc visibility for
            // checkpoint/restore ("only audit these calls and do not do
            // traditional S_IRUGO permission checks"). The old CAP_SYS_ADMIN
            // gate wrongly returned EPERM to unprivileged callers.
            let raw_idx = semid as usize;
            let buf_ptr = arg as *mut SemidDsUapi;
            if buf_ptr.is_null()
                || !access_ok(buf_ptr as usize, core::mem::size_of::<SemidDsUapi>())
            {
                return -(errno::EFAULT as i64);
            }
            if raw_idx >= IPC_IDS_MAX {
                return -(errno::EINVAL as i64);
            }
            let mut ds = SemidDsUapi {
                sem_perm: IpcPermUapi::default(),
                sem_otime: 0,
                #[cfg(feature = "x86_64")]
                sem_otime_high: 0,
                sem_ctime: 0,
                #[cfg(feature = "x86_64")]
                sem_ctime_high: 0,
                sem_nsems: 0,
                __unused3: 0,
                __unused4: 0,
            };
            let result_id: i32;
            {
                let slots = SEM_IDS.slots.lock();
                if let Some(ref entry) = slots[raw_idx] {
                    if entry.deleted {
                        return -(errno::EINVAL as i64);
                    }
                    ds.sem_perm = entry.inner.perm.to_uapi();
                    ds.sem_otime = entry.inner.sem_otime.load(Ordering::Relaxed);
                    ds.sem_ctime = entry.inner.sem_ctime.load(Ordering::Relaxed);
                    ds.sem_nsems = entry.inner.nsems() as u64;
                    result_id = ipc_build_id(raw_idx, entry.inner.perm.seq);
                } else {
                    return -(errno::EINVAL as i64);
                }
            }
            // SAFETY: buf_ptr was null-checked and access_ok-validated above;
            // ds is a stack-local repr(C) struct of exactly that size.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &ds as *const SemidDsUapi as *const u8,
                    core::mem::size_of::<SemidDsUapi>(),
                );
            }
            result_id as i64
        }
        19 => {
            // SEM_INFO — like IPC_INFO but returns current usage
            let buf_ptr = arg as *mut SemInfoUapi;
            if buf_ptr.is_null() || !access_ok(buf_ptr as usize, core::mem::size_of::<SemInfoUapi>()) {
                return -(errno::EFAULT as i64);
            }
            let mut total_sems: usize = 0;
            {
                let slots = SEM_IDS.slots.lock();
                for entry in slots.iter() {
                    if let Some(ref e) = entry {
                        if !e.deleted {
                            total_sems += e.inner.nsems();
                        }
                    }
                }
            }
            let info = SemInfoUapi {
                semmap: 0,
                semmni: SEM_IDS.count() as i32,
                semmns: total_sems as i32,
                semmnu: 0,
                semmsl: 256,
                semopm: 500,
                semume: 0,
                semusz: SEM_IDS.count() as i32,
                semvmx: 32767,
                semaem: total_sems as i32,
            };
            // SAFETY: buf_ptr was null-checked and access_ok-validated above.
            unsafe {
                copy_to_user(
                    buf_ptr as *mut u8,
                    &info as *const SemInfoUapi as *const u8,
                    core::mem::size_of::<SemInfoUapi>(),
                );
            }
            // Return: highest used slot index, -1 when empty (see IPC_INFO).
            sem_max_idx() as i64
        }
        _ => return -(errno::EINVAL as i64),
    }
}

/// /proc/sysvipc/sem contents — one line per semaphore set (Linux ipc/util.c
/// formatting). LTP's GET_USED_ARRAYS() counts lines.
pub fn proc_show() -> alloc::vec::Vec<u8> {
    use alloc::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = writeln!(
        out,
        "       key      semid perms      nsems   uid   gid  cuid  cgid      otime      ctime"
    );
    let slots = SEM_IDS.slots.lock();
    for (i, slot) in slots.iter().enumerate() {
        if let Some(ref entry) = slot {
            if entry.deleted {
                continue;
            }
            let set = &entry.inner;
            let id = ipc_build_id(i, set.perm.seq);
            let _ = writeln!(
                out,
                "{:10} {:10} {:6o} {:8} {:6} {:6} {:6} {:6} {:11} {:11}",
                set.perm.key,
                id,
                set.perm.mode & 0o7777,
                set.nsems(),
                set.perm.uid,
                set.perm.gid,
                set.perm.cuid,
                set.perm.cgid,
                set.sem_otime.load(Ordering::Relaxed),
                set.sem_ctime.load(Ordering::Relaxed),
            );
        }
    }
    out.into_bytes()
}

/// Highest allocated sem slot index, or -1 when empty (ipc_get_maxidx).
fn sem_max_idx() -> i32 {
    let slots = SEM_IDS.slots.lock();
    for i in (0..IPC_IDS_MAX).rev() {
        if slots[i].is_some() {
            return i as i32;
        }
    }
    -1
}

/// sys_semtimedop — Semaphore operations with timeout (NR 192)
pub fn sys_semtimedop(args: [u64; 6]) -> i64 {
    let semid = args[0] as i32;
    let sops_ptr = args[1] as *const SemBuf;
    let nsops = args[2] as usize;
    let timeout_ptr = args[3] as *const u8;

    if nsops == 0 {
        return -(errno::EINVAL as i64);
    }
    if nsops > 500 {
        // SEMOPM (semopm max) — Linux returns E2BIG (LTP semop02).
        return -(errno::E2BIG as i64);
    }
    if sops_ptr.is_null() || !access_ok(sops_ptr as usize, nsops * core::mem::size_of::<SemBuf>()) {
        return -(errno::EFAULT as i64);
    }

    // Compute deadline. A sub-jiffy timeout cannot be waited; when the ops
    // cannot proceed immediately, Linux ipc/sem.c answers EAGAIN (not
    // ETIMEDOUT) — LTP semop02 blocks ops behind a 10us timeout and
    // expects EAGAIN.
    let mut sub_jiffy_timeout = false;
    let deadline = if !timeout_ptr.is_null() {
        if !access_ok(timeout_ptr as usize, 16) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: timeout_ptr was access_ok-validated for 16 bytes above;
        // get_user is the exception-table copy path (SUM=0 safe). A failed
        // read must be EFAULT — the old unwrap_or(0) turned an unmapped
        // timeout page into {0,0} and later surfaced as ETIMEDOUT
        // (LTP semop02 passes a bad timeout pointer and expects EFAULT).
        let ts_sec = match unsafe { get_user(timeout_ptr as *const i64) } {
            Some(v) => v,
            None => return -(errno::EFAULT as i64),
        };
        let ts_nsec = match unsafe { get_user((timeout_ptr as *const i64).add(1)) } {
            Some(v) => v,
            None => return -(errno::EFAULT as i64),
        };
        if ts_sec < 0 || ts_nsec < 0 || ts_nsec >= 1_000_000_000 {
            return -(errno::EINVAL as i64);
        }
        let timeout_jiffies = (ts_sec as u64) * crate::drivers::timer::HZ as u64
            + (ts_nsec as u64) * crate::drivers::timer::HZ as u64 / 1_000_000_000;
        if timeout_jiffies == 0 {
            sub_jiffy_timeout = true;
            None
        } else {
            Some(crate::drivers::timer::get_jiffies() + timeout_jiffies)
        }
    } else {
        None
    };

    // Copy sops from userspace through the exception-table path
    // (review IPC M: uaccess 裸访改 copy_from_user).
    let mut sops: alloc::vec::Vec<SemBuf> = alloc::vec::Vec::with_capacity(nsops);
    // SAFETY: sops_ptr was access_ok-validated for nsops * size_of::<SemBuf>()
    // above; sops holds exactly that many SemBuf values.
    unsafe {
        sops.set_len(nsops);
        if crate::arch::uaccess::copy_from_user(
            sops.as_mut_ptr() as *mut u8,
            sops_ptr as *const u8,
            nsops * core::mem::size_of::<SemBuf>(),
        ) != 0
        {
            return -(errno::EFAULT as i64);
        }
    }

    // Permission: write when any op alters a value (sem_op != 0), otherwise
    // read — reference __do_semtimedop does ipcperms(alter ? S_IWUGO :
    // S_IRUGO); the old unconditional write demand wrongly failed an
    // all-zero (wait-for-zero) operation on a read-only set.
    let alter = sops.iter().any(|s| s.sem_op != 0);
    let idx = match SEM_IDS.find_with_perms(semid, if alter { 0o2 } else { 0o4 }) {
        Ok(i) => i,
        Err(e) => return e as i64,
    };

    // Get nsems and validate sem_num for all operations
    let nsems_in_set;
    {
        let slots = SEM_IDS.slots.lock();
        nsems_in_set = match slots[idx] {
            Some(ref entry) => entry.inner.nsems(),
            None => return -(errno::EINVAL as i64),
        };
    }

    for sop in &sops {
        if sop.sem_num as usize >= nsems_in_set {
            // Linux ipc/sem.c: EFBIG when sem_num is out of range
            // (LTP semop02 passes -1 and PSEMS+1 as sem_num).
            return -(errno::EFBIG as i64);
        }
    }

    // First pass: try to apply all operations atomically
    // This is a simplified version — Linux does a two-pass undo algorithm
    'outer: loop {
        let result = try_apply_semops(idx, &sops, semid);
        match result {
            Ok(()) => return 0,
            Err(e) => {
                // IPC_NOWAIT and the operation cannot proceed: EAGAIN goes
                // straight back to userspace (Linux semantics).
                if e == -errno::EAGAIN {
                    return -(errno::EAGAIN as i64);
                }
                if e == SEMOP_NEED_BLOCK {
                    if sub_jiffy_timeout {
                        return -(errno::EAGAIN as i64);
                    }
                    // Blocking P/Z operation: sleep on the set's wait queue
                    let blocking_idx = find_blocking_op(idx, &sops);
                    match blocking_idx {
                        None => return -(errno::EAGAIN as i64),
                        Some(_) => {
                            // Check for signals
                            if crate::signal::signal_pending() {
                                return -(errno::EINTR as i64);
                            }

                            // Check timeout
                            if let Some(dl) = deadline {
                                if crate::drivers::timer::get_jiffies() >= dl {
                                    // Timeout expiry is EAGAIN, matching the
                                    // reference sleep loop (`if (timed_out)
                                    // error = -EAGAIN;`) and man semop(2)
                                    // ("EAGAIN ... or the time limit
                                    // specified in timeout expired"). The
                                    // old ETIMEDOUT was never a documented
                                    // semtimedop outcome.
                                    return -(errno::EAGAIN as i64);
                                }
                            }

                            let current = match crate::sched::current() {
                                Some(t) => t,
                                None => return -(errno::ESRCH as i64),
                            };

                            // Block on the semaphore set's wait queue.
                            // Increment ncnt/zcnt for the blocking semaphore
                            // so GETNCNT/GETZCNT return accurate counts.
                            {
                                let slots = SEM_IDS.slots.lock();
                                if let Some(ref entry) = slots[idx] {
                                    if entry.deleted {
                                        return -(errno::EIDRM as i64);
                                    }
                                    // Increment waiter count for the blocking semaphore.
                                    if let Some(ref sems) = *entry.inner.sems.lock() {
                                        let block_sem = sops[blocking_idx.unwrap()].sem_num as usize;
                                        if block_sem < sems.len() {
                                            if sops[blocking_idx.unwrap()].sem_op < 0 {
                                                sems[block_sem].ncnt.fetch_add(1, Ordering::Relaxed);
                                            } else if sops[blocking_idx.unwrap()].sem_op == 0 {
                                                sems[block_sem].zcnt.fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                    // R39: register + set INTERRUPTIBLE
                                    // atomically under the WQ's own lock
                                    // (prepare_to_wait). The old add()+
                                    // set_state pair ran under the
                                    // SEM_IDS slots lock, but the WAKER
                                    // (V / IPC_RMID) takes the WQ lock — a
                                    // wake landing between the two calls
                                    // consumed the one-shot token (entry
                                    // marked woken) while Task::wake_up
                                    // dropped it (target still RUNNING);
                                    // the R7-B6 retry below covers a V's
                                    // condition but NOT an RMID wake, so
                                    // the sleeper blocked forever.
                                    entry.inner.wq.prepare_to_wait(
                                        current as *mut _,
                                        false,
                                        true,
                                    );
                                }
                            }

                            // R7-B6: re-check AFTER registering on the wait
                            // queue. A V that completed between our failed
                            // try_apply and the registration already ran
                            // wake_up_all on an empty queue — that wakeup is
                            // lost and we would sleep forever. With our
                            // entry now registered, retrying either succeeds
                            // (skip the sleep entirely) or any later V wakes
                            // us through the queue.
                            match try_apply_semops(idx, &sops, semid) {
                                Ok(()) => {
                                    // Undo the waiter registration and return.
                                    {
                                        let slots = SEM_IDS.slots.lock();
                                        if let Some(ref entry) = slots[idx] {
                                            if let Some(ref sems) = *entry.inner.sems.lock() {
                                                let block_sem = sops[blocking_idx.unwrap()].sem_num as usize;
                                                if block_sem < sems.len() {
                                                    if sops[blocking_idx.unwrap()].sem_op < 0 {
                                                        sems[block_sem].ncnt.fetch_sub(1, Ordering::Relaxed);
                                                    } else if sops[blocking_idx.unwrap()].sem_op == 0 {
                                                        sems[block_sem].zcnt.fetch_sub(1, Ordering::Relaxed);
                                                    }
                                                }
                                            }
                                            entry.inner.wq.remove(current as *mut _);
                                        }
                                    }
                                    // Registration block set INTERRUPTIBLE —
                                    // nobody will wake us (we skip sleeping),
                                    // so restore RUNNING explicitly.
                                    (*current).set_state(
                                        crate::process::task::TaskState::new(
                                            crate::process::task::TaskState::RUNNING,
                                        ),
                                    );
                                    crate::sched::dequeue_task(&*current);
                                    return 0;
                                }
                                Err(_) => { /* still blocked — sleep below */ }
                            }

                            // R22-1: a signal delivered while we were still
                            // RUNNING generates no wakeup — recheck before
                            // sleeping or the waiter is unkillable until an
                            // unrelated V arrives.
                            if crate::signal::signal_pending() {
                                {
                                    let slots = SEM_IDS.slots.lock();
                                    if let Some(ref entry) = slots[idx] {
                                        if let Some(ref sems) = *entry.inner.sems.lock() {
                                            let block_sem = sops[blocking_idx.unwrap()].sem_num as usize;
                                            if block_sem < sems.len() {
                                                if sops[blocking_idx.unwrap()].sem_op < 0 {
                                                    sems[block_sem].ncnt.fetch_sub(1, Ordering::Relaxed);
                                                } else if sops[blocking_idx.unwrap()].sem_op == 0 {
                                                    sems[block_sem].zcnt.fetch_sub(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                        entry.inner.wq.remove(current as *mut _);
                                    }
                                }
                                (*current).set_state(crate::process::task::TaskState::new(crate::process::task::TaskState::RUNNING));
                                crate::sched::dequeue_task(&*current);
                                return -(errno::EINTR as i64);
                            }
                            // Arm a wakeup timer for the deadline so a
                            // never-satisfied semaphore still returns
                            // ETIMEDOUT (nothing else would wake us).
                            let timer_id = deadline
                                .map(|dl| crate::timer::add_timer_wakeup(
                                    dl, crate::sched::get_current_pid(),
                                ))
                                .unwrap_or(0);
                            // R32 (NEW-3 twin): timer pool exhausted — a
                            // timed semop would sleep forever without the
                            // deadline timer. Unregister and fail instead.
                            if deadline.is_some() && timer_id == 0 {
                                {
                                    let slots = SEM_IDS.slots.lock();
                                    if let Some(ref entry) = slots[idx] {
                                        if let Some(ref sems) = *entry.inner.sems.lock() {
                                            let block_sem = sops[blocking_idx.unwrap()].sem_num as usize;
                                            if block_sem < sems.len() {
                                                if sops[blocking_idx.unwrap()].sem_op < 0 {
                                                    sems[block_sem].ncnt.fetch_sub(1, Ordering::Relaxed);
                                                } else if sops[blocking_idx.unwrap()].sem_op == 0 {
                                                    sems[block_sem].zcnt.fetch_sub(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                        entry.inner.wq.remove(current as *mut _);
                                    }
                                }
                                (*current).set_state(
                                    crate::process::task::TaskState::new(
                                        crate::process::task::TaskState::RUNNING,
                                    ),
                                );
                                crate::sched::dequeue_task(&*current);
                                return -(errno::ENOMEM as i64);
                            }
                            crate::sched::schedule();
                            if timer_id != 0 {
                                crate::timer::del_timer(timer_id);
                            }

                            // Decrement ncnt/zcnt after wakeup (we're no longer waiting).
                            {
                                let slots = SEM_IDS.slots.lock();
                                if let Some(ref entry) = slots[idx] {
                                    if let Some(ref sems) = *entry.inner.sems.lock() {
                                        let block_sem = sops[blocking_idx.unwrap()].sem_num as usize;
                                        if block_sem < sems.len() {
                                            if sops[blocking_idx.unwrap()].sem_op < 0 {
                                                sems[block_sem].ncnt.fetch_sub(1, Ordering::Relaxed);
                                            } else if sops[blocking_idx.unwrap()].sem_op == 0 {
                                                sems[block_sem].zcnt.fetch_sub(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                    entry.inner.wq.remove(current as *mut _);
                                }
                            }

                            continue 'outer;
                        }
                    }
                } else {
                    return e as i64;
                }
            }
        }
    }
}

/// Try to apply all semop operations atomically.
/// Returns Ok if all succeed, Err if any would block.
/// Records SEM_UNDO adjustments in the current task's undo table.
/// R9-11: true when the slot's seq no longer matches the caller's semid
/// (the slot was RMID'd and re-created between capture and use).
fn e_seq_mismatch(entry: &crate::ipc::util::IpcObjectEntry<SemArray>, semid: i32) -> bool {
    super::util::ipc_id_seq(semid) != entry.inner.perm.seq
}

fn try_apply_semops(idx: usize, sops: &[SemBuf], semid: i32) -> Result<(), i32> {
    let slots = SEM_IDS.slots.lock();
    let entry = match slots[idx] {
        Some(ref e) if !e.deleted => e,
        _ => return Err(-errno::EIDRM),
    };
    // R9-11: RMID frees the slot immediately; a fresh semget can reoccupy
    // idx with a NEW seq before a woken waiter's next iteration. Without
    // this check the waiter applied its ops to a stranger's set.
    if e_seq_mismatch(entry, semid) {
        return Err(-errno::EIDRM);
    }

    if let Some(ref sems) = *entry.inner.sems.lock() {
        // Working copy: each op must see the effect of the previous ones on
        // the same semaphore (Linux perform_atomic_semop). The old code
        // computed every op from the same pre-op value, so semop(-1,-1) on
        // value=1 "succeeded" while only decrementing once (review 2R.16).
        let mut work: alloc::vec::Vec<i32> = (0..sems.len())
            .map(|i| sems[i].value.load(Ordering::Relaxed))
            .collect();
        // First pass: compute all resulting values, accumulated.
        let mut new_vals = alloc::vec::Vec::with_capacity(sops.len());
        for sop in sops {
            let cur = work[sop.sem_num as usize];
            let new_val = cur.wrapping_add(sop.sem_op as i32);
            new_vals.push(new_val);
            work[sop.sem_num as usize] = new_val;
        }

        // Second pass: verify all operations can succeed
        for (i, &new_val) in new_vals.iter().enumerate() {
            // Positive op must not exceed SEMVMX (per Linux perform_atomic_semop)
            if sops[i].sem_op > 0 && new_val > SEMVMX {
                return Err(-(errno::ERANGE as i32));
            }
            if sops[i].sem_op < 0 && new_val < 0 {
                if sops[i].sem_flg & super::IPC_NOWAIT as u16 != 0 {
                    return Err(-errno::EAGAIN);
                }
                return Err(SEMOP_NEED_BLOCK);
            }
            if sops[i].sem_op == 0 && new_val != 0 {
                if sops[i].sem_flg & super::IPC_NOWAIT as u16 != 0 {
                    return Err(-errno::EAGAIN);
                }
                return Err(SEMOP_NEED_BLOCK);
            }
        }

        // Third pass: apply all atomically
        // Save original values for rollback if SEM_UNDO adjustment exceeds SEMAEM.
        let orig_vals: alloc::vec::Vec<i32> = sops.iter().map(|s| sems[s.sem_num as usize].value.load(Ordering::Relaxed)).collect();
        let caller_pid = get_current_pid();
        for (i, &new_val) in new_vals.iter().enumerate() {
            sems[sops[i].sem_num as usize].value.store(new_val, Ordering::Relaxed);
            // Each operated semaphore records the caller as its last
            // operator (semctl GETPID; Linux perform_atomic_semop).
            sems[sops[i].sem_num as usize].pid.store(caller_pid, Ordering::Relaxed);
        }

        // Record SEM_UNDO adjustments for current process
        if let Some(task) = crate::sched::current() {
            let mut undo_table = task.sem_undo.lock();
            for sop in sops {
                if sop.sem_flg & super::util::SEM_UNDO != 0 {
                    // Dedup: accumulate into existing entry for same (semid, sem_num)
                    let mut found = false;
                    for entry in undo_table.iter_mut() {
                        if entry.semid == semid && entry.sem_num == sop.sem_num {
                            let new_adj = entry.adjustment.wrapping_add(sop.sem_op as i32);
                            if new_adj.abs() > SEMAEM {
                                // Roll back: restore original semaphore values.
                                for (j, &orig) in orig_vals.iter().enumerate() {
                                    sems[sops[j].sem_num as usize].value.store(orig, Ordering::Relaxed);
                                }
                                return Err(-(errno::ERANGE as i32));
                            }
                            entry.adjustment = new_adj;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        let adj = sop.sem_op as i32;
                        if adj.abs() > SEMAEM {
                            for (j, &orig) in orig_vals.iter().enumerate() {
                                sems[sops[j].sem_num as usize].value.store(orig, Ordering::Relaxed);
                            }
                            return Err(-(errno::ERANGE as i32));
                        }
                        undo_table.push(super::util::SemUndoEntry {
                            semid,
                            sem_num: sop.sem_num,
                            adjustment: adj,
                        });
                    }
                }
            }
        }

        entry.inner.sem_otime.store(ipc_current_time(), Ordering::Relaxed);
        entry.inner.sem_padid.store(get_current_pid(), Ordering::Relaxed);

        // Wake up other processes waiting on this semaphore set
        entry.inner.wq.wake_up_all();

        return Ok(());
    }
    Err(-errno::EIDRM)
}

/// Find the index of the first operation that needs to block.
fn find_blocking_op(idx: usize, sops: &[SemBuf]) -> Option<usize> {
    let slots = SEM_IDS.slots.lock();
    let entry = match slots[idx] {
        Some(ref e) if !e.deleted => e,
        _ => return None,
    };

    if let Some(ref sems) = *entry.inner.sems.lock() {
        for (i, sop) in sops.iter().enumerate() {
            if sop.sem_op < 0 {
                let cur = sems[sop.sem_num as usize].value.load(Ordering::Relaxed);
                if (cur + sop.sem_op as i32) < 0 {
                    return Some(i);
                }
            }
            if sop.sem_op == 0 {
                let cur = sems[sop.sem_num as usize].value.load(Ordering::Relaxed);
                if cur != 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Reverse all SEM_UNDO adjustments for a process exiting.
/// Called from do_exit() during process cleanup.
pub fn sem_undo_exit(task: *mut crate::process::Task) {
    if task.is_null() {
        return;
    }

    // Take the undo table (replaces with empty Vec)
    let entries: alloc::vec::Vec<super::util::SemUndoEntry>;
    // SAFETY: task was null-checked above and is a valid pointer to the exiting
    // task passed from do_exit; sem_undo lock is safe to acquire here.
    unsafe {
        let mut undo_table = (*task).sem_undo.lock();
        entries = core::mem::take(&mut *undo_table);
    }

    // Reverse each adjustment
    for entry in entries {
        let idx = match SEM_IDS.find(entry.semid) {
            Some(i) => i,
            None => continue, // Set already deleted, skip
        };

        let slots = SEM_IDS.slots.lock();
        if let Some(ref e) = slots[idx] {
            if !e.deleted {
                if let Some(ref sems) = *e.inner.sems.lock() {
                    let snum = entry.sem_num as usize;
                    if snum < sems.len() {
                        sems[snum].value.fetch_sub(entry.adjustment, Ordering::Relaxed);
                    }
                }
                // Wake waiters since values changed
                e.inner.wq.wake_up_all();
            }
        }
    }
}

/// sys_semop — Semaphore operations (NR 193)
/// Delegates to sys_semtimedop with NULL timeout.
pub fn sys_semop(args: [u64; 6]) -> i64 {
    // semop has NO timeout argument (NR 193): the 4th register the caller's
    // libc wrapper left in place is garbage, not a timespec pointer. Pass a
    // NULL timeout down explicitly — forwarding args verbatim made
    // access_ok/get_user fail with EFAULT (or, pre-uaccess-hardening,
    // silently read {0,0} and behave like a zero timeout), so blocked
    // semop() calls never actually blocked (LTP semctl01 GETNCNT=0,
    // semop04 churn).
    let mut a = args;
    a[3] = 0;
    sys_semtimedop(a)
}
