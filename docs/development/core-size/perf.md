# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# perf — hot 319 lines (stay) / cold 1551 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| perf/mod.rs | `new_stash` | 238 | 22 |  |
| perf/mod.rs | `perf_tick_dep_sync` | 260 | 26 |  |
| perf/mod.rs | `note_ctxsw` | 286 | 12 |  |
| perf/mod.rs | `note_pfault` | 298 | 12 |  |
| perf/mod.rs | `refresh_wanted_bits` | 310 | 27 |  |
| perf/mod.rs | `note_tick` | 337 | 7 | yes |
| perf/mod.rs | `note_tick_fp` | 344 | 19 | yes |
| perf/mod.rs | `any_armed` | 363 | 56 | yes |
| perf/mod.rs | `new` | 419 | 8 | yes |
| perf/mod.rs | `ring_covers` | 427 | 27 | yes |
| perf/mod.rs | `ring_frame_member` | 454 | 27 | yes |
| perf/mod.rs | `register` | 481 | 20 |  |
| perf/mod.rs | `unregister` | 501 | 33 | yes |
| perf/mod.rs | `node` | 534 | 49 |  |
| perf/mod.rs | `ring_ht_hash` | 583 | 5 |  |
| perf/mod.rs | `ring_frames_live` | 588 | 4 | yes |
| perf/mod.rs | `ring_frame_insert` | 592 | 26 |  |
| perf/mod.rs | `ring_frame_remove` | 618 | 21 |  |
| perf/mod.rs | `ring_frame_check` | 639 | 42 | yes |
| perf/mod.rs | `ring_slice` | 681 | 13 |  |
| perf/mod.rs | `ring_put` | 694 | 18 |  |
| perf/mod.rs | `ring_read_user_tail` | 712 | 11 |  |
| perf/mod.rs | `ring_publish_head` | 723 | 15 |  |
| perf/mod.rs | `ring_audit` | 738 | 26 |  |
| perf/mod.rs | `emit_record` | 764 | 64 |  |
| perf/mod.rs | `header` | 828 | 8 |  |
| perf/mod.rs | `now_ns` | 836 | 10 |  |
| perf/mod.rs | `user_callchain` | 846 | 41 |  |
| perf/mod.rs | `kernel_callchain` | 887 | 35 |  |
| perf/mod.rs | `softirq_tick` | 922 | 96 | yes |
| perf/mod.rs | `emit_sample` | 1018 | 98 |  |
| perf/mod.rs | `hook_comm` | 1116 | 30 |  |
| perf/mod.rs | `hook_mmap2` | 1146 | 47 |  |
| perf/mod.rs | `hook_fork` | 1193 | 27 |  |
| perf/mod.rs | `hook_exit` | 1220 | 28 |  |
| perf/mod.rs | `read_value` | 1248 | 48 |  |
| perf/mod.rs | `perf_read` | 1296 | 35 |  |
| perf/mod.rs | `perf_poll` | 1331 | 24 |  |
| perf/mod.rs | `event_of` | 1355 | 9 |  |
| perf/mod.rs | `perf_close` | 1364 | 69 |  |
| perf/mod.rs | `perf_poll_wait` | 1433 | 10 |  |
| perf/mod.rs | `perf_poll_unwait` | 1443 | 14 |  |
| perf/mod.rs | `perf_mmap_handler` | 1457 | 259 |  |
| perf/mod.rs | `perf_file_ioctl` | 1716 | 8 |  |
| perf/mod.rs | `perf_ioctl` | 1724 | 84 |  |
| perf/mod.rs | `perf_open_einval_diag` | 1808 | 15 |  |
| perf/mod.rs | `sys_perf_event_open` | 1823 | 285 |  |
