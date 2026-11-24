# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# trace — hot 116 lines (stay) / cold 1976 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| trace/mod.rs | `armed` | 114 | 6 |  |
| trace/mod.rs | `full_name` | 119 | 8 |  |
| trace/mod.rs | `format_file` | 126 | 47 |  |
| trace/mod.rs | `pack` | 172 | 9 |  |
| trace/mod.rs | `pack` | 180 | 18 |  |
| trace/mod.rs | `pack` | 197 | 22 |  |
| trace/mod.rs | `pack` | 218 | 22 |  |
| trace/mod.rs | `pack` | 239 | 21 |  |
| trace/mod.rs | `pack` | 259 | 623 |  |
| trace/mod.rs | `find_event` | 881 | 16 |  |
| trace/mod.rs | `tracing_on` | 896 | 6 |  |
| trace/mod.rs | `set_tracing_on` | 901 | 22 |  |
| trace/mod.rs | `set_event_pids` | 922 | 12 |  |
| trace/mod.rs | `event_pids` | 933 | 18 |  |
| trace/mod.rs | `event_pid_passes` | 950 | 77 |  |
| trace/mod.rs | `find_option` | 1026 | 6 |  |
| trace/mod.rs | `option_enabled` | 1031 | 85 |  |
| trace/mod.rs | `slot` | 1115 | 10 |  |
| trace/mod.rs | `slot_mut` | 1124 | 11 |  |
| trace/mod.rs | `new` | 1134 | 34 |  |
| trace/mod.rs | `push` | 1167 | 43 |  |
| trace/mod.rs | `records_written` | 1209 | 9 |  |
| trace/mod.rs | `records_buffered` | 1217 | 9 |  |
| trace/mod.rs | `render_flags` | 1225 | 34 |  |
| trace/mod.rs | `record` | 1258 | 45 | yes |
| trace/mod.rs | `clear` | 1302 | 9 |  |
| trace/mod.rs | `clear_cpu` | 1310 | 12 |  |
| trace/mod.rs | `record_marker` | 1321 | 34 |  |
| trace/mod.rs | `merge_by_timestamp` | 1354 | 7 |  |
| trace/mod.rs | `snapshot` | 1360 | 12 |  |
| trace/mod.rs | `drain` | 1371 | 17 |  |
| trace/mod.rs | `snapshot_cpu` | 1387 | 10 |  |
| trace/mod.rs | `drain_cpu` | 1396 | 29 |  |
| trace/mod.rs | `save_snapshot` | 1424 | 44 |  |
| trace/mod.rs | `clear_snapshot` | 1467 | 10 |  |
| trace/mod.rs | `snapshot_records` | 1476 | 12 |  |
| trace/mod.rs | `render_snapshot` | 1487 | 44 |  |
| trace/mod.rs | `cpu_stats` | 1530 | 31 |  |
| trace/mod.rs | `collect_ring` | 1560 | 18 |  |
| trace/mod.rs | `trace_header` | 1577 | 25 |  |
| trace/mod.rs | `render_record` | 1601 | 39 |  |
| trace/mod.rs | `render_trace` | 1639 | 13 |  |
| trace/mod.rs | `render_pipe` | 1651 | 12 |  |
| trace/mod.rs | `render_trace_cpu` | 1662 | 34 |  |
| trace/mod.rs | `render_pipe_cpu` | 1695 | 14 |  |
| trace/mod.rs | `comm_from_words` | 1708 | 9 |  |
| trace/mod.rs | `comm_to_words` | 1716 | 12 | yes |
| trace/mod.rs | `task_state_str` | 1727 | 28 |  |
| trace/mod.rs | `ascii_from_word` | 1754 | 10 |  |
| trace/mod.rs | `ioprio_class` | 1763 | 14 |  |
| trace/mod.rs | `fmt_sched_wakeup` | 1776 | 15 |  |
| trace/mod.rs | `fmt_sched_process_fork` | 1790 | 18 |  |
| trace/mod.rs | `fmt_sched_process_exit` | 1807 | 15 |  |
| trace/mod.rs | `fmt_trace_marker` | 1821 | 15 |  |
| trace/mod.rs | `ps_render` | 1835 | 27 |  |
| trace/mod.rs | `fmt_page_fault` | 1861 | 19 |  |
| trace/mod.rs | `gfp_flags_text` | 1879 | 71 |  |
| trace/mod.rs | `print_flags_walk` | 1949 | 25 |  |
| trace/mod.rs | `map_flags_text` | 1973 | 21 |  |
| trace/mod.rs | `mflags_text` | 1993 | 12 |  |
| trace/mod.rs | `free_flags_text` | 2004 | 26 |  |
| trace/mod.rs | `alloc_call_site` | 2029 | 52 | yes |
| trace/mod.rs | `plumbing_verdict` | 2080 | 57 |  |
| trace/mod.rs | `str_to_comm16` | 2136 | 7 | yes |
