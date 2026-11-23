# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# perf — hot 286 lines (stay) / cold 1631 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| perf/mod.rs | `new_stash` | 238 | 23 |  |
| perf/mod.rs | `perf_tick_dep_sync` | 260 | 27 |  |
| perf/mod.rs | `note_ctxsw` | 286 | 13 |  |
| perf/mod.rs | `note_pfault` | 298 | 13 |  |
| perf/mod.rs | `refresh_wanted_bits` | 310 | 28 |  |
| perf/mod.rs | `note_tick` | 337 | 8 | yes |
| perf/mod.rs | `note_tick_fp` | 344 | 20 | yes |
| perf/mod.rs | `any_armed` | 363 | 57 | yes |
| perf/mod.rs | `new` | 419 | 9 |  |
| perf/mod.rs | `ring_covers` | 427 | 28 | yes |
| perf/mod.rs | `ring_frame_member` | 454 | 28 | yes |
| perf/mod.rs | `register` | 481 | 21 |  |
| perf/mod.rs | `unregister` | 501 | 34 |  |
| perf/mod.rs | `node` | 534 | 50 |  |
| perf/mod.rs | `ring_ht_hash` | 583 | 6 |  |
| perf/mod.rs | `ring_frames_live` | 588 | 5 | yes |
| perf/mod.rs | `ring_frame_insert` | 592 | 27 |  |
| perf/mod.rs | `ring_frame_remove` | 618 | 22 |  |
| perf/mod.rs | `ring_frame_check` | 639 | 43 | yes |
| perf/mod.rs | `ring_slice` | 681 | 14 |  |
| perf/mod.rs | `ring_put` | 694 | 19 |  |
| perf/mod.rs | `ring_read_user_tail` | 712 | 12 |  |
| perf/mod.rs | `ring_publish_head` | 723 | 16 |  |
| perf/mod.rs | `ring_audit` | 738 | 27 |  |
| perf/mod.rs | `emit_record` | 764 | 65 |  |
| perf/mod.rs | `header` | 828 | 9 |  |
| perf/mod.rs | `now_ns` | 836 | 11 |  |
| perf/mod.rs | `user_callchain` | 846 | 42 |  |
| perf/mod.rs | `kernel_callchain` | 887 | 36 |  |
| perf/mod.rs | `softirq_tick` | 922 | 97 | yes |
| perf/mod.rs | `emit_sample` | 1018 | 99 |  |
| perf/mod.rs | `hook_comm` | 1116 | 31 |  |
| perf/mod.rs | `hook_mmap2` | 1146 | 48 |  |
| perf/mod.rs | `hook_fork` | 1193 | 28 |  |
| perf/mod.rs | `hook_exit` | 1220 | 29 |  |
| perf/mod.rs | `read_value` | 1248 | 49 |  |
| perf/mod.rs | `perf_read` | 1296 | 36 |  |
| perf/mod.rs | `perf_poll` | 1331 | 25 |  |
| perf/mod.rs | `event_of` | 1355 | 10 |  |
| perf/mod.rs | `perf_close` | 1364 | 70 |  |
| perf/mod.rs | `perf_poll_wait` | 1433 | 11 |  |
| perf/mod.rs | `perf_poll_unwait` | 1443 | 15 |  |
| perf/mod.rs | `perf_mmap_handler` | 1457 | 260 |  |
| perf/mod.rs | `perf_file_ioctl` | 1716 | 9 |  |
| perf/mod.rs | `perf_ioctl` | 1724 | 85 |  |
| perf/mod.rs | `perf_open_einval_diag` | 1808 | 16 |  |
| perf/mod.rs | `sys_perf_event_open` | 1823 | 286 |  |
