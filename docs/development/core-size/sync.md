# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# sync — hot 7139 lines (stay) / cold 13393 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| sync/condvar.rs | `new` | 64 | 8 | yes |
| sync/condvar.rs | `init` | 71 | 29 | yes |
| sync/condvar.rs | `wait` | 99 | 61 | yes |
| sync/condvar.rs | `wait_interruptible` | 159 | 59 |  |
| sync/condvar.rs | `signal` | 217 | 41 |  |
| sync/condvar.rs | `broadcast` | 257 | 14 |  |
| sync/condvar.rs | `default` | 270 | 5 | yes |
| sync/edges.rs | `read_bytes_via_task` | 38 | 34 | yes |
| sync/edges.rs | `read_cstr_via_task` | 71 | 21 | yes |
| sync/edges.rs | `read_u32_via_task` | 91 | 22 | yes |
| sync/edges.rs | `word_read` | 112 | 10 | yes |
| sync/edges.rs | `word_cas` | 121 | 13 | yes |
| sync/edges.rs | `pt_walk` | 133 | 11 | yes |
| sync/edges.rs | `read_volatile_u64` | 143 | 8 | yes |
| sync/edges.rs | `read_volatile_u32` | 150 | 9 | yes |
| sync/edges.rs | `copy_from_user_bytes` | 158 | 9 | yes |
| sync/edges.rs | `word_write` | 166 | 6 | yes |
| sync/futex.rs | `fxdv_latch_devmgr` | 79 | 31 |  |
| sync/futex.rs | `fxrs_word_add` | 109 | 25 |  |
| sync/futex.rs | `fxrs_word_watched` | 133 | 58 |  |
| sync/futex.rs | `fuxw_push` | 190 | 103 |  |
| sync/futex.rs | `fxrs_latch` | 292 | 38 | yes |
| sync/futex.rs | `rwfz_exit_watch_tgid` | 329 | 80 |  |
| sync/futex.rs | `resolve_futex_phys` | 408 | 54 |  |
| sync/futex.rs | `new` | 461 | 13 | yes |
| sync/futex.rs | `vma_is_shared` | 473 | 24 |  |
| sync/futex.rs | `new_user` | 496 | 44 |  |
| sync/futex.rs | `matches` | 539 | 24 | yes |
| sync/futex.rs | `task_futex_mm_id` | 562 | 20 | yes |
| sync/futex.rs | `current_futex_mm_id` | 581 | 12 |  |
| sync/futex.rs | `ftx_trace` | 592 | 18 |  |
| sync/futex.rs | `ftx_opname` | 609 | 14 |  |
| sync/futex.rs | `ftx_trace` | 622 | 104 |  |
| sync/futex.rs | `waiter_of` | 725 | 41 |  |
| sync/futex.rs | `waiters_of_tgid` | 765 | 52 |  |
| sync/futex.rs | `ffrt_chain_walk` | 816 | 252 |  |
| sync/futex.rs | `ffrt_mutex_scan` | 1067 | 232 |  |
| sync/futex.rs | `fuxpark_latch` | 1298 | 114 |  |
| sync/futex.rs | `fuxwatch_retire_page` | 1411 | 36 |  |
| sync/futex.rs | `fuxwatch_retire_page` | 1446 | 7 |  |
| sync/futex.rs | `fuxwatch_retire_all` | 1452 | 32 |  |
| sync/futex.rs | `fuxwatch_parse` | 1483 | 56 |  |
| sync/futex.rs | `fuxwatch2_parse` | 1538 | 24 |  |
| sync/futex.rs | `fuxwatch_arm_round` | 1561 | 118 |  |
| sync/futex.rs | `fuxwatch_arm_round` | 1678 | 5 |  |
| sync/futex.rs | `fuxwatch_page_matches` | 1682 | 12 | yes |
| sync/futex.rs | `fuxwatch_mm_armed` | 1693 | 9 | yes |
| sync/futex.rs | `fuxwatch_over_budget` | 1701 | 10 |  |
| sync/futex.rs | `fuxwatch_log_write` | 1710 | 121 | yes |
| sync/futex.rs | `fuxw_word_wake_summary` | 1830 | 44 |  |
| sync/futex.rs | `fuxw_owner_park_state` | 1873 | 42 |  |
| sync/futex.rs | `fuxw_word_shape` | 1914 | 67 |  |
| sync/futex.rs | `deep_ufw` | 1980 | 75 |  |
| sync/futex.rs | `fuxwatch_read_word` | 2054 | 25 |  |
| sync/futex.rs | `fuxwatch_take_step` | 2078 | 18 | yes |
| sync/futex.rs | `fuxwatch_page_watched` | 2095 | 14 | yes |
| sync/futex.rs | `fuxwatch_read_word_at` | 2108 | 16 | yes |
| sync/futex.rs | `fuxwatch_addr` | 2123 | 6 | yes |
| sync/futex.rs | `fuxwatch2_addr` | 2128 | 13 | yes |
| sync/futex.rs | `rd_word` | 2140 | 18 |  |
| sync/futex.rs | `heap_class` | 2157 | 17 |  |
| sync/futex.rs | `ffrt_lock_region_scan` | 2173 | 278 |  |
| sync/futex.rs | `oh67_poll_decode` | 2450 | 72 |  |
| sync/futex.rs | `oh67_m2_decode` | 2521 | 229 |  |
| sync/futex.rs | `oh68_fs_base` | 2749 | 5 |  |
| sync/futex.rs | `oh68_fs_base` | 2753 | 9 |  |
| sync/futex.rs | `oh68_ctx_of` | 2761 | 41 |  |
| sync/futex.rs | `oh68_ctx_of` | 2801 | 10 |  |
| sync/futex.rs | `oh68_latch_owner_task` | 2810 | 40 |  |
| sync/futex.rs | `oh68_holder_decode` | 2849 | 17 |  |
| sync/futex.rs | `oh68_holder_decode_tgid` | 2865 | 671 |  |
| sync/futex.rs | `sqw_canon` | 3535 | 16 |  |
| sync/futex.rs | `sqwalk_step` | 3550 | 9 |  |
| sync/futex.rs | `sqwalk_step_deep` | 3558 | 84 |  |
| sync/futex.rs | `sqwalk_slice` | 3641 | 687 |  |
| sync/futex.rs | `sqwalk_resume` | 4327 | 7 |  |
| sync/futex.rs | `ffrt_mutex_heap_map_done` | 4333 | 7 |  |
| sync/futex.rs | `ffrt_mutex_heap_map_rearm` | 4339 | 16 |  |
| sync/futex.rs | `ffrt_mutex_heap_map` | 4354 | 151 |  |
| sync/futex.rs | `ffrt_lock_holder_scan` | 4504 | 151 |  |
| sync/futex.rs | `futex_stuck_census` | 4654 | 1803 | yes |
| sync/futex.rs | `alloc_waiter` | 6456 | 34 |  |
| sync/futex.rs | `free_waiter` | 6489 | 12 |  |
| sync/futex.rs | `pool_full_backoff` | 6500 | 39 |  |
| sync/futex.rs | `futex_hash` | 6538 | 41 |  |
| sync/futex.rs | `futex_wake` | 6578 | 16 |  |
| sync/futex.rs | `futex_wake_in_mm` | 6593 | 162 | yes |
| sync/futex.rs | `ph` | 6754 | 193 | yes |
| sync/futex.rs | `futex_wait` | 6946 | 6 |  |
| sync/futex.rs | `futex_wait_timeout` | 6951 | 178 |  |
| sync/futex.rs | `ph` | 7128 | 156 | yes |
| sync/futex.rs | `remove_waiter` | 7283 | 102 |  |
| sync/futex.rs | `futex_cleanup` | 7384 | 83 |  |
| sync/futex.rs | `futex_wait_bitset` | 7466 | 39 | yes |
| sync/futex.rs | `futex_parse_timeout_ns` | 7504 | 50 | yes |
| sync/futex.rs | `futex_wake_bitset` | 7553 | 6 | yes |
| sync/futex.rs | `futex_to_flags` | 7558 | 37 | yes |
| sync/futex.rs | `futex_requeue` | 7594 | 340 |  |
| sync/futex.rs | `futex_wake_op_decode` | 7933 | 23 |  |
| sync/futex.rs | `sign_extend12` | 7955 | 12 |  |
| sync/futex.rs | `futex_wake_op` | 7966 | 73 |  |
| sync/futex.rs | `debug_dump_futex_state` | 8038 | 72 |  |
| sync/futex.rs | `do_futex` | 8109 | 622 |  |
| sync/futex.rs | `sys_futex_handler` | 8730 | 817 | yes |
| sync/futex.rs | `rwfrz_tgid_frozen` | 9546 | 45 |  |
| sync/futex.rs | `note_task_unlink` | 9590 | 32 |  |
| sync/futex.rs | `exit_ring_dump_tgid` | 9621 | 33 |  |
| sync/futex.rs | `kill_dump_tgid` | 9653 | 155 |  |
| sync/futex.rs | `kd_fd_type` | 9807 | 35 |  |
| sync/futex.rs | `kill_dump_wedge` | 9841 | 279 |  |
| sync/futex.rs | `rwfz_op_log` | 10119 | 35 |  |
| sync/futex.rs | `read_park_word_via_task` | 10153 | 21 |  |
| sync/futex.rs | `rwfrz_reader_scan` | 10173 | 292 |  |
| sync/futex.rs | `musl_anchor` | 10464 | 46 |  |
| sync/futex.rs | `musl_off_class` | 10509 | 17 |  |
| sync/futex.rs | `malloc_holder_scan` | 10525 | 177 |  |
| sync/futex.rs | `ctd_decode` | 10701 | 532 |  |
| sync/futex.rs | `ctd_label` | 11232 | 44 |  |
| sync/futex.rs | `probe_file_path` | 11275 | 23 |  |
| sync/futex.rs | `apppark_rets_public` | 11297 | 9 | yes |
| sync/futex.rs | `apppark_rets` | 11305 | 91 |  |
| sync/futex.rs | `rwfz_emit_prio` | 11395 | 28 |  |
| sync/futex.rs | `rwfz_emit` | 11422 | 10 |  |
| sync/futex.rs | `rwfz_stack_frames` | 11431 | 241 |  |
| sync/futex.rs | `waiter_clusters` | 11671 | 50 |  |
| sync/futex.rs | `rwlock_frozen_shape` | 11720 | 10 |  |
| sync/futex.rs | `freeze_scan` | 11729 | 722 |  |
| sync/futex.rs | `rwfz_neighbor_dump` | 12450 | 143 |  |
| sync/futex.rs | `rwfz_dump_tgid` | 12592 | 50 | yes |
| sync/futex_pi.rs | `key_is_kernel` | 101 | 6 |  |
| sync/futex_pi.rs | `kernel_key` | 106 | 18 |  |
| sync/futex_pi.rs | `task_live_rank` | 123 | 48 |  |
| sync/futex_pi.rs | `waiter_rank` | 170 | 56 |  |
| sync/futex_pi.rs | `pi_bucket_lock` | 225 | 11 |  |
| sync/futex_pi.rs | `word_read` | 235 | 8 | yes |
| sync/futex_pi.rs | `word_cas` | 242 | 10 | yes |
| sync/futex_pi.rs | `pi_waiter_alloc` | 251 | 24 |  |
| sync/futex_pi.rs | `pi_waiter_free` | 274 | 6 |  |
| sync/futex_pi.rs | `pi_waiter_handed` | 279 | 7 |  |
| sync/futex_pi.rs | `pi_state_find` | 285 | 14 |  |
| sync/futex_pi.rs | `pi_state_get_or_create` | 298 | 22 |  |
| sync/futex_pi.rs | `pi_state_maybe_free` | 319 | 17 |  |
| sync/futex_pi.rs | `pi_queue_insert` | 335 | 51 |  |
| sync/futex_pi.rs | `pi_queue_unlink` | 385 | 54 |  |
| sync/futex_pi.rs | `pi_state_waiter_count` | 438 | 19 |  |
| sync/futex_pi.rs | `pi_restore_saved` | 456 | 40 |  |
| sync/futex_pi.rs | `pi_adjust_owner_boost` | 495 | 110 |  |
| sync/futex_pi.rs | `pi_owner_handoff` | 604 | 21 |  |
| sync/futex_pi.rs | `pi_state_key` | 624 | 7 |  |
| sync/futex_pi.rs | `pi_state_owner` | 630 | 13 |  |
| sync/futex_pi.rs | `pi_waiter_refresh_rank` | 642 | 59 |  |
| sync/futex_pi.rs | `pi_chain_walk` | 700 | 34 |  |
| sync/futex_pi.rs | `pi_blocked_state_of` | 733 | 32 |  |
| sync/futex_pi.rs | `pi_chain_hits` | 764 | 35 |  |
| sync/futex_pi.rs | `my_task` | 798 | 13 |  |
| sync/futex_pi.rs | `handed_fixup` | 810 | 3 |  |
| sync/futex_pi.rs | `death_take` | 812 | 48 |  |
| sync/futex_pi.rs | `futex_lock_pi` | 859 | 206 |  |
| sync/futex_pi.rs | `futex_trylock_pi` | 1064 | 31 |  |
| sync/futex_pi.rs | `futex_unlock_pi` | 1094 | 120 |  |
| sync/futex_pi.rs | `pi_requeue_attach` | 1213 | 26 |  |
| sync/futex_pi.rs | `pi_requeue_detach` | 1238 | 28 |  |
| sync/futex_pi.rs | `rt_waiter_enter` | 1265 | 40 |  |
| sync/futex_pi.rs | `rt_waiter_leave` | 1304 | 24 |  |
| sync/futex_pi.rs | `rt_release_handoff` | 1327 | 39 |  |
| sync/futex_pi.rs | `futex_pi_exit_cleanup` | 1365 | 128 |  |
| sync/lockkey.rs | `new` | 135 | 10 | yes |
| sync/lockkey.rs | `lock` | 144 | 13 | yes |
| sync/lockkey.rs | `try_lock` | 156 | 19 | yes |
| sync/lockkey.rs | `try_lock_claim` | 174 | 12 |  |
| sync/lockkey.rs | `lock_claim` | 185 | 11 |  |
| sync/lockkey.rs | `unlock` | 195 | 15 | yes |
| sync/lockkey.rs | `lock_nested` | 209 | 39 |  |
| sync/lockkey.rs | `new` | 247 | 10 | yes |
| sync/lockkey.rs | `lock_irqsave` | 256 | 13 | yes |
| sync/lockkey.rs | `lock_nested` | 268 | 23 |  |
| sync/lockkey.rs | `proof` | 290 | 9 |  |
| sync/lockkey.rs | `deref` | 298 | 8 | yes |
| sync/lockkey.rs | `deref_mut` | 305 | 29 | yes |
| sync/lockkey.rs | `drop` | 333 | 9 | yes |
| sync/lockkey.rs | `proof` | 341 | 9 |  |
| sync/mutex.rs | `locked_owner_alive_wait` | 76 | 99 |  |
| sync/mutex.rs | `new` | 174 | 26 | yes |
| sync/mutex.rs | `new_rt` | 199 | 23 |  |
| sync/mutex.rs | `try_acquire_or_recurse` | 221 | 31 |  |
| sync/mutex.rs | `try_acquire_or_recurse_fair` | 251 | 39 |  |
| sync/mutex.rs | `optimistic_probe` | 289 | 52 |  |
| sync/mutex.rs | `try_acquire_wake_token` | 340 | 24 |  |
| sync/mutex.rs | `lock` | 363 | 29 | yes |
| sync/mutex.rs | `lock_killable` | 391 | 5 |  |
| sync/mutex.rs | `lock_inner` | 395 | 365 |  |
| sync/mutex.rs | `rt_leave` | 759 | 14 |  |
| sync/mutex.rs | `guard` | 772 | 5 | yes |
| sync/mutex.rs | `release` | 776 | 89 |  |
| sync/mutex.rs | `release_if_owner` | 864 | 60 |  |
| sync/mutex.rs | `vw_state` | 923 | 25 |  |
| sync/mutex.rs | `debug_census_line` | 947 | 36 |  |
| sync/mutex.rs | `drop` | 982 | 5 | yes |
| sync/percpu.rs | `reentry_events` | 54 | 22 | yes |
| sync/percpu.rs | `clone` | 75 | 41 | yes |
| sync/percpu.rs | `new` | 115 | 18 | yes |
| sync/percpu.rs | `with` | 132 | 69 | yes |
| sync/percpu.rs | `with_preempt` | 200 | 65 | yes |
| sync/percpu.rs | `cpu` | 264 | 9 | yes |
| sync/percpu.rs | `deref` | 272 | 8 | yes |
| sync/percpu.rs | `deref_mut` | 279 | 8 | yes |
| sync/percpu.rs | `drop` | 286 | 14 | yes |
| sync/rcu.rs | `new` | 45 | 9 | yes |
| sync/rcu.rs | `init` | 53 | 38 | yes |
| sync/rcu.rs | `rcu_read_lock` | 90 | 9 | yes |
| sync/rcu.rs | `rcu_read_unlock` | 98 | 22 | yes |
| sync/rcu.rs | `call_rcu` | 119 | 55 |  |
| sync/rcu.rs | `rcu_note_context_switch` | 173 | 19 | yes |
| sync/rcu.rs | `synchronize_rcu` | 191 | 58 |  |
| sync/rcu.rs | `synchronize_rcu_bounded` | 248 | 17 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_checked` | 264 | 18 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_sleepy` | 281 | 26 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_pollfirst` | 306 | 24 | yes |
| sync/rcu.rs | `synchronize_rcu_forced` | 329 | 26 |  |
| sync/rcu.rs | `shared_grace_target_pub` | 354 | 5 |  |
| sync/rcu.rs | `shared_grace_target` | 358 | 22 |  |
| sync/rcu.rs | `shared_grace_done` | 379 | 8 |  |
| sync/rcu.rs | `shared_grace_done_pub` | 386 | 9 |  |
| sync/rcu.rs | `grace_waiter_hold` | 394 | 8 |  |
| sync/rcu.rs | `rcu_gen_now` | 401 | 13 |  |
| sync/rcu.rs | `rcu_grace_watermark` | 413 | 24 |  |
| sync/rcu.rs | `report_self_quiescent` | 436 | 14 |  |
| sync/rcu.rs | `grace_waiter_release` | 449 | 9 |  |
| sync/rcu.rs | `grace_lagging_cpus` | 457 | 28 |  |
| sync/rcu.rs | `synchronize_rcu_forced_sleepy` | 484 | 18 |  |
| sync/rcu.rs | `synchronize_rcu_forced_polled` | 501 | 22 |  |
| sync/rcu.rs | `wait_qs_sleepy` | 522 | 282 |  |
| sync/rcu.rs | `all_online_cpus_reported_pub` | 803 | 5 |  |
| sync/rcu.rs | `all_online_cpus_reported` | 807 | 24 |  |
| sync/rcu.rs | `cpu_quiescent` | 830 | 9 |  |
| sync/rcu.rs | `announce_and_wait` | 838 | 51 |  |
| sync/rcu.rs | `rcu_softirq_handler` | 888 | 51 |  |
| sync/rcu.rs | `init` | 938 | 21 | yes |
| sync/rcu.rs | `lock_cb_list` | 958 | 11 |  |
| sync/rcu.rs | `unlock_cb_list` | 968 | 4 |  |
| sync/rwlock.rs | `new` | 56 | 9 | yes |
| sync/rwlock.rs | `read` | 64 | 28 | yes |
| sync/rwlock.rs | `try_read` | 91 | 16 | yes |
| sync/rwlock.rs | `read_unlock` | 106 | 20 | yes |
| sync/rwlock.rs | `write` | 125 | 53 | yes |
| sync/rwlock.rs | `try_write` | 177 | 9 | yes |
| sync/rwlock.rs | `write_unlock` | 185 | 6 | yes |
| sync/rwlock.rs | `is_locked` | 190 | 9 | yes |
| sync/rwlock.rs | `has_writer` | 198 | 21 | yes |
| sync/rwlock.rs | `fmt` | 218 | 10 | yes |
| sync/rwlock.rs | `new` | 227 | 24 | yes |
| sync/rwlock.rs | `read` | 250 | 12 | yes |
| sync/rwlock.rs | `write` | 261 | 11 | yes |
| sync/rwlock.rs | `read_irqsave` | 271 | 12 | yes |
| sync/rwlock.rs | `write_irqsave` | 282 | 12 | yes |
| sync/rwlock.rs | `read_bh` | 293 | 11 | yes |
| sync/rwlock.rs | `write_bh` | 303 | 9 | yes |
| sync/rwlock.rs | `try_read` | 311 | 13 | yes |
| sync/rwlock.rs | `try_write` | 323 | 12 | yes |
| sync/rwlock.rs | `is_locked` | 334 | 10 | yes |
| sync/rwlock.rs | `into_inner` | 343 | 19 | yes |
| sync/rwlock.rs | `deref` | 361 | 9 | yes |
| sync/rwlock.rs | `drop` | 369 | 20 | yes |
| sync/rwlock.rs | `deref` | 388 | 9 | yes |
| sync/rwlock.rs | `deref_mut` | 396 | 9 | yes |
| sync/rwlock.rs | `drop` | 404 | 21 | yes |
| sync/rwlock.rs | `deref` | 424 | 9 | yes |
| sync/rwlock.rs | `drop` | 432 | 22 | yes |
| sync/rwlock.rs | `deref` | 453 | 9 | yes |
| sync/rwlock.rs | `deref_mut` | 461 | 9 | yes |
| sync/rwlock.rs | `drop` | 469 | 21 | yes |
| sync/rwlock.rs | `deref` | 489 | 9 | yes |
| sync/rwlock.rs | `drop` | 497 | 20 | yes |
| sync/rwlock.rs | `deref` | 516 | 9 | yes |
| sync/rwlock.rs | `deref_mut` | 524 | 9 | yes |
| sync/rwlock.rs | `drop` | 532 | 11 | yes |
| sync/rwlock.rs | `preempt_disable` | 542 | 12 | yes |
| sync/rwlock.rs | `preempt_disable_at` | 553 | 9 | yes |
| sync/rwlock.rs | `preempt_enable` | 561 | 9 | yes |
| sync/rwlock.rs | `irq_save` | 569 | 7 | yes |
| sync/rwlock.rs | `irq_restore` | 575 | 6 | yes |
| sync/rwlock.rs | `bh_disable` | 580 | 8 | yes |
| sync/rwlock.rs | `bh_enable` | 587 | 6 | yes |
| sync/rwsem.rs | `time_after` | 148 | 13 |  |
| sync/rwsem.rs | `stall_report_ok` | 160 | 62 |  |
| sync/rwsem.rs | `new` | 221 | 13 | yes |
| sync/rwsem.rs | `owner_value` | 233 | 7 |  |
| sync/rwsem.rs | `set_owner_writer` | 239 | 7 |  |
| sync/rwsem.rs | `clear_owner` | 245 | 10 | yes |
| sync/rwsem.rs | `set_reader_owned` | 254 | 16 |  |
| sync/rwsem.rs | `owner_state_of` | 269 | 15 |  |
| sync/rwsem.rs | `set_nonspinnable` | 283 | 22 |  |
| sync/rwsem.rs | `clear_nonspinnable` | 304 | 11 |  |
| sync/rwsem.rs | `down_read` | 314 | 16 |  |
| sync/rwsem.rs | `down_write` | 329 | 21 |  |
| sync/rwsem.rs | `try_read` | 349 | 23 | yes |
| sync/rwsem.rs | `try_write` | 371 | 17 | yes |
| sync/rwsem.rs | `up_read` | 387 | 19 |  |
| sync/rwsem.rs | `up_write` | 405 | 12 |  |
| sync/rwsem.rs | `is_locked` | 416 | 8 | yes |
| sync/rwsem.rs | `is_contended` | 423 | 9 |  |
| sync/rwsem.rs | `rwsem_wake` | 431 | 21 |  |
| sync/rwsem.rs | `mark_wake` | 451 | 92 |  |
| sync/rwsem.rs | `try_write_lock` | 542 | 66 | yes |
| sync/rwsem.rs | `can_spin_on_owner` | 607 | 23 |  |
| sync/rwsem.rs | `spin_on_owner` | 629 | 43 |  |
| sync/rwsem.rs | `try_write_lock_unqueued` | 671 | 22 |  |
| sync/rwsem.rs | `rspin_threshold_ns` | 692 | 10 |  |
| sync/rwsem.rs | `optimistic_spin` | 701 | 60 |  |
| sync/rwsem.rs | `down_read_slowpath` | 760 | 163 |  |
| sync/rwsem.rs | `cond_wake_waiter` | 922 | 20 |  |
| sync/rwsem.rs | `down_write_slowpath` | 941 | 129 |  |
| sync/rwsem.rs | `debug_state` | 1069 | 14 |  |
| sync/rwsem.rs | `name` | 1082 | 9 | yes |
| sync/rwsem.rs | `pid_hash_lookup_pinned` | 1090 | 9 | yes |
| sync/rwsem.rs | `task_put` | 1098 | 6 | yes |
| sync/rwsem.rs | `owner_pid_on_cpu` | 1103 | 13 |  |
| sync/rwsem.rs | `preempt_disable_at` | 1115 | 9 | yes |
| sync/rwsem.rs | `preempt_enable` | 1123 | 6 | yes |
| sync/rwsem.rs | `preempt_disable` | 1128 | 23 | yes |
| sync/rwsem.rs | `fmt` | 1150 | 10 | yes |
| sync/rwsem.rs | `new` | 1159 | 11 | yes |
| sync/rwsem.rs | `read` | 1169 | 9 | yes |
| sync/rwsem.rs | `write` | 1177 | 9 | yes |
| sync/rwsem.rs | `try_read` | 1185 | 12 | yes |
| sync/rwsem.rs | `try_write` | 1196 | 10 | yes |
| sync/rwsem.rs | `is_locked` | 1205 | 6 | yes |
| sync/rwsem.rs | `is_contended` | 1210 | 10 |  |
| sync/rwsem.rs | `diag` | 1219 | 20 |  |
| sync/rwsem.rs | `deref` | 1238 | 9 | yes |
| sync/rwsem.rs | `drop` | 1246 | 16 | yes |
| sync/rwsem.rs | `deref` | 1261 | 9 | yes |
| sync/rwsem.rs | `deref_mut` | 1269 | 9 | yes |
| sync/rwsem.rs | `drop` | 1277 | 5 | yes |
| sync/semaphore.rs | `cas_take` | 24 | 47 |  |
| sync/semaphore.rs | `new` | 70 | 12 | yes |
| sync/semaphore.rs | `init` | 81 | 24 | yes |
| sync/semaphore.rs | `down` | 104 | 90 | yes |
| sync/semaphore.rs | `down_interruptible` | 193 | 93 |  |
| sync/semaphore.rs | `down_trylock` | 285 | 29 |  |
| sync/semaphore.rs | `up` | 313 | 20 | yes |
| sync/semaphore.rs | `count` | 332 | 32 | yes |
| sync/semaphore.rs | `new` | 363 | 20 | yes |
| sync/semaphore.rs | `lock` | 382 | 28 | yes |
| sync/semaphore.rs | `try_lock` | 409 | 16 | yes |
| sync/semaphore.rs | `unlock` | 424 | 29 | yes |
| sync/semaphore.rs | `new` | 452 | 8 | yes |
| sync/semaphore.rs | `drop` | 459 | 24 | yes |
| sync/semaphore.rs | `guard` | 482 | 5 | yes |
| sync/seqlock.rs | `new` | 44 | 9 | yes |
| sync/seqlock.rs | `write_lock` | 52 | 20 | yes |
| sync/seqlock.rs | `write_unlock` | 71 | 7 | yes |
| sync/seqlock.rs | `try_write_lock` | 77 | 13 | yes |
| sync/seqlock.rs | `read_begin` | 89 | 8 | yes |
| sync/seqlock.rs | `read_retry` | 96 | 7 | yes |
| sync/seqlock.rs | `is_locked` | 102 | 25 | yes |
| sync/seqlock.rs | `new` | 126 | 10 | yes |
| sync/seqlock.rs | `write` | 135 | 23 | yes |
| sync/seqlock.rs | `try_write` | 157 | 19 | yes |
| sync/seqlock.rs | `read` | 175 | 13 | yes |
| sync/seqlock.rs | `read_preempt_disabled` | 187 | 7 | yes |
| sync/seqlock.rs | `is_locked` | 193 | 7 | yes |
| sync/seqlock.rs | `read_inner` | 199 | 38 | yes |
| sync/seqlock.rs | `deref` | 236 | 8 | yes |
| sync/seqlock.rs | `deref_mut` | 243 | 8 | yes |
| sync/seqlock.rs | `drop` | 250 | 16 | yes |
| sync/seqlock.rs | `preempt_disable` | 265 | 6 | yes |
| sync/seqlock.rs | `preempt_enable` | 270 | 9 | yes |
| sync/seqlock.rs | `preempt_disable_bh` | 278 | 6 | yes |
| sync/seqlock.rs | `preempt_enable_bh` | 283 | 4 | yes |
| sync/spinlock.rs | `note_acquire_evidence` | 136 | 26 | yes |
| sync/spinlock.rs | `acquire_evidence_fresh` | 161 | 16 | yes |
| sync/spinlock.rs | `caller_return_address` | 176 | 66 | yes |
| sync/spinlock.rs | `locked_word` | 241 | 6 | yes |
| sync/spinlock.rs | `new` | 246 | 18 | yes |
| sync/spinlock.rs | `record_owner` | 263 | 15 | yes |
| sync/spinlock.rs | `clear_owner` | 277 | 20 | yes |
| sync/spinlock.rs | `held_word` | 296 | 15 | yes |
| sync/spinlock.rs | `holder_cpu` | 310 | 21 | yes |
| sync/spinlock.rs | `held_by` | 330 | 7 | yes |
| sync/spinlock.rs | `record_owner` | 336 | 5 | yes |
| sync/spinlock.rs | `clear_owner` | 340 | 43 | yes |
| sync/spinlock.rs | `spin_may_preempt` | 382 | 13 | yes |
| sync/spinlock.rs | `lock` | 394 | 74 | yes |
| sync/spinlock.rs | `deadlock_warn` | 467 | 330 | yes |
| sync/spinlock.rs | `puthex` | 796 | 12 | yes |
| sync/spinlock.rs | `putdec` | 807 | 19 | yes |
| sync/spinlock.rs | `try_lock` | 825 | 17 | yes |
| sync/spinlock.rs | `unlock` | 841 | 43 | yes |
| sync/spinlock.rs | `force_unlock_terminal` | 883 | 7 | yes |
| sync/spinlock.rs | `is_locked` | 889 | 6 | yes |
| sync/spinlock.rs | `reset` | 894 | 13 | yes |
| sync/spinlock.rs | `data_offset` | 906 | 20 | yes |
| sync/spinlock.rs | `new` | 925 | 34 | yes |
| sync/spinlock.rs | `lock` | 958 | 11 | yes |
| sync/spinlock.rs | `lock_irq` | 968 | 12 | yes |
| sync/spinlock.rs | `lock_irqsave` | 979 | 13 | yes |
| sync/spinlock.rs | `lock_bh` | 991 | 9 | yes |
| sync/spinlock.rs | `try_lock` | 999 | 13 | yes |
| sync/spinlock.rs | `try_lock_irqsave` | 1011 | 14 | yes |
| sync/spinlock.rs | `is_locked` | 1024 | 9 | yes |
| sync/spinlock.rs | `get_ref` | 1032 | 10 | yes |
| sync/spinlock.rs | `get_mut_unchecked` | 1041 | 10 | yes |
| sync/spinlock.rs | `into_inner` | 1050 | 17 | yes |
| sync/spinlock.rs | `deref` | 1066 | 9 | yes |
| sync/spinlock.rs | `deref_mut` | 1074 | 9 | yes |
| sync/spinlock.rs | `drop` | 1082 | 38 | yes |
| sync/spinlock.rs | `deref` | 1119 | 9 | yes |
| sync/spinlock.rs | `deref_mut` | 1127 | 9 | yes |
| sync/spinlock.rs | `drop` | 1135 | 29 | yes |
| sync/spinlock.rs | `unlock_irqretain` | 1163 | 23 | yes |
| sync/spinlock.rs | `deref` | 1185 | 9 | yes |
| sync/spinlock.rs | `deref_mut` | 1193 | 9 | yes |
| sync/spinlock.rs | `drop` | 1201 | 11 | yes |
| sync/spinlock.rs | `preempt_disable` | 1211 | 12 | yes |
| sync/spinlock.rs | `preempt_disable_at` | 1222 | 10 | yes |
| sync/spinlock.rs | `preempt_enable` | 1231 | 10 | yes |
| sync/spinlock.rs | `irq_save` | 1240 | 9 | yes |
| sync/spinlock.rs | `irq_restore` | 1248 | 7 | yes |
| sync/spinlock.rs | `bh_disable` | 1254 | 9 | yes |
| sync/spinlock.rs | `bh_enable` | 1262 | 11 | yes |
| sync/spinlock.rs | `raw_spin_lock` | 1272 | 3 | yes |
| sync/spinlock.rs | `raw_spin_unlock` | 1274 | 3 | yes |
| sync/spinlock.rs | `raw_spin_is_locked` | 1276 | 3 | yes |
| sync/spinlock.rs | `raw_spin_trylock` | 1278 | 2 | yes |
