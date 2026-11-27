# Debug subsystem — Linux alignment matrix, delete list, gating audit

> Status: authoritative inventory driving the debug-alignment execution lanes
> (2026-11-27 user directive: the debug subsystem must fully align with Linux —
> every feature matching the reference kernel's shape, all bespoke ad-hoc
> print-style debug code deleted, all debug code release-gated with zero
> release-performance impact).
>
> Base: `main` @ 830da5bed (all `file:line` references below are against that
> tree; they will drift as the execution lanes land — re-verify before editing).
>
> Method: every Rux-side claim was verified by reading the cited file at the
> cited location in this tree. Linux-side claims describe the standard upstream
> interface shapes (Documentation/, fs/proc, kernel/trace, include/trace/events);
> where a detail is uncertain it is marked *unverified* rather than invented.

Rux today has three debug layers:

1. **kernel/src/trace/** — the tracepoint framework and tracefs (4,637 lines in
   `mod.rs` + `files.rs`): 109 events + 1 marker pseudo-event, per-CPU ring
   buffers, the reference's trace text format, and a tracefs file set captured
   from a live Linux `/sys/kernel/tracing`.
2. **kernel/src/dfx/** — 39 files, 19,754 lines: Linux-shaped detectors
   (oops, hung_task, softlockup, taint, backtrace, ksyms, pstore) plus ~25
   bespoke hunt/profiler instruments wired directly into production paths.
3. **Scattered ad-hoc debug** outside dfx/: cmdline-string-gated print
   families (`rt2xdbg`, `cycdbg`, the `sprobe` OH-hunt prints, `binderprobe`,
   the evdev `IN-EV` prints), the frozen `dfx=` switch table
   (`dfx/switches.rs`), and the GRQ one-word event ring (`sched/sched_ev.rs`).

---

## §1 Feature matrix

Classifications: **ALIGNED** (exists, matches the Linux shape; deltas noted) ·
**RESHAPE** (exists but bespoke shape; Linux target shape specified) ·
**MISSING** (absent; fill plan + priority) · **DELETE** (bespoke, no Linux
counterpart; see §2).

### 1.1 printk / dmesg / devkmsg

| # | Linux surface | Linux shape | Rux status | Rux evidence | Class |
|---|---|---|---|---|---|
| M1 | printk ring + 8 levels (`KERN_EMERG..KERN_DEBUG`) | levelled ring buffer, `console_loglevel` filter | ALIGNED | `kernel/src/printk.rs:30-42` (levels), `:184` (console level), `:275-279` (level filter before ring) | ALIGNED. Delta: console-level filtering happens BEFORE the ring entry for kernel prints — the record itself is level-tagged and still readable via syslog, but Linux filters at console emission only. The userspace path is correct (M3). |
| M2 | `syslog(2)` types 0–10 | read/clear/console-level control, `check_syslog_permissions` | ALIGNED | `kernel/src/printk.rs:787-866` (all types incl. size queries 9/10, console on/off 6/7, level set 8), permission check at `:804-805` | ALIGNED |
| M3 | `/dev/kmsg` | every write enters the ring regardless of console level; readers use sequence numbers | ALIGNED | devfs node `kernel/src/fs/devfs/mod.rs:545`, `mknod` at `printk.rs:1830`; semantics `printk.rs:285-304` (record always enters ring; console echo gated by `printk_devkmsg`) | ALIGNED |
| M4 | `/proc/kmsg` | klogd consumption, shared ring sequence with klogctl | ALIGNED | `kernel/src/fs/procfs/mod.rs:672`; shared-sequence note `printk.rs:1866` | ALIGNED |
| M5 | `/proc/sys/kernel/printk` (4-value) | current/default/minimum/boot console level | ALIGNED (with fixed defaults) | `kernel/src/fs/procfs/sysctl.rs:429-437` — read emits `"{cur} 4 1 7"`; write bounds 1..=15 and sets the one live level | ALIGNED. Delta: only the current level is stored (`printk.rs:184`); default/min/boot-delay are constant 4/1/7. |
| M6 | `/proc/sys/kernel/printk_devkmsg` | ratelimit/policy knob for userspace kmsg writes | ALIGNED | `kernel/src/fs/procfs/sysctl.rs:450-465` | ALIGNED |
| M7 | `/proc/sys/kernel/dmesg_restrict` | gate unprivileged syslog/kmsg reads | absent | no `dmesg_restrict` in `sysctl.rs` (grep); `syslog` permission check at `printk.rs:804` hardcodes the default-open policy | **MISSING P1** — one atomic + the permission check branch; trivial. |
| M8 | `log_buf_len=` boot param | ring size boot knob | absent | capacity is a compile-time constant (`RING_BUFFER_CAPACITY`, `printk.rs`); no cmdline consumer | **MISSING P3** |
| M9 | `klogd`-style persistent log + `/var/log/kmsg` delta flush | — (distro/userspace convention; kernel provides the ring) | RESHAPE | `printk.rs:2158-2330` — Rux implements a **kernel-thread klogd** writing an ext4 persistent log | RESHAPE: Linux keeps this in userspace (rsyslog/journald read `/proc/kmsg` and `/dev/kmsg`). Move the flush loop to a userspace consumer; the kernel only needs the ring. Low priority (P3) — functionality exists, ownership is wrong. |

### 1.2 Tracing framework

| # | Linux surface | Linux shape | Rux status | Rux evidence | Class |
|---|---|---|---|---|---|
| M10 | Tracepoints (`TRACE_EVENT`) | ~1,000+ events in ~90 groups under `available_events`; per-event enable + `tracing_on` two-level gate; per-CPU ring buffers; standard record text format | ALIGNED (framework) / RESHAPE (coverage) | framework `kernel/src/trace/mod.rs` — 109 events declared via the `tracepoints!` macro (each entry `EVENT_X / fmt / trace_x : "group", "name", id,`); two-level gate `mod.rs` header comment + `TraceEvent::armed` (one relaxed load) + `record()` checks `tracing_on`; record format byte-captured from a Linux host (`mod.rs` 25–58) | ALIGNED as a mechanism. Coverage deltas are itemized in 1.2.1–1.2.3 below. |
| M11 | tracefs layout `/sys/kernel/tracing` | `tracing_on, trace, trace_pipe, available_tracers, current_tracer, available_events, set_event, buffer_size_kb, snapshot, options/, events/<group>/{enable,filter}, events/<group>/<event>/{enable,filter,id,format}, set_event_pid, per_cpu/cpuN/...` | ALIGNED | `kernel/src/trace/files.rs:12-41` (the implemented layout, mode+format per file), `register()` at `:118`; streaming `trace_pipe` semantics `fs/tracefs.rs:184-188`; `set_event` grammar `files.rs:589-597`; `trace_event=` boot param `files.rs:557-585`; `per_cpu/cpuN/{trace,trace_pipe,stats,buffer_size_kb}` `files.rs:477-496` | ALIGNED. Deltas (each an easy P3 fill): no `tracing_cpumask`, no `saved_cmdlines`/`saved_tgids`, no `error_log`, no `trace_marker_raw`, no per-cpu `snapshot`/`snapshot_raw` (Rux's own comment notes this, `files.rs:478-481`), no `set_ftrace_pid`. The same instance is mounted at both `/sys/kernel/tracing` and `/sys/kernel/debug/tracing` (`fs/mount.rs:545-600`), matching the reference automount. |
| M12 | `trace_marker` | writes recorded whenever tracing is on, no event switch; rendered as `ftrace:tracing_mark_write` | ALIGNED | `kernel/src/trace/files.rs:439-447`; pseudo-event `trace/mod.rs:2760-2773` (`EVENT_TRACE_MARKER`, deliberately not in `EVENTS`) | ALIGNED. Delta: no raw (binary) variant. |
| M13 | snapshot buffer | `echo 1 > snapshot` swaps the live buffer with the saved copy | ALIGNED (partial) | `files.rs:456-470` — global saved buffer: "1" save+restart, "0" clear, read renders it | RESHAPE→ALIGNED: Linux is per-CPU (`per_cpu/cpuN/snapshot`, `snapshot_raw`); Rux has one global buffer. Acceptable; complete per-cpu views in P3. |
| M14 | Event filtering (`events/<g>/<e>/filter`) | predicate engine over event fields (`echo 'field==x' > filter`) | filter files exist, engine absent | `files.rs:66` writes the standard global-filter text; per-event `filter` files serve `"none"` | **MISSING P2** (engine) — files are ALIGNED placeholders. Fill: minimal `field cmp value` parser over the declared fields. |
| M15 | ftrace tracers (`function`, `function_graph`, irqsoff/preemptoff/wakeup/hwlat/...) | `current_tracer` selects a tracer; `trace` shows its output; `available_filter_functions` | absent | `files.rs:20-21` — `available_tracers`/`current_tracer` exist, both serve only `"nop"` (write of anything else = EINVAL) | **MISSING P2** (function + function_graph first). Requires dynamic-ftrace hook infrastructure (M16). Until then the two files honestly report nop-only — keep. |
| M16 | dynamic ftrace (`set_ftrace_filter`, `available_filter_functions`, `-pg`/`mcount`-style patching) | NOP-patched call sites filtered at runtime | absent | no references in tree (grep `set_ftrace_filter` = 0) | **MISSING P2** — prerequisite for M15. Big lift; batch after the delete/gating lanes. |
| M17 | kprobes / kretprobes | `/sys/kernel/tracing/kprobe_events` (`p:`/`r:` descriptors), `/sys/kernel/debug/kprobes/{list,blacklist}`; `perf probe` integration | absent | grep `kprobe` finds only unrelated identifiers (`mpkprobe`, `fuxstuck`-era names) | **MISSING P2** — `kprobe_events` with p/r + basic fetchargs is the highest-value fill after the filter engine; gives the hunt lanes a general instrument so bespoke probes stop being written per-hunt. |
| M18 | `set_event_pid` | per-pid event filtering | ALIGNED | `files.rs:139-173` (write `set_event_pids`); fixed 16-slot table `trace/mod.rs:2804-2806` | ALIGNED. Delta: fixed 16 slots vs the reference's growable list. |
| M19 | `buffer_size_kb` | global + per-cpu ring size, rounded to page order | ALIGNED | `files.rs` global node + per-cpu nodes (`files.rs:477-496`) | ALIGNED |

#### 1.2.1 Standard-group events present (Linux-named)

49 events in 16 standard groups. Truly Linux-named ones (40):

- `sched` 9/9-shape: `sched_switch, sched_waking, sched_wakeup, sched_wakeup_new,
  sched_process_fork, sched_process_exit, sched_process_free, sched_process_wait,
  sched_migrate_task` — all real upstream names. Missing upstream sched events:
  `sched_kthread_stop{,_ret}`, `sched_stat_{wait,sleep,iowait,blocked,runtime}`,
  `sched_pi_setprio`, `sched_move_numa` family, `sched_update_nr_running` (~11).
- `raw_syscalls` 2/2: `sys_enter`, `sys_exit` — complete.
- `exceptions` 2: `page_fault_user/kernel` — upstream x86 has exactly these;
  riscv64 upstream does not define them (arch delta, acceptable).
- `block` 2/~10: `block_rq_issue`, `block_rq_complete`. Missing:
  `block_rq_insert`, `block_rq_requeue`, `block_bio_queue/complete`,
  `block_split`, `block_plug/unplug`, `block_rq_remap`.
- `signal` 1/2: `signal_generate`; missing `signal_deliver`.
- `oom` 1/1: `mark_victim` — complete.
- `printk` 1/1: `console` — complete.
- `irq` 2/5: `irq_handler_entry/exit`; missing `softirq_raise/entry/exit`.
- `kmem` 4/12: `kmalloc`, `kfree`, `mm_page_alloc`, `mm_page_free`; missing
  `kmalloc_node`, `kmem_cache_alloc{,_node}`, `kmem_cache_free`,
  `mm_page_alloc_zone_locked`, `mm_page_free_batched`, `mm_page_pcpu_drain`,
  `rss_stat`.
- `vmscan` 1/~15: `mm_vmscan_lru_shrink_inactive`; missing kswapd and
  direct-reclaim lifecycle events.
- `ext4` 7/~90: `ext4_sync_file_enter/exit`, `ext4_da_write_begin/end`,
  `ext4_ext_map_blocks_enter/exit`, `ext4_free_blocks` — all real names;
  coverage is ~8% of the upstream ext4 set.
- `jbd2` 2/~10: `jbd2_start_commit`, `jbd2_end_commit`.
- `binder` 5: `binder_transaction{,_received,_alloc_buf,_buffer_release}`,
  `binder_wait_for_work` — Android-common names; ~half the binder set.
- `input` 1: `input_event` (the other three evdev_* are bespoke, see 1.2.2).

#### 1.2.2 Linux-shaped groups with bespoke event names (9 events)

These ride standard group names but are NOT upstream events:

- `net` 5: `lo_xmit, udp_deliver, udp_wake, sock_wait, pkt_drop`. Upstream
  shape: `net_dev_queue`, `net_dev_xmit`, `netif_receive_skb`, `napi_poll`,
  `sock_exceed_recv_limit`. **RESHAPE P3**: rename/replace to upstream names
  where an upstream event covers the same site.
- `timer` 1: `itimer_arm`. Upstream: `itimer_state`, `itimer_expire`
  (+ `timer_start/expire_entry/expire_exit`, `hrtimer_*`). **RESHAPE P2**
  (the whole timer family is worth a batch: hrtimer events are heavily used
  by latency analysis on Linux).
- `input` 3: `evdev_open/read/wake` (bespoke). Keep behind the upstream
  `input_event` or fold in; P3.

#### 1.2.3 Groups upstream has that Rux entirely lacks

`task` (newtask/rename), `module`, `initcall`, `rcu`, `tlb` (`tlb_flush`),
`lock` (lockdep events, gated on M24), `cgroup`, `percpu`, `writeback`,
`filemap`, `mmap`/`mmap_lock`, `power` (`cpu_idle`), `watchdog`, `sock`/`tcp`/`udp`,
`x86_fpu`, `bpf`, `sunrpc`. All **MISSING P3** except `task:task_newtask/rename`
and `tlb:tlb_flush` which are cheap and broadly useful (**P2**).

#### 1.2.4 Bespoke `rux:` group (60 events)

All 60 events are wired (each has a live emitter outside trace/ — verified by
grep over `EVENT_`/`trace_*` references). They gate the §2 instruments. When a
§2 instrument dies, its event dies with it. Standard-shaped exceptions that
should SURVIVE the cleanup and graduate to upstream-shaped groups:

- `rux:epoll_ctl_insert/remove`, `rux:epoll_wait_scan/exit`,
  `rux:epoll_delivery` — upstream has no epoll tracepoints; these are the
  epoll hot-path instruments. KEEP only if the IPC/latency gate needs them;
  otherwise DELETE with the epoll probe families (§2).
- `rux:hilog_top/census/kmsg` — OH log-fabric census (§2).
- `rux:futex_miss`, `rux:sigtrace`, `rux:schedtrace`, `rux:mmforensics`,
  `rux:sysring`, `rux:taskdump`, `rux:memwatch`, ... — one per §2 module.

### 1.3 kprobes-adjacent and perf

| # | Linux surface | Linux shape | Rux status | Rux evidence | Class |
|---|---|---|---|---|---|
| M20 | `perf_event_open(2)` + mmap ring | software/hardware/breakpoint/tracepoint types; sampling ring; ioctls; `perf_event_paranoid` | ALIGNED (software subset) | `kernel/src/perf/mod.rs:6-33` — `PERF_TYPE_SOFTWARE` CPU_CLOCK/TASK_CLOCK/PAGE_FAULTS/CONTEXT_SWITCHES only; sampling records in uapi field order; sideband COMM/FORK/EXIT/MMAP2/LOST; ioctls ENABLE/DISABLE/REFRESH/RESET/PERIOD/SET_OUTPUT/ID; tick-granular sampler (hard-IRQ `note_tick` + softirq emit); paranoid=2 default (`mod.rs:1825`) | RESHAPE (declared subset). The in-tree tick sampler IS the alignment vehicle; deltas: freq>HZ coalesces to tick rate (documented `mod.rs:25-28`), no PMU/hardware events, no breakpoint/tracepoint event types, no `PERF_FORMAT_GROUP`. P2: add PERF_TYPE_TRACEPOINT wiring to the 109 events — cheap and makes hiperf/`perf trace`-class flows useful. |
| M21 | perf sysctls | `perf_event_paranoid`, `perf_max_sample_rate`, `perf_cpu_time_max_percent`, `perf_event_mlock_kb` | ALIGNED | `kernel/src/fs/procfs/sysctl.rs:688-738` (all four, with writers) | ALIGNED |
| M22 | kallsyms | build-time symbol table; `/proc/kallsyms` `addr type name` lines; `kptr_restrict` masking; module symbols | ALIGNED (kernel) / MISSING (masking, module syms) | table `kernel/src/dfx/ksyms.rs` (build-time nm+addr2line, demangled v0 names, target-checked); `/proc/kallsyms` `kernel/src/fs/procfs/kallsyms.rs:29-40` (`{:016x} {type} {name}`); the file's own comment names the delta: no `kptr_restrict` — real addresses printed to everyone; module symbols absent (only kernel text is in the table) | RESHAPE P2: add `kptr_restrict` (see M25) and mask `/proc/kallsyms` + `%pK` accordingly; module symbols with the module loader (P3). |
| M23 | seekable kallsyms | lseek + read semantics over the file | ALIGNED | procfs dynamic files are seekable snapshot files (`fs/procfs/procfile.rs`); `/proc/kallsyms` rides the same node type | ALIGNED |

### 1.4 debugfs / sysrq

| # | Linux surface | Linux shape | Rux status | Rux evidence | Class |
|---|---|---|---|---|---|
| M24 | debugfs | `/sys/kernel/debug`, kernel-owned tree, bool/seq files, no user create/rename | ALIGNED (filesystem) | `kernel/src/fs/debugfs.rs:8-27` (the conventions copied from Linux incl. `debugfs_create_bool` Y/N), `create_dir`/`create_file` at `:104/:146`; mounted per `fs/mount.rs:545-569` | ALIGNED. Contents are the issue: today it hosts `/dfx` knobs (§2) + the `/tracing` alias. Target content after cleanup: kprobes list, tracing alias, subsystem knobs — Linux-shaped only. |
| M25 | `kptr_restrict` | `%pK` + file-level address masking | absent | named as absent in `fs/procfs/kallsyms.rs:26-29`; no sysctl | **MISSING P2** |
| M26 | sysrq command table | `/proc/sysrq-trigger` (first char or `_`-sequence), keyboard path, `/proc/sys/kernel/sysrq` mask, upstream key set `b c d e f g h i j k l m n o p q r s t u v w x y z 0-9` | RESHAPE | `kernel/src/fs/procfs/sysrq.rs:29-133` — trigger file only; handles `b,c,d,e,f,i,k,l,m,n,o,p,q,r,s,t,u,v,w`, `x/y/z` explicit arch note, `0-9` loglevel, everything else prints help; `k,q,r,u,v` print explicit "not supported"; `t` = `dfx::taskdump::dump_all_tasks("sysrq")` (`:119`), `m` = meminfo + `dfx::memwatch::dump_mem()` (`:107`), `w` = blocked tasks, `l` = per-cpu stacks | RESHAPE P2. Deltas: (1) no `/proc/sys/kernel/sysrq` mask sysctl — add; (2) no keyboard SysRq path (evdev wired nothing — grep confirms) — P3, needs the console/keyboard combo handling; (3) `g` (kgdb) absent — kgdb itself is MISSING P3; (4) `j` (thaw FIFREEZE) not in the table — falls to help; add when freeze/thaw lands. The on-demand trigger table itself is upstream-faithful including the "explicit not supported" honesty lines. |
| M27 | LKDTM-style oops provoquer | debugfs write that deliberately faults | ALIGNED (as LKDTM class) | `kernel/src/dfx/mod.rs:274-312` — `/sys/kernel/debug/dfx/provoke-oops` (pagefault/illegal) | ALIGNED — but the file lives under `/dfx/`; move to a Linux-shaped home (e.g. LKDTM-style `/sys/kernel/debug/lkdtm/`) in the gating sweep. |

### 1.5 procfs debug surfaces

| # | Linux surface | Rux status | Rux evidence | Class |
|---|---|---|---|---|
| M28 | `/proc/meminfo` | ALIGNED | `fs/procfs/meminfo.rs` | ALIGNED |
| M29 | `/proc/vmstat` | ALIGNED (subset of keys) | counters `kernel/src/mm/vmstat.rs`; render `fs/procfs/vmstat.rs` | ALIGNED; fill remaining standard keys opportunistically (P3). |
| M30 | `/proc/buddyinfo` | ALIGNED | `fs/procfs/mod.rs:638`, generator `fs/procfs/vmstat.rs::generate_buddyinfo` | ALIGNED |
| M31 | `/proc/zoneinfo` | absent | not registered in `procfs/mod.rs` root init (grep) | **MISSING P2** — zones exist (`mm/zone.rs`); render per-zone free/min/low/high + watermark boost. |
| M32 | `/proc/pagetypeinfo` | absent | not registered | **MISSING P3** |
| M33 | `/proc/slabinfo` | RESHAPE (stub) | `fs/procfs/mod.rs:646, 2891-2896` — v2.1 header, zero cache rows ("Rux's allocators are not SLAB") | RESHAPE P2: once small_alloc/slab expose class stats, render real rows. Until then the stub is honest and node-exists matters for OH init chmod — keep the node. |
| M34 | `/proc/vmallocinfo` | RESHAPE (empty stub) | `fs/procfs/mod.rs:647, 2897-2901` | RESHAPE P3: track vmalloc ranges or drop the file (OH init chmod only needs node-exists; an empty file is acceptable indefinitely). |
| M35 | `/proc/interrupts` | ALIGNED | `fs/procfs/interrupts.rs` | ALIGNED |
| M36 | `/proc/softirqs` | ALIGNED | `fs/procfs/softirqs.rs` | ALIGNED |
| M37 | `/proc/schedstat` + `/proc/[pid]/schedstat` + `/proc/sys/kernel/sched_schedstats` | ALIGNED | `fs/procfs/schedstat.rs`, `sched/stats.rs:14-46` (gate default OFF like the reference; accounting edges in `__schedule`/enqueue), sysctl `procfs/mod.rs:872-877` | ALIGNED — this is the model row for the whole matrix: right files, right default, static-key-class gate. |
| M38 | `/proc/timer_list` | ALIGNED | `fs/procfs/timer_list.rs` | ALIGNED |
| M39 | `/proc/locks` | ALIGNED | `fs/procfs/locks.rs` | ALIGNED |
| M40 | `/proc/[pid]/stat` | ALIGNED | `fs/procfs/pid.rs` `generate_stat` | ALIGNED. Delta: field 35 `wchan` is hardcoded `"0"` (`pid.rs:698`). |
| M41 | `/proc/[pid]/status` | ALIGNED | `fs/procfs/pid.rs` `generate_status` | ALIGNED |
| M42 | `/proc/[pid]/stack` | absent | no such PidFileKind (`procfs/mod.rs:1507-1521` full list) | **MISSING P1** — the kernel stack walker exists (`dfx/backtrace.rs`, used by oops/hung_task); render `generate_stack(pid)` from the same walk. |
| M43 | `/proc/[pid]/wchan` | absent as a file; fake in stat | `pid.rs:698` | **MISSING P1** — one symbol lookup (`dfx/ksyms`) of the task's blocked-in function; also fixes M40's field 35. |
| M44 | `/proc/[pid]/syscall` | absent | not in PidFileKind list | **MISSING P2** — register set is available (ptrace GETREGSET); render `nr arg0..5 sp pc`. |
| M45 | `/proc/[pid]/{maps,smaps,environ,comm,cmdline,oom_score,oom_score_adj,cgroup}` | ALIGNED | `procfs/mod.rs:1507-1521` (all present with generators) | ALIGNED |

### 1.6 Detectors and dumpers

| # | Linux surface | Linux shape | Rux status | Rux evidence | Class |
|---|---|---|---|---|---|
| M46 | hung task detector (`khungtaskd`) | kthread scans D-state tasks; `sysctl kernel.hung_task_timeout_secs` (default 120), `hung_task_warnings`, `hung_task_panic`; reports pid + stack | RESHAPE (built, NOT STARTED) | `kernel/src/dfx/hung_task.rs` (khungtaskd kthread, 120 s default `:19`, pid-hash scan) — but `init()` is **commented out** at `kernel/src/dfx/mod.rs:186-188` ("requires kthread infrastructure... enable after testing"). No hung_task sysctls exist in `sysctl.rs`. | **RESHAPE P1** — this is the single cheapest big win: uncomment init, verify 1096/1096 + boots, add the three sysctls. Fold §2's `slowtask` into it. |
| M47 | soft lockup detector | per-CPU timestamp touched from `scheduler_tick`; `kernel.watchdog`, `watchdog_thresh`, `softlockup_panic` sysctls | RESHAPE (no knobs) | `kernel/src/dfx/softlockup.rs:18` (10 s threshold), touch from scheduler_tick, enabled unconditionally at `dfx::init()` (`softlockup.rs:55-63`) | RESHAPE P1: add `kernel.watchdog` (0/1 master) + `watchdog_thresh`; today there is no way to turn it off, which is itself a Linux divergence (upstream defaults it ON but knobbed). |
| M48 | hard lockup detector (NMI/PMU based) | perf-based NMI watchdog | absent | no NMI class infra (riscv64 has none; x86 could use PMU later) | **MISSING P3** (roadmap row; no plan — needs PMU work from M20). |
| M49 | lockdep | CONFIG_LOCKDEP; `/proc/lockdep{,_stats,_chains}`; `lock:` events | absent | grep `lockdep` = one unrelated rwsem comment | **MISSING P3** (roadmap row only, per mission; large compile-time subsystem). |
| M50 | lock stat | CONFIG_LOCK_STAT; `/proc/lock_stat` | absent | grep = 0 | **MISSING P3** (roadmap row). |
| M51 | schedstats | see M37 | — | — | ALIGNED (M37). |
| M52 | PSI | `/proc/pressure/{cpu,memory,io}` (avg10/avg60/avg300 + total), `kernel.psi=` | absent | grep PSI = 0 | **MISSING P2** — stall tracking in the wait paths; the waitqueue changes are small but the accounting discipline must follow the merge-gate rules (measured, gated off by default like upstream `CONFIG_PSI=n`). |
| M53 | SLUB debug + cache accounting | `slub_debug=` boot param, per-cache `/sys/kernel/slab/<cache>/`, real `/proc/slabinfo` rows | absent | grep `slub_debug` = 0; M33 stub | **MISSING P2** for accounting (feeds M33), P3 for `slub_debug` poison/red-zone semantics (roadmap row). |
| M54 | KASAN / UBSAN / KCSAN | compile-time sanitizer instrumentation | absent (Rux has its own audited-unsafe regime instead — `dfx/unsafe.rs` + INV families) | grep sanitizer = 0 | **MISSING P3 — roadmap rows only** (per mission, no implementation plan). |
| M55 | WARN/BUG family | `WARN_ON`, `WARN_ONCE`, `BUG_ON` with cut-here reports + taint | ALIGNED | `kernel/src/dfx/bug.rs` + macros `dfx/mod.rs:331-381` (`warn_on!`, `warn_on_once!`, `bug_on!`, `bug!`) | ALIGNED |
| M56 | kernel taint | `Tainted:` line + `/proc/sys/kernel/tainted` | ALIGNED | `kernel/src/dfx/taint.rs`; sysctl `fs/procfs/sysctl.rs:145` | ALIGNED |
| M57 | Oops | fault report (regs + stack + `Tainted`), `panic_on_oops` policy, task-kills-SIGSEGV, `TAINT_DIE` | ALIGNED | `kernel/src/dfx/oops.rs` (Linux 5.15-shaped report lines named per line in comments; `oops=panic` boot param `:init_from_cmdline`; sysctl `sysctl.rs:154-175`; pstore capture) | ALIGNED |
| M58 | pstore/ramoops | reserved RAM region; `/sys/fs/pstore/console-ramoops-0`, `dmesg-ramoops-0` (0444); backend params `ramoops.mem_address/mem_size/record_size`; `pmsg`; reboot reason | ALIGNED (core) | `kernel/src/dfx/pstore.rs` (128 KiB DTB-anchored carve, `memblock_reserve`, panic capture staging sink, magic-last write, `early_scan` re-expose); `/sys/fs/pstore` files via `fs/sysfs.rs:1507-1514` (0444); oops flavor `dfx/oops.rs` | RESHAPE P3: no backend module-params (the carve is DTB-anchored instead — a deliberate divergence worth a design-divergences entry), no `pmsg`, no `reboot_reason`, records die under `-initrd` boots (documented in `pstore.rs`). |
| M59 | kdump / crash dump | `crashkernel=` reserve + kexec-crash + `/proc/vmcore` | absent | grep kdump/crash_dump = 0 | **MISSING P3 — roadmap row** (kexec is a prerequisite subsystem of its own). |
| M60 | dynamic debug (dyndbg) | `/proc/dynamic_debug/control`, `pr_debug` sites toggleable | absent | grep dyndbg = 0; kernel prints are `pr_info!/pr_warn!` (`printk.rs`) with no site table | **MISSING P2** — pairs with the gating sweep: after M7/M25 land, a site table turns every remaining `pr_debug`-class print into a controlled surface instead of an ad-hoc family. |
| M61 | watchdog infra (`/dev/watchdog` + core) | char dev with WDIOC ioctls; `nowayout`; pretimeout | ALIGNED (subset) | `kernel/src/drivers/watchdog.rs:8-14` (WDIOC_SETTIMEOUT/GETTIMEOUT/SETPRETIMEOUT/GETPRETIMEOUT/KEEPALIVE, built for OH `watchdog_service`) | ALIGNED for the consumer in the image. The dfx spinlock/deadlock watchdog (`dfx=watchdog` retired token → `rux:watchdog`/`rux:taskdump` events) is §2/exception-class material. |
| M62 | `panic` policy | `panic=`, `oops=panic`, `kernel.panic` sysctl, reboot timeout | ALIGNED | `panic_policy.rs`; `oops=panic` (`dfx/oops.rs`); sysctl `panic_on_oops` (`sysctl.rs:154-175`) | ALIGNED |
| M63 | ptrace debug verbs | TRACEME/ATTACH/PEEK*/POKE*/GETREGSET/SETREGSET/CONT/SINGLESTEP/SETOPTIONS/GETSIGINFO | ALIGNED (subset, documented) | `kernel/src/process/ptrace.rs:8-23` — explicitly out of scope: `PTRACE_SYSCALL`, hardware watchpoints, SEIZE/LISTEN/INTERRUPT | RESHAPE P3: `PTRACE_SYSCALL` is the strace-critical gap; add with the syscall-entry trace hooks. |
| M64 | seccomp | modes 0/1/2, filter inheritance | exists (relevant infra, not debug output) | `kernel/src/process/fork.rs:1041-1045` (mode + filter inherited across fork) | ALIGNED (not in scope further) |
| M65 | `/proc/sys/kernel/core_pattern` | crash dump command template | ALIGNED | `fs/procfs/sysctl.rs:567-573` | ALIGNED |

### 1.7 Merge-gate measurement tooling (EXCEPTION class — keep, gate, reshape over time)

These guard named merge-gate domains (memory alloc/reclaim, scheduling, fs hot
paths, IRQ paths, IPC, binder) or ARE the perf-gate vehicles. Per the directive
they are NOT delete rows; they must be release-gated (§3) and gradually
re-expressed on the trace framework. Matrix entries:

| Instrument | Domain | Evidence | Class |
|---|---|---|---|
| `dfx/syscost.rs` (2,309 L, 53 sites) | all domains — the Tier-1 vehicle | `dfx/syscost.rs:8-13` (`syscost=1`, "costs nothing when off") | ALIGNED-as-instrument; keep; long-term: per-exit counters belong in tracepoint form (`raw_syscalls` + histogram), keep the census probes |
| `dfx/fsw.rs` (754 L, 83 sites) | fs hot paths (ext4/bio) | `dfx/fsw.rs:6-11` (`fsw=1`); sysctl-style counters | RESHAPE: replace counters with `ext4`/`jbd2`/`block` tracepoints over time; until then it stays the pbench fs-gate counter source |
| `dfx/filerd.rs` (156 L, 19 sites) | fs hot paths (read(2) anatomy) | `dfx/mod.rs:92-95` (`rux:filerd`) | RESHAPE → `ext4` tracepoints; keep until parity |
| `dfx/taskdump.rs` (1,742 L, 200 sites) | ALL — sysrq-t/show-state equivalent; also the line-funnel for gated prints | `dfx/taskdump.rs` header; sysrq `t` caller `fs/procfs/sysrq.rs:119` | ALIGNED to sysrq-t. The `IN-EV`-style per-file print riders (evdev §2-B6) must move to real events |
| `dfx/cctx.rs` (281 L, 71 sites) | scheduling + IPC latency rows (lat_sem/lat_unix/lat_ctx/lat_pipe) | `dfx/cctx.rs:6-30` (`cctx=1`, "zero traffic when off, one Relaxed load per stamp") | RESHAPE: sched_switch/sched_waking events already carry much of this; fold brackets into tracepoints as the ring gains ns-precision |
| `dfx/forkprof.rs` (388 L, 83 sites) | perf-gate P4 fork/exec rows | `dfx/mod.rs:149-152` (`forkprof=1`) | RESHAPE → `sched_process_fork`-family events with duration fields |
| `dfx/mmprof.rs` (404 L, 57 sites) | OH-launcher mmap/mprotect perf | `dfx/mod.rs:153-156` (`mmprof=1`) | RESHAPE → future `mmap`/`mm` events |
| `dfx/allocwatch.rs` (465 L, 24 sites) | memory: small_alloc class-lock guardianship (named guarded domain) | `dfx/mod.rs:30-34` | KEEP (guard) |
| `dfx/memwatch.rs` (1,126 L, 51 sites) | memory: heap-leak accounting + sysrq-m dump | `dfx/memwatch.rs:6-12`; sysrq `m` caller `sysrq.rs:107` | KEEP (guard); reshape dump toward `/proc`-style sources |
| `dfx/ledger.rs` (481 L, 5 sites) | memory: fake-OOM census reconciliation (goal-1) | `dfx/mod.rs:66-69` | KEEP (goal-1 active) |
| `dfx/mmforensics.rs` (426 L, 18 sites) | memory: page-table/frame forensics (goal-1 fake-OOM family) | `dfx/mod.rs:28`; call sites in mm/page_alloc, pcp, page_desc, zone, page_cache | KEEP (goal-1 active); long-term align to upstream `page_owner` shape |
| `dfx/driftwatch.rs` (171 L, 25 sites) | memory: zero-put/free-refusal attribution (goal-1 family) | `dfx/mod.rs:70-72` | KEEP (goal-1 active) |
| `dfx/metawatch.rs` (167 L, 1 site) | memory: buddy free-list invariant audit | `dfx/mod.rs:85-88` (`rux:metawatch` gate at `timer.rs:1162-1163`) | KEEP (guard, event-gated) |
| `dfx/leakorigin.rs` (1,194 L, 26 sites) | scheduling: preempt-offset leak attribution | `dfx/mod.rs:124-129` | DELETE-candidate after the leaked-preempt hunt closes (see §2 — kept only if the hunt reopens) |
| `dfx/r83hunt.rs` (677 L, 8 sites) | scheduling: task-header audit (R83 hunt) | `dfx/mod.rs:75-76` | DELETE (hunt closed; §2) |
| `dfx/slowtask.rs` (465 L, 2 sites) | scheduling: 8 s-stuck syscall audit | `dfx/mod.rs:140-144` | RESHAPE→fold into hung_task (M46); delete the bespoke body |
| `dfx/reaper.rs` (481 L, 17 sites) | scheduling: reap/exit lifecycle audit | `dfx/mod.rs:165-169` | RESHAPE→`sched_process_{fork,exit,free}` events already exist — replace prints with events; delete module |
| `dfx/lostpark.rs` (95 L, 9 sites) | scheduling: lost-wake event ring | `dfx/mod.rs:157-160` (`rux:lostpark`) | KEEP (guard, event-gated) |
| `dfx/treehunt.rs` (235 L, 1 site) | scheduling: parent-pointer invariant validator | `dfx/mod.rs:135-139`; timer tick `timer.rs:572` | RESHAPE→`debug_assert!` invariant + delete periodic scanner (§2) |
| `sched/wakewatch.rs` (560+ L, wake/pick sites in sched.rs, timer.rs) | scheduling: wake→pick latency forensics | `sched/wakewatch.rs:5-30`; `sched.rs:3357,5362,5933`; `timer.rs:1858-1886` | KEEP (guard: WAKE-REFUSED census is a Tier-1 band); reshape prints → `sched_waking`/`sched_switch` events |
| `sched/sched_ev.rs` (GRQ one-word ring, 44 sites) | scheduling: GRQ no-print rule's evidence ring | `sched/sched_ev.rs:5-15` | KEEP (guard — the GRQ critical-section rule depends on it) |
| `sched/rt.rs`+`sched.rs` rt2xdbg prints (15 sites) | scheduling (timer-gap lane) | §2-B2 | DELETE |
| `dfx/pathstash.rs` (126 L, 19 sites) | fs: last-open stash for TXN-HANG | `dfx/mod.rs:130-134` | DELETE (hunt closed; §2) |
| `dfx/scmtap.rs` (117 L, 20 sites) | IPC: SCM_RIGHTS relay tap | `dfx/mod.rs:145-148` | DELETE (hunt closed; §2) |
| `dfx/unixrd.rs` (242 L, 66 sites) | IPC: AF_UNIX recv phase anatomy (24 µs writev class guardianship) | `dfx/mod.rs:109-113` (`unixrd=1`) | KEEP (guard); reshape: the writev path is the named IPC band — keep gated anatomy until a unix tracepoint family exists |
| `dfx/spinwatch.rs` (148 L, 5 sites) | IPC: epoll zero-event census | `dfx/mod.rs:119-123` | DELETE (hunt closed; §2) |
| `dfx/hilogcensus.rs` (558 L, 6 sites) | OH log-fabric census (not a merge-gate domain) | `dfx/mod.rs:114-118` | DELETE (§2) — OH campaign rigs keep it via git history |
| binder census + `binderprobe` prints (`ipc/binder.rs:1312-1319`, ~5 gated sites + census stamps) | binder: SLOW-TXN guardianship | `binder.rs:1312-1325` | KEEP the census (guard: rux:bindertrace event), DELETE the print family (§2-B4) |

---

## §2 Delete list

Every bespoke instrument and ad-hoc debug print with NO Linux counterpart.
Judged individually; call-site counts are greps of the 830da5bed tree
(`crate::dfx::<mod>::` outside the module's own file). "Replacement" is the
Linux-shaped surface that carries the need after deletion.

### 2.0 Deletion status (executed 2026-11-28, branch dbg-delete)

All twelve S2.1 modules and the S2.2 print families below are DELETED
(commits on branch `dbg-delete`, base 4efd954b3):

| Module / family | Status |
|---|---|
| dfx/scribble.rs | DELETED @ 434244c7a |
| dfx/wildwatch.rs | DELETED @ 07e32cbd0 |
| dfx/slowtask.rs | DELETED @ 217284fe2 |
| dfx/leakorigin.rs | DELETED @ 1a9a08d51 |
| dfx/r83hunt.rs | DELETED @ b82eeed6c |
| dfx/treehunt.rs | DELETED @ 9d7757322 |
| dfx/schb.rs | DELETED @ 87c268ff7 (drct= pace counters reshaped onto syscost — merge-gate memory row keeps its instrument) |
| dfx/scmtap.rs | DELETED @ 4923d9151 |
| dfx/spinwatch.rs | DELETED @ 9914af33b |
| dfx/hilogcensus.rs | DELETED @ b19519ed4 |
| dfx/pathstash.rs | DELETED @ 6b19289bd |
| dfx/reaper.rs | DELETED @ 7b004db26 |
| rt2xdbg prints | DELETED @ be5221250 |
| cycdbg prints | DELETED @ 717fd4ce1 |
| fuxwatch/rwfrzwatch engines | DELETED @ d80e3d899 (fuxstuck census family kept) |
| OH-hunt prints + sprobe master gate | DELETED @ 5d50dd39f |
| binderprobe print family | DELETED @ bc2240d86 (census stamps kept) |
| evdev IN-EV/IN-OPEN/echo prints | DELETED @ 37e4ed796 (input:* tracepoints are the surface) |
| kpreempt | KEPT (conflict vs protection rules: it is the R1 RT kernel-preemption hook — preempt_schedule_irq-shaped behavior named in perf-gate A/B rows — not a print family; the doc row described a pre-verdict-cache shape) |
| grqwedge cmdline twin | ALREADY CONSOLIDATED (the whole rig is behind the compile-time dfx-grq-wedge feature; nothing left to delete) |
| faaround / mpkprobe / mpkguard / negmemo / cov_dump | UNTOUCHED (behavior toggles / compile-time-gated; flagged to their owning lanes per the rows) |
| scribblepin / censusat= | died with scribble / schb |
| dfx= switch table | shrunk to the two kept fault injectors (claiminject, oncpuinject); dfx= parsing stays for them |

Kept exception-class instruments are untouched: syscost, fsw, filerd,
taskdump, cctx, forkprof, mmprof, allocwatch, memwatch, ledger,
mmforensics, driftwatch, metawatch, lostpark, unixrd, wakewatch,
sched_ev, oops, pstore, ksyms, backtrace, bug, taint, hung_task,
softlockup, monowatch, and trace/ entirely.

### 2.1 Whole-module deletions (kernel/src/dfx/)

| Module | L / sites | Original purpose (hunt/campaign) | Deletion risk | Replacement | Verdict |
|---|---|---|---|---|---|
| `dfx/scribble.rs` | 1,644 / 48 | scribble-hunt: task-header corruption audit + gdb-park actions (park on pid1/64th task for QEMU watchpoints) — the quiet-statics wildwrite line | 48 sites across task registration/pick paths; `switches::set` cross-links the 3 park switches (`switches.rs:257-283`); x86_64-only so riscv64 builds unaffected | None direct. The park-ACTIONS are a kgdb/kdb-class capability: keep the three gdb-park switches ONLY if the wildwrite line reopens; otherwise delete module + `Scribble*` switches + `debugfs` knob rows. The detection engine's prints were already event-gated (`rux:scribble`). | **DELETE** (condition: wildwrite hunt owner sign-off) |
| `dfx/r83hunt.rs` | 677 / 8 | R74/R83 producer hunt: live task-header audit + victim dumps (E8-REPAR family) | 8 call sites; `rux:r83scan` event dies with it | `sched_ev` ring already captures task-lifecycle events; oops/taskdump cover victim dumps | **DELETE** |
| `dfx/treehunt.rs` | 235 / 1 | leg-29A: live task whose parent points at released memory (sc304 signal-Arc Oops) | single tick call `timer.rs:572` + `rux:treehunt` event | Convert to `debug_assert!` in the parent-clearing path (the invariant survives as a cheap check, the periodic scanner dies) | **DELETE** (after invariant lands) |
| `dfx/wildwatch.rs` | 332 / 3 | wildwrite-hunt: RO-guard PTE tripwire on hpet quiet-statics + canary checksum | `dfx/mod.rs:184` calls `wildwatch::arm()` UNCONDITIONALLY on every x86_64 boot (PTE write-protects + CR0.WP) — a real release boot cost today; timer checksum gated by the switch (`timer.rs:1152`) | None (guard-page technique). If the wildwrite hunt is closed: delete module, arm() call, `WildWatch` switch, debugfs knob. If it stays: gate `arm()` behind the switch so release boots pay nothing | **DELETE or arm-gate** (decision with the wildwrite owner; either way `mod.rs:184` must stop being unconditional) |
| `dfx/leakorigin.rs` | 1,194 / 26 | preempt-offset leak hunt: outermost acquire call-site attribution | 26 sites in sched/entry paths; `rux:leakhunt`/`rux:acqdiv` events die with it | `sched_switch`/`sched_waking` events + the preempt_count debug class (`kpreempt` prints, §2-B2) cover re-diagnosis | **DELETE** (condition: leaked-preempt hunt owner sign-off) |
| `dfx/slowtask.rs` | 465 / 2 | leg-33B: tasks stuck ≥8 s on one syscall entry (nested-call cascade leaf naming) | 2 sites (syscall entry + timer) | hung_task (M46) once started — extend it with a RUNNING-state variant or a `sched:sched_stat_blocked`-style report; delete the bespoke body | **DELETE** (after M46 lands + one parity boot) |
| `dfx/pathstash.rs` | 126 / 19 | leg-28 TXN-HANG: name the file a D-state thread is stuck opening | 19 sys_openat-path sites (hot path stashes on EVERY open) | hung_task report can walk the task's fd/path state on demand; no permanent stash needed | **DELETE** |
| `dfx/reaper.rs` | 481 / 17 | reaper-waitid lane: SPAWN/ZEXIT/REAP4/ZLINGER audit of OH app-cycle children | 17 sites in fork/exit paths | `sched_process_{fork,exit,free,wait}` events (all exist, `trace/mod.rs:458-695`) — the prints ARE those events in worse shape | **DELETE** (convert the two still-referenced reports to event consumers first) |
| `dfx/schb.rs` | 643 / 8 | sc-series quiet-quiet hunt: soft-wedge heartbeat + one-shot wedge census (`censusat=`) | 8 sites; kthread + timer hook | taskdump periodic dump (`rux:taskdump`) + hung_task cover the wedge census | **DELETE** |
| `dfx/scmtap.rs` | 117 / 20 | gdm session-fifo hunt: per unix-fd hop/drop lines | 20 sites IN the unix SCM hot path (every fd transfer pays the armed-check) | `binder_transaction`-style unix fd event doesn't exist upstream — if the IPC gate ever needs it, it becomes a tracepoint; until then delete | **DELETE** |
| `dfx/spinwatch.rs` | 148 / 5 | hilog-flood: zero-event epoll-return census | 5 epoll-path sites | `rux:epoll_wait_exit` event carries the zero-return count; the interest-set dump dies | **DELETE** |
| `dfx/hilogcensus.rs` | 558 / 6 | hilog-flood: per-caller unix DGRAM + /dev/kmsg write census | hooked into the devkmsg write path (`printk.rs:1756`) | None (OH-campaign-specific); OH rigs re-arm from git history if the flood returns | **DELETE** |

Kept (exception class — see §1.7 for matrix rows): `syscost, fsw, filerd,
taskdump, cctx, forkprof, mmprof, allocwatch, memwatch, ledger, mmforensics,
driftwatch, metawatch, lostpark, unixrd, oops, pstore, ksyms, backtrace, bug,
taint, hung_task, softlockup`, `sched/wakewatch.rs`, `sched/sched_ev.rs`,
`mm/unwatch.rs` (hp89 PTE-clearing attribution ring — goal-1 family, memory
domain; keep while the fake-OOM/mallocng lines are active).

### 2.2 Scattered ad-hoc print families outside dfx/

| Family | Locations (file:line, count) | Purpose | Risk | Replacement | Verdict |
|---|---|---|---|---|---|
| `rt2xdbg` prints | `sched/sched.rs:414,521,543,2288,4311`, `sched/rt.rs:592`, `sync/futex.rs:6746,7119`, `process/task.rs:2447`, `arch/unsafe.rs:12422` (15 sites) | timer-gap lane: pinned-RT wedge-state, throttle lift/set, futex WAKE walk dumps | **Each site calls `cmdline::get_param` at runtime — full cmdline walk + heap `String` allocation per call EVEN WHEN OFF** (see §3-G1) | `sched_switch`/`sched_waking` + wakewatch census cover the wedge shapes | **DELETE** |
| `cycdbg` prints | `timer.rs:1204` (+ ring) | cyclictest tail fire-lateness attribution | cmdline walk on the hrtimer fire path | pbench P-cases + `timer` events (M-timer family fill) | **DELETE** |
| OH-hunt prints (`sprobe` family: mtprobe/bindwalk/bindrec/sbxbind/devopen-probe/mntslow/appclone/drm ioctl census) | `syscall/file.rs` (22), `fs/vfs.rs` (7), `drivers/drm/mod.rs` (6), `syscall/process.rs` (5), `syscall/memory.rs` (3), `sched/wakewatch.rs` (3), `trace/mod.rs` (4), `net/netlink.rs` (2), `process/task.rs` (2), `syscall/{dispatch,io,misc}.rs`, `fs/{devfs,procfs}/mod.rs` (~60 sites total) | OH boot hunts: mount/open/bind ioctl census prints; ~1.7k lines/boot at LOGLEVEL=4 | master gate `oh_hunt_prints_on()` (`dfx/switches.rs:214-217`) — now default-off behind `rux:ohhunt`; the PRINTS themselves have no Linux counterpart | `raw_syscalls`, `block`, ext4/net events already cover the observation needs | **DELETE** (the prints; keep the master-gate fn only until the last family dies, then delete it too) |
| `binderprobe` print family (SAREG/SACSA/SLRD/SLTX/btx/payload-head) | `ipc/binder.rs:1319` (gate fn) + gated sites `:9478,10896,11435,11964,13870` | binder SA-chain forensics prints (~7k lines/boot when armed; measured 3 ms sender-window inflation) | second-level gate behind `rux:bindertrace`; hot-path short-circuit is correct | `rux:bindertrace` census (QPICK/BRTT stamps) + upstream `binder_transaction` family | **DELETE** the print family; KEEP the census stamps (binder guard) |
| evdev `IN-EV`/`IN-OPEN`/itt prints | `drivers/input/evdev.rs:292-302` (push_event), `:825-838` (IN-OPEN + itt funnel), `:1760-1806` (itt_msg/line_atomic_write helpers) | inputtrace: raw arrival proof via taskdump funnel | gated by `rux:inputtrace` (correct), but the funnel bypasses the ring buffer — raw UART lines outside tracefs | `rux:evdev_read/open/wake` events already exist — emit those instead | **DELETE** prints, convert call sites to the existing events |
| `wakewatch` console prints (default-ON under census) | `sched/wakewatch.rs:287` (`print_budget_ok() && probe_prints_on()`), `:513-515` (sprobe default TRUE), `ww_msg` → `putc_brief` | wake→pick outlier lines (~87 µs/byte UART charged to the picked task) | prints fire whenever `rux:wwcensus` is armed unless `sprobe=0`; rigs that arm the census get UART storms | same data via `sched_waking`/`sched_switch` records in the ring | **RESHAPE** (print paths → ring records; keep the counters) |
| `fuxwatch` / `rwfrzwatch` futex word watchpoints | `sync/futex.rs:1486-1495` (rwfrzwatch arm), `fuxwatch_parse` (`dfx/switches.rs:296-297`), watch engine `futex.rs:6742+` | rwlock-livelock + fuxstuck hunts: watch one word's full wait/wake lifecycle | address-latched at boot; pi_min/timer-gap hunts closed | `rux:fuxstuck` event + `rux:futex_miss` | **DELETE** (after one confirming boot without them) |
| `kpreempt` trap prints | `arch/riscv64/trap.rs:164`, `arch/x86_64/trap.rs:787,796` | preempt-count audit at trap | cmdline walk per trap in the audit path; off-path cost is the walk | preempt-leak hunts closed (leakorigin §2.1) | **DELETE** |
| `faaround` toggle | `arch/unsafe.rs:9252,21538` | fault-around workaround behavior toggle (experiment, not observability) | behavior injector class | land the fixed behavior or make it a proper sysctl | **DELETE-or-promote** (decision with mm lane; not a debug instrument) |
| `grqwedge` injection | `sched/sched.rs:2753` | reproducible GRQ wedge injection rig | dup of the compile-time `dfx-grq-wedge` feature (kernel/Cargo.toml:60-66) | keep exactly one: the compile-time feature | **DELETE** the cmdline twin |
| `mpkprobe` / `mpkguard` | `kernel/src/lib.rs:382,388` (+ `arch/x86_64/mpk.rs`) | MPK capability probe + standing guard (safety-campaign experiments) | behavior/safety experiment, not observability | outside debug scope; keep until the MPK decision, then move out of the boot path | **NOTE** (not a delete-row; flag for the safety lane) |
| `negmemo` memo | `dfx/fsw.rs:270-291` + `fs/vfs.rs` consumer | negative-lookup memo performance prototype | behavior-changing experiment riding a dfx file | land as a proper dcache feature or delete | **NOTE** (fs lane decision) |
| `scribblepin` | `dfx/scribble.rs:354` | pin for gdb watch rigs | dies with scribble (§2.1) | — | **DELETE** (with scribble) |
| `censusat=` | `dfx/schb.rs:264-270` | schb census timer arm | dies with schb (§2.1) | — | **DELETE** (with schb) |
| `cov_dump` | `kernel/src/cov.rs` + sysctl `sysctl.rs:166-171` | coverage dump for `make coverage` | COMPILE-TIME gated (`cov-dump` feature, kernel/Cargo.toml:37-40) — correctly gated | keep | **KEEP** (compliant) |

### 2.3 The frozen `dfx=` switch table

`kernel/src/dfx/switches.rs` is already shrunk to SIX permanent switches
(`switches.rs:70-81`): `claiminject`, `oncpuinject` (scheduling fault
injectors — reproduce wedge shapes; no tracepoint form, documented
`switches.rs:38-45`), `scribblepark/scribblepid1/scribblesweep` (gdb-park
actions — die with §2.1 scribble), `wildwatch` (§2.1). The 43 retired tokens
are gone from the table (`switches.rs:15-33` ledger; unknown tokens warn
`switches.rs:311-316`).

Consumers of `switches::enabled` in production code: `timer.rs:893,910,1152`
(ClaimInject/OncpuInject/WildWatch), `dfx/wildwatch.rs:96`. That is the whole
list — the table is nearly empty already. **Execution**: delete the table +
`dfx=` parsing (`switches.rs:292-319`) once scribble/wildwatch resolve; keep
ClaimInject/OncpuInject ONLY if a naming home outside debug is wanted (they
are fault injectors — Linux's closest counterpart is KCSAN/KUnit fault
injection, i.e. a compile-time test subsystem; move there or delete).

Known boot-flag consumers to sweep with it (grep `get_param`): `dfx` itself,
`sprobe`, `syscost/fsw/cctx/unixrd/forkprof/mmprof/hilogcensus/spinwatch`
(exception-class keys — keep, they are the gated arming keys), `oops`,
`scribblepin`, `censusat`, `grqwedge`, `kpreempt`, `faaround`, `tscclk`,
`irqthread`, `mpkprobe`, `mpkguard`, `fuxwatch`, `fuxwatch2`, `rwfrzwatch`,
`negmemo`, `binderprobe`, `rt2xdbg`, `cycdbg`, `idlepoll`.

---

## §3 Release-gating compliance audit

**Mechanism today.** `Kernel.toml` is compiled into `kernel/src/config.rs` by
`kernel/build.rs` (no `[features]` debug section exists there — the feature
mechanism for debug code is Cargo features in `kernel/Cargo.toml:30-66`):
`dfx-lock-owner`, `dfx-taskdump-bt`, `dfx-futex-trace`,
`dfx-aa64-uaccess-selftest`, `dfx-grq-wedge`, `cov-dump`, plus the platform
features. Per the 2026-11-19 directive (release defaults all debug off; only
exemption = measured zero impact), the ONLY compile-time-gated debug is the six
features above; everything else is compiled into every release build and
gated at RUNTIME:

- trace events: `TraceEvent::armed()` — one relaxed load per site
  (`trace/mod.rs:117-121`), `tracing_on` checked inside `record()`;
- dfx instruments: internal `on()`/`armed()` relaxed-load checks
  (`syscost.rs:10-13` documents the contract);
- cmdline families: `cmdline::get_param` — **no cache**
  (`kernel/src/cmdline.rs:725-737`): every call walks the whole cmdline string
  and ALLOCATES a `String` for the value.

**Verdict.** Zero-traffic-when-off is largely achieved (one load per site —
measured-band evidence exists for the merge-gate instruments). COMPILED OUT in
release: almost nothing. Both are short of the directive's bar ("zero
release-performance impact"), and three families are actively mis-gated:

| # | Offender | file:line | Cost when release boots today | Required gate |
|---|---|---|---|---|
| G1 | `rt2xdbg` hot-path checks (15 sites) | `sched/sched.rs:414,521,543,2288,4311`, `sched/rt.rs:592`, `sync/futex.rs:6746,7119`, `process/task.rs:2447`, `arch/unsafe.rs:12422` | cmdline full walk + heap allocation PER EVENT on the futex-wake and scheduler paths even when off | **DELETE (§2-B2)**; any survivor must read a cached `AtomicBool` |
| G2 | `wildwatch::arm()` unconditional | `dfx/mod.rs:184` | PTE write-protects + CR0.WP mutation on EVERY x86_64 boot; canary checksum tick gated but arming is not | delete or arm behind the switch (§2.1) |
| G3 | `wakewatch` prints default-ON when census armed | `sched/wakewatch.rs:513-515` (`sprobe` default TRUE), `:287` | hunt rigs get 87 µs/byte UART lines; release is safe (census off) but the family violates print hygiene | prints → ring records (§2.2) |
| G4 | `scribble` compiled into every x86_64 build | `dfx/mod.rs:73-74` (`cfg(feature = "x86_64")` is the PLATFORM feature, not a debug feature) — 1,644 lines + 48 sites | code size + task-registration branch load; park actions reachable via debugfs knobs | compile-time feature `dfx-scribble` or delete (§2.1) |
| G5 | `get_param` no-cache walk | `kernel/src/cmdline.rs:725-737` — ~44 distinct cmdline keys consumed at runtime, several on hot paths (`binder_probe_on()` `ipc/binder.rs:1319` × 5 sites, `negmemo` `fsw.rs:278-291` on lookup paths, `sprobe` × 2) | per-call cmdline walk + String alloc | parse once at init into a static key table (atomic cached values); or compile-out per §2 deletions |
| G6 | hung_task dead code | `dfx/mod.rs:186-188` (init commented) | code present, never runs — alignment gap (M46), not a perf cost | start it (P1 fill) |
| G7 | Everything else (109 events, ~25 dfx instruments, exception-class probes) | trace sites e.g. `sched/sched.rs:4855` (armed + record), probes per §1.7 | one relaxed load per site when off — the measured-zero class the directive exempts | acceptable; document as the exemption with the syscost/pbench bands |

**Compliant today**: `cov-dump` (feature), `dfx-futex-trace` (feature, off),
`dfx-taskdump-bt` (feature, off), `dfx-lock-owner` (feature, off),
`dfx-grq-wedge` (feature, off), `dfx-aa64-uaccess-selftest` (feature),
`sched_schedstats` (default OFF, M37), PSI absent, all 109 events (armed
default false except the marker, `trace/mod.rs:2766-2773`), `tracing_on`
default OFF (`trace/mod.rs:2792`).

**Gating sweep (execution batch B-G)**, in order: (1) §2-B2 deletions remove
G1; (2) cache cmdline keys at init for every survivor (fixes G5 globally);
(3) `scribble` feature-gate (G4); (4) wildwatch arm-gate or delete (G2);
(5) audit new debug code in review against "compile-time feature or measured
exemption" — the merge-gate checklist gains one line.

---

## §4 Execution order

Constraint: the P0 ext4/jbd2 data-corruption fix for 0fbf51a63 lands FIRST;
batches B4, B6 and any ext4/jbd2-tracepoint fill rebase on top of it and must
not touch the same files until it merges.

| Batch | Scope (files) | Content | Depends on | Lane collisions to avoid |
|---|---|---|---|---|
| **B1** — cheap deletes | `dfx/{r83hunt,treehunt,schb,scmtap,spinwatch,hilogcensus,pathstash,reaper}.rs` + their call sites (timer.rs, sysv_sem.rs, epoll, unix, printk.rs:1756, fork/exit paths) + `trace/mod.rs` event rows (`r83scan, treehunt, schb, scmtap, spinwatch, hilog_* , sysforensic, txnhang, fuxstuck`) | 8 module deletions (~3,000 lines), invariant assert for treehunt, reaper prints → existing sched events | — | None (no ext4/jbd2). timer.rs is shared with B2/B5 — sequence those |
| **B2** — rt2xdbg/cycdbg/kpreempt deletes | `sched/{sched,rt}.rs`, `sync/futex.rs`, `process/task.rs`, `arch/unsafe.rs`, `arch/{riscv64,x86_64}/trap.rs`, `timer.rs` | 18 print sites + the fuxwatch/rwfrzwatch engines; fixes G1 | B1 for timer.rs sequencing | sched.rs is hot with the sched lanes — land via the scheduler lane's queue |
| **B3** — OH-hunt print deletes | `syscall/{file,process,memory,dispatch,io,misc}.rs`, `fs/vfs.rs`, `fs/{devfs,procfs}/mod.rs`, `drivers/drm/mod.rs`, `net/netlink.rs`, `process/task.rs`, then delete `oh_hunt_prints_on` + `sprobe` (`dfx/switches.rs:203-217`) | ~60 sites; the master gate dies last | — | file.rs is high-traffic; rebase on 0fbf51a63 if it touches fs hot paths |
| **B4** — binder print delete + inputtrace conversion | `ipc/binder.rs` (print family + `binder_probe_on`), `drivers/input/evdev.rs` (IN-EV/IN-OPEN → `trace_evdev_*`), keep census stamps | IPC/binder guarded-domain clean-up | 0fbf51a63 if binder.rs shares ext4 hot-path context (verify at land time) | binder.rs is binder-domain exclusive |
| **B5** — big-module deletes (sign-off gated) | `dfx/{scribble,leakorigin,slowtask,wildwatch}.rs` + switches (`switches.rs` Scribble*/WildWatch rows + `dfx=` parse in `switches.rs:292-319`) + `dfx/mod.rs:184` arm call | ~3,600 lines; fixes G2/G4 | hunt-owner sign-offs; M46 for slowtask | switches.rs is shared with B-G |
| **B6** — exception-class reshape wave 1 | `dfx/{fsw,filerd,reaper-fold}.rs`, `ipc/binder.rs` census, `sched/wakewatch.rs` prints→records | convert counters/prints to existing tracepoint families where parity exists | 0fbf51a63 (ext4/jbd2 files) | ext4 tracepoint fills same wave |
| **B-G** — gating sweep | `kernel/src/cmdline.rs` (cached key table), `dfx/mod.rs` (knob rehome LKDTM-style), Cargo.toml (`dfx-scribble`), review checklist line | fixes G5 + G4 + knob layout | B1-B5 to avoid deleting what it edits | — |
| **F1 (P1 fills)** | `dfx/hung_task.rs` + `dfx/mod.rs:186-188` + sysctls; `softlockup.rs` + watchdog sysctls; `fs/procfs/pid.rs` (stack, wchan, stat field 35); `sysctl.rs` (dmesg_restrict, kptr_restrict, kernel.sysrq); `fs/procfs/zoneinfo.rs` new | the P1 list below | none; pid.rs/sysctl.rs shared with M44/M25 — sequence within F1 | sysctl.rs shared with B-G (key table) — merge F1 first |
| **F2 (P2 fills)** | trace: filter engine (`trace/mod.rs`+`files.rs`), `kprobe_events`, perf TRACEPOINT type, missing standard events (sched_stat_*, softirq_*, kmem_*, block_bio_*, signal_deliver, timer/hrtimer family, task_*, tlb_flush), PSI, slabinfo rows, dyndbg, `/proc/[pid]/syscall` | grouped per subsystem to keep review scope sane | 0fbf51a63 for the ext4/jbd2/vmscan event batches | one subsystem per lane |
| **F3 (P3)** | ftrace function/function_graph + dynamic ftrace (after kprobes proves the hook mechanism), lockdep/lock_stat, kgdb, hard lockup, kdump, sanitizers, pagetypeinfo, vmallocinfo, module syms, pstore params | roadmap rows | F2 hook mechanism | — |

**P1 fill list** (F1, in dependency order): (1) start khungtaskd + hung_task
sysctls (M46) — biggest alignment gap per line of work; (2) watchdog/softlockup
sysctls (M47); (3) `/proc/[pid]/stack` + `/proc/[pid]/wchan` + stat field 35
(M42/M43/M40); (4) `dmesg_restrict` + `kptr_restrict` + `kernel.sysrq` sysctls
(M7/M25/M26); (5) `/proc/zoneinfo` (M31).

---

## Appendix: counts (base 830da5bed)

- Trace events: **109** (49 standard-group + 60 bespoke `rux:`) + 1 marker
  pseudo-event; all 109 have live emitters; `tracing_on` default off.
- `dfx/`: 39 files, 19,754 lines, split: §2.1 delete modules 12 files /
  6,620 L; exception-keeps 15 files / 9,207 L; shared infra (mod, switches,
  oops, pstore, ksyms, backtrace, bug, taint, hung_task, softlockup, unsafe
  edges, monowatch) 12 files / 3,927 L.
- `crate::dfx::` call sites: 1,331 across 107 files.
- Scattered ad-hoc print sites: ~90 (rt2xdbg 15, OH-hunt ~60, binderprobe 5,
  evdev ~4, kpreempt 3, cycdbg/grqwedge/faaround ~4).
- Compile-time-gated debug today: 6 Cargo features only; runtime-relaxed-load
  sites: everything else; active mis-gates: G1-G4.
- Matrix tally (§1, 65 rows): ALIGNED 34, RESHAPE 11, MISSING 20
  (P1 x3 — dmesg_restrict M7, /proc/[pid]/stack M42, /proc/[pid]/wchan M43;
  P2 x10; P3 x7), plus the hung-task-start row (M46, RESHAPE P1) and
  the §1.7 exception-class table (22 instruments kept, gated).
  §2 delete rows: 12 modules + 15 scattered families.
