//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel key retention service — a minimal, reference-faithful model.
//!
//! Implements add_key(2) / request_key(2) / keyctl(2) against a small
//! global registry of keys and keyrings, mirroring the observable
//! behavior of security/keys/ (keyctl.c, process_keys.c, keyring.c):
//!
//! - Per-task thread/process/session keyring slots live in `Cred`
//!   (fork-inherited via the cred clone; exec clears the thread
//!   keyring, matching reference exec_keys()).
//! - Per-uid user and user-session keyrings are created on demand
//!   (descriptions "_uid.<uid>" / "_uid_ses.<uid>", user keyring
//!   linked into the user session keyring, like look_up_user_keyrings).
//! - Session resolution follows lookup_user_key(): accessing
//!   KEY_SPEC_SESSION_KEYRING always installs something (the shared
//!   user-session ring when !create); a CREATE-flag lookup on the
//!   shared ring or a fresh cred instead joins a NEW anonymous ring —
//!   the mechanism systemd relies on when every service
//!   (KeyringMode=private, the default) calls
//!   keyctl(KEYCTL_JOIN_SESSION_KEYRING, NULL) at spawn: 57 of those
//!   per ub2404 boot used to surface as "unknown syscall 250" ENOSYS.
//! - add_key replaces-in-place (same type+description linked in the
//!   destination ring), honors the user/logon 32767-byte payload cap
//!   (EINVAL) and keyring plen>0 (EINVAL); unsupported key types give
//!   ENODEV (key_type_lookup), NULL-payload-with-length gives EFAULT
//!   (CVE-2017-15274, LTP add_key02).
//! - request_key searches thread→process→session; a miss with a NULL
//!   callout is ENOKEY. With a callout the reference upcalls
//!   /sbin/request-key through the usermode-helper subsystem, which
//!   this kernel does not have: we answer ENOENT, the errno the
//!   reference produces when the helper cannot be executed.
//! - Keyctl commands 0-22 are implemented to man-page/keyctl.c errno
//!   fidelity (non-keyring targets → ENOTDIR from keyring_clear;
//!   unknown command → EOPNOTSUPP; permission denial → EACCES;
//!   owner-only attribute changes → EPERM).
//!
//! Known divergences from the reference (deliberate, bounded):
//! - No quotas: the /proc/sys/kernel/keys/{root_maxkeys,...} nodes
//!   exist and are writable but not enforced.
//! - Keys persist in the registry until revoked-cleaned or
//!   invalidated; there is no asynchronous garbage collector
//!   (keyctl02's ENOKEY-after-invalidate check passes because
//!   INVALIDATE makes the serial unresolvable immediately).
//! - No request-key authorisation tokens (INSTANTIATE/NEGATE/REJECT
//!   answer ENOKEY without an auth key, which nothing can obtain).
//! - DH_COMPUTE / RESTRICT_KEYRING / WATCH_KEY answer EOPNOTSUPP.
//!
//! LTP targets verified against: add_key01..04, keyctl01..03.

use crate::arch::uaccess::{access_ok, strncpy_from_user};
use crate::process::task::Cred;
use crate::sync::spinlock::Spinlock;
use crate::syscall::errno;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

// ============================================================================
// UAPI constants (include/uapi/linux/keyctl.h, linux/keyctl.h)
// ============================================================================

/// Special keyring IDs (key_serial_t is i32; negative = special).
pub const KEY_SPEC_THREAD_KEYRING: i32 = -1;
pub const KEY_SPEC_PROCESS_KEYRING: i32 = -2;
pub const KEY_SPEC_SESSION_KEYRING: i32 = -3;
pub const KEY_SPEC_USER_KEYRING: i32 = -4;
pub const KEY_SPEC_USER_SESSION_KEYRING: i32 = -5;
pub const KEY_SPEC_GROUP_KEYRING: i32 = -6;
pub const KEY_SPEC_REQKEY_AUTH_KEY: i32 = -7;

/// keyctl() commands — uapi/linux/keyctl.h numbering. The block from 12
/// up must match the real ABI exactly: 14/15 are SET_REQKEY_KEYRING /
/// SET_TIMEOUT (not REJECT/INSTANTIATE_IOV), which shift everything
/// above NEGATE by the historical insertions (LTP keyctl04 /
/// request_key02 both drive the real numbers).
const KEYCTL_GET_KEYRING_ID: u64 = 0;
const KEYCTL_JOIN_SESSION_KEYRING: u64 = 1;
const KEYCTL_UPDATE: u64 = 2;
const KEYCTL_REVOKE: u64 = 3;
const KEYCTL_CHOWN: u64 = 4;
const KEYCTL_SETPERM: u64 = 5;
const KEYCTL_DESCRIBE: u64 = 6;
const KEYCTL_CLEAR: u64 = 7;
const KEYCTL_LINK: u64 = 8;
const KEYCTL_UNLINK: u64 = 9;
const KEYCTL_SEARCH: u64 = 10;
const KEYCTL_READ: u64 = 11;
const KEYCTL_INSTANTIATE: u64 = 12;
const KEYCTL_NEGATE: u64 = 13;
const KEYCTL_SET_REQKEY_KEYRING: u64 = 14;
const KEYCTL_SET_TIMEOUT: u64 = 15;
const KEYCTL_ASSUME_AUTHORITY: u64 = 16;
const KEYCTL_GET_SECURITY: u64 = 17;
const KEYCTL_SESSION_TO_PARENT: u64 = 18;
const KEYCTL_REJECT: u64 = 19;
const KEYCTL_INSTANTIATE_IOV: u64 = 20;
const KEYCTL_INVALIDATE: u64 = 21;
const KEYCTL_GET_PERSISTENT: u64 = 22;
const KEYCTL_DH_COMPUTE: u64 = 23;
// 24-28: KEYCTL_PKEY_{QUERY,ENCRYPT,DECRYPT,SIGN,VERIFY} — no public-key
// machinery; the catch-all answers EOPNOTSUPP like an unconfigured host.
const KEYCTL_RESTRICT_KEYRING: u64 = 29;
const KEYCTL_MOVE: u64 = 30;
const KEYCTL_CAPABILITIES: u64 = 31;
const KEYCTL_WATCH_KEY: u64 = 32;

const KEYCTL_MOVE_EXCL: u64 = 1;

/// Permission bits, one nibble-wide field per class
/// (possessor << 24 | user << 16 | group << 8 | other).
const KEY_VIEW: u32 = 0x01;
const KEY_READ: u32 = 0x02;
const KEY_WRITE: u32 = 0x04;
const KEY_SEARCH: u32 = 0x08;
const KEY_LINK: u32 = 0x10;
const KEY_SETATTR: u32 = 0x20;
const KEY_ALL: u32 = 0x3f;
const KEY_PERM_MASK: u32 = 0x3f3f_3f3f;
/// New keys: possessor+user full (keyring_alloc default for user keys).
const KEY_DEFAULT_PERM: u32 = (KEY_ALL << 24) | (KEY_ALL << 16);

/// KEY_MAX_DESC_SIZE.
const KEY_MAX_DESC_SIZE: usize = 4096;
/// user/logon payload cap (user_instantiate).
const USER_PAYLOAD_MAX: usize = 32767;
/// KEY_NEGATIVE_TIMEOUT: seconds a negatively-instantiated key survives.
const KEY_NEGATIVE_TIMEOUT: u64 = 60;

// ============================================================================
// Registry
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyType {
    Keyring,
    User,
    Logon,
}

impl KeyType {
    fn name(self) -> &'static str {
        match self {
            KeyType::Keyring => "keyring",
            KeyType::User => "user",
            KeyType::Logon => "logon",
        }
    }
}

struct Key {
    /// Own serial (possession check needs it without a KEYS lookup).
    serial: u32,
    ktype: KeyType,
    desc: Vec<u8>,
    /// User/Logon payload bytes.
    payload: Vec<u8>,
    /// Serials of keys linked into this keyring (KeyType::Keyring only).
    links: Vec<u32>,
    uid: u32,
    gid: u32,
    perm: u32,
    /// CLOCK_REALTIME seconds; 0 = never expires.
    expiry: u64,
    revoked: bool,
    /// INVALIDATEd: unresolvable by serial, ignored by search.
    dead: bool,
    /// Negatively instantiated (failed request_key upcall): unreadable
    /// (KEYCTL_READ → ENOKEY, CVE-2017-12192), skipped by search, expires
    /// after KEY_NEGATIVE_TIMEOUT seconds.
    negative: bool,
}

static KEYS: Spinlock<BTreeMap<u32, Key>> = Spinlock::new(BTreeMap::new());
static NEXT_SERIAL: AtomicU32 = AtomicU32::new(1);

/// uid -> (user keyring serial, user session keyring serial).
static USER_KEYRINGS: Spinlock<BTreeMap<u32, (u32, u32)>> =
    Spinlock::new(BTreeMap::new());
/// uid -> persistent keyring serial (KEYCTL_GET_PERSISTENT).
static PERSISTENT_KEYRINGS: Spinlock<BTreeMap<u32, u32>> =
    Spinlock::new(BTreeMap::new());

// Quota knobs: readable/writable via /proc/sys/kernel/keys/* but not
// enforced (documented divergence above).
pub static ROOT_MAXKEYS: AtomicI32 = AtomicI32::new(1_000_000);
pub static ROOT_MAXBYTES: AtomicI32 = AtomicI32::new(25_000_000);
pub static MAXKEYS: AtomicI32 = AtomicI32::new(200);
pub static MAXBYTES: AtomicI32 = AtomicI32::new(20_000);

/// The /proc/sys/kernel/keys quota rows (IpcHooks): the framework's
/// sysctl generates/writes through these instead of touching the
/// service-owned statics directly.
pub fn key_quota(q: rux::api::ipc::KeyQuota) -> i32 {
    let s = match q {
        rux::api::ipc::KeyQuota::RootMaxKeys => &ROOT_MAXKEYS,
        rux::api::ipc::KeyQuota::RootMaxBytes => &ROOT_MAXBYTES,
        rux::api::ipc::KeyQuota::MaxKeys => &MAXKEYS,
        rux::api::ipc::KeyQuota::MaxBytes => &MAXBYTES,
    };
    s.load(core::sync::atomic::Ordering::Relaxed)
}
pub fn set_key_quota(q: rux::api::ipc::KeyQuota, v: i32) {
    let s = match q {
        rux::api::ipc::KeyQuota::RootMaxKeys => &ROOT_MAXKEYS,
        rux::api::ipc::KeyQuota::RootMaxBytes => &ROOT_MAXBYTES,
        rux::api::ipc::KeyQuota::MaxKeys => &MAXKEYS,
        rux::api::ipc::KeyQuota::MaxBytes => &MAXBYTES,
    };
    s.store(v, core::sync::atomic::Ordering::Relaxed);
}

fn alloc_serial() -> u32 {
    // Serial 0 is never a valid key ID (key_serial_t 0 = "no key").
    NEXT_SERIAL.fetch_add(1, Ordering::Relaxed).max(1)
}

fn now_realtime_secs() -> u64 {
    let (s, _ns) = crate::timer::realtime_secs_nanos();
    s.max(0) as u64
}

/// Live-state check shared by every positive-serial resolution.
/// Mirrors key_lookup + lookup_user_key state checks: invalid serial →
/// ENOKEY, invalidated → ENOKEY, revoked → EKEYREVOKED, expired →
/// EKEYEXPIRED.
fn state_err(key: &Key) -> Option<i32> {
    if key.dead {
        Some(errno::ENOKEY)
    } else if key.revoked {
        Some(errno::EKEYREVOKED)
    } else if key.expiry != 0 && now_realtime_secs() >= key.expiry {
        Some(errno::EKEYEXPIRED)
    } else {
        None
    }
}

fn class_perm(key: &Key, cred: &Cred, needed: u32, possessed: bool) -> bool {
    // Possessor class: the reference grants the possessor nibble to keys
    // reachable from the caller's own keyrings (key_task_permission:
    // is_key_possessed). `possessed` is computed by possessed_by() at
    // the call sites (before the KEYS lock is taken).
    if possessed && (key.perm >> 24) & needed == needed {
        return true;
    }
    // No root override: the reference key permission bits apply to root
    // as well (key_task_permission has no CAP_SYS_ADMIN bypass).
    let shift = if key.uid == cred.fsuid {
        16 // user class
    } else if key.gid == cred.egid || cred.in_group(key.gid) {
        8 // group class
    } else {
        0 // other class
    };
    (key.perm >> shift) & needed == needed
}

/// Depth-1 possession: `serial` is one of the caller's cred-attached
/// rings or is linked directly into one of them (reference possession is
/// full reachability; depth-1 covers keys created into the caller's own
/// thread/process/session rings). Locks KEYS briefly — callers must NOT
/// hold the KEYS lock.
fn possessed_by(serial: u32, cred: &Cred) -> bool {
    let slots = [
        cred.thread_keyring,
        cred.process_keyring,
        cred.session_keyring,
    ];
    let keys = KEYS.lock();
    for s in slots.into_iter().flatten() {
        if s == serial {
            return true;
        }
        if let Some(r) = keys.get(&s) {
            if r.ktype == KeyType::Keyring && r.links.contains(&serial) {
                return true;
            }
        }
    }
    false
}

// ============================================================================
// Current-task helpers
// ============================================================================

/// Snapshot of the caller's cred identity for key ownership/permission.
/// Kernel-context (no current task) answers root, like the boot-time
/// callers.
fn current_cred() -> Cred {
    match crate::sched::current() {
        Some(t) => t.cred().clone(),
        None => Cred::new_init(),
    }
}

fn set_session_slot(serial: u32) {
    if let Some(t) = crate::sched::current() {
        t.cred_mut().session_keyring = Some(serial);
    }
}

fn set_thread_slot(serial: u32) {
    if let Some(t) = crate::sched::current() {
        t.cred_mut().thread_keyring = Some(serial);
    }
}

fn set_process_slot(serial: u32) {
    if let Some(t) = crate::sched::current() {
        t.cred_mut().process_keyring = Some(serial);
    }
}

/// Called from execve (setup_new_exec): the thread keyring is cleared on
/// exec (reference exec_keys()); process and session keyrings persist.
pub fn exec_clear_thread_keyring() {
    if let Some(t) = crate::sched::current() {
        t.cred_mut().thread_keyring = None;
    }
}

// ============================================================================
// Keyring creation / slot management
// ============================================================================

fn create_key(ktype: KeyType, desc: &[u8], payload: Vec<u8>, cred: &Cred) -> u32 {
    let serial = alloc_serial();
    let key = Key {
        serial,
        ktype,
        desc: desc.to_vec(),
        payload,
        links: Vec::new(),
        uid: cred.fsuid,
        gid: cred.fsgid,
        perm: KEY_DEFAULT_PERM,
        expiry: 0,
        revoked: false,
        dead: false,
        negative: false,
    };
    KEYS.lock().insert(serial, key);
    serial
}

/// look_up_user_keyrings(): create the per-uid pair on demand, user
/// keyring linked into the user session keyring.
fn user_keyrings(uid: u32, cred: &Cred) -> (u32, u32) {
    let mut rings = USER_KEYRINGS.lock();
    if let Some(&pair) = rings.get(&uid) {
        return pair;
    }
    let ses = create_key(KeyType::Keyring, &format!("_uid_ses.{}", uid).into_bytes(), Vec::new(), cred);
    let usr = create_key(KeyType::Keyring, &format!("_uid.{}", uid).into_bytes(), Vec::new(), cred);
    // Link the user keyring into the user session keyring (reference:
    // install_user_keyrings links uid keyring into uid session ring).
    if let Some(k) = KEYS.lock().get_mut(&ses) {
        if k.ktype == KeyType::Keyring {
            k.links.push(usr);
        }
    }
    rings.insert(uid, (usr, ses));
    (usr, ses)
}

/// Create a fresh anonymous session keyring and install it on the
/// caller's cred. Returns the new serial (join_session_keyring(NULL)).
fn join_anonymous_session(cred: &Cred) -> u32 {
    let serial = create_key(KeyType::Keyring, b"_anon", Vec::new(), cred);
    set_session_slot(serial);
    serial
}

/// Resolve a keyring ID (special or positive) to a serial.
/// `create` is KEY_LOOKUP_CREATE. Returns -(errno) on failure.
///
/// Order of special handling mirrors lookup_user_key():
/// - thread/process: create-on-demand only with `create`;
/// - session: ALWAYS installs something (the shared user-session ring
///   when !create); a `create` lookup over the shared ring upgrades to
///   a fresh anonymous ring so per-service keys never land in the
///   uid-shared ring;
/// - user/user-session: created on demand regardless of `create`;
/// - group ring: ENOKEY (not implemented in the reference either);
/// - reqkey auth: ENOKEY (no authorisation tokens in this model).
fn resolve_ring(id: i32, create: bool, cred: &Cred) -> Result<u32, i32> {
    match id {
        KEY_SPEC_THREAD_KEYRING => {
            if let Some(s) = cred.thread_keyring {
                Ok(s)
            } else if create {
                let s = create_key(KeyType::Keyring, b"_tid", Vec::new(), cred);
                set_thread_slot(s);
                Ok(s)
            } else {
                Err(errno::ENOKEY)
            }
        }
        KEY_SPEC_PROCESS_KEYRING => {
            if let Some(s) = cred.process_keyring {
                Ok(s)
            } else if create {
                let s = create_key(KeyType::Keyring, b"_pid", Vec::new(), cred);
                set_process_slot(s);
                Ok(s)
            } else {
                Err(errno::ENOKEY)
            }
        }
        KEY_SPEC_SESSION_KEYRING => {
            match cred.session_keyring {
                Some(s) => {
                    let is_uid_ring = USER_KEYRINGS
                        .lock()
                        .get(&cred.uid)
                        .map(|&(_, ses)| ses == s)
                        .unwrap_or(false);
                    if create && is_uid_ring {
                        Ok(join_anonymous_session(cred))
                    } else {
                        Ok(s)
                    }
                }
                None => {
                    let (_, ses) = user_keyrings(cred.uid, cred);
                    if create {
                        Ok(join_anonymous_session(cred))
                    } else {
                        set_session_slot(ses);
                        Ok(ses)
                    }
                }
            }
        }
        KEY_SPEC_USER_KEYRING => Ok(user_keyrings(cred.uid, cred).0),
        KEY_SPEC_USER_SESSION_KEYRING => Ok(user_keyrings(cred.uid, cred).1),
        KEY_SPEC_GROUP_KEYRING | KEY_SPEC_REQKEY_AUTH_KEY => Err(errno::ENOKEY),
        _ => {
            if id <= 0 {
                // Unknown special ID: lookup_user_key default case.
                return Err(errno::EINVAL);
            }
            // Possession must be computed before the KEYS lock below.
            let possessed = possessed_by(id as u32, cred);
            let keys = KEYS.lock();
            match keys.get(&(id as u32)) {
                None => Err(errno::ENOKEY),
                Some(k) => {
                    if let Some(e) = state_err(k) {
                        return Err(e);
                    }
                    if !class_perm(k, cred, KEY_VIEW, possessed) {
                        // Permission denial is EACCES in key lookups.
                        return Err(errno::EACCES);
                    }
                    Ok(id as u32)
                }
            }
        }
    }
}

/// Resolve a key ID that must be a keyring (ENOTDIR otherwise, like
/// keyring_clear()).
fn resolve_strict_ring(id: i32, create: bool, cred: &Cred) -> Result<u32, i32> {
    let serial = resolve_ring(id, create, cred)?;
    let keys = KEYS.lock();
    match keys.get(&serial) {
        Some(k) if k.ktype == KeyType::Keyring => Ok(serial),
        Some(_) => Err(errno::ENOTDIR),
        None => Err(errno::ENOKEY),
    }
}

/// Search outcome: a live match, the errno of the FIRST unusable match
/// (keyring_search returns -EKEYREVOKED/-EKEYEXPIRED/-ENOKEY when the
/// only matching key is in a bad state — request_key(2) surfaces exactly
/// these), or nothing.
enum SearchOutcome {
    Found(u32),
    StateErr(i32),
    NotFound,
}

/// Depth-first search of a keyring tree for a live key of the given
/// type+description (search_keyring semantics: skips
/// revoked/expired/dead/unviewable).
fn search_tree(start: u32, ktype: KeyType, desc: &[u8], cred: &Cred) -> SearchOutcome {
    let keys = KEYS.lock();
    let mut stack = Vec::new();
    let mut visited = Vec::new();
    let mut state_err: Option<i32> = None;
    stack.push(start);
    while let Some(cur) = stack.pop() {
        if visited.contains(&cur) {
            continue;
        }
        visited.push(cur);
        let ring = match keys.get(&cur) {
            Some(r) if r.ktype == KeyType::Keyring => r,
            _ => continue,
        };
        for &child in &ring.links {
            if visited.contains(&child) {
                continue;
            }
            if let Some(k) = keys.get(&child) {
                if k.ktype == ktype && k.desc == desc {
                    if let Some(e) = state_err_of(k) {
                        // Negative keys read as ENOKEY; revoked/expired
                        // keep their errno for the caller to report.
                        state_err = state_err.or(Some(e));
                        continue;
                    }
                    if class_perm(k, cred, KEY_SEARCH, false) {
                        return SearchOutcome::Found(child);
                    }
                }
                if k.ktype == KeyType::Keyring {
                    stack.push(child);
                }
            }
        }
    }
    match state_err {
        Some(e) => SearchOutcome::StateErr(e),
        None => SearchOutcome::NotFound,
    }
}

/// state_err extended with negative instantiation (search face of
/// CVE-2017-12192: a negatively-instantiated match answers ENOKEY).
fn state_err_of(key: &Key) -> Option<i32> {
    if key.negative {
        Some(errno::ENOKEY)
    } else {
        state_err(key)
    }
}

// ============================================================================
// User-memory helpers
// ============================================================================

fn read_cstr(ptr: u64, max: usize) -> Result<Vec<u8>, i32> {
    if ptr == 0 {
        return Err(errno::EFAULT);
    }
    let mut buf = [0u8; KEY_MAX_DESC_SIZE + 1];
    let n = max.min(buf.len());
    match strncpy_from_user(ptr as *const u8, n, &mut buf) {
        Ok(s) => Ok(s.to_vec()),
        Err(_) => Err(errno::EFAULT),
    }
}

fn read_payload(ptr: u64, plen: usize) -> Result<Vec<u8>, i32> {
    if plen == 0 {
        return Ok(Vec::new());
    }
    if ptr == 0 || !access_ok(ptr as usize, plen) {
        return Err(errno::EFAULT);
    }
    let mut v = alloc::vec![0u8; plen];
    // SAFETY: v holds plen bytes; ptr was access_ok-checked and
    // copy_from_user is the exception-table path.
    if crate::arch::uaccess_safe::copy_in(&mut v, ptr as usize) != 0 {
        return Err(errno::EFAULT);
    }
    Ok(v)
}

/// Copy `data` out to (buf, buflen); like the reference read paths the
/// RETURN is the full size regardless of truncation.
fn copy_out_sized(buf: u64, buflen: u64, data: &[u8]) -> i64 {
    if buf != 0 && buflen > 0 {
        let n = (buflen as usize).min(data.len());
        // SAFETY: n bytes to a validated user buffer via the
        // exception-table path.
        let _ = crate::arch::uaccess_safe::copy_out(buf as usize, &data[..n]);
    }
    data.len() as i64
}

// ============================================================================
// Syscall: add_key(2)
// ============================================================================

/// add_key(type, description, payload, plen, keyring) → serial.
pub fn sys_add_key(args: [u64; 6]) -> i64 {
    let type_ptr = args[0];
    let desc_ptr = args[1];
    let payload_ptr = args[2];
    let plen = args[3] as usize;
    let ring_id = args[4] as u32 as i32;

    // key_type_lookup first: unsupported types → ENODEV before any
    // payload access (LTP add_key02 TCONFs on exactly this).
    let type_str = match read_cstr(type_ptr, 32) {
        Ok(t) => t,
        Err(e) => return -(e as i64),
    };
    let ktype = match &type_str[..] {
        b"keyring" => KeyType::Keyring,
        b"user" => KeyType::User,
        b"logon" => KeyType::Logon,
        _ => return -(errno::ENODEV as i64),
    };

    let desc = match read_cstr(desc_ptr, KEY_MAX_DESC_SIZE) {
        Ok(d) => d,
        Err(e) => return -(e as i64),
    };
    // Logon descriptions carry "service:description" (a ':' is
    // required); mirror the reference parse check.
    if ktype == KeyType::Logon && !desc.contains(&b':') {
        return -(errno::EINVAL as i64);
    }

    // Type payload limits BEFORE the copy (user_instantiate /
    // logon_instantiate cap; keyring rings take no payload).
    match ktype {
        KeyType::Keyring if plen > 0 => return -(errno::EINVAL as i64),
        KeyType::User | KeyType::Logon if plen > USER_PAYLOAD_MAX => {
            return -(errno::EINVAL as i64)
        }
        _ => {}
    }
    let payload = match read_payload(payload_ptr, plen) {
        Ok(p) => p,
        Err(e) => return -(e as i64),
    };

    let cred = current_cred();

    // Destination ring: KEY_LOOKUP_CREATE always (sys_add_key).
    let ring = match resolve_ring(ring_id, true, &cred) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let ring_possessed = possessed_by(ring, &cred);
    {
        let keys = KEYS.lock();
        if let Some(k) = keys.get(&ring) {
            if k.ktype != KeyType::Keyring {
                return -(errno::ENOTDIR as i64);
            }
            if !class_perm(k, &cred, KEY_WRITE, ring_possessed) {
                return -(errno::EACCES as i64);
            }
        } else {
            return -(errno::ENOKEY as i64);
        }
    }

    // key_create_or_update: a live key with the same type+desc linked
    // here is UPDATED in place (same serial); a revoked one is replaced.
    let existing = {
        let keys = KEYS.lock();
        keys.get(&ring)
            .and_then(|r| r.links.iter().copied().find(|&s| {
                keys.get(&s)
                    .map(|k| k.ktype == ktype && k.desc == desc)
                    .unwrap_or(false)
            }))
    };

    if let Some(old) = existing {
        let old_possessed = possessed_by(old, &cred);
        let mut keys = KEYS.lock();
        if let Some(k) = keys.get_mut(&old) {
            // add_key must not update an uninstantiated/negative key
            // (CVE-2017-15299): a negative match is unlinked and a fresh
            // positive key takes its place.
            if state_err(k).is_none() && !k.negative {
                if !class_perm(k, &cred, KEY_WRITE, old_possessed) {
                    return -(errno::EACCES as i64);
                }
                // Keyring type has no update method: replace instead
                // (reference creates a new keyring and relinks).
                if k.ktype == KeyType::Keyring {
                    drop(keys);
                    let serial = create_key(ktype, &desc, Vec::new(), &cred);
                    unlink_from_ring(ring, old);
                    link_into_ring(ring, serial);
                    return serial as i64;
                }
                k.payload = payload;
                return old as i64;
            }
            // Revoked/expired: unlink the dead link, fall through to
            // create a fresh key under the same description.
            drop(keys);
            unlink_from_ring(ring, old);
        }
    }

    let serial = create_key(ktype, &desc, payload, &cred);
    link_into_ring(ring, serial);
    serial as i64
}

fn link_into_ring(ring: u32, key: u32) {
    let mut keys = KEYS.lock();
    if let Some(r) = keys.get_mut(&ring) {
        if r.ktype == KeyType::Keyring && !r.links.contains(&key) {
            r.links.push(key);
        }
    }
}

fn unlink_from_ring(ring: u32, key: u32) {
    let mut keys = KEYS.lock();
    if let Some(r) = keys.get_mut(&ring) {
        r.links.retain(|&s| s != key);
    }
}

// ============================================================================
// Syscall: request_key(2)
// ============================================================================

/// request_key(type, description, callout_info, dest_keyring) → serial.
pub fn sys_request_key(args: [u64; 6]) -> i64 {
    let type_ptr = args[0];
    let desc_ptr = args[1];
    let callout_ptr = args[2];
    let dest = args[3] as u32 as i32;

    let type_str = match read_cstr(type_ptr, 32) {
        Ok(t) => t,
        Err(e) => return -(e as i64),
    };
    let ktype = match &type_str[..] {
        b"keyring" => KeyType::Keyring,
        b"user" => KeyType::User,
        b"logon" => KeyType::Logon,
        _ => return -(errno::ENODEV as i64),
    };
    let desc = match read_cstr(desc_ptr, KEY_MAX_DESC_SIZE) {
        Ok(d) => d,
        Err(e) => return -(e as i64),
    };

    let cred = current_cred();

    // Search the caller's three keyrings, thread → process → session
    // (request_key_and_link order). A match in a bad state propagates
    // its errno (request_key02: EKEYREVOKED / EKEYEXPIRED).
    let found = match cred.thread_keyring {
        Some(s) => search_tree(s, ktype, &desc, &cred),
        None => SearchOutcome::NotFound,
    };
    let found = match found {
        SearchOutcome::NotFound => match cred.process_keyring {
            Some(s) => search_tree(s, ktype, &desc, &cred),
            None => SearchOutcome::NotFound,
        },
        other => other,
    };
    let found = match found {
        SearchOutcome::NotFound => match cred.session_keyring {
            Some(s) => search_tree(s, ktype, &desc, &cred),
            None => SearchOutcome::NotFound,
        },
        other => other,
    };

    match found {
        SearchOutcome::Found(serial) => {
            if dest != 0 {
                match resolve_ring(dest, true, &cred) {
                    Ok(r) => link_into_ring(r, serial),
                    Err(e) => return -(e as i64),
                }
            }
            serial as i64
        }
        SearchOutcome::StateErr(e) => -(e as i64),
        SearchOutcome::NotFound if callout_ptr == 0 => -(errno::ENOKEY as i64),
        SearchOutcome::NotFound => {
            // Miss with callout: the reference constructs the key and
            // upcalls /sbin/request-key via the usermode helper. This
            // kernel has no usermode helper: the construction fails, the
            // key is negatively instantiated (reject, 60 s) and stays
            // linked in the destination keyring — exactly the
            // post-failure state keyctl07 observes (CVE-2017-12192).
            // The destination ring's Write permission is checked FIRST
            // (request_key_and_link's KEY_NEED_WRITE lookup): a denied
            // ring answers EACCES with the ring untouched
            // (CVE-2017-17807 / request_key04).
            let dest_serial = if dest != 0 {
                match resolve_ring(dest, true, &cred) {
                    Ok(r) => Some(r),
                    Err(e) => return -(e as i64),
                }
            } else {
                // dest=0: the caller's request-key default
                // (cred->jit_keyring). DEFAULT/SESSION → session ring,
                // THREAD/PROCESS → that ring, USER/USER_SESSION → the
                // per-uid rings, REQUESTOR → session (no auth context
                // exists without upcalls).
                let defl = cred.reqkey_defl;
                let id: i32 = match defl {
                    1 => KEY_SPEC_THREAD_KEYRING,
                    2 => KEY_SPEC_PROCESS_KEYRING,
                    _ => KEY_SPEC_SESSION_KEYRING,
                };
                let serial = if defl == 5 || defl == 6 {
                    let (usr, ses) = user_keyrings(cred.uid, &cred);
                    if defl == 5 { usr } else { ses }
                } else {
                    match resolve_ring(id, true, &cred) {
                        Ok(r) => r,
                        Err(e) => return -(e as i64),
                    }
                };
                Some(serial)
            };
            if let Some(r) = dest_serial {
                let dest_possessed = possessed_by(r, &cred);
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&r) {
                    if !class_perm(k, &cred, KEY_WRITE, dest_possessed) {
                        return -(errno::EACCES as i64);
                    }
                } else {
                    return -(errno::ENOKEY as i64);
                }
            }
            // Negatively instantiate into the destination ring.
            let serial = alloc_serial();
            KEYS.lock().insert(
                serial,
                Key {
                    serial,
                    ktype,
                    desc,
                    payload: Vec::new(),
                    links: Vec::new(),
                    uid: cred.fsuid,
                    gid: cred.fsgid,
                    perm: KEY_DEFAULT_PERM,
                    expiry: now_realtime_secs() + KEY_NEGATIVE_TIMEOUT,
                    revoked: false,
                    dead: false,
                    negative: true,
                },
            );
            if let Some(r) = dest_serial {
                link_into_ring(r, serial);
            }
            // No /sbin/request-key in this kernel: the errno the
            // reference caller sees when the helper cannot be executed.
            -(errno::ENOENT as i64)
        }
    }
}

// ============================================================================
// Syscall: keyctl(2)
// ============================================================================

pub fn sys_keyctl(args: [u64; 6]) -> i64 {
    let cmd = args[0] as u32 as u64;
    let a1 = args[1];
    let a2 = args[2];
    let a3 = args[3];

    let cred = current_cred();
    let id = a1 as u32 as i32;

    match cmd {
        KEYCTL_GET_KEYRING_ID => {
            let create = a2 != 0;
            match resolve_ring(id, create, &cred) {
                Ok(s) => s as i64,
                Err(e) => -(e as i64),
            }
        }

        KEYCTL_JOIN_SESSION_KEYRING => {
            if a1 == 0 {
                // Anonymous join: fresh ring, installed, serial back.
                let s = join_anonymous_session(&cred);
                return s as i64;
            }
            let name = match read_cstr(a1, KEY_MAX_DESC_SIZE) {
                Ok(n) => n,
                Err(e) => return -(e as i64),
            };
            // join_session_keyring: a leading '.' is reserved.
            if name.first() == Some(&b'.') {
                return -(errno::EPERM as i64);
            }
            if let Some(cur) = cred.session_keyring {
                // Joining the ring already installed answers 0
                // (reference quirk: `keyring == new->session_keyring`).
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&cur) {
                    if k.ktype == KeyType::Keyring && k.desc == name {
                        return 0;
                    }
                }
            }
            // find_keyring_by_name over the registry.
            let found = {
                let keys = KEYS.lock();
                keys.iter()
                    .find(|(_, k)| {
                        k.ktype == KeyType::Keyring
                            && !k.dead
                            && !k.revoked
                            && k.desc == name
                            && class_perm(k, &cred, KEY_SEARCH, false)
                    })
                    .map(|(s, _)| *s)
            };
            let serial = match found {
                Some(s) => s,
                None => create_key(KeyType::Keyring, &name, Vec::new(), &cred),
            };
            set_session_slot(serial);
            serial as i64
        }

        KEYCTL_UPDATE => {
            // keyctl_update_key: plen is capped at PAGE_SIZE.
            let plen = a3 as usize;
            if plen > 4096 {
                return -(errno::EINVAL as i64);
            }
            let payload = match read_payload(a2, plen) {
                Ok(p) => p,
                Err(e) => return -(e as i64),
            };
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    if let Some(e) = state_err(k) {
                        return -(e as i64);
                    }
                    if !class_perm(k, &cred, KEY_WRITE, possessed) {
                        return -(errno::EACCES as i64);
                    }
                    match k.ktype {
                        KeyType::Keyring => -(errno::EOPNOTSUPP as i64),
                        KeyType::User | KeyType::Logon => {
                            if payload.len() > USER_PAYLOAD_MAX {
                                return -(errno::EINVAL as i64);
                            }
                            k.payload = payload;
                            0
                        }
                    }
                }
            }
        }

        KEYCTL_REVOKE => {
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    if !class_perm(k, &cred, KEY_SETATTR, possessed) {
                        return -(errno::EACCES as i64);
                    }
                    k.revoked = true;
                    0
                }
            }
        }

        KEYCTL_CHOWN => {
            let uid = a2 as u32;
            let gid = a3 as u32;
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    // Owner-only (keyctl_chown: -EPERM).
                    if k.uid != cred.fsuid && cred.euid != 0 {
                        return -(errno::EPERM as i64);
                    }
                    if uid != (u32::MAX) {
                        k.uid = uid;
                    }
                    if gid != (u32::MAX) {
                        k.gid = gid;
                    }
                    0
                }
            }
        }

        KEYCTL_SETPERM => {
            // keyctl(KEYCTL_SETPERM, id, perm): perm is arg2 — reading it
            // from arg3 left the perm to whatever garbage the variadic
            // wrapper had there (LTP keyctl05's SETPERM/UPDATE race saw
            // EINVAL from mask garbage). The reference validates the mask
            // BEFORE the lookup (keyctl_setperm_key checks
            // perm & ~(POS|USR|GRP|OTH ALL) first): a bad mask answers
            // EINVAL even when the key is unviewable or gone.
            let perm = a2 as u32;
            if perm & !KEY_PERM_MASK != 0 {
                return -(errno::EINVAL as i64);
            }
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    if k.uid != cred.fsuid && cred.euid != 0 {
                        return -(errno::EPERM as i64);
                    }
                    k.perm = perm;
                    0
                }
            }
        }

        KEYCTL_DESCRIBE => {
            let buf = a2;
            let buflen = a3;
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let keys = KEYS.lock();
            let k = match keys.get(&serial) {
                None => return -(errno::ENOKEY as i64),
                Some(k) => k,
            };
            if !class_perm(k, &cred, KEY_VIEW, possessed) {
                return -(errno::EACCES as i64);
            }
            let desc = alloc::format!(
                "{};{};{};{:08x};{}\0",
                k.ktype.name(),
                k.uid,
                k.gid,
                k.perm,
                core::str::from_utf8(&k.desc).unwrap_or("")
            );
            copy_out_sized(buf, buflen, desc.as_bytes())
        }

        KEYCTL_CLEAR => {
            let serial = match resolve_strict_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    if !class_perm(k, &cred, KEY_WRITE, possessed) {
                        return -(errno::EACCES as i64);
                    }
                    k.links.clear();
                    0
                }
            }
        }

        KEYCTL_LINK => {
            let key_id = id;
            let ring_id = a2 as u32 as i32;
            let key = match resolve_ring(key_id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let ring = match resolve_strict_ring(ring_id, true, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            {
                let ring_possessed = possessed_by(ring, &cred);
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&ring) {
                    if !class_perm(k, &cred, KEY_WRITE, ring_possessed) {
                        return -(errno::EACCES as i64);
                    }
                }
            }
            if key == ring {
                return -(errno::EINVAL as i64);
            }
            link_into_ring(ring, key);
            0
        }

        KEYCTL_UNLINK => {
            let key_id = id;
            let ring_id = a2 as u32 as i32;
            let key = match resolve_ring(key_id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let ring = match resolve_strict_ring(ring_id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            {
                let ring_possessed = possessed_by(ring, &cred);
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&ring) {
                    if !class_perm(k, &cred, KEY_WRITE, ring_possessed) {
                        return -(errno::EACCES as i64);
                    }
                }
            }
            unlink_from_ring(ring, key);
            0
        }

        KEYCTL_SEARCH => {
            let type_ptr = a2;
            let desc_ptr = a3;
            let dest = args[4] as u32 as i32;
            let ring = match resolve_strict_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            {
                let ring_possessed = possessed_by(ring, &cred);
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&ring) {
                    if !class_perm(k, &cred, KEY_SEARCH, ring_possessed) {
                        return -(errno::EACCES as i64);
                    }
                }
            }
            let type_str = match read_cstr(type_ptr, 32) {
                Ok(t) => t,
                Err(e) => return -(e as i64),
            };
            let ktype = match &type_str[..] {
                b"keyring" => KeyType::Keyring,
                b"user" => KeyType::User,
                b"logon" => KeyType::Logon,
                _ => return -(errno::ENODEV as i64),
            };
            let desc = match read_cstr(desc_ptr, KEY_MAX_DESC_SIZE) {
                Ok(d) => d,
                Err(e) => return -(e as i64),
            };
            match search_tree(ring, ktype, &desc, &cred) {
                SearchOutcome::NotFound => -(errno::ENOKEY as i64),
                SearchOutcome::StateErr(e) => -(e as i64),
                SearchOutcome::Found(serial) => {
                    if dest != 0 {
                        match resolve_strict_ring(dest, true, &cred) {
                            Ok(r) => link_into_ring(r, serial),
                            Err(e) => return -(e as i64),
                        }
                    }
                    serial as i64
                }
            }
        }

        KEYCTL_READ => {
            let buf = a2;
            let buflen = a3;
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let keys = KEYS.lock();
            let k = match keys.get(&serial) {
                None => return -(errno::ENOKEY as i64),
                Some(k) => k,
            };
            if !class_perm(k, &cred, KEY_READ, possessed) {
                return -(errno::EACCES as i64);
            }
            // Negative keys are unreadable: KEYCTL_READ answers ENOKEY
            // (CVE-2017-12192, commit 37863c43b2c6).
            if k.negative {
                return -(errno::ENOKEY as i64);
            }
            match k.ktype {
                // Logon payloads are never readable from user space.
                KeyType::Logon => -(errno::EOPNOTSUPP as i64),
                KeyType::User => copy_out_sized(buf, buflen, &k.payload),
                KeyType::Keyring => {
                    // Linked serials, u32 LE each (keyring_read).
                    let mut bytes = Vec::with_capacity(k.links.len() * 4);
                    for &s in &k.links {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    copy_out_sized(buf, buflen, &bytes)
                }
            }
        }

        // Authorisation-token commands: no request-key upcall means no
        // auth key can ever exist — ENOKEY is the reference answer for
        // a missing auth token.
        KEYCTL_INSTANTIATE | KEYCTL_NEGATE | KEYCTL_REJECT | KEYCTL_INSTANTIATE_IOV => {
            -(errno::ENOKEY as i64)
        }

        KEYCTL_SET_REQKEY_KEYRING => {
            // keyctl_set_reqkey_keyring: record the default destination
            // (cred->jit_keyring). THREAD/PROCESS create their ring on
            // demand (keeping an existing one — CVE-2017-7472 / keyctl04:
            // the thread keyring must NOT be replaced); NO_CHANGE returns
            // the current setting; GROUP_KEYRING and any other value are
            // EINVAL. Returns the previous setting.
            let option = a1 as u32 as i32;
            let old = cred.reqkey_defl;
            if option == -1 {
                // KEY_REQKEY_DEFL_NO_CHANGE
                return old as i64;
            }
            let valid = matches!(option, 0..=6);
            if !valid {
                return -(errno::EINVAL as i64);
            }
            match option {
                1 => {
                    if cred.thread_keyring.is_none() {
                        let s = create_key(
                            KeyType::Keyring, b"_tid", Vec::new(), &cred,
                        );
                        set_thread_slot(s);
                    }
                }
                2 => {
                    if cred.process_keyring.is_none() {
                        let s = create_key(
                            KeyType::Keyring, b"_pid", Vec::new(), &cred,
                        );
                        set_process_slot(s);
                    }
                }
                _ => {}
            }
            if let Some(t) = crate::sched::current() {
                // SAFETY: sched::current() returns the running task.
                t.cred_mut().reqkey_defl = option;
            }
            old as i64
        }

        KEYCTL_SET_TIMEOUT => {
            // keyctl_set_timeout(id, timeout): KEY_NEED_SETATTR on the
            // key; timeout 0 clears, otherwise expiry = now + timeout
            // (seconds, CLOCK_REALTIME). request_key02's expired-key case
            // rides the expiry the next search observes.
            let timeout = a2 as u64;
            let serial = match resolve_ring(id, true, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let mut keys = KEYS.lock();
            match keys.get_mut(&serial) {
                None => -(errno::ENOKEY as i64),
                Some(k) => {
                    if !class_perm(k, &cred, KEY_SETATTR, possessed) {
                        return -(errno::EACCES as i64);
                    }
                    k.expiry = if timeout == 0 {
                        0
                    } else {
                        now_realtime_secs() + timeout
                    };
                    0
                }
            }
        }

        KEYCTL_ASSUME_AUTHORITY => {
            // keyctl_assume_authority: id 0 clears the (nonexistent)
            // authorisation; any other id needs an auth key, and none can
            // exist without request-key upcalls.
            if a1 == 0 {
                0
            } else {
                -(errno::ENOKEY as i64)
            }
        }

        KEYCTL_GET_SECURITY => {
            // No LSM key labels in this kernel: the reference's
            // security_key_getsecurity fallback is -EOPNOTSUPP.
            -(errno::EOPNOTSUPP as i64)
        }

        KEYCTL_SESSION_TO_PARENT => {
            // Not modeled (would need cross-task cred writes into the
            // parent); no LTP row on the list drives it.
            -(errno::EOPNOTSUPP as i64)
        }

        KEYCTL_INVALIDATE => {
            let serial = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let possessed = possessed_by(serial, &cred);
            let mut keys = KEYS.lock();
            match keys.get(&serial) {
                None => return -(errno::ENOKEY as i64),
                Some(k) => {
                    if !class_perm(k, &cred, KEY_SEARCH, possessed) {
                        return -(errno::EACCES as i64);
                    }
                }
            }
            // The serial must become unresolvable from here on
            // (keyctl02 polls REVOKE → ENOKEY after INVALIDATE): drop
            // every inbound link and remove the registry entry.
            for k in keys.values_mut() {
                if k.ktype == KeyType::Keyring {
                    k.links.retain(|&s| s != serial);
                }
            }
            keys.remove(&serial);
            0
        }

        KEYCTL_GET_PERSISTENT => {
            let uid = a1 as u32;
            let ring_id = a2 as u32 as i32;
            let target = if uid == u32::MAX { cred.fsuid } else { uid };
            if target != cred.fsuid && cred.euid != 0 {
                return -(errno::EPERM as i64);
            }
            let serial = {
                let mut persistent = PERSISTENT_KEYRINGS.lock();
                if let Some(&s) = persistent.get(&target) {
                    s
                } else {
                    let s = create_key(
                        KeyType::Keyring,
                        &format!("_persistent.{}", target).into_bytes(),
                        Vec::new(),
                        &cred,
                    );
                    persistent.insert(target, s);
                    s
                }
            };
            if ring_id != 0 {
                match resolve_strict_ring(ring_id, true, &cred) {
                    Ok(r) => link_into_ring(r, serial),
                    Err(e) => return -(e as i64),
                }
            }
            serial as i64
        }

        KEYCTL_DH_COMPUTE | KEYCTL_RESTRICT_KEYRING | KEYCTL_WATCH_KEY => {
            -(errno::EOPNOTSUPP as i64)
        }

        KEYCTL_MOVE => {
            let from_ring = a2 as u32 as i32;
            let to_ring = a3 as u32 as i32;
            let flags = args[4];
            if flags & !KEYCTL_MOVE_EXCL != 0 {
                return -(errno::EINVAL as i64);
            }
            let key = match resolve_ring(id, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let from = match resolve_strict_ring(from_ring, false, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            let to = match resolve_strict_ring(to_ring, true, &cred) {
                Ok(s) => s,
                Err(e) => return -(e as i64),
            };
            {
                let from_possessed = possessed_by(from, &cred);
                let to_possessed = possessed_by(to, &cred);
                let keys = KEYS.lock();
                for (r, possessed) in [(from, from_possessed), (to, to_possessed)] {
                    if let Some(k) = keys.get(&r) {
                        if !class_perm(k, &cred, KEY_WRITE, possessed) {
                            return -(errno::EACCES as i64);
                        }
                    }
                }
            }
            if flags & KEYCTL_MOVE_EXCL != 0 {
                let keys = KEYS.lock();
                if let Some(k) = keys.get(&to) {
                    if k.ktype == KeyType::Keyring && k.links.contains(&key) {
                        return -(errno::EEXIST as i64);
                    }
                }
            }
            unlink_from_ring(from, key);
            link_into_ring(to, key);
            0
        }

        KEYCTL_CAPABILITIES => {
            // KEYCTL_CAPS0_{BIG_KEY=0x01, INVALIDATE=0x02,
            // RESTRICT_KEYRING=0x04, MOVE=0x08}; we implement
            // INVALIDATE and MOVE, nothing else.
            let caps: [u8; 16] = [
                0x0a, 0, 0, 0, 0, 0, 0, 0, // caps0
                0, 0, 0, 0, 0, 0, 0, 0, // caps1
            ];
            let n = (a2 as usize).min(caps.len());
            if a1 != 0 && n > 0 {
                // n bytes to a user buffer via the exception-table path
                // (the safe slice twin of the historical copy_to_user
                // wrap).
                crate::arch::uaccess_safe::copy_out(a1 as usize, &caps[..n]);
            }
            n as i64
        }

        _ => -(errno::EOPNOTSUPP as i64),
    }
}
