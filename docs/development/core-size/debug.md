# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# debug — hot 8411 lines (stay) / cold 5857 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| debug/allocwatch.rs | `slot_of` | 65 | 20 | yes |
| debug/allocwatch.rs | `new` | 84 | 20 | yes |
| debug/allocwatch.rs | `armed` | 103 | 5 | yes |
| debug/allocwatch.rs | `enabled` | 107 | 15 | yes |
| debug/allocwatch.rs | `enter_keyed` | 121 | 23 |  |
| debug/allocwatch.rs | `exit_keyed` | 143 | 15 |  |
| debug/allocwatch.rs | `guard` | 157 | 13 | yes |
| debug/allocwatch.rs | `drop` | 169 | 9 | yes |
| debug/allocwatch.rs | `stamp_live` | 177 | 21 |  |
| debug/allocwatch.rs | `stuck_report` | 197 | 26 |  |
| debug/allocwatch.rs | `tick_stuck_scan` | 222 | 47 |  |
| debug/allocwatch.rs | `tick_check` | 268 | 199 | yes |
| debug/backtrace.rs | `new` | 44 | 11 | yes |
| debug/backtrace.rs | `flush` | 54 | 11 | yes |
| debug/backtrace.rs | `drop` | 64 | 10 | yes |
| debug/backtrace.rs | `write_str` | 73 | 30 |  |
| debug/backtrace.rs | `current_frame_pointer` | 102 | 27 | yes |
| debug/backtrace.rs | `caller_return_address` | 128 | 31 | yes |
| debug/backtrace.rs | `unwind_frame` | 158 | 25 |  |
| debug/backtrace.rs | `walk_stack_trace` | 182 | 35 | yes |
| debug/backtrace.rs | `dump_stack` | 216 | 64 | yes |
| debug/backtrace.rs | `save_regs` | 279 | 48 |  |
| debug/backtrace.rs | `save_regs` | 326 | 46 |  |
| debug/backtrace.rs | `save_regs` | 371 | 30 |  |
| debug/backtrace.rs | `dump_regs_inline` | 400 | 7 | yes |
| debug/backtrace.rs | `dump_regs` | 406 | 18 |  |
| debug/backtrace.rs | `dump_csrs` | 423 | 24 |  |
| debug/backtrace.rs | `dump_csrs` | 446 | 9 |  |
| debug/backtrace.rs | `dump_csrs` | 454 | 20 |  |
| debug/bug.rs | `warn` | 26 | 39 |  |
| debug/bug.rs | `bug` | 64 | 31 |  |
| debug/bug.rs | `warn_on_once_check` | 94 | 7 |  |
| debug/cctx.rs | `now` | 134 | 11 | yes |
| debug/cctx.rs | `add` | 144 | 12 | yes |
| debug/cctx.rs | `bump` | 155 | 12 | yes |
| debug/cctx.rs | `on` | 166 | 21 | yes |
| debug/cctx.rs | `v` | 186 | 5 | yes |
| debug/cctx.rs | `avg` | 190 | 12 |  |
| debug/cctx.rs | `dump` | 201 | 82 | yes |
| debug/driftwatch.rs | `note_zero_put` | 111 | 7 | yes |
| debug/driftwatch.rs | `report` | 117 | 4 | yes |
| debug/filerd.rs | `on` | 70 | 7 | yes |
| debug/filerd.rs | `now` | 76 | 7 | yes |
| debug/filerd.rs | `charge` | 82 | 8 | yes |
| debug/filerd.rs | `note_miss` | 89 | 6 | yes |
| debug/filerd.rs | `generate` | 94 | 4 | yes |
| debug/forensics_edges.rs | `adopt_task` | 47 | 11 |  |
| debug/forensics_edges.rs | `t_pid` | 57 | 8 | yes |
| debug/forensics_edges.rs | `t_comm` | 64 | 8 |  |
| debug/forensics_edges.rs | `t_preempt_count` | 71 | 13 | yes |
| debug/forensics_edges.rs | `raw_qword` | 83 | 16 |  |
| debug/forensics_edges.rs | `raw_u32` | 98 | 12 |  |
| debug/forensics_edges.rs | `location` | 109 | 9 | yes |
| debug/forensics_edges.rs | `location_ref` | 117 | 11 |  |
| debug/forensics_edges.rs | `mark_first_refusal` | 127 | 13 | yes |
| debug/forkprof.rs | `stamp_wake` | 91 | 8 |  |
| debug/forkprof.rs | `stamp_child` | 98 | 8 | yes |
| debug/forkprof.rs | `consume_wake` | 105 | 70 | yes |
| debug/forkprof.rs | `trace_on` | 174 | 11 | yes |
| debug/forkprof.rs | `trace_on_armed` | 184 | 7 |  |
| debug/forkprof.rs | `trace` | 190 | 13 | yes |
| debug/forkprof.rs | `report_trace` | 202 | 66 |  |
| debug/forkprof.rs | `on` | 267 | 23 | yes |
| debug/forkprof.rs | `now` | 289 | 7 | yes |
| debug/forkprof.rs | `add` | 295 | 11 | yes |
| debug/forkprof.rs | `bump` | 305 | 6 | yes |
| debug/forkprof.rs | `peek` | 310 | 11 | yes |
| debug/forkprof.rs | `report` | 320 | 70 | yes |
| debug/fsw.rs | `max_update` | 208 | 17 |  |
| debug/fsw.rs | `open_exec_stamp` | 224 | 18 | yes |
| debug/fsw.rs | `open_close` | 241 | 18 | yes |
| debug/fsw.rs | `note_walk_miss_enoent` | 258 | 5 | yes |
| debug/fsw.rs | `note_neghit` | 262 | 5 | yes |
| debug/fsw.rs | `note_negstale` | 266 | 13 | yes |
| debug/fsw.rs | `negmemo_on` | 278 | 23 | yes |
| debug/fsw.rs | `pc_now` | 300 | 11 | yes |
| debug/fsw.rs | `pc_add` | 310 | 17 | yes |
| debug/fsw.rs | `on` | 326 | 37 | yes |
| debug/fsw.rs | `note_bread` | 362 | 5 | yes |
| debug/fsw.rs | `note_dev_read` | 366 | 5 | yes |
| debug/fsw.rs | `note_dev_write` | 370 | 5 | yes |
| debug/fsw.rs | `note_sync_dirty` | 374 | 5 | yes |
| debug/fsw.rs | `note_sync_dirty_skip` | 378 | 5 | yes |
| debug/fsw.rs | `note_evict` | 382 | 10 | yes |
| debug/fsw.rs | `note_sync_all` | 391 | 5 | yes |
| debug/fsw.rs | `note_pub_flush` | 395 | 8 | yes |
| debug/fsw.rs | `note_pub_scoped` | 402 | 8 | yes |
| debug/fsw.rs | `note_pub_cstr` | 409 | 5 | yes |
| debug/fsw.rs | `note_pub_hiwat` | 413 | 5 | yes |
| debug/fsw.rs | `note_pub_batch` | 417 | 6 | yes |
| debug/fsw.rs | `note_mkdir_us` | 422 | 21 | yes |
| debug/fsw.rs | `note_j_dedup` | 442 | 8 | yes |
| debug/fsw.rs | `note_drop_block` | 449 | 9 | yes |
| debug/fsw.rs | `note_rmdir_us` | 457 | 20 | yes |
| debug/fsw.rs | `note_unlink_us` | 476 | 32 | yes |
| debug/fsw.rs | `note_rmdir_phases` | 507 | 25 | yes |
| debug/fsw.rs | `note_pub_lazy` | 531 | 5 | yes |
| debug/fsw.rs | `note_pub_qmax` | 535 | 22 | yes |
| debug/fsw.rs | `note_read_inode` | 556 | 5 | yes |
| debug/fsw.rs | `note_write_inode` | 560 | 5 | yes |
| debug/fsw.rs | `note_dir_lookup` | 564 | 8 | yes |
| debug/fsw.rs | `note_list_dir` | 571 | 5 | yes |
| debug/fsw.rs | `note_new_inode` | 575 | 5 | yes |
| debug/fsw.rs | `note_alloc_run` | 579 | 5 | yes |
| debug/fsw.rs | `note_free_run` | 583 | 7 | yes |
| debug/fsw.rs | `note_j_start` | 589 | 5 | yes |
| debug/fsw.rs | `note_j_stop` | 593 | 5 | yes |
| debug/fsw.rs | `note_j_lazy` | 597 | 5 | yes |
| debug/fsw.rs | `note_j_commit_fast` | 601 | 8 | yes |
| debug/fsw.rs | `note_j_commit_slow` | 608 | 5 | yes |
| debug/fsw.rs | `note_j_revoke` | 612 | 7 | yes |
| debug/fsw.rs | `note_biglock` | 618 | 5 |  |
| debug/fsw.rs | `note_biglock_contend` | 622 | 7 |  |
| debug/fsw.rs | `note_walk` | 628 | 5 | yes |
| debug/fsw.rs | `note_walk_hit` | 632 | 5 | yes |
| debug/fsw.rs | `note_walk_miss` | 636 | 12 | yes |
| debug/fsw.rs | `tick` | 647 | 23 | yes |
| debug/fsw.rs | `dump` | 669 | 4 | yes |
| debug/hilogcensus.rs | `on` | 60 | 9 | yes |
| debug/hilogcensus.rs | `compat_arm` | 68 | 27 | yes |
| debug/hilogcensus.rs | `new` | 94 | 9 | yes |
| debug/hilogcensus.rs | `bytes` | 102 | 8 | yes |
| debug/hilogcensus.rs | `set` | 109 | 7 | yes |
| debug/hilogcensus.rs | `same` | 115 | 9 |  |
| debug/hilogcensus.rs | `set_content` | 123 | 8 |  |
| debug/hilogcensus.rs | `content_str` | 130 | 35 |  |
| debug/hilogcensus.rs | `new` | 164 | 30 | yes |
| debug/hilogcensus.rs | `new` | 193 | 17 | yes |
| debug/hilogcensus.rs | `parse_hilogmsg` | 209 | 38 |  |
| debug/hilogcensus.rs | `unix_send_pre` | 246 | 54 | yes |
| debug/hilogcensus.rs | `bump_tag` | 299 | 28 |  |
| debug/hilogcensus.rs | `unix_send_post` | 326 | 24 | yes |
| debug/hilogcensus.rs | `kmsg_write` | 349 | 26 |  |
| debug/hilogcensus.rs | `comm_str` | 374 | 16 | yes |
| debug/hilogcensus.rs | `tag_str` | 389 | 11 |  |
| debug/hilogcensus.rs | `dump` | 399 | 161 | yes |
| debug/hung_task.rs | `now_ns` | 28 | 11 |  |
| debug/hung_task.rs | `init` | 38 | 109 | yes |
| debug/hung_task.rs | `check_tasks` | 146 | 45 |  |
| debug/hung_task.rs | `report_hung_task` | 190 | 133 |  |
| debug/ksyms.rs | `table_valid` | 55 | 15 | yes |
| debug/ksyms.rs | `name_at` | 69 | 15 |  |
| debug/ksyms.rs | `symbol_name` | 83 | 9 | yes |
| debug/ksyms.rs | `lookup` | 91 | 35 | yes |
| debug/ksyms.rs | `format_symbol` | 125 | 14 | yes |
| debug/leakorigin.rs | `slot_of` | 147 | 14 | yes |
| debug/leakorigin.rs | `note_acquire` | 160 | 88 | yes |
| debug/leakorigin.rs | `phex` | 247 | 12 | yes |
| debug/leakorigin.rs | `pdec` | 258 | 68 | yes |
| debug/leakorigin.rs | `report_acquire_divergent` | 325 | 128 |  |
| debug/leakorigin.rs | `note_release` | 452 | 27 |  |
| debug/leakorigin.rs | `ph` | 478 | 151 | yes |
| debug/leakorigin.rs | `note_hardirq_acquire` | 628 | 23 | yes |
| debug/leakorigin.rs | `note_softirq_acquire` | 650 | 30 | yes |
| debug/leakorigin.rs | `note_softirq_release` | 679 | 9 |  |
| debug/leakorigin.rs | `report_hardirq_origin` | 687 | 22 | yes |
| debug/leakorigin.rs | `putd` | 708 | 51 |  |
| debug/leakorigin.rs | `report_softirq_origin` | 758 | 22 | yes |
| debug/leakorigin.rs | `putd` | 779 | 73 |  |
| debug/leakorigin.rs | `check_user_return` | 851 | 39 | yes |
| debug/leakorigin.rs | `report_parked_site` | 889 | 6 | yes |
| debug/leakorigin.rs | `pd` | 894 | 48 | yes |
| debug/leakorigin.rs | `report_incumbent_site` | 941 | 15 | yes |
| debug/leakorigin.rs | `report_outermost` | 955 | 21 | yes |
| debug/leakorigin.rs | `putd` | 975 | 78 |  |
| debug/leakorigin.rs | `emit` | 1052 | 10 | yes |
| debug/leakorigin.rs | `put_i64` | 1061 | 140 |  |
| debug/ledger.rs | `dec_buf` | 47 | 26 |  |
| debug/ledger.rs | `new` | 72 | 6 | yes |
| debug/ledger.rs | `push` | 77 | 12 | yes |
| debug/ledger.rs | `dec` | 88 | 5 |  |
| debug/ledger.rs | `emit` | 92 | 11 | yes |
| debug/ledger.rs | `compact_line` | 102 | 32 |  |
| debug/ledger.rs | `census_line` | 133 | 25 | yes |
| debug/ledger.rs | `oomfx_line` | 157 | 52 | yes |
| debug/ledger.rs | `dump` | 208 | 254 | yes |
| debug/ledger.rs | `run_pending` | 461 | 19 | yes |
| debug/ledger.rs | `series_tick` | 479 | 14 |  |
| debug/lostpark.rs | `armed` | 39 | 7 | yes |
| debug/lostpark.rs | `record` | 45 | 16 | yes |
| debug/lostpark.rs | `dump_once` | 60 | 4 | yes |
| debug/memwatch.rs | `bucket` | 76 | 12 |  |
| debug/memwatch.rs | `armed` | 87 | 49 | yes |
| debug/memwatch.rs | `kernel_va_readable` | 135 | 35 |  |
| debug/memwatch.rs | `plausible_fp` | 169 | 6 |  |
| debug/memwatch.rs | `plausible_fp` | 174 | 10 |  |
| debug/memwatch.rs | `walk_fp_chain` | 183 | 33 | yes |
| debug/memwatch.rs | `walk_fp_chain` | 215 | 22 | yes |
| debug/memwatch.rs | `walk_fp_chain` | 236 | 20 | yes |
| debug/memwatch.rs | `chain_key` | 255 | 25 |  |
| debug/memwatch.rs | `sanitized_ra` | 279 | 13 |  |
| debug/memwatch.rs | `sanitized_ra` | 291 | 8 |  |
| debug/memwatch.rs | `sanitized_ra` | 298 | 12 |  |
| debug/memwatch.rs | `note_realloc` | 309 | 30 | yes |
| debug/memwatch.rs | `note_alloc` | 338 | 143 | yes |
| debug/memwatch.rs | `live_intern` | 480 | 35 |  |
| debug/memwatch.rs | `mark_pages` | 514 | 26 |  |
| debug/memwatch.rs | `note_free` | 539 | 51 | yes |
| debug/memwatch.rs | `ppage_site_table` | 589 | 68 |  |
| debug/memwatch.rs | `psite_intern` | 656 | 29 |  |
| debug/memwatch.rs | `note_page_alloc` | 684 | 31 | yes |
| debug/memwatch.rs | `note_page_free` | 714 | 32 | yes |
| debug/memwatch.rs | `tracked_index` | 745 | 24 |  |
| debug/memwatch.rs | `put_dec` | 768 | 8 | yes |
| debug/memwatch.rs | `kroot_pollution` | 775 | 65 |  |
| debug/memwatch.rs | `put_hex` | 839 | 18 |  |
| debug/memwatch.rs | `dump_mem` | 856 | 108 | yes |
| debug/memwatch.rs | `dump_heap_ledger` | 963 | 192 |  |
| debug/memwatch.rs | `dump_page_ledger` | 1154 | 60 |  |
| debug/metawatch.rs | `tick` | 58 | 38 | yes |
| debug/metawatch.rs | `scan` | 95 | 20 |  |
| debug/metawatch.rs | `kind_str` | 114 | 16 |  |
| debug/metawatch.rs | `report` | 129 | 40 | yes |
| debug/mmforensics.rs | `note_ref` | 97 | 36 | yes |
| debug/mmforensics.rs | `current_root_ppn` | 132 | 15 |  |
| debug/mmforensics.rs | `ring_history` | 146 | 165 |  |
| debug/mmforensics.rs | `capture_frame` | 310 | 116 | yes |
| debug/mmprof.rs | `on` | 102 | 21 | yes |
| debug/mmprof.rs | `now` | 122 | 8 | yes |
| debug/mmprof.rs | `add` | 129 | 14 | yes |
| debug/mmprof.rs | `bump` | 142 | 10 | yes |
| debug/mmprof.rs | `bucket_len` | 151 | 23 | yes |
| debug/mmprof.rs | `bump_comm` | 173 | 60 | yes |
| debug/mmprof.rs | `tick` | 232 | 23 | yes |
| debug/mmprof.rs | `ensure_dump_thread` | 254 | 43 | yes |
| debug/mmprof.rs | `fmt_cell` | 296 | 13 | yes |
| debug/mmprof.rs | `stat` | 308 | 15 | yes |
| debug/mmprof.rs | `report_line` | 322 | 84 | yes |
| debug/mod.rs | `init` | 169 | 50 | yes |
| debug/mod.rs | `register_debugfs_knobs` | 218 | 171 |  |
| debug/mod.rs | `walk_read` | 388 | 13 |  |
| debug/monowatch.rs | `clocksource_name` | 63 | 23 |  |
| debug/monowatch.rs | `tick` | 85 | 46 | yes |
| debug/monowatch.rs | `report` | 130 | 20 | yes |
| debug/obs_hooks.rs | `obs_register_hooks` | 115 | 6 | yes |
| debug/obs_hooks.rs | `hooks` | 120 | 10 |  |
| debug/obs_hooks.rs | `syscost_dump` | 129 | 5 |  |
| debug/obs_hooks.rs | `fsw_dump` | 133 | 5 |  |
| debug/obs_hooks.rs | `mmprof_report_line` | 137 | 5 |  |
| debug/obs_hooks.rs | `schb_init` | 141 | 5 |  |
| debug/obs_hooks.rs | `mmprof_ensure_dump_thread` | 145 | 5 |  |
| debug/obs_hooks.rs | `spinwatch_compat_arm` | 149 | 5 |  |
| debug/obs_hooks.rs | `trace_files_register` | 153 | 5 |  |
| debug/obs_hooks.rs | `trace_files_init_from_cmdline` | 157 | 5 |  |
| debug/obs_hooks.rs | `driftwatch_report` | 161 | 5 |  |
| debug/obs_hooks.rs | `filerd_generate` | 165 | 5 |  |
| debug/obs_hooks.rs | `softlockup_report` | 169 | 5 |  |
| debug/obs_hooks.rs | `lostpark_dump_once` | 173 | 5 |  |
| debug/obs_hooks.rs | `schb_oom_anat_dump` | 177 | 5 |  |
| debug/obs_hooks.rs | `reaper_note_nameset` | 181 | 5 |  |
| debug/obs_hooks.rs | `reaper_on_zombie` | 185 | 12 |  |
| debug/obs_hooks.rs | `reaper_on_reap_wait4` | 196 | 5 |  |
| debug/obs_hooks.rs | `reaper_on_reap_waitid` | 200 | 25 |  |
| debug/obs_hooks.rs | `reaper_poll_note` | 224 | 4 |  |
| debug/oops.rs | `init_from_cmdline` | 84 | 10 | yes |
| debug/oops.rs | `panic_on_oops` | 93 | 6 | yes |
| debug/oops.rs | `set_panic_on_oops` | 98 | 13 | yes |
| debug/oops.rs | `oops` | 110 | 151 | yes |
| debug/oops.rs | `comm_str` | 260 | 10 | yes |
| debug/pathstash.rs | `stash_open` | 57 | 10 |  |
| debug/pathstash.rs | `stash_file` | 66 | 30 | yes |
| debug/pathstash.rs | `stash_file_lazy` | 95 | 11 | yes |
| debug/pathstash.rs | `lookup_file` | 105 | 20 |  |
| debug/pathstash.rs | `lookup_open` | 124 | 4 |  |
| debug/pstore.rs | `new` | 115 | 13 | yes |
| debug/pstore.rs | `reserve` | 127 | 18 | yes |
| debug/pstore.rs | `reserve_at` | 144 | 11 |  |
| debug/pstore.rs | `region_virt` | 154 | 15 |  |
| debug/pstore.rs | `early_scan` | 168 | 36 |  |
| debug/pstore.rs | `record_name` | 203 | 10 | yes |
| debug/pstore.rs | `has_record` | 212 | 6 | yes |
| debug/pstore.rs | `read_record` | 217 | 25 |  |
| debug/pstore.rs | `arm_sink` | 241 | 7 |  |
| debug/pstore.rs | `sink` | 247 | 18 |  |
| debug/pstore.rs | `panic_capture` | 264 | 16 |  |
| debug/pstore.rs | `oops_capture` | 279 | 10 |  |
| debug/pstore.rs | `write_record` | 288 | 74 |  |
| debug/pstore.rs | `write_str` | 361 | 9 |  |
| debug/reaper.rs | `on` | 105 | 7 | yes |
| debug/reaper.rs | `secs` | 111 | 10 | yes |
| debug/reaper.rs | `decode_kernel_code` | 120 | 10 | yes |
| debug/reaper.rs | `decode_wstatus` | 129 | 15 | yes |
| debug/reaper.rs | `decode_si_code` | 143 | 13 | yes |
| debug/reaper.rs | `comm_str` | 155 | 8 | yes |
| debug/reaper.rs | `note_nameset` | 162 | 8 | yes |
| debug/reaper.rs | `on_zombie` | 169 | 8 | yes |
| debug/reaper.rs | `on_reap_wait4` | 176 | 8 | yes |
| debug/reaper.rs | `on_reap_waitid` | 183 | 27 | yes |
| debug/reaper.rs | `poll_note` | 209 | 8 | yes |
| debug/reaper.rs | `zombie_scan` | 216 | 113 | yes |
| debug/schb.rs | `note_proc_open` | 107 | 34 |  |
| debug/schb.rs | `note_cow` | 140 | 11 | yes |
| debug/schb.rs | `note_drct` | 150 | 12 | yes |
| debug/schb.rs | `note_slow` | 161 | 9 | yes |
| debug/schb.rs | `pace_snapshot` | 169 | 17 | yes |
| debug/schb.rs | `oom_anat_dump` | 185 | 76 | yes |
| debug/schb.rs | `init` | 260 | 31 | yes |
| debug/schb.rs | `comm_str` | 290 | 230 | yes |
| debug/schb.rs | `wedge_census` | 519 | 126 | yes |
| debug/scribble.rs | `ring_log` | 139 | 15 | yes |
| debug/scribble.rs | `ring_dump` | 153 | 68 | yes |
| debug/scribble.rs | `audit_frames` | 220 | 28 | yes |
| debug/scribble.rs | `audit_frames` | 247 | 6 | yes |
| debug/scribble.rs | `audit_record` | 252 | 21 | yes |
| debug/scribble.rs | `audit_drain` | 272 | 57 |  |
| debug/scribble.rs | `parse_hex_u64` | 328 | 19 |  |
| debug/scribble.rs | `pin_park_check` | 346 | 37 |  |
| debug/scribble.rs | `task_plausible` | 382 | 45 | yes |
| debug/scribble.rs | `double_seed` | 426 | 56 | yes |
| debug/scribble.rs | `bad_switch_arg` | 481 | 41 | yes |
| debug/scribble.rs | `switch_window_heal` | 521 | 43 | yes |
| debug/scribble.rs | `putc` | 563 | 17 |  |
| debug/scribble.rs | `puts` | 579 | 7 |  |
| debug/scribble.rs | `put_hex` | 585 | 11 |  |
| debug/scribble.rs | `put_dec` | 595 | 19 | yes |
| debug/scribble.rs | `dump_qwords` | 613 | 29 |  |
| debug/scribble.rs | `hash_of` | 641 | 6 |  |
| debug/scribble.rs | `slot_of` | 646 | 17 | yes |
| debug/scribble.rs | `rd` | 662 | 7 | yes |
| debug/scribble.rs | `gen_of` | 668 | 10 |  |
| debug/scribble.rs | `register` | 677 | 106 | yes |
| debug/scribble.rs | `unregister` | 782 | 13 | yes |
| debug/scribble.rs | `note_thread_write` | 794 | 19 |  |
| debug/scribble.rs | `quiesce` | 812 | 49 | yes |
| debug/scribble.rs | `verify` | 860 | 254 | yes |
| debug/scribble.rs | `verify_gs` | 1113 | 60 | yes |
| debug/scribble.rs | `verify_consistency` | 1172 | 143 | yes |
| debug/scribble.rs | `slot_divergence` | 1314 | 50 | yes |
| debug/scribble.rs | `double_run` | 1363 | 102 | yes |
| debug/scribble.rs | `crash_report` | 1464 | 99 | yes |
| debug/scribble.rs | `report` | 1562 | 97 | yes |
| debug/slowtask.rs | `fs_family` | 101 | 6 |  |
| debug/slowtask.rs | `tick` | 106 | 18 | yes |
| debug/slowtask.rs | `audit` | 123 | 271 |  |
| debug/slowtask.rs | `print_parked_chain` | 393 | 73 |  |
| debug/softlockup.rs | `now_ns` | 30 | 13 |  |
| debug/softlockup.rs | `init` | 42 | 13 | yes |
| debug/softlockup.rs | `touch` | 54 | 15 | yes |
| debug/softlockup.rs | `last_touch_ns` | 68 | 26 | yes |
| debug/softlockup.rs | `note_irq_counts` | 93 | 24 |  |
| debug/softlockup.rs | `cpu_irq_frozen_ms` | 116 | 20 | yes |
| debug/softlockup.rs | `check` | 135 | 75 | yes |
| debug/softlockup.rs | `report` | 209 | 4 | yes |
| debug/spinwatch.rs | `on` | 41 | 6 | yes |
| debug/spinwatch.rs | `compat_arm` | 46 | 7 | yes |
| debug/spinwatch.rs | `last_report_n` | 52 | 20 | yes |
| debug/spinwatch.rs | `new` | 71 | 26 | yes |
| debug/spinwatch.rs | `record` | 96 | 42 | yes |
| debug/switches.rs | `name` | 172 | 10 | yes |
| debug/switches.rs | `index` | 181 | 22 | yes |
| debug/switches.rs | `from_name` | 202 | 7 | yes |
| debug/switches.rs | `enabled` | 208 | 16 | yes |
| debug/switches.rs | `oh_hunt_prints_on` | 223 | 38 | yes |
| debug/switches.rs | `set` | 260 | 42 | yes |
| debug/switches.rs | `init_from_cmdline` | 301 | 29 | yes |
| debug/syscost.rs | `zone_cur` | 346 | 14 | yes |
| debug/syscost.rs | `seg_close` | 359 | 39 |  |
| debug/syscost.rs | `rest_site_open` | 397 | 48 |  |
| debug/syscost.rs | `irq_note_off` | 444 | 12 | yes |
| debug/syscost.rs | `irq_note_off_full` | 455 | 46 |  |
| debug/syscost.rs | `irq_note_on` | 500 | 11 | yes |
| debug/syscost.rs | `irq_note_on_full` | 510 | 30 |  |
| debug/syscost.rs | `zone_hardirq_enter` | 539 | 28 | yes |
| debug/syscost.rs | `zone_hardirq_exit` | 566 | 22 | yes |
| debug/syscost.rs | `zone_softirq_enter` | 587 | 27 | yes |
| debug/syscost.rs | `zone_softirq_exit` | 613 | 25 | yes |
| debug/syscost.rs | `zone_uart_push` | 637 | 25 |  |
| debug/syscost.rs | `zone_uart_pop` | 661 | 26 |  |
| debug/syscost.rs | `open_cli_segment` | 686 | 28 | yes |
| debug/syscost.rs | `if0_now` | 713 | 40 |  |
| debug/syscost.rs | `uart_putc_note` | 752 | 59 |  |
| debug/syscost.rs | `pr_slot` | 810 | 36 |  |
| debug/syscost.rs | `note_printk_filtered` | 845 | 9 |  |
| debug/syscost.rs | `note_printk_ring` | 853 | 16 |  |
| debug/syscost.rs | `note_printk_con` | 868 | 71 |  |
| debug/syscost.rs | `recv_describe` | 938 | 23 |  |
| debug/syscost.rs | `recv_census` | 960 | 98 |  |
| debug/syscost.rs | `note_read_fail` | 1057 | 104 | yes |
| debug/syscost.rs | `boot_ms` | 1160 | 10 |  |
| debug/syscost.rs | `note_errno` | 1169 | 61 |  |
| debug/syscost.rs | `note_read_data` | 1229 | 75 | yes |
| debug/syscost.rs | `on` | 1303 | 32 | yes |
| debug/syscost.rs | `note_read_eof` | 1334 | 56 | yes |
| debug/syscost.rs | `note_read_eagain` | 1389 | 46 | yes |
| debug/syscost.rs | `enter` | 1434 | 28 |  |
| debug/syscost.rs | `flood_account` | 1461 | 41 |  |
| debug/syscost.rs | `exit` | 1501 | 42 |  |
| debug/syscost.rs | `tick_user` | 1542 | 12 | yes |
| debug/syscost.rs | `tick_kern` | 1553 | 13 | yes |
| debug/syscost.rs | `tick_idle` | 1565 | 14 | yes |
| debug/syscost.rs | `rip_sample` | 1578 | 45 |  |
| debug/syscost.rs | `tick` | 1622 | 15 | yes |
| debug/taint.rs | `add_taint` | 64 | 6 | yes |
| debug/taint.rs | `get_taints` | 69 | 6 | yes |
| debug/taint.rs | `tainted` | 74 | 28 |  |
| debug/taint.rs | `taint_string` | 101 | 22 |  |
| debug/taint.rs | `taint_string_arr` | 122 | 6 |  |
| debug/taskdump.rs | `line_atomic_write` | 77 | 5 |  |
| debug/taskdump.rs | `putc` | 81 | 19 |  |
| debug/taskdump.rs | `putc_brief` | 99 | 15 | yes |
| debug/taskdump.rs | `staged_putc` | 113 | 37 |  |
| debug/taskdump.rs | `flush_stage` | 149 | 18 |  |
| debug/taskdump.rs | `puts` | 166 | 7 |  |
| debug/taskdump.rs | `put_hex` | 172 | 11 |  |
| debug/taskdump.rs | `put_dec` | 182 | 20 | yes |
| debug/taskdump.rs | `taskdump_raw_line` | 201 | 8 | yes |
| debug/taskdump.rs | `taskdump_dec` | 208 | 6 | yes |
| debug/taskdump.rs | `dump_syscall_ring_for` | 213 | 41 | yes |
| debug/taskdump.rs | `debug_task_read_u32` | 253 | 8 |  |
| debug/taskdump.rs | `debug_track_word` | 260 | 17 |  |
| debug/taskdump.rs | `debug_dump_user_stack` | 276 | 88 |  |
| debug/taskdump.rs | `bt_frame` | 363 | 23 |  |
| debug/taskdump.rs | `bt_text_window` | 385 | 13 |  |
| debug/taskdump.rs | `bt_user_window` | 397 | 5 |  |
| debug/taskdump.rs | `vw_wait_counters_line` | 401 | 12 | yes |
| debug/taskdump.rs | `dump_all_tasks` | 412 | 445 | yes |
| debug/taskdump.rs | `state_name` | 856 | 36 |  |
| debug/taskdump.rs | `policy_name` | 891 | 57 |  |
| debug/taskdump.rs | `frozen_ipi_probe` | 947 | 78 |  |
| debug/taskdump.rs | `dump_frozen_cpu_truth` | 1024 | 149 |  |
| debug/taskdump.rs | `parked_kstack_syms` | 1172 | 194 |  |
| debug/taskdump.rs | `defer_emit_line` | 1365 | 71 |  |
| debug/taskdump.rs | `ensure_defer_thread` | 1435 | 197 |  |
| debug/taskdump.rs | `sleep_snapshot_early` | 1631 | 38 | yes |
| debug/taskdump.rs | `pid1spin_audit` | 1668 | 79 | yes |
| debug/unixrd.rs | `now` | 113 | 11 | yes |
| debug/unixrd.rs | `add` | 123 | 12 | yes |
| debug/unixrd.rs | `bump` | 134 | 9 | yes |
| debug/unixrd.rs | `addb` | 142 | 12 | yes |
| debug/unixrd.rs | `on` | 153 | 22 | yes |
| debug/unixrd.rs | `v` | 174 | 6 | yes |
| debug/unixrd.rs | `avg` | 179 | 11 |  |
| debug/unixrd.rs | `dump` | 189 | 55 | yes |
| debug/wildwatch.rs | `arm` | 43 | 2 |  |
| debug/wildwatch.rs | `verify` | 44 | 2 | yes |
| debug/wildwatch.rs | `is_armed` | 45 | 26 | yes |
| debug/wildwatch.rs | `is_armed` | 70 | 21 | yes |
| debug/wildwatch.rs | `arm` | 90 | 97 |  |
| debug/wildwatch.rs | `enable_write_protect` | 186 | 9 |  |
| debug/wildwatch.rs | `fill_canary` | 194 | 19 |  |
| debug/wildwatch.rs | `verify` | 212 | 64 | yes |
| debug/wildwatch.rs | `record` | 275 | 28 | yes |
| debug/wildwatch.rs | `table_virt` | 302 | 17 |  |
| debug/wildwatch.rs | `resolve_phys` | 318 | 41 |  |
| debug/wildwatch.rs | `protect_page` | 358 | 123 |  |
