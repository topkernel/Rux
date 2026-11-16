# DFX-MIGRATE2 — the dfx= switch → tracepoint migration map (round 5)

Branch `dfx-migrate2` (base fe5538c4e, main @ round-4 merge). Standing
directive (2026-11-02): NEW kernel observability goes through
TRACEPOINTS in the unified trace-event framework; the bespoke `dfx=`
switch table is FROZEN legacy that must not grow. Round 4 moved 13
families (62 → 49 rows, events 61-70). This round finishes the sweep:
**switches.rs 49 → 6 rows**, 40 new `rux:` events (ids 71-110).

## What moved (retired token → new event; 40 events, ids 71-110)

| id | retired `dfx=` token | new gate event | ring record added at |
|---|---|---|---|
| 71 | `schb` | `rux:schb` | SC-HB beat row (pidmax/flow/bw15) + SC-WEDGE census row |
| 72 | `parkdump` | `rux:parkdump` | RWFRZ-PARK and APP-PARK dump rows (uaddr/val) |
| 73 | `sleepsnap` | `rux:sleepsnap` | snapshot-round row |
| 74 | `ksprobe` | `rux:ksprobe` | gate-only (residue hits stay serial) |
| 75 | `wwcensus` | `rux:wwcensus` | WW heartbeat row (wakes/picks) |
| 76 | `bindertrace` | `rux:bindertrace` | gate-only (binder:* reference events carry the timeline) |
| 77 | `syscalltrace` | `rux:syscalltrace` | gate-only (raw_syscalls:* carries the boundary) |
| 78 | `inputtrace` | `rux:inputtrace` | gate-only (per-event prints are the observation) |
| 79 | `exthunt` | `rux:exthunt` | EXT-HUNT-READHOLE row (ino/logical) |
| 80 | `sfddbg` | `rux:sfddbg` | gate-only |
| 81 | `dbtrace` | `rux:dbtrace` | gate-only |
| 82 | `datatap` | `rux:datatap` | gate-only |
| 83 | `ams46` | `rux:ams46` | gate-only (census prints are the observation) |
| 84 | `vmap` | `rux:vmap` | gate-only |
| 85 | `poolcensus` | `rux:poolcensus` | POOL census row per proc + POOL-SPAWN row |
| 86 | `slowtask` | `rux:slowtask` | SLOWTASK row per task + SUMMARY row |
| 87 | `execcost` | `rux:execcost` | gate-only (ladder print is the observation) |
| 88 | `tfdcensus` | `rux:tfdcensus` | gate-only |
| 89 | `scmtap` | `rux:scmtap` | gate-only |
| 90 | `sysforensic` | `rux:sysforensic` | gate-only |
| 91 | `filerd` | `rux:filerd` | gate-only (/proc/filerd is the report) |
| 92 | `lostpark` | `rux:lostpark` | ring-dump row (entries count) |
| 93 | `allocwatch` | `rux:allocwatch` | ALLOC-STUCK report row (class/stuck-ms) |
| 94 | `r83scan` | `rux:r83scan` | gate-only (hit prints stay serial) |
| 95 | `leakhunt` | `rux:leakhunt` | gate-only (PREEMPT-LEAK reports stay serial) |
| 96 | `acqdiv` | `rux:acqdiv` (arms rux:leakhunt with it) | gate-only |
| 97 | `metawatch` | `rux:metawatch` | gate-only (META-SMASH multi-line stays serial) |
| 98 | `scribble` | `rux:scribble` | gate-only (engine gate; CONS reports stay serial) |
| 99 | `monowatch` | `rux:monowatch` | MONOWATCH-JUMP row (last/now) |
| 100 | `epollscan` | `rux:epollscan` | gate-only (rux:epoll_* family carries rows) |
| 101 | `treehunt` | `rux:treehunt` | row per flagged task (kind/pid/parent) |
| 102 | `tcptrace` | `rux:tcptrace` | gate-only |
| 103 | `strace1` | `rux:strace1` | gate-only |
| 104 | `ripsample` | `rux:ripsample` | gate-only (60 s histogram report) |
| 105 | `socksnap` | `rux:socksnap` | snapshot-round row |
| 106 | `writetrace` | `rux:writetrace` | gate-only |
| 107 | `ledger` | `rux:ledger` | series-tick row (jiffies) |
| 108 | `pid1spin` | `rux:pid1spin` | audit row per PID1SPIN line (sys/rip) |
| 109 | `steptrace` | `rux:steptrace` | gate-only |
| 110 | `pctlmon` | `rux:pctlmon` | gate-only (lifecycle prints are the observation) |

All 40 use the `rux:schedtrace` record shape (kind/pid/a/b), declared
in the one `tracepoints!` table in `kernel/src/trace/mod.rs`.

### Dead rows deleted outright (probe bodies no longer exist)

- `thpark` — the user-stack park-dump rode the txn-hunt audit, which
  round 4 replaced with `rux:txnhang`; zero call sites anywhere. The
  token stays a boot ALIAS arming its old SCHED_AND_SYSCALLS timeline
  (standard retired notice).
- `syncguard` / `syncwatch` — the reference-parity same-proc
  transaction rejection became UNCONDITIONAL kernel behavior
  (9349d7459 "the wedge family's door"); the switches gated nothing.
  `dead_dfx_tokens()` prints
  `dfx: token 'X' retired; instrument removed (no events to arm)`.
- NOTE: `pctlmon` was initially misclassified as dead (a truncated
  survey grep); `net/unix.rs` `pctlmon_on()` is live and it MIGRATED
  (event 110). A live hunt rig was observed this session booting
  `dfx=pctlmon` — the alias keeps it working.

## The mechanics (same recipe as round 4)

- **Gate swap**: 137 `switches::enabled(DfxSwitch::X)` sites became
  `crate::trace::EVENT_X.armed()` (script `run/gate_swap2.py`; one
  relaxed load per site, the old switch's exact cost class).
- **Module-static engines rewired to events**: `scribble::ENABLED` →
  `EVENT_SCRIBBLE.armed()` (20 sites incl. sched/context/buddy),
  `leakorigin::ENABLED` → `EVENT_LEAKHUNT.armed()`, `DIVERGENT_PRINT`
  → `EVENT_ACQDIV.armed()`, `filerd::on()` → `EVENT_FILERD.armed()`,
  `lostpark::arm()/armed()` → `EVENT_LOSTPARK.armed()`. Runtime arming
  via tracefs now works for all of them (the old statics were only
  boot-settable through debugfs; same contract, wider surface).
- **Serial echo**: every hunt print stays, gated by the SAME event
  enable — arming `rux:schb` via `set_event` at runtime turns the
  SC-HB lines on exactly like `dfx=schb` did at boot.
- **Boot alias**: `retired_trace_events(name)` maps each retired token
  to its family event FIRST plus the reference-family events the old
  `trace_alias` armed (schb/parkdump/sleepsnap → sched:*, syscalltrace/
  sysforensic → raw_syscalls:*, bindertrace/dbtrace → binder:*, vmap/
  poolcensus/allocwatch/leakhunt/monowatch → kmem:*, writetrace →
  page_fault_kernel+block:*, sfddbg/exthunt/filerd → ext4:/jbd2:*,
  epollscan → rux:epoll_*, execcost/treehunt → sched:*+raw_syscalls:*),
  arms them, turns `tracing_on` on, and prints the standard
  `dfx: token 'X' retired; armed trace events (first: ...)` notice.
  `trace_alias()` itself is deleted (no surviving switch had one).
- **Table shrink**: `dfx_switch_table!` **49 → 6 rows**; `SWITCH_COUNT`
  6; indices re-compacted (cmdline lookup is by name, invisible). The
  per-switch debugfs files for the 43 retired rows are gone with their
  rows; `events/rux/<name>/enable` replaces them.
- `oh_hunt_prints_on()` unchanged (keys off `rux:ohhunt` + `sprobe=1`).
- `#![recursion_limit = "4096"]` (the tracepoints! muncher recurses
  once per event; 109 events now — 512 overflowed at 149 declarations).

## The six permanent switches (with justification)

| switch | class | why it can never be a trace-event enable |
|---|---|---|
| `claiminject` | behavior injection | leaks a protocol-impossible CPU claim onto LINKED CFS tasks to reproduce the g2 wedge — changes scheduling decisions, i.e. a fault injector, not observability |
| `oncpuinject` | behavior injection | stamps the full lost-release ownership shape onto pickable tasks (R85 adjudication injector) — same class |
| `scribblepark` | gdb-park action | parks the CPU at the first confirmed scribble hit so an external gdb can arm QEMU watchpoints — an action taken on the machine; arming it also arms `rux:scribble` (the detection engine) |
| `scribblepid1` | gdb-park action | parks when pid 1 registers (boot-fixed addresses for persistent watchpoints); arms `rux:scribble` |
| `scribblesweep` | gdb-park action | parks at the 64th task registration (quiet-region watchpoints); arms `rux:scribble` |
| `wildwatch` | active detector (state-mutating mechanism) | arming write-protects the hpet guard cluster's PTEs in three aliases and sets CR0.WP — the detection mechanism IS kernel-state mutation; the #PF hit dump rides the existing kernel fault printer. Runtime-safe arming is not possible |

The scribble DETECTION (canary writes + verification + CONS reports)
migrated to `rux:scribble` like every passive family — only the park
actions and the PTE-mutating wildwatch stay. metawatch/r83scan/
allocwatch/leakhunt/lostpark/ksprobe migrated fully: their engines
only read kernel state or write private side tables.

## Verification

Gates: `PLATFORM=x86_64 make build` rc=0, riscv64 `make build` rc=0,
`PLATFORM=aarch64 make build` rc=0, riscv64 `make test` —
**ALL TESTS PASSED, suite verdict green, wrapper RC=0** (same green
shape as the round-4 log; `run/gate2-riscv-maketest.log`).

Probe boots (x86_64 KVM q35, 2 CPU/2G, probe-as-init ext4; driver
`run/dfx-migrate2-verify.sh`, probe `run/dfx_mig2_probe.c` — round-4
probe + a 35 s sleep before the tracefs dump so the heartbeat
families' ring rows exist):

- **Boot A — `trace_event=<38 quiet new events>`** (canonical path;
  scribble and steptrace excluded — one PUB line per context switch /
  two lines per syscall flood the UART and the probe never reaches its
  dump; they get dedicated boots below): probe completed, 0 panics;
  all **40 new event names present in available_events**; serial
  carries the family cadences (SC-HB/SC-ANAT, WW/WW2/WW3/WW4/WW5,
  SLOWTASK + STK chains, sleepsnap, LP-CENSUS, PREEMPT-LEAK,
  PID1SPIN); the trace ring holds the structured rows —
  `wwcensus: kind=BEAT` (1 Hz), `schb: kind=BEAT` (10 s),
  `slowtask: kind=SLOWTASK` per reported task, `sleepsnap: kind=SNAP`,
  `ledger: kind=CENSUS`, `lostpark: kind=DUMP`:
  ```
  <idle>-0 [001] d.s..  1.183491: wwcensus: kind=BEAT pid=0 a=0x13 b=0x12
  <idle>-0 [001] d.s..  1.248110: slowtask: kind=SLOWTASK pid=300 a=0xffffffff00000002 b=0x0
  ```
- **Boot B — `dfx=<37 retired tokens + thpark + syncguard,syncwatch>`**
  (alias path): probe completed, 0 panics; **39 `retired; armed trace
  events` notices** (37 quiet + thpark) + the 2
  `retired; instrument removed` dead notices; serial family SET
  identical to boot A (15 line classes, counts within boot-duration
  noise); the ring additionally carries the alias-armed reference
  rows (`sys_enter`/`sched_switch` — the same timeline a pre-migration
  `dfx=` boot recorded; ring wrapped 4182/1618 because raw_syscalls
  records every syscall).
- **Boot C — no tokens**: probe completed, 0 panics, **0 hunt lines**
  (the single regex hit was the probe's own available_events listing
  naming `rux:scribble` — an event name, not a hunt print).
- **Boot D — `trace_event=rux:scribble`** (30 s): 13,212 PUB ring-log
  lines through the event gate alone, 0 panics — the engine keys off
  the event exactly as it keyed off the switch.
- **Boot E — `dfx=scribble,steptrace`** (30 s, alias): 12,241 PUB
  lines + both alias notices, 0 panics — the loud families' alias path
  arms the identical gate.

Numbers: A hunt-lines 1732 vs B 1692 (cadence-audit counts differ
run-to-run; tag sets identical). Logs: `run/mig2-{a,b,c,d,e}-serial.log`.

## Hunt-lane usage notes (unchanged from round 4)

- Boot arming (either path) also turns `tracing_on` on — serial echo
  AND ring records start together.
- Runtime arming via `set_event` arms the gate (serial echo responds)
  but does NOT flip `tracing_on`; ring records additionally need
  `echo 1 > tracing_on` (reference semantics).
- Scripts that poked `/sys/kernel/debug/dfx/<token>` for the 43
  retired rows move to `events/rux/<event>/enable`.
- `dfx=scribblepark/-pid1/-sweep` still work as switches AND arm
  `rux:scribble` (the engine) as a side effect.
- `dfx=schb` semantics note: the schbd kthread still starts only at
  boot (init-time check, as before — the old debugfs knob also could
  not start it post-boot); runtime-arming `rux:schb` enables the pace
  counters and gates, not a new kthread.

## Scoreboard

- switches.rs: **49 → 6 rows** (round 4: 62 → 49; rounds total: 62 → 6).
- trace/mod.rs: +40 events (ids 71-110; the table's 109 events now
  span ids 1-110).
- Tokens: 39 retired-alias + 1 retired-dead-with-timeline (thpark) +
  2 retired-dead (syncguard, syncwatch) + 6 permanent switches.
- The injection class (claiminject/oncpuinject), the gdb-park actions
  (scribblepark/pid1/sweep) and the PTE-mutating wildwatch are the
  documented permanent remainder — everything observational is now a
  tracepoint.
