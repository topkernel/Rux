# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# debug — hot 6642 lines (stay) / cold 8239 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| debug/allocwatch.rs | `slot_of` | 65 | 19 |  |
| debug/allocwatch.rs | `new` | 84 | 19 | yes |
| debug/allocwatch.rs | `armed` | 103 | 4 | yes |
| debug/allocwatch.rs | `enabled` | 107 | 14 | yes |
| debug/allocwatch.rs | `enter_keyed` | 121 | 22 |  |
| debug/allocwatch.rs | `exit_keyed` | 143 | 14 |  |
| debug/allocwatch.rs | `guard` | 157 | 12 | yes |
| debug/allocwatch.rs | `drop` | 169 | 8 | yes |
| debug/allocwatch.rs | `stamp_live` | 177 | 20 |  |
| debug/allocwatch.rs | `stuck_report` | 197 | 25 |  |
| debug/allocwatch.rs | `tick_stuck_scan` | 222 | 46 |  |
| debug/allocwatch.rs | `tick_check` | 268 | 198 | yes |
| debug/backtrace.rs | `new` | 44 | 10 | yes |
| debug/backtrace.rs | `flush` | 54 | 10 | yes |
| debug/backtrace.rs | `drop` | 64 | 9 | yes |
| debug/backtrace.rs | `write_str` | 73 | 29 | yes |
| debug/backtrace.rs | `current_frame_pointer` | 102 | 26 | yes |
| debug/backtrace.rs | `caller_return_address` | 128 | 30 | yes |
| debug/backtrace.rs | `unwind_frame` | 158 | 24 |  |
| debug/backtrace.rs | `walk_stack_trace` | 182 | 34 | yes |
| debug/backtrace.rs | `dump_stack` | 216 | 63 | yes |
| debug/backtrace.rs | `save_regs` | 279 | 47 |  |
| debug/backtrace.rs | `save_regs` | 326 | 45 |  |
| debug/backtrace.rs | `save_regs` | 371 | 29 |  |
| debug/backtrace.rs | `dump_regs_inline` | 400 | 6 |  |
| debug/backtrace.rs | `dump_regs` | 406 | 17 |  |
| debug/backtrace.rs | `dump_csrs` | 423 | 23 |  |
| debug/backtrace.rs | `dump_csrs` | 446 | 8 |  |
| debug/backtrace.rs | `dump_csrs` | 454 | 19 |  |
| debug/bug.rs | `warn` | 26 | 38 |  |
| debug/bug.rs | `bug` | 64 | 30 |  |
| debug/bug.rs | `warn_on_once_check` | 94 | 6 |  |
| debug/cctx.rs | `now` | 134 | 10 | yes |
| debug/cctx.rs | `add` | 144 | 11 | yes |
| debug/cctx.rs | `bump` | 155 | 11 | yes |
| debug/cctx.rs | `on` | 166 | 20 | yes |
| debug/cctx.rs | `v` | 186 | 4 |  |
| debug/cctx.rs | `avg` | 190 | 11 |  |
| debug/cctx.rs | `dump` | 201 | 81 | yes |
| debug/driftwatch.rs | `note_zero_put` | 111 | 5 | yes |
| debug/driftwatch.rs | `report` | 116 | 56 |  |
| debug/filerd.rs | `on` | 70 | 6 | yes |
| debug/filerd.rs | `now` | 76 | 6 | yes |
| debug/filerd.rs | `charge` | 82 | 7 |  |
| debug/filerd.rs | `note_miss` | 89 | 6 |  |
| debug/filerd.rs | `measure_tsc_hz` | 95 | 19 |  |
| debug/filerd.rs | `generate` | 114 | 43 |  |
| debug/forensics_edges.rs | `adopt_task` | 47 | 10 |  |
| debug/forensics_edges.rs | `t_pid` | 57 | 7 | yes |
| debug/forensics_edges.rs | `t_comm` | 64 | 7 |  |
| debug/forensics_edges.rs | `t_preempt_count` | 71 | 12 | yes |
| debug/forensics_edges.rs | `raw_qword` | 83 | 15 |  |
| debug/forensics_edges.rs | `raw_u32` | 98 | 11 |  |
| debug/forensics_edges.rs | `location` | 109 | 8 |  |
| debug/forensics_edges.rs | `location_ref` | 117 | 10 |  |
| debug/forensics_edges.rs | `mark_first_refusal` | 127 | 12 | yes |
| debug/forkprof.rs | `stamp_wake` | 91 | 7 |  |
| debug/forkprof.rs | `stamp_child` | 98 | 7 |  |
| debug/forkprof.rs | `consume_wake` | 105 | 69 | yes |
| debug/forkprof.rs | `trace_on` | 174 | 10 | yes |
| debug/forkprof.rs | `trace_on_armed` | 184 | 6 |  |
| debug/forkprof.rs | `trace` | 190 | 12 | yes |
| debug/forkprof.rs | `report_trace` | 202 | 65 |  |
| debug/forkprof.rs | `on` | 267 | 22 | yes |
| debug/forkprof.rs | `now` | 289 | 6 | yes |
| debug/forkprof.rs | `add` | 295 | 10 | yes |
| debug/forkprof.rs | `bump` | 305 | 5 | yes |
| debug/forkprof.rs | `peek` | 310 | 10 |  |
| debug/forkprof.rs | `report` | 320 | 69 |  |
| debug/fsw.rs | `max_update` | 208 | 16 |  |
| debug/fsw.rs | `open_exec_stamp` | 224 | 17 |  |
| debug/fsw.rs | `open_close` | 241 | 17 |  |
| debug/fsw.rs | `note_walk_miss_enoent` | 258 | 4 | yes |
| debug/fsw.rs | `note_neghit` | 262 | 4 | yes |
| debug/fsw.rs | `note_negstale` | 266 | 12 | yes |
| debug/fsw.rs | `negmemo_on` | 278 | 22 | yes |
| debug/fsw.rs | `pc_now` | 300 | 10 | yes |
| debug/fsw.rs | `pc_add` | 310 | 16 | yes |
| debug/fsw.rs | `on` | 326 | 36 | yes |
| debug/fsw.rs | `note_bread` | 362 | 4 | yes |
| debug/fsw.rs | `note_dev_read` | 366 | 4 | yes |
| debug/fsw.rs | `note_dev_write` | 370 | 4 | yes |
| debug/fsw.rs | `note_sync_dirty` | 374 | 4 | yes |
| debug/fsw.rs | `note_sync_dirty_skip` | 378 | 4 | yes |
| debug/fsw.rs | `note_evict` | 382 | 9 | yes |
| debug/fsw.rs | `note_sync_all` | 391 | 4 | yes |
| debug/fsw.rs | `note_pub_flush` | 395 | 7 | yes |
| debug/fsw.rs | `note_pub_scoped` | 402 | 7 | yes |
| debug/fsw.rs | `note_pub_cstr` | 409 | 4 | yes |
| debug/fsw.rs | `note_pub_hiwat` | 413 | 4 | yes |
| debug/fsw.rs | `note_pub_batch` | 417 | 5 | yes |
| debug/fsw.rs | `note_mkdir_us` | 422 | 20 |  |
| debug/fsw.rs | `note_j_dedup` | 442 | 7 |  |
| debug/fsw.rs | `note_drop_block` | 449 | 8 | yes |
| debug/fsw.rs | `note_rmdir_us` | 457 | 19 |  |
| debug/fsw.rs | `note_unlink_us` | 476 | 31 |  |
| debug/fsw.rs | `note_rmdir_phases` | 507 | 24 |  |
| debug/fsw.rs | `note_pub_lazy` | 531 | 4 |  |
| debug/fsw.rs | `note_pub_qmax` | 535 | 21 | yes |
| debug/fsw.rs | `note_read_inode` | 556 | 4 |  |
| debug/fsw.rs | `note_write_inode` | 560 | 4 |  |
| debug/fsw.rs | `note_dir_lookup` | 564 | 7 |  |
| debug/fsw.rs | `note_list_dir` | 571 | 4 |  |
| debug/fsw.rs | `note_new_inode` | 575 | 4 |  |
| debug/fsw.rs | `note_alloc_run` | 579 | 4 |  |
| debug/fsw.rs | `note_free_run` | 583 | 6 |  |
| debug/fsw.rs | `note_j_start` | 589 | 4 |  |
| debug/fsw.rs | `note_j_stop` | 593 | 4 |  |
| debug/fsw.rs | `note_j_lazy` | 597 | 4 |  |
| debug/fsw.rs | `note_j_commit_fast` | 601 | 7 |  |
| debug/fsw.rs | `note_j_commit_slow` | 608 | 4 |  |
| debug/fsw.rs | `note_j_revoke` | 612 | 6 |  |
| debug/fsw.rs | `note_biglock` | 618 | 4 |  |
| debug/fsw.rs | `note_biglock_contend` | 622 | 6 |  |
| debug/fsw.rs | `note_walk` | 628 | 4 | yes |
| debug/fsw.rs | `note_walk_hit` | 632 | 4 | yes |
| debug/fsw.rs | `note_walk_miss` | 636 | 11 | yes |
| debug/fsw.rs | `tick` | 647 | 17 | yes |
| debug/fsw.rs | `v` | 664 | 5 |  |
| debug/fsw.rs | `dump` | 669 | 86 | yes |
| debug/hilogcensus.rs | `on` | 60 | 8 | yes |
| debug/hilogcensus.rs | `compat_arm` | 68 | 26 |  |
| debug/hilogcensus.rs | `new` | 94 | 8 | yes |
| debug/hilogcensus.rs | `bytes` | 102 | 7 | yes |
| debug/hilogcensus.rs | `set` | 109 | 6 | yes |
| debug/hilogcensus.rs | `same` | 115 | 8 |  |
| debug/hilogcensus.rs | `set_content` | 123 | 7 |  |
| debug/hilogcensus.rs | `content_str` | 130 | 34 |  |
| debug/hilogcensus.rs | `new` | 164 | 29 | yes |
| debug/hilogcensus.rs | `new` | 193 | 16 | yes |
| debug/hilogcensus.rs | `parse_hilogmsg` | 209 | 37 |  |
| debug/hilogcensus.rs | `unix_send_pre` | 246 | 53 |  |
| debug/hilogcensus.rs | `bump_tag` | 299 | 27 |  |
| debug/hilogcensus.rs | `unix_send_post` | 326 | 23 |  |
| debug/hilogcensus.rs | `kmsg_write` | 349 | 25 |  |
| debug/hilogcensus.rs | `comm_str` | 374 | 15 |  |
| debug/hilogcensus.rs | `tag_str` | 389 | 10 |  |
| debug/hilogcensus.rs | `dump` | 399 | 160 | yes |
| debug/hung_task.rs | `now_ns` | 28 | 10 |  |
| debug/hung_task.rs | `init` | 38 | 108 | yes |
| debug/hung_task.rs | `check_tasks` | 146 | 44 |  |
| debug/hung_task.rs | `report_hung_task` | 190 | 132 |  |
| debug/ksyms.rs | `table_valid` | 55 | 14 |  |
| debug/ksyms.rs | `name_at` | 69 | 14 |  |
| debug/ksyms.rs | `symbol_name` | 83 | 8 |  |
| debug/ksyms.rs | `lookup` | 91 | 34 | yes |
| debug/ksyms.rs | `format_symbol` | 125 | 13 | yes |
| debug/leakorigin.rs | `slot_of` | 147 | 13 |  |
| debug/leakorigin.rs | `note_acquire` | 160 | 87 | yes |
| debug/leakorigin.rs | `phex` | 247 | 11 | yes |
| debug/leakorigin.rs | `pdec` | 258 | 67 | yes |
| debug/leakorigin.rs | `report_acquire_divergent` | 325 | 127 |  |
| debug/leakorigin.rs | `note_release` | 452 | 26 |  |
| debug/leakorigin.rs | `ph` | 478 | 150 | yes |
| debug/leakorigin.rs | `note_hardirq_acquire` | 628 | 22 | yes |
| debug/leakorigin.rs | `note_softirq_acquire` | 650 | 29 | yes |
| debug/leakorigin.rs | `note_softirq_release` | 679 | 8 |  |
| debug/leakorigin.rs | `report_hardirq_origin` | 687 | 21 | yes |
| debug/leakorigin.rs | `putd` | 708 | 50 |  |
| debug/leakorigin.rs | `report_softirq_origin` | 758 | 21 | yes |
| debug/leakorigin.rs | `putd` | 779 | 72 |  |
| debug/leakorigin.rs | `check_user_return` | 851 | 38 | yes |
| debug/leakorigin.rs | `report_parked_site` | 889 | 5 | yes |
| debug/leakorigin.rs | `pd` | 894 | 47 | yes |
| debug/leakorigin.rs | `report_incumbent_site` | 941 | 14 | yes |
| debug/leakorigin.rs | `report_outermost` | 955 | 20 | yes |
| debug/leakorigin.rs | `putd` | 975 | 77 |  |
| debug/leakorigin.rs | `emit` | 1052 | 9 | yes |
| debug/leakorigin.rs | `put_i64` | 1061 | 139 |  |
| debug/ledger.rs | `dec_buf` | 47 | 25 |  |
| debug/ledger.rs | `new` | 72 | 5 | yes |
| debug/ledger.rs | `push` | 77 | 11 | yes |
| debug/ledger.rs | `dec` | 88 | 4 |  |
| debug/ledger.rs | `emit` | 92 | 10 | yes |
| debug/ledger.rs | `compact_line` | 102 | 31 |  |
| debug/ledger.rs | `census_line` | 133 | 24 | yes |
| debug/ledger.rs | `oomfx_line` | 157 | 51 | yes |
| debug/ledger.rs | `dump` | 208 | 253 | yes |
| debug/ledger.rs | `run_pending` | 461 | 18 | yes |
| debug/ledger.rs | `series_tick` | 479 | 13 |  |
| debug/lostpark.rs | `armed` | 37 | 6 | yes |
| debug/lostpark.rs | `record` | 43 | 16 | yes |
| debug/lostpark.rs | `dump_once` | 59 | 37 |  |
| debug/memwatch.rs | `bucket` | 76 | 11 |  |
| debug/memwatch.rs | `armed` | 87 | 48 | yes |
| debug/memwatch.rs | `kernel_va_readable` | 135 | 34 |  |
| debug/memwatch.rs | `plausible_fp` | 169 | 5 |  |
| debug/memwatch.rs | `plausible_fp` | 174 | 9 |  |
| debug/memwatch.rs | `walk_fp_chain` | 183 | 32 | yes |
| debug/memwatch.rs | `walk_fp_chain` | 215 | 21 | yes |
| debug/memwatch.rs | `walk_fp_chain` | 236 | 19 | yes |
| debug/memwatch.rs | `chain_key` | 255 | 24 |  |
| debug/memwatch.rs | `sanitized_ra` | 279 | 12 |  |
| debug/memwatch.rs | `sanitized_ra` | 291 | 7 |  |
| debug/memwatch.rs | `sanitized_ra` | 298 | 11 |  |
| debug/memwatch.rs | `note_realloc` | 309 | 29 | yes |
| debug/memwatch.rs | `note_alloc` | 338 | 142 | yes |
| debug/memwatch.rs | `live_intern` | 480 | 34 |  |
| debug/memwatch.rs | `mark_pages` | 514 | 25 |  |
| debug/memwatch.rs | `note_free` | 539 | 50 | yes |
| debug/memwatch.rs | `ppage_site_table` | 589 | 67 |  |
| debug/memwatch.rs | `psite_intern` | 656 | 28 |  |
| debug/memwatch.rs | `note_page_alloc` | 684 | 30 | yes |
| debug/memwatch.rs | `note_page_free` | 714 | 31 | yes |
| debug/memwatch.rs | `tracked_index` | 745 | 23 |  |
| debug/memwatch.rs | `put_dec` | 768 | 7 |  |
| debug/memwatch.rs | `kroot_pollution` | 775 | 64 |  |
| debug/memwatch.rs | `put_hex` | 839 | 17 |  |
| debug/memwatch.rs | `dump_mem` | 856 | 107 |  |
| debug/memwatch.rs | `dump_heap_ledger` | 963 | 191 |  |
| debug/memwatch.rs | `dump_page_ledger` | 1154 | 59 |  |
| debug/metawatch.rs | `tick` | 58 | 37 | yes |
| debug/metawatch.rs | `scan` | 95 | 19 |  |
| debug/metawatch.rs | `kind_str` | 114 | 15 |  |
| debug/metawatch.rs | `report` | 129 | 39 |  |
| debug/mmforensics.rs | `note_ref` | 97 | 35 | yes |
| debug/mmforensics.rs | `current_root_ppn` | 132 | 14 |  |
| debug/mmforensics.rs | `ring_history` | 146 | 164 |  |
| debug/mmforensics.rs | `capture_frame` | 310 | 115 | yes |
| debug/mmprof.rs | `on` | 102 | 20 | yes |
| debug/mmprof.rs | `now` | 122 | 7 | yes |
| debug/mmprof.rs | `add` | 129 | 13 | yes |
| debug/mmprof.rs | `bump` | 142 | 9 | yes |
| debug/mmprof.rs | `bucket_len` | 151 | 22 |  |
| debug/mmprof.rs | `bump_comm` | 173 | 59 |  |
| debug/mmprof.rs | `tick` | 232 | 22 | yes |
| debug/mmprof.rs | `ensure_dump_thread` | 254 | 42 |  |
| debug/mmprof.rs | `fmt_cell` | 296 | 12 |  |
| debug/mmprof.rs | `stat` | 308 | 14 |  |
| debug/mmprof.rs | `report_line` | 322 | 83 |  |
| debug/mod.rs | `init` | 162 | 47 | yes |
| debug/mod.rs | `register_debugfs_knobs` | 209 | 170 |  |
| debug/mod.rs | `walk_read` | 379 | 12 |  |
| debug/monowatch.rs | `clocksource_name` | 63 | 22 |  |
| debug/monowatch.rs | `tick` | 85 | 45 | yes |
| debug/monowatch.rs | `report` | 130 | 19 |  |
| debug/oops.rs | `init_from_cmdline` | 84 | 9 |  |
| debug/oops.rs | `panic_on_oops` | 93 | 5 |  |
| debug/oops.rs | `set_panic_on_oops` | 98 | 12 |  |
| debug/oops.rs | `oops` | 110 | 150 | yes |
| debug/oops.rs | `comm_str` | 260 | 9 |  |
| debug/pathstash.rs | `stash_open` | 57 | 9 |  |
| debug/pathstash.rs | `stash_file` | 66 | 29 |  |
| debug/pathstash.rs | `stash_file_lazy` | 95 | 10 |  |
| debug/pathstash.rs | `lookup_file` | 105 | 19 |  |
| debug/pathstash.rs | `lookup_open` | 124 | 3 |  |
| debug/pstore.rs | `new` | 115 | 12 | yes |
| debug/pstore.rs | `reserve` | 127 | 17 |  |
| debug/pstore.rs | `reserve_at` | 144 | 10 |  |
| debug/pstore.rs | `region_virt` | 154 | 14 |  |
| debug/pstore.rs | `early_scan` | 168 | 35 |  |
| debug/pstore.rs | `record_name` | 203 | 9 |  |
| debug/pstore.rs | `has_record` | 212 | 5 |  |
| debug/pstore.rs | `read_record` | 217 | 24 |  |
| debug/pstore.rs | `arm_sink` | 241 | 6 |  |
| debug/pstore.rs | `sink` | 247 | 17 |  |
| debug/pstore.rs | `panic_capture` | 264 | 15 |  |
| debug/pstore.rs | `oops_capture` | 279 | 9 |  |
| debug/pstore.rs | `write_record` | 288 | 73 |  |
| debug/pstore.rs | `write_str` | 361 | 8 | yes |
| debug/reaper.rs | `on` | 97 | 5 | yes |
| debug/reaper.rs | `secs` | 102 | 9 |  |
| debug/reaper.rs | `decode_kernel_code` | 111 | 9 |  |
| debug/reaper.rs | `decode_wstatus` | 120 | 14 |  |
| debug/reaper.rs | `decode_si_code` | 134 | 12 |  |
| debug/reaper.rs | `comm_str` | 146 | 11 |  |
| debug/reaper.rs | `note_nameset` | 157 | 49 |  |
| debug/reaper.rs | `nameset_of` | 206 | 10 |  |
| debug/reaper.rs | `on_zombie` | 216 | 54 |  |
| debug/reaper.rs | `comm_of` | 270 | 9 |  |
| debug/reaper.rs | `on_reap_wait4` | 279 | 23 |  |
| debug/reaper.rs | `on_reap_waitid` | 302 | 35 |  |
| debug/reaper.rs | `poll_note` | 337 | 33 |  |
| debug/reaper.rs | `zombie_scan` | 370 | 112 | yes |
| debug/schb.rs | `note_proc_open` | 107 | 33 |  |
| debug/schb.rs | `note_cow` | 140 | 10 | yes |
| debug/schb.rs | `note_drct` | 150 | 11 |  |
| debug/schb.rs | `note_slow` | 161 | 8 | yes |
| debug/schb.rs | `pace_snapshot` | 169 | 16 |  |
| debug/schb.rs | `oom_anat_dump` | 185 | 75 |  |
| debug/schb.rs | `init` | 260 | 30 | yes |
| debug/schb.rs | `comm_str` | 290 | 229 |  |
| debug/schb.rs | `wedge_census` | 519 | 125 |  |
| debug/scmtap.rs | `enabled` | 45 | 6 | yes |
| debug/scmtap.rs | `who` | 51 | 14 |  |
| debug/scmtap.rs | `tap_tx` | 65 | 6 |  |
| debug/scmtap.rs | `tap_tx_err` | 71 | 6 |  |
| debug/scmtap.rs | `tap_rx` | 77 | 6 |  |
| debug/scmtap.rs | `tap_rx_drop` | 83 | 12 |  |
| debug/scmtap.rs | `tap_orphan` | 95 | 12 |  |
| debug/scmtap.rs | `tap_fifo_last_writer` | 107 | 10 |  |
| debug/scribble.rs | `ring_log` | 139 | 14 | yes |
| debug/scribble.rs | `ring_dump` | 153 | 67 | yes |
| debug/scribble.rs | `audit_frames` | 220 | 27 | yes |
| debug/scribble.rs | `audit_frames` | 247 | 5 | yes |
| debug/scribble.rs | `audit_record` | 252 | 20 | yes |
| debug/scribble.rs | `audit_drain` | 272 | 56 |  |
| debug/scribble.rs | `parse_hex_u64` | 328 | 18 |  |
| debug/scribble.rs | `pin_park_check` | 346 | 36 |  |
| debug/scribble.rs | `task_plausible` | 382 | 44 | yes |
| debug/scribble.rs | `double_seed` | 426 | 55 |  |
| debug/scribble.rs | `bad_switch_arg` | 481 | 40 | yes |
| debug/scribble.rs | `switch_window_heal` | 521 | 42 | yes |
| debug/scribble.rs | `putc` | 563 | 16 |  |
| debug/scribble.rs | `puts` | 579 | 6 |  |
| debug/scribble.rs | `put_hex` | 585 | 10 |  |
| debug/scribble.rs | `put_dec` | 595 | 18 |  |
| debug/scribble.rs | `dump_qwords` | 613 | 28 |  |
| debug/scribble.rs | `hash_of` | 641 | 5 |  |
| debug/scribble.rs | `slot_of` | 646 | 16 |  |
| debug/scribble.rs | `rd` | 662 | 6 |  |
| debug/scribble.rs | `gen_of` | 668 | 9 |  |
| debug/scribble.rs | `register` | 677 | 105 |  |
| debug/scribble.rs | `unregister` | 782 | 12 | yes |
| debug/scribble.rs | `note_thread_write` | 794 | 18 |  |
| debug/scribble.rs | `quiesce` | 812 | 48 | yes |
| debug/scribble.rs | `verify` | 860 | 253 | yes |
| debug/scribble.rs | `verify_gs` | 1113 | 59 | yes |
| debug/scribble.rs | `verify_consistency` | 1172 | 142 | yes |
| debug/scribble.rs | `slot_divergence` | 1314 | 49 | yes |
| debug/scribble.rs | `double_run` | 1363 | 101 | yes |
| debug/scribble.rs | `crash_report` | 1464 | 98 | yes |
| debug/scribble.rs | `report` | 1562 | 96 |  |
| debug/slowtask.rs | `fs_family` | 101 | 5 |  |
| debug/slowtask.rs | `tick` | 106 | 17 | yes |
| debug/slowtask.rs | `audit` | 123 | 270 |  |
| debug/slowtask.rs | `print_parked_chain` | 393 | 72 |  |
| debug/softlockup.rs | `now_ns` | 34 | 12 |  |
| debug/softlockup.rs | `init` | 46 | 12 | yes |
| debug/softlockup.rs | `touch` | 58 | 14 | yes |
| debug/softlockup.rs | `last_touch_ns` | 72 | 25 |  |
| debug/softlockup.rs | `note_irq_counts` | 97 | 23 |  |
| debug/softlockup.rs | `cpu_irq_frozen_ms` | 120 | 19 | yes |
| debug/softlockup.rs | `check` | 139 | 119 | yes |
| debug/spinwatch.rs | `on` | 41 | 8 | yes |
| debug/spinwatch.rs | `compat_arm` | 49 | 15 |  |
| debug/spinwatch.rs | `last_report_n` | 64 | 19 |  |
| debug/spinwatch.rs | `new` | 83 | 25 | yes |
| debug/spinwatch.rs | `record` | 108 | 41 | yes |
| debug/switches.rs | `name` | 172 | 9 | yes |
| debug/switches.rs | `index` | 181 | 21 | yes |
| debug/switches.rs | `from_name` | 202 | 6 |  |
| debug/switches.rs | `enabled` | 208 | 15 | yes |
| debug/switches.rs | `oh_hunt_prints_on` | 223 | 37 | yes |
| debug/switches.rs | `set` | 260 | 41 | yes |
| debug/switches.rs | `init_from_cmdline` | 301 | 28 |  |
| debug/syscost.rs | `zone_cur` | 346 | 13 |  |
| debug/syscost.rs | `seg_close` | 359 | 38 |  |
| debug/syscost.rs | `rest_site_open` | 397 | 47 |  |
| debug/syscost.rs | `irq_note_off` | 444 | 11 | yes |
| debug/syscost.rs | `irq_note_off_full` | 455 | 45 |  |
| debug/syscost.rs | `irq_note_on` | 500 | 10 | yes |
| debug/syscost.rs | `irq_note_on_full` | 510 | 29 |  |
| debug/syscost.rs | `zone_hardirq_enter` | 539 | 27 | yes |
| debug/syscost.rs | `zone_hardirq_exit` | 566 | 21 | yes |
| debug/syscost.rs | `zone_softirq_enter` | 587 | 26 | yes |
| debug/syscost.rs | `zone_softirq_exit` | 613 | 24 | yes |
| debug/syscost.rs | `zone_uart_push` | 637 | 24 |  |
| debug/syscost.rs | `zone_uart_pop` | 661 | 25 |  |
| debug/syscost.rs | `open_cli_segment` | 686 | 27 | yes |
| debug/syscost.rs | `if0_now` | 713 | 39 |  |
| debug/syscost.rs | `uart_putc_note` | 752 | 58 |  |
| debug/syscost.rs | `pr_slot` | 810 | 35 |  |
| debug/syscost.rs | `note_printk_filtered` | 845 | 8 |  |
| debug/syscost.rs | `note_printk_ring` | 853 | 15 |  |
| debug/syscost.rs | `note_printk_con` | 868 | 70 |  |
| debug/syscost.rs | `recv_describe` | 938 | 22 |  |
| debug/syscost.rs | `recv_census` | 960 | 97 |  |
| debug/syscost.rs | `note_read_fail` | 1057 | 103 |  |
| debug/syscost.rs | `boot_ms` | 1160 | 9 |  |
| debug/syscost.rs | `note_errno` | 1169 | 60 |  |
| debug/syscost.rs | `note_read_data` | 1229 | 74 |  |
| debug/syscost.rs | `on` | 1303 | 31 | yes |
| debug/syscost.rs | `note_read_eof` | 1334 | 55 |  |
| debug/syscost.rs | `note_read_eagain` | 1389 | 45 |  |
| debug/syscost.rs | `enter` | 1434 | 27 |  |
| debug/syscost.rs | `flood_account` | 1461 | 40 |  |
| debug/syscost.rs | `exit` | 1501 | 41 |  |
| debug/syscost.rs | `tick_user` | 1542 | 11 | yes |
| debug/syscost.rs | `tick_kern` | 1553 | 12 | yes |
| debug/syscost.rs | `tick_idle` | 1565 | 13 | yes |
| debug/syscost.rs | `rip_sample` | 1578 | 44 |  |
| debug/syscost.rs | `tick` | 1622 | 14 | yes |
| debug/syscost.rs | `dump` | 1636 | 654 | yes |
| debug/taint.rs | `add_taint` | 64 | 5 | yes |
| debug/taint.rs | `get_taints` | 69 | 5 |  |
| debug/taint.rs | `tainted` | 74 | 27 |  |
| debug/taint.rs | `taint_string` | 101 | 21 |  |
| debug/taint.rs | `taint_string_arr` | 122 | 5 |  |
| debug/taskdump.rs | `line_atomic_write` | 77 | 4 |  |
| debug/taskdump.rs | `putc` | 81 | 18 |  |
| debug/taskdump.rs | `putc_brief` | 99 | 14 | yes |
| debug/taskdump.rs | `staged_putc` | 113 | 36 |  |
| debug/taskdump.rs | `flush_stage` | 149 | 17 |  |
| debug/taskdump.rs | `puts` | 166 | 6 |  |
| debug/taskdump.rs | `put_hex` | 172 | 10 |  |
| debug/taskdump.rs | `put_dec` | 182 | 19 |  |
| debug/taskdump.rs | `taskdump_raw_line` | 201 | 7 | yes |
| debug/taskdump.rs | `taskdump_dec` | 208 | 5 | yes |
| debug/taskdump.rs | `dump_syscall_ring_for` | 213 | 40 |  |
| debug/taskdump.rs | `debug_task_read_u32` | 253 | 7 |  |
| debug/taskdump.rs | `debug_track_word` | 260 | 16 |  |
| debug/taskdump.rs | `debug_dump_user_stack` | 276 | 87 |  |
| debug/taskdump.rs | `bt_frame` | 363 | 22 |  |
| debug/taskdump.rs | `bt_text_window` | 385 | 12 |  |
| debug/taskdump.rs | `bt_user_window` | 397 | 4 |  |
| debug/taskdump.rs | `vw_wait_counters_line` | 401 | 11 | yes |
| debug/taskdump.rs | `dump_all_tasks` | 412 | 444 | yes |
| debug/taskdump.rs | `state_name` | 856 | 35 |  |
| debug/taskdump.rs | `policy_name` | 891 | 56 |  |
| debug/taskdump.rs | `frozen_ipi_probe` | 947 | 77 |  |
| debug/taskdump.rs | `dump_frozen_cpu_truth` | 1024 | 148 |  |
| debug/taskdump.rs | `parked_kstack_syms` | 1172 | 193 |  |
| debug/taskdump.rs | `defer_emit_line` | 1365 | 70 |  |
| debug/taskdump.rs | `ensure_defer_thread` | 1435 | 196 |  |
| debug/taskdump.rs | `sleep_snapshot_early` | 1631 | 37 | yes |
| debug/taskdump.rs | `pid1spin_audit` | 1668 | 78 | yes |
| debug/unixrd.rs | `now` | 113 | 10 | yes |
| debug/unixrd.rs | `add` | 123 | 11 | yes |
| debug/unixrd.rs | `bump` | 134 | 8 | yes |
| debug/unixrd.rs | `addb` | 142 | 11 |  |
| debug/unixrd.rs | `on` | 153 | 21 | yes |
| debug/unixrd.rs | `v` | 174 | 5 |  |
| debug/unixrd.rs | `avg` | 179 | 10 |  |
| debug/unixrd.rs | `dump` | 189 | 54 | yes |
| debug/wildwatch.rs | `arm` | 43 | 1 |  |
| debug/wildwatch.rs | `verify` | 44 | 1 | yes |
| debug/wildwatch.rs | `is_armed` | 45 | 25 | yes |
| debug/wildwatch.rs | `is_armed` | 70 | 20 | yes |
| debug/wildwatch.rs | `arm` | 90 | 96 |  |
| debug/wildwatch.rs | `enable_write_protect` | 186 | 8 | yes |
| debug/wildwatch.rs | `fill_canary` | 194 | 18 |  |
| debug/wildwatch.rs | `verify` | 212 | 63 | yes |
| debug/wildwatch.rs | `record` | 275 | 27 | yes |
| debug/wildwatch.rs | `table_virt` | 302 | 16 |  |
| debug/wildwatch.rs | `resolve_phys` | 318 | 40 |  |
| debug/wildwatch.rs | `protect_page` | 358 | 122 |  |
