# gnome-dbus2 — D-Bus reliability under the GNOME session

Branch: feature/dbus-reply-loss (from main @ 41d7aad1)
Worktree: /home/william/rux-agents/gnome-dbus2
Runs: runs/ (r1-baseline, r2-activation, ...)
Test rig: run/dbusdrv.py (cold-copies gnome.img, injects run/dbustest.sh +
run/dbuswire.py, boots -smp 2 -m 1G tcg-multi, harvests `RSLT:` lines).
dbuswire.py = pure-python3 D-Bus wire peer/caller (validated against the
host dbus-daemon first; replies must carry DESTINATION to be routed).

## Image facts (non-bugs, from debugfs inspection of gnome.img)

- `/usr/bin/gjs` does NOT exist. org.gnome.ScreenSaver.service has
  `Exec=/usr/bin/gjs /usr/share/gnome-shell/org.gnome.ScreenSaver`, so
  "Failed to execute program org.gnome.ScreenSaver: No such file or
  directory" is genuine ENOENT — image content, NOT a kernel bug.
- The launch helper is at `/usr/lib/dbus-1.0/dbus-daemon-launch-helper`,
  mode 0755 owner 1000:1000 — NOT setuid in this image. The setuid exec
  path is not exercised as built.
- org.freedesktop.login1.service (system bus) has `Exec=/bin/false` —
  deliberately; Xorg's logind call must trigger activation → helper →
  /bin/false exits → dbus-daemon sends an error reply. On Linux the
  caller gets a fast error; in fire-r74j Xorg saw a 25s reply TIMEOUT
  instead. That's the minimal system-bus repro (E1 in the matrix).
- No python3-dbus / gi in the image; dbus-send, dbus-monitor, gdbus,
  gsettings, dbus-run-session all present. No `dconf` CLI; dconf goes
  through gsettings (GSETTINGS_BACKEND=dconf → dconf-service over the
  session bus).

## fire-r74j postmortem facts (reference, kernel == main)

- SIGDEATH lines (serial.log):
  - pid=389 dbus-daemon sig=9 ppid=331 si_pid=331 (t=415s)
  - pid=450 dbus-daemon sig=9 ppid=329 si_pid=329 (t=641s)
  - pid=451 dbus-daemon-lau sig=9 ppid=1 si_pid=329 (t=642s)
  => si_code=SI_USER: a kill() from pid 329/331, each killing a
  dbus-daemon listed as its child. start-gnome.sh spawns both buses
  with `--fork`, whose pre-fork parent should have exited long before —
  so either those parents are still alive (blocked readiness read?) or
  our si_pid attribution is wrong. KILLTRACE instrumentation added
  (signal.rs send path, sender pid+comm, skips same-tgid group kills).
- No "oom-killer:" or "pagefault: Out of memory" lines in fire-r74j —
  the dbus-daemon SIGKILLs are NOT OOM kills.

## r1-baseline (main kernel, minimal matrix, -smp 2 -m 1G)

| test | outcome |
|---|---|
| A1 dbus-send unknown name | fast ServiceUnknown (0s) OK |
| A2 raw caller unknown name | ServiceUnknown in 36ms OK |
| A3 100x ListNames | 100/100 returns, p50=9ms max=10ms OK |
| B1 activation org.rux.Test | NOT triggered — MY BUG: dbus-daemon started before XDG_DATA_DIRS export, so /root/xdg never scanned → instant ServiceUnknown. Fixed for r2. |
| B2 100x Echo to peer | n/a (no peer) |
| C1 under 4 burners | n/a |
| C2 100x ListNames under 4 burners | 100/100 returns, p50=10ms max=27ms OK — no starvation |
| C3 unknown name under load | 64ms ServiceUnknown OK |
| D1 gsettings/dconf | python harness crashed: **time.sleep → OSError EFAULT** (see below); dconf-service WAS activated (ps shows pid 338) |
| E1 system bus login1 | dbus-send flag typo (--timeout vs --reply-timeout), retried in r2 |

### r1 anomalies (kernel-relevant)

1. **nanosleep/clock_nanosleep sporadic EFAULT**: python
   subprocess._wait → time.sleep raised OSError(14, 'Bad address').
   EFAULT sources in sys_clock_nanosleep: access_ok fail, or
   copy_from_user uncopied>0 (user page not resolvable at that instant).
   Suspect ties to EARLY-ROOT-FREE page-table lifetime warnings seen in
   the same run ("root owner pid=327 ALIVE, freed in pid=324 context")
   — a fork/mm teardown freeing a page-table root still in use would
   randomly unmap pages of live processes. (Same family as the known
   fake-OOM blocker.) F1 test added: 200 sleeps interleaved with
   fork/waitpid.
2. **dbus-daemon pid=337 (session bus) SIGKILLed by pid=311 during D1**
   (si_pid=311 SI_USER). pid 311 is most plausibly the session
   dbus-daemon itself (it activated dconf-service as 338). A
   dbus-daemon SIGKILLing another dbus-daemon child is bizarre;
   KILLTRACE will name the sender in r2.
3. EARLY-ROOT-FREE warnings at burner teardown.

## r2-activation (KILLTRACE kernel)

| test | outcome |
|---|---|
| A1-A3 | same as r1: all pass, 100/100 ListNames p50=8ms |
| B1 session activation (Exec=python3.10 dbuswire.py serve) | **org.freedesktop.DBus.Error.Spawn.ChildExited after 5.4s**; peer-owned marker missing — the activated child died before owning the name |
| B2 retry | same ChildExited 5.4s |
| C2 100x under 4 burners | 100/100 OK, p50=10ms max=27ms |
| D1 gsettings/dconf set+get | **rc=0, set 5s / get 2-3s — dconf round-trips fine** in the minimal case (activation of ca.desrt.dconf via the session bus works) |
| F1 200x time.sleep in python | **200/200 OSError EFAULT** — deterministic! |
| E1 system bus login1 activation | **TimedOut after 25s (service_start_timeout)** — dbus-daemon never noticed the activation child exited. THE XORG LOGIND REPRO. |

KILLTRACE hit: `pid=311(dbus-daemon) -> pid=342(dbus-daemon) tgid=342
ppid=311 SIGKILL` — the session dbus-daemon SIGKILLs its own fork child
whose comm was still dbus-daemon (pre-exec), while dconf-service
(pid 343) ran fine concurrently. => an activation-spawned child missed
its exec deadline and was killed by the dbus spawn machinery.

## r3-exectrace (EXECTRACE kernel)

peer-wrap.sh (sh wrapper capturing the activated child's stderr) shows:

```
Traceback ... self.sock.connect(addr)
FileNotFoundError: [Errno 2] No such file or directory
```

MY test bug: dbus passes DBUS_SESSION_BUS_ADDRESS with a `,guid=...`
suffix to activated children; dbuswire's parser fed the whole string to
connect() → ENOENT. Fixed in run/dbuswire.py (split on comma).

EXECTRACE facts (jiffies @100Hz):
- dbus-daemon's activation spawn shape: dbus-daemon forks a SITTER
  (stays comm=dbus-daemon, no exec), sitter forks the exec child.
- Wrapper chain exec'd FINE: /bin/sh in 12 jiffies, python3.10 in ~300
  (3s, TCG). **execve of activated children works.**
- dconf-service exec: 5 jiffies. launch-helper exec: 5 jiffies.
- The launch helper (pid 573, direct child of the system dbus-daemon,
  exec OK at jiffies 20209) **never exec'd /bin/false** — E1's 25s
  service_start_timeout then fired. The helper either starved or
  blocked after exec.
- B1v2 (sh wrapper): python died at ~8s (my connect bug); dbus-daemon
  NEVER reported ChildExited and the activation stayed pending — later
  calls (B2/C1 50s later) timed out WITHOUT a respawn (peer.log has
  only one "wrap start") => the sitter's exit never reached
  dbus-daemon either.
- G1 (bare python time.sleep, no fork): FAILS with EFAULT — and NONE
  of the instrumented sleep-syscall EFAULT branches printed. The
  failing syscall is something else entirely (EFAULTTRAC added for r4).
- G2 (sleep right after fork+waitpid): EFAULT, same.

## Working hypotheses

1. **Lost child-exit notification** (exit.rs defer_exit_notify →
   __schedule → SIGCHLD + wait_chldexit wake): system-bus activation
   never notices the helper exit (E1, deterministic 2/2); session
   activation loses the SITTER's forwarded exit sometimes (r3 B1v2).
   r2's session ChildExited at 5.4s shows the machinery CAN work —
   the loss is a race. EXITNOTIFY trace added for r4.
2. **PICK-SKIP-ALL** scheduler pick anomalies appear exactly at the
   stuck moments (pid 573 post-exec) — the freshly exec'd helper may be
   stranded runnable while CPUs idle (transient claim states also
   produce this print, so it is not yet conclusive).
3. **python time.sleep EFAULT** — syscall-level EFAULTTRAC in r4 will
   name the syscall.

## r4-exitnotify (EXITNOTIFY + EFAULTTRAC + dfx=taskdump + guid fix)

| test | outcome |
|---|---|
| B1v2 activation (guid fix) | peer-owned OK; first call 35s (TCG python startup dominated), B2: **100/100 Echo p50=18ms max=38ms** |
| C1 100x Echo under 4 burners | **100/100 p50=28ms max=220ms — zero timeouts under load** |
| C2 ListNames under load | 100/100 |
| D1 gsettings/dconf | set 6s, get 3s rc=0 — fine |
| F1/G1/G2 sleep | EFAULT still 200/200, G1 bare python fails |
| E1 login1 system activation | TimedOut 25s — STILL (2nd deterministic repro) |

### EFAULT root cause FOUND + FIXED

EFAULTTRAC: `pid=<F1 python> nr=72 a0=0 a1=0` x59. Syscall 72 =
**pselect6** — python's sleep path calls pselect6(0, NULL, NULL, NULL,
&ts, NULL), the classic pure-sleep idiom. Our pselect6_common REJECTED
all-NULL fd sets with EFAULT (bogus guard). Linux sleeps and returns 0.

FIX (kernel/src/syscall/misc.rs): removed the EFAULT guard; empty fd
sets + timeout = pure sleep, POSIX/Linux parity. This also fixes
sys_select (same core).

### E1 helper chain forensics (obs-sys.log + EXECTRACE + EXITNOTIFY)

- system dbus-daemon = 568. Activation forked sitter 575 (comm stays
  dbus-daemon) which forked 576 → exec'd launch-helper OK (10 jiffies).
- **575 went ZOMBIE at t=0.25s and was NEVER reaped by 568** for the
  whole run; **NO EXITNOTIFY line for child=575** → the deferred
  exit-notification (do_exit → defer_exit_notify → __schedule consumer)
  was never delivered. dbus-daemon 568 therefore never ran its child
  watch → activation pending → 25s service_start_timeout. THIS IS THE
  XORG/LOGIND REPRO AT KERNEL LEVEL.
- 576 (helper) after exec: never appeared in /proc listing again, never
  exec'd /bin/false, no EXITNOTIFY, no SIGDEATH — it was stranded
  runnable (r3's PICK-SKIP-ALL showed pid=573, same role, with
  oncpu=0 claim=-1 pk=1 — looks pickable but skipped).
- The /proc observer python (571) ITSELF got stranded the same way at
  t≈8s: repeated PICK-SKIP-ALL from BOTH CPUs with oncpu=0 claim=-1
  pk=1, and it never reached the end of its loop.

### Scheduler wedge instrumentation + holes fixed

1. PICK-SKIP-ALL now prints the pick-time refusing leg (1=on_cpu set,
   2=own-claim non-prev, 3=ownership refused (live claim), 4=affinity,
   5=cgroup-throttled) instead of re-reading state later (racy).
2. `__schedule`'s `next == prev` early return SKIPPED
   process_deferred_exit_notify_cpu — a pending exit notify could sit
   in the per-CPU slot until the next real switch on that CPU. Fixed:
   process the slot on the early-return path too (guarded: never fire
   prev's own notify while prev still runs — the deferred protocol
   exists to prevent exactly that reap race).

## r5-fixes (both fixes in) — WIN CONDITIONS MET

| test | r2 (main kernel) | r5 (fixed kernel) |
|---|---|---|
| F1 sleep loop (python) | 200/200 EFAULT | **0/200 errors** |
| G1 bare python sleep | fails rc=1 | **rc=0** |
| G2 sleep after fork | EFAULT | **ok** |
| E1 login1 system activation | TimedOut 25s (no reply) | **Spawn.PermissionsInvalid in 2s** (correct: the image's helper is 0755, not setuid 4750; Linux dbus returns exactly this error) |
| A3 100x ListNames | 100/100 p50=9ms | 100/100 p50=13ms |
| B2 100x Echo to activated peer | (didn't reach) | 100/100 p50=18ms |
| C1 100x Echo under 4 burners | (didn't reach) | 100/100 p50=28ms max=220ms |
| D1 gsettings/dconf | set 5s/get 2s rc=0 | set 6s/get 2s rc=0 |

E1's PermissionsInvalid is the right Linux-visible behavior: the image
ships /usr/lib/dbus-1.0/dbus-daemon-launch-helper mode 0755 (real
Ubuntu: 4750 root:messagebus). On a properly-setuid image the helper
would proceed to exec /bin/false and the caller would get the
Spawn.ChildExited-style error instead. Either way: a fast error reply,
never a silent 25s stall.

## SIGKILL sender identified (fire-r74j + KILLTRACE runs)

The mid-session dbus-daemon/launch-helper SIGKILLs (t=415 pid=389,
t=641 pid=450/451) are **not OOM and not external**: KILLTRACE shows
dbus-daemon killing its OWN activation spawn children (fork children
whose comm is still dbus-daemon = pre-exec or mid-handshake). libdbus's
spawn machinery kills children that miss their exec/own-name deadline
(observed kills at ~5s). The children were slow/stranded because of (a)
the lost exit-notify above and (b) the transient pick-skip contention
(leg=1 prints). With the notify fix the machinery completes instead of
killing.

## Remaining known issues (other agents' territory)

- Transient pick-skip contention still visible (PICK-SKIP-ALL leg=1/3,
  tasks repeatedly skipped then picked) — the R65/R74 ownership
  protocol family. The new leg codes make it diagnosable.
- EARLY-ROOT-FREE page-table warnings at fork teardown (fake-OOM
  family, under active work elsewhere).
- One-off mremap EFAULT (nr=216) seen once in python startup — not
  investigated.

## Commits (branch feature/dbus-reply-loss)

- 337a2dbf fix(syscall): allow pselect6/select with all-NULL fd sets
- 44dd986a fix(sched): deliver deferred exit-notify on __schedule's
  same-task repick
- 61d2ac67 dfx: KILLTRACE/EXECTRACE/EXITNOTIFY/NSFAULT/EFAULTTRAC +
  pick-skip legs
- fc71620c test(dbus): minimal D-Bus matrix rig + wire peer

## Full-session validation (runs/full-r1, fire6.py, 2100s, FIRE-PASS)

Kernel = this branch (both fixes). All 5 milestones reached (screen
lit t≈501s, desktop visible, 17 gsd daemons, gnome-shell up).

| metric (postmortem) | reference fire-r74j (main) | full-r1 (fixed) |
|---|---|---|
| gsd/Shell "failed to register before timeout" | 18 | **0** |
| "failed to commit changes to dconf: Timeout" | 1 | **0** |
| ScreenSaver exec ENOENT spam | 1 | 0 |
| Xorg "Did not receive a reply" (logind get-session) | 1 (t=157s) | 1 (t=169s) |
| mid-session dbus-daemon SIGKILL deaths | 389@415s, 450+451@641s | 385+386@723s (one pair; session survived — gsd registrations still 0 timeouts) |

The remaining Xorg login1 stall did NOT reproduce in the minimal rig
even under load with a GDBus caller (r6 E2:
GDBus.Error:...Spawn.PermissionsInvalid delivered in 8s with 2 CPU
burners). Suspect residual: the transient pick-skip contention under
the full-session load (PICK-SKIP-ALL leg=1/3 prints, R65/R74 family)
or single-thread-TCG timing. NEXT EXPERIMENT if chasing it: full
session with thread=multi + the new leg codes; watch the helper's
EXITNOTIFY/EXECTRACE window around Xorg's first logind call; also try
a properly setuid helper image (chmod 4750 in a test copy) so the
activation path goes all the way to /bin/false instead of the
PermissionsInvalid early exit.

## Final minimal-matrix numbers (r5/r6, fixed kernel, -smp2 -m1G)

- A1 dbus-send unknown name: instant ServiceUnknown
- A2 raw caller unknown name: 65ms ServiceUnknown
- A3 100x ListNames: 100/100, p50=13ms, max=21ms
- B1 activation: peer owns name (chain ~35s under TCG incl. python
  startup; dbus reports nothing lost)
- B2 100x Echo to activated peer: 100/100, p50=18ms, max=29ms
- C1 100x Echo under 4 burners: 100/100, p50=28ms, max=220ms
- C2 100x ListNames under load: 100/100
- D1 gsettings/dconf: set 6s / get 2s, rc=0, values round-trip
- E1 dbus-send login1 system: PermissionsInvalid in 2-4s (was: 25s
  TimedOut with no reply)
- E2 gdbus login1 under 2 burners: PermissionsInvalid in 8s
- F1 sleep loop: 0/200 errors (was 200/200 EFAULT)
- G1/G2 python sleep: pass (was EFAULT)
