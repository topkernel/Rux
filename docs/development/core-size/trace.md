# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# trace — hot 116 lines (stay) / cold 2542 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| trace/files.rs | `parse_bool` | 74 | 14 |  |
| trace/files.rs | `bool_write` | 87 | 13 |  |
| trace/files.rs | `set_state` | 99 | 12 |  |
| trace/files.rs | `groups` | 110 | 9 |  |
| trace/files.rs | `register` | 118 | 414 |  |
| trace/files.rs | `apply_event_token` | 531 | 36 |  |
| trace/files.rs | `init_from_cmdline` | 566 | 32 |  |
| trace/files.rs | `set_event_write` | 597 | 21 |  |
| trace/files.rs | `describe` | 617 | 15 |  |
| trace/mod.rs | `armed` | 111 | 6 |  |
| trace/mod.rs | `full_name` | 116 | 8 |  |
| trace/mod.rs | `format_file` | 123 | 47 |  |
| trace/mod.rs | `pack` | 169 | 9 |  |
| trace/mod.rs | `pack` | 177 | 18 |  |
| trace/mod.rs | `pack` | 194 | 22 |  |
| trace/mod.rs | `pack` | 215 | 22 |  |
| trace/mod.rs | `pack` | 236 | 21 |  |
| trace/mod.rs | `pack` | 256 | 623 |  |
| trace/mod.rs | `find_event` | 878 | 16 |  |
| trace/mod.rs | `tracing_on` | 893 | 6 |  |
| trace/mod.rs | `set_tracing_on` | 898 | 22 |  |
| trace/mod.rs | `set_event_pids` | 919 | 12 |  |
| trace/mod.rs | `event_pids` | 930 | 18 |  |
| trace/mod.rs | `event_pid_passes` | 947 | 77 |  |
| trace/mod.rs | `find_option` | 1023 | 6 |  |
| trace/mod.rs | `option_enabled` | 1028 | 85 |  |
| trace/mod.rs | `slot` | 1112 | 10 |  |
| trace/mod.rs | `slot_mut` | 1121 | 11 |  |
| trace/mod.rs | `new` | 1131 | 34 |  |
| trace/mod.rs | `push` | 1164 | 43 |  |
| trace/mod.rs | `records_written` | 1206 | 9 |  |
| trace/mod.rs | `records_buffered` | 1214 | 9 |  |
| trace/mod.rs | `render_flags` | 1222 | 34 |  |
| trace/mod.rs | `record` | 1255 | 45 | yes |
| trace/mod.rs | `clear` | 1299 | 9 |  |
| trace/mod.rs | `clear_cpu` | 1307 | 12 |  |
| trace/mod.rs | `record_marker` | 1318 | 34 |  |
| trace/mod.rs | `merge_by_timestamp` | 1351 | 7 |  |
| trace/mod.rs | `snapshot` | 1357 | 12 |  |
| trace/mod.rs | `drain` | 1368 | 17 |  |
| trace/mod.rs | `snapshot_cpu` | 1384 | 10 |  |
| trace/mod.rs | `drain_cpu` | 1393 | 29 |  |
| trace/mod.rs | `save_snapshot` | 1421 | 44 |  |
| trace/mod.rs | `clear_snapshot` | 1464 | 10 |  |
| trace/mod.rs | `snapshot_records` | 1473 | 12 |  |
| trace/mod.rs | `render_snapshot` | 1484 | 44 |  |
| trace/mod.rs | `cpu_stats` | 1527 | 31 |  |
| trace/mod.rs | `collect_ring` | 1557 | 18 |  |
| trace/mod.rs | `trace_header` | 1574 | 25 |  |
| trace/mod.rs | `render_record` | 1598 | 39 |  |
| trace/mod.rs | `render_trace` | 1636 | 13 |  |
| trace/mod.rs | `render_pipe` | 1648 | 12 |  |
| trace/mod.rs | `render_trace_cpu` | 1659 | 34 |  |
| trace/mod.rs | `render_pipe_cpu` | 1692 | 14 |  |
| trace/mod.rs | `comm_from_words` | 1705 | 9 |  |
| trace/mod.rs | `comm_to_words` | 1713 | 12 | yes |
| trace/mod.rs | `task_state_str` | 1724 | 28 |  |
| trace/mod.rs | `ascii_from_word` | 1751 | 10 |  |
| trace/mod.rs | `ioprio_class` | 1760 | 14 |  |
| trace/mod.rs | `fmt_sched_wakeup` | 1773 | 15 |  |
| trace/mod.rs | `fmt_sched_process_fork` | 1787 | 18 |  |
| trace/mod.rs | `fmt_sched_process_exit` | 1804 | 15 |  |
| trace/mod.rs | `fmt_trace_marker` | 1818 | 15 |  |
| trace/mod.rs | `ps_render` | 1832 | 27 |  |
| trace/mod.rs | `fmt_page_fault` | 1858 | 19 |  |
| trace/mod.rs | `gfp_flags_text` | 1876 | 71 |  |
| trace/mod.rs | `print_flags_walk` | 1946 | 25 |  |
| trace/mod.rs | `map_flags_text` | 1970 | 21 |  |
| trace/mod.rs | `mflags_text` | 1990 | 12 |  |
| trace/mod.rs | `free_flags_text` | 2001 | 26 |  |
| trace/mod.rs | `alloc_call_site` | 2026 | 52 | yes |
| trace/mod.rs | `plumbing_verdict` | 2077 | 57 |  |
| trace/mod.rs | `str_to_comm16` | 2133 | 7 | yes |
