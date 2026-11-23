# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# trace — hot 227 lines (stay) / cold 2358 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| trace/files.rs | `parse_bool` | 74 | 13 |  |
| trace/files.rs | `bool_write` | 87 | 12 |  |
| trace/files.rs | `set_state` | 99 | 11 | yes |
| trace/files.rs | `groups` | 110 | 8 |  |
| trace/files.rs | `register` | 118 | 413 |  |
| trace/files.rs | `apply_event_token` | 531 | 35 |  |
| trace/files.rs | `init_from_cmdline` | 566 | 31 |  |
| trace/files.rs | `set_event_write` | 597 | 20 |  |
| trace/files.rs | `describe` | 617 | 14 |  |
| trace/mod.rs | `armed` | 111 | 5 | yes |
| trace/mod.rs | `full_name` | 116 | 7 |  |
| trace/mod.rs | `format_file` | 123 | 46 |  |
| trace/mod.rs | `pack` | 169 | 8 |  |
| trace/mod.rs | `pack` | 177 | 17 |  |
| trace/mod.rs | `pack` | 194 | 21 |  |
| trace/mod.rs | `pack` | 215 | 21 |  |
| trace/mod.rs | `pack` | 236 | 20 |  |
| trace/mod.rs | `pack` | 256 | 622 |  |
| trace/mod.rs | `find_event` | 878 | 15 |  |
| trace/mod.rs | `tracing_on` | 893 | 5 |  |
| trace/mod.rs | `set_tracing_on` | 898 | 21 |  |
| trace/mod.rs | `set_event_pids` | 919 | 11 |  |
| trace/mod.rs | `event_pids` | 930 | 17 |  |
| trace/mod.rs | `event_pid_passes` | 947 | 76 |  |
| trace/mod.rs | `find_option` | 1023 | 5 |  |
| trace/mod.rs | `option_enabled` | 1028 | 84 |  |
| trace/mod.rs | `slot` | 1112 | 9 |  |
| trace/mod.rs | `slot_mut` | 1121 | 10 |  |
| trace/mod.rs | `new` | 1131 | 33 | yes |
| trace/mod.rs | `push` | 1164 | 42 | yes |
| trace/mod.rs | `records_written` | 1206 | 8 |  |
| trace/mod.rs | `records_buffered` | 1214 | 8 |  |
| trace/mod.rs | `render_flags` | 1222 | 33 |  |
| trace/mod.rs | `record` | 1255 | 44 | yes |
| trace/mod.rs | `clear` | 1299 | 8 | yes |
| trace/mod.rs | `clear_cpu` | 1307 | 11 |  |
| trace/mod.rs | `record_marker` | 1318 | 33 |  |
| trace/mod.rs | `merge_by_timestamp` | 1351 | 6 |  |
| trace/mod.rs | `snapshot` | 1357 | 11 |  |
| trace/mod.rs | `drain` | 1368 | 16 | yes |
| trace/mod.rs | `snapshot_cpu` | 1384 | 9 |  |
| trace/mod.rs | `drain_cpu` | 1393 | 28 |  |
| trace/mod.rs | `save_snapshot` | 1421 | 43 |  |
| trace/mod.rs | `clear_snapshot` | 1464 | 9 |  |
| trace/mod.rs | `snapshot_records` | 1473 | 11 |  |
| trace/mod.rs | `render_snapshot` | 1484 | 43 |  |
| trace/mod.rs | `cpu_stats` | 1527 | 30 |  |
| trace/mod.rs | `collect_ring` | 1557 | 17 |  |
| trace/mod.rs | `trace_header` | 1574 | 24 |  |
| trace/mod.rs | `render_record` | 1598 | 38 |  |
| trace/mod.rs | `render_trace` | 1636 | 12 |  |
| trace/mod.rs | `render_pipe` | 1648 | 11 |  |
| trace/mod.rs | `render_trace_cpu` | 1659 | 33 |  |
| trace/mod.rs | `render_pipe_cpu` | 1692 | 13 |  |
| trace/mod.rs | `comm_from_words` | 1705 | 8 |  |
| trace/mod.rs | `comm_to_words` | 1713 | 11 | yes |
| trace/mod.rs | `task_state_str` | 1724 | 27 |  |
| trace/mod.rs | `ascii_from_word` | 1751 | 9 |  |
| trace/mod.rs | `ioprio_class` | 1760 | 13 |  |
| trace/mod.rs | `fmt_sched_wakeup` | 1773 | 14 |  |
| trace/mod.rs | `fmt_sched_process_fork` | 1787 | 17 |  |
| trace/mod.rs | `fmt_sched_process_exit` | 1804 | 14 |  |
| trace/mod.rs | `fmt_trace_marker` | 1818 | 14 |  |
| trace/mod.rs | `ps_render` | 1832 | 26 |  |
| trace/mod.rs | `fmt_page_fault` | 1858 | 18 |  |
| trace/mod.rs | `gfp_flags_text` | 1876 | 70 |  |
| trace/mod.rs | `print_flags_walk` | 1946 | 24 |  |
| trace/mod.rs | `map_flags_text` | 1970 | 20 |  |
| trace/mod.rs | `mflags_text` | 1990 | 11 |  |
| trace/mod.rs | `free_flags_text` | 2001 | 25 |  |
| trace/mod.rs | `alloc_call_site` | 2026 | 51 | yes |
| trace/mod.rs | `plumbing_verdict` | 2077 | 56 |  |
| trace/mod.rs | `str_to_comm16` | 2133 | 6 | yes |
