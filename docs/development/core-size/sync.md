# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# sync — hot 5711 lines (stay) / cold 14396 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| sync/condvar.rs | `new` | 64 | 7 | yes |
| sync/condvar.rs | `init` | 71 | 28 | yes |
| sync/condvar.rs | `wait` | 99 | 60 | yes |
| sync/condvar.rs | `wait_interruptible` | 159 | 58 |  |
| sync/condvar.rs | `signal` | 217 | 40 |  |
| sync/condvar.rs | `broadcast` | 257 | 13 |  |
| sync/condvar.rs | `default` | 270 | 4 | yes |
| sync/edges.rs | `read_bytes_via_task` | 38 | 33 | yes |
| sync/edges.rs | `read_cstr_via_task` | 71 | 20 | yes |
| sync/edges.rs | `read_u32_via_task` | 91 | 21 | yes |
| sync/edges.rs | `word_read` | 112 | 9 | yes |
| sync/edges.rs | `word_cas` | 121 | 12 | yes |
| sync/edges.rs | `pt_walk` | 133 | 10 | yes |
| sync/edges.rs | `read_volatile_u64` | 143 | 7 | yes |
| sync/edges.rs | `read_volatile_u32` | 150 | 8 | yes |
| sync/edges.rs | `copy_from_user_bytes` | 158 | 8 | yes |
| sync/edges.rs | `word_write` | 166 | 5 | yes |
| sync/futex.rs | `fxdv_latch_devmgr` | 79 | 30 |  |
| sync/futex.rs | `fxrs_word_add` | 109 | 24 |  |
| sync/futex.rs | `fxrs_word_watched` | 133 | 57 |  |
| sync/futex.rs | `fuxw_push` | 190 | 102 |  |
| sync/futex.rs | `fxrs_latch` | 292 | 37 |  |
| sync/futex.rs | `rwfz_exit_watch_tgid` | 329 | 79 |  |
| sync/futex.rs | `resolve_futex_phys` | 408 | 53 |  |
| sync/futex.rs | `new` | 461 | 12 | yes |
| sync/futex.rs | `vma_is_shared` | 473 | 23 |  |
| sync/futex.rs | `new_user` | 496 | 43 | yes |
| sync/futex.rs | `matches` | 539 | 23 |  |
| sync/futex.rs | `task_futex_mm_id` | 562 | 19 |  |
| sync/futex.rs | `current_futex_mm_id` | 581 | 11 |  |
| sync/futex.rs | `ftx_trace` | 592 | 17 |  |
| sync/futex.rs | `ftx_opname` | 609 | 13 |  |
| sync/futex.rs | `ftx_trace` | 622 | 103 |  |
| sync/futex.rs | `waiter_of` | 725 | 40 |  |
| sync/futex.rs | `waiters_of_tgid` | 765 | 51 |  |
| sync/futex.rs | `ffrt_chain_walk` | 816 | 251 |  |
| sync/futex.rs | `ffrt_mutex_scan` | 1067 | 231 |  |
| sync/futex.rs | `fuxpark_latch` | 1298 | 113 |  |
| sync/futex.rs | `fuxwatch_retire_page` | 1411 | 35 |  |
| sync/futex.rs | `fuxwatch_retire_page` | 1446 | 6 |  |
| sync/futex.rs | `fuxwatch_retire_all` | 1452 | 31 |  |
| sync/futex.rs | `fuxwatch_parse` | 1483 | 55 |  |
| sync/futex.rs | `fuxwatch2_parse` | 1538 | 23 |  |
| sync/futex.rs | `fuxwatch_arm_round` | 1561 | 117 |  |
| sync/futex.rs | `fuxwatch_arm_round` | 1678 | 4 |  |
| sync/futex.rs | `fuxwatch_page_matches` | 1682 | 11 | yes |
| sync/futex.rs | `fuxwatch_mm_armed` | 1693 | 8 | yes |
| sync/futex.rs | `fuxwatch_over_budget` | 1701 | 9 |  |
| sync/futex.rs | `fuxwatch_log_write` | 1710 | 120 | yes |
| sync/futex.rs | `fuxw_word_wake_summary` | 1830 | 43 |  |
| sync/futex.rs | `fuxw_owner_park_state` | 1873 | 41 |  |
| sync/futex.rs | `fuxw_word_shape` | 1914 | 66 |  |
| sync/futex.rs | `deep_ufw` | 1980 | 74 |  |
| sync/futex.rs | `fuxwatch_read_word` | 2054 | 24 |  |
| sync/futex.rs | `fuxwatch_take_step` | 2078 | 17 | yes |
| sync/futex.rs | `fuxwatch_page_watched` | 2095 | 13 | yes |
| sync/futex.rs | `fuxwatch_read_word_at` | 2108 | 15 | yes |
| sync/futex.rs | `fuxwatch_addr` | 2123 | 5 | yes |
| sync/futex.rs | `fuxwatch2_addr` | 2128 | 12 | yes |
| sync/futex.rs | `rd_word` | 2140 | 17 |  |
| sync/futex.rs | `heap_class` | 2157 | 16 |  |
| sync/futex.rs | `ffrt_lock_region_scan` | 2173 | 277 |  |
| sync/futex.rs | `oh67_poll_decode` | 2450 | 71 |  |
| sync/futex.rs | `oh67_m2_decode` | 2521 | 228 |  |
| sync/futex.rs | `oh68_fs_base` | 2749 | 4 |  |
| sync/futex.rs | `oh68_fs_base` | 2753 | 8 |  |
| sync/futex.rs | `oh68_ctx_of` | 2761 | 40 |  |
| sync/futex.rs | `oh68_ctx_of` | 2801 | 9 |  |
| sync/futex.rs | `oh68_latch_owner_task` | 2810 | 39 |  |
| sync/futex.rs | `oh68_holder_decode` | 2849 | 16 |  |
| sync/futex.rs | `oh68_holder_decode_tgid` | 2865 | 670 |  |
| sync/futex.rs | `sqw_canon` | 3535 | 15 |  |
| sync/futex.rs | `sqwalk_step` | 3550 | 8 |  |
| sync/futex.rs | `sqwalk_step_deep` | 3558 | 83 |  |
| sync/futex.rs | `sqwalk_slice` | 3641 | 686 |  |
| sync/futex.rs | `sqwalk_resume` | 4327 | 6 |  |
| sync/futex.rs | `ffrt_mutex_heap_map_done` | 4333 | 6 |  |
| sync/futex.rs | `ffrt_mutex_heap_map_rearm` | 4339 | 15 |  |
| sync/futex.rs | `ffrt_mutex_heap_map` | 4354 | 150 |  |
| sync/futex.rs | `ffrt_lock_holder_scan` | 4504 | 150 |  |
| sync/futex.rs | `futex_stuck_census` | 4654 | 1802 | yes |
| sync/futex.rs | `alloc_waiter` | 6456 | 33 |  |
| sync/futex.rs | `free_waiter` | 6489 | 11 |  |
| sync/futex.rs | `pool_full_backoff` | 6500 | 38 |  |
| sync/futex.rs | `futex_hash` | 6538 | 40 |  |
| sync/futex.rs | `futex_wake` | 6578 | 15 |  |
| sync/futex.rs | `futex_wake_in_mm` | 6593 | 161 |  |
| sync/futex.rs | `ph` | 6754 | 192 | yes |
| sync/futex.rs | `futex_wait` | 6946 | 5 |  |
| sync/futex.rs | `futex_wait_timeout` | 6951 | 177 |  |
| sync/futex.rs | `ph` | 7128 | 155 | yes |
| sync/futex.rs | `remove_waiter` | 7283 | 101 |  |
| sync/futex.rs | `futex_cleanup` | 7384 | 82 |  |
| sync/futex.rs | `futex_wait_bitset` | 7466 | 38 |  |
| sync/futex.rs | `futex_parse_timeout_ns` | 7504 | 49 |  |
| sync/futex.rs | `futex_wake_bitset` | 7553 | 5 |  |
| sync/futex.rs | `futex_to_flags` | 7558 | 36 |  |
| sync/futex.rs | `futex_requeue` | 7594 | 339 |  |
| sync/futex.rs | `futex_wake_op_decode` | 7933 | 22 |  |
| sync/futex.rs | `sign_extend12` | 7955 | 11 |  |
| sync/futex.rs | `futex_wake_op` | 7966 | 72 |  |
| sync/futex.rs | `debug_dump_futex_state` | 8038 | 71 |  |
| sync/futex.rs | `do_futex` | 8109 | 621 |  |
| sync/futex.rs | `sys_futex_handler` | 8730 | 816 |  |
| sync/futex.rs | `rwfrz_tgid_frozen` | 9546 | 44 |  |
| sync/futex.rs | `note_task_unlink` | 9590 | 31 |  |
| sync/futex.rs | `exit_ring_dump_tgid` | 9621 | 32 |  |
| sync/futex.rs | `kill_dump_tgid` | 9653 | 154 |  |
| sync/futex.rs | `kd_fd_type` | 9807 | 34 |  |
| sync/futex.rs | `kill_dump_wedge` | 9841 | 278 |  |
| sync/futex.rs | `rwfz_op_log` | 10119 | 34 |  |
| sync/futex.rs | `read_park_word_via_task` | 10153 | 20 |  |
| sync/futex.rs | `rwfrz_reader_scan` | 10173 | 291 |  |
| sync/futex.rs | `musl_anchor` | 10464 | 45 |  |
| sync/futex.rs | `musl_off_class` | 10509 | 16 |  |
| sync/futex.rs | `malloc_holder_scan` | 10525 | 176 |  |
| sync/futex.rs | `ctd_decode` | 10701 | 531 |  |
| sync/futex.rs | `ctd_label` | 11232 | 43 |  |
| sync/futex.rs | `probe_file_path` | 11275 | 22 |  |
| sync/futex.rs | `apppark_rets_public` | 11297 | 8 | yes |
| sync/futex.rs | `apppark_rets` | 11305 | 90 | yes |
| sync/futex.rs | `rwfz_emit_prio` | 11395 | 27 |  |
| sync/futex.rs | `rwfz_emit` | 11422 | 9 |  |
| sync/futex.rs | `rwfz_stack_frames` | 11431 | 240 |  |
| sync/futex.rs | `waiter_clusters` | 11671 | 49 |  |
| sync/futex.rs | `rwlock_frozen_shape` | 11720 | 9 |  |
| sync/futex.rs | `freeze_scan` | 11729 | 721 |  |
| sync/futex.rs | `rwfz_neighbor_dump` | 12450 | 142 |  |
| sync/futex.rs | `rwfz_dump_tgid` | 12592 | 49 |  |
| sync/futex_pi.rs | `key_is_kernel` | 101 | 5 |  |
| sync/futex_pi.rs | `kernel_key` | 106 | 17 |  |
| sync/futex_pi.rs | `task_live_rank` | 123 | 47 |  |
| sync/futex_pi.rs | `waiter_rank` | 170 | 55 |  |
| sync/futex_pi.rs | `pi_bucket_lock` | 225 | 10 |  |
| sync/futex_pi.rs | `word_read` | 235 | 7 | yes |
| sync/futex_pi.rs | `word_cas` | 242 | 9 | yes |
| sync/futex_pi.rs | `pi_waiter_alloc` | 251 | 23 |  |
| sync/futex_pi.rs | `pi_waiter_free` | 274 | 5 |  |
| sync/futex_pi.rs | `pi_waiter_handed` | 279 | 6 |  |
| sync/futex_pi.rs | `pi_state_find` | 285 | 13 |  |
| sync/futex_pi.rs | `pi_state_get_or_create` | 298 | 21 |  |
| sync/futex_pi.rs | `pi_state_maybe_free` | 319 | 16 |  |
| sync/futex_pi.rs | `pi_queue_insert` | 335 | 50 |  |
| sync/futex_pi.rs | `pi_queue_unlink` | 385 | 53 |  |
| sync/futex_pi.rs | `pi_state_waiter_count` | 438 | 18 |  |
| sync/futex_pi.rs | `pi_restore_saved` | 456 | 39 |  |
| sync/futex_pi.rs | `pi_adjust_owner_boost` | 495 | 109 |  |
| sync/futex_pi.rs | `pi_owner_handoff` | 604 | 20 |  |
| sync/futex_pi.rs | `pi_state_key` | 624 | 6 |  |
| sync/futex_pi.rs | `pi_state_owner` | 630 | 12 |  |
| sync/futex_pi.rs | `pi_waiter_refresh_rank` | 642 | 58 |  |
| sync/futex_pi.rs | `pi_chain_walk` | 700 | 33 |  |
| sync/futex_pi.rs | `pi_blocked_state_of` | 733 | 31 |  |
| sync/futex_pi.rs | `pi_chain_hits` | 764 | 34 |  |
| sync/futex_pi.rs | `my_task` | 798 | 12 |  |
| sync/futex_pi.rs | `handed_fixup` | 810 | 2 |  |
| sync/futex_pi.rs | `death_take` | 812 | 47 |  |
| sync/futex_pi.rs | `futex_lock_pi` | 859 | 205 |  |
| sync/futex_pi.rs | `futex_trylock_pi` | 1064 | 30 |  |
| sync/futex_pi.rs | `futex_unlock_pi` | 1094 | 119 |  |
| sync/futex_pi.rs | `pi_requeue_attach` | 1213 | 25 |  |
| sync/futex_pi.rs | `pi_requeue_detach` | 1238 | 27 |  |
| sync/futex_pi.rs | `rt_waiter_enter` | 1265 | 39 |  |
| sync/futex_pi.rs | `rt_waiter_leave` | 1304 | 23 |  |
| sync/futex_pi.rs | `rt_release_handoff` | 1327 | 38 |  |
| sync/futex_pi.rs | `futex_pi_exit_cleanup` | 1365 | 127 |  |
| sync/lockkey.rs | `new` | 135 | 9 | yes |
| sync/lockkey.rs | `lock` | 144 | 12 | yes |
| sync/lockkey.rs | `try_lock` | 156 | 18 | yes |
| sync/lockkey.rs | `try_lock_claim` | 174 | 11 |  |
| sync/lockkey.rs | `lock_claim` | 185 | 10 |  |
| sync/lockkey.rs | `unlock` | 195 | 14 | yes |
| sync/lockkey.rs | `lock_nested` | 209 | 38 |  |
| sync/lockkey.rs | `new` | 247 | 9 | yes |
| sync/lockkey.rs | `lock_irqsave` | 256 | 12 | yes |
| sync/lockkey.rs | `lock_nested` | 268 | 22 |  |
| sync/lockkey.rs | `proof` | 290 | 8 |  |
| sync/lockkey.rs | `deref` | 298 | 7 | yes |
| sync/lockkey.rs | `deref_mut` | 305 | 28 | yes |
| sync/lockkey.rs | `drop` | 333 | 8 | yes |
| sync/lockkey.rs | `proof` | 341 | 8 |  |
| sync/mutex.rs | `locked_owner_alive_wait` | 76 | 98 |  |
| sync/mutex.rs | `new` | 174 | 25 | yes |
| sync/mutex.rs | `new_rt` | 199 | 22 | yes |
| sync/mutex.rs | `try_acquire_or_recurse` | 221 | 30 |  |
| sync/mutex.rs | `try_acquire_or_recurse_fair` | 251 | 38 |  |
| sync/mutex.rs | `optimistic_probe` | 289 | 51 |  |
| sync/mutex.rs | `try_acquire_wake_token` | 340 | 23 |  |
| sync/mutex.rs | `lock` | 363 | 28 | yes |
| sync/mutex.rs | `lock_killable` | 391 | 4 | yes |
| sync/mutex.rs | `lock_inner` | 395 | 364 |  |
| sync/mutex.rs | `rt_leave` | 759 | 13 |  |
| sync/mutex.rs | `guard` | 772 | 4 | yes |
| sync/mutex.rs | `release` | 776 | 88 |  |
| sync/mutex.rs | `release_if_owner` | 864 | 59 |  |
| sync/mutex.rs | `vw_state` | 923 | 24 |  |
| sync/mutex.rs | `debug_census_line` | 947 | 35 |  |
| sync/mutex.rs | `drop` | 982 | 4 | yes |
| sync/percpu.rs | `reentry_events` | 54 | 21 | yes |
| sync/percpu.rs | `clone` | 75 | 40 | yes |
| sync/percpu.rs | `new` | 115 | 17 | yes |
| sync/percpu.rs | `with` | 132 | 68 | yes |
| sync/percpu.rs | `with_preempt` | 200 | 64 | yes |
| sync/percpu.rs | `cpu` | 264 | 8 | yes |
| sync/percpu.rs | `deref` | 272 | 7 | yes |
| sync/percpu.rs | `deref_mut` | 279 | 7 | yes |
| sync/percpu.rs | `drop` | 286 | 13 | yes |
| sync/rcu.rs | `new` | 45 | 8 | yes |
| sync/rcu.rs | `init` | 53 | 37 | yes |
| sync/rcu.rs | `rcu_read_lock` | 90 | 8 | yes |
| sync/rcu.rs | `rcu_read_unlock` | 98 | 21 | yes |
| sync/rcu.rs | `call_rcu` | 119 | 54 |  |
| sync/rcu.rs | `rcu_note_context_switch` | 173 | 18 | yes |
| sync/rcu.rs | `synchronize_rcu` | 191 | 57 |  |
| sync/rcu.rs | `synchronize_rcu_bounded` | 248 | 16 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_checked` | 264 | 17 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_sleepy` | 281 | 25 |  |
| sync/rcu.rs | `synchronize_rcu_bounded_pollfirst` | 306 | 23 |  |
| sync/rcu.rs | `synchronize_rcu_forced` | 329 | 25 |  |
| sync/rcu.rs | `shared_grace_target_pub` | 354 | 4 |  |
| sync/rcu.rs | `shared_grace_target` | 358 | 21 |  |
| sync/rcu.rs | `shared_grace_done` | 379 | 7 |  |
| sync/rcu.rs | `shared_grace_done_pub` | 386 | 8 |  |
| sync/rcu.rs | `grace_waiter_hold` | 394 | 7 |  |
| sync/rcu.rs | `rcu_gen_now` | 401 | 12 |  |
| sync/rcu.rs | `rcu_grace_watermark` | 413 | 23 |  |
| sync/rcu.rs | `report_self_quiescent` | 436 | 13 |  |
| sync/rcu.rs | `grace_waiter_release` | 449 | 8 |  |
| sync/rcu.rs | `grace_lagging_cpus` | 457 | 27 |  |
| sync/rcu.rs | `synchronize_rcu_forced_sleepy` | 484 | 17 |  |
| sync/rcu.rs | `synchronize_rcu_forced_polled` | 501 | 21 |  |
| sync/rcu.rs | `wait_qs_sleepy` | 522 | 281 |  |
| sync/rcu.rs | `all_online_cpus_reported_pub` | 803 | 4 |  |
| sync/rcu.rs | `all_online_cpus_reported` | 807 | 23 |  |
| sync/rcu.rs | `cpu_quiescent` | 830 | 8 |  |
| sync/rcu.rs | `announce_and_wait` | 838 | 50 |  |
| sync/rcu.rs | `rcu_softirq_handler` | 888 | 50 |  |
| sync/rcu.rs | `init` | 938 | 20 | yes |
| sync/rcu.rs | `lock_cb_list` | 958 | 10 |  |
| sync/rcu.rs | `unlock_cb_list` | 968 | 3 |  |
| sync/rwlock.rs | `new` | 56 | 8 | yes |
| sync/rwlock.rs | `read` | 64 | 27 | yes |
| sync/rwlock.rs | `try_read` | 91 | 15 | yes |
| sync/rwlock.rs | `read_unlock` | 106 | 19 | yes |
| sync/rwlock.rs | `write` | 125 | 52 | yes |
| sync/rwlock.rs | `try_write` | 177 | 8 | yes |
| sync/rwlock.rs | `write_unlock` | 185 | 5 | yes |
| sync/rwlock.rs | `is_locked` | 190 | 8 | yes |
| sync/rwlock.rs | `has_writer` | 198 | 20 | yes |
| sync/rwlock.rs | `fmt` | 218 | 9 | yes |
| sync/rwlock.rs | `new` | 227 | 23 | yes |
| sync/rwlock.rs | `read` | 250 | 11 | yes |
| sync/rwlock.rs | `write` | 261 | 10 | yes |
| sync/rwlock.rs | `read_irqsave` | 271 | 11 | yes |
| sync/rwlock.rs | `write_irqsave` | 282 | 11 | yes |
| sync/rwlock.rs | `read_bh` | 293 | 10 | yes |
| sync/rwlock.rs | `write_bh` | 303 | 8 | yes |
| sync/rwlock.rs | `try_read` | 311 | 12 | yes |
| sync/rwlock.rs | `try_write` | 323 | 11 | yes |
| sync/rwlock.rs | `is_locked` | 334 | 9 | yes |
| sync/rwlock.rs | `into_inner` | 343 | 18 | yes |
| sync/rwlock.rs | `deref` | 361 | 8 | yes |
| sync/rwlock.rs | `drop` | 369 | 19 | yes |
| sync/rwlock.rs | `deref` | 388 | 8 | yes |
| sync/rwlock.rs | `deref_mut` | 396 | 8 | yes |
| sync/rwlock.rs | `drop` | 404 | 20 | yes |
| sync/rwlock.rs | `deref` | 424 | 8 | yes |
| sync/rwlock.rs | `drop` | 432 | 21 | yes |
| sync/rwlock.rs | `deref` | 453 | 8 | yes |
| sync/rwlock.rs | `deref_mut` | 461 | 8 | yes |
| sync/rwlock.rs | `drop` | 469 | 20 | yes |
| sync/rwlock.rs | `deref` | 489 | 8 | yes |
| sync/rwlock.rs | `drop` | 497 | 19 | yes |
| sync/rwlock.rs | `deref` | 516 | 8 | yes |
| sync/rwlock.rs | `deref_mut` | 524 | 8 | yes |
| sync/rwlock.rs | `drop` | 532 | 10 | yes |
| sync/rwlock.rs | `preempt_disable` | 542 | 11 | yes |
| sync/rwlock.rs | `preempt_disable_at` | 553 | 8 | yes |
| sync/rwlock.rs | `preempt_enable` | 561 | 8 | yes |
| sync/rwlock.rs | `irq_save` | 569 | 6 | yes |
| sync/rwlock.rs | `irq_restore` | 575 | 5 | yes |
| sync/rwlock.rs | `bh_disable` | 580 | 7 | yes |
| sync/rwlock.rs | `bh_enable` | 587 | 5 | yes |
| sync/rwsem.rs | `time_after` | 148 | 12 |  |
| sync/rwsem.rs | `stall_report_ok` | 160 | 61 |  |
| sync/rwsem.rs | `new` | 221 | 12 | yes |
| sync/rwsem.rs | `owner_value` | 233 | 6 |  |
| sync/rwsem.rs | `set_owner_writer` | 239 | 6 |  |
| sync/rwsem.rs | `clear_owner` | 245 | 9 | yes |
| sync/rwsem.rs | `set_reader_owned` | 254 | 15 |  |
| sync/rwsem.rs | `owner_state_of` | 269 | 14 |  |
| sync/rwsem.rs | `set_nonspinnable` | 283 | 21 |  |
| sync/rwsem.rs | `clear_nonspinnable` | 304 | 10 |  |
| sync/rwsem.rs | `down_read` | 314 | 15 |  |
| sync/rwsem.rs | `down_write` | 329 | 20 |  |
| sync/rwsem.rs | `try_read` | 349 | 22 | yes |
| sync/rwsem.rs | `try_write` | 371 | 16 | yes |
| sync/rwsem.rs | `up_read` | 387 | 18 |  |
| sync/rwsem.rs | `up_write` | 405 | 11 |  |
| sync/rwsem.rs | `is_locked` | 416 | 7 | yes |
| sync/rwsem.rs | `is_contended` | 423 | 8 |  |
| sync/rwsem.rs | `rwsem_wake` | 431 | 20 |  |
| sync/rwsem.rs | `mark_wake` | 451 | 91 |  |
| sync/rwsem.rs | `try_write_lock` | 542 | 65 | yes |
| sync/rwsem.rs | `can_spin_on_owner` | 607 | 22 |  |
| sync/rwsem.rs | `spin_on_owner` | 629 | 42 |  |
| sync/rwsem.rs | `try_write_lock_unqueued` | 671 | 21 |  |
| sync/rwsem.rs | `rspin_threshold_ns` | 692 | 9 |  |
| sync/rwsem.rs | `optimistic_spin` | 701 | 59 |  |
| sync/rwsem.rs | `down_read_slowpath` | 760 | 162 |  |
| sync/rwsem.rs | `cond_wake_waiter` | 922 | 19 |  |
| sync/rwsem.rs | `down_write_slowpath` | 941 | 128 |  |
| sync/rwsem.rs | `debug_state` | 1069 | 13 |  |
| sync/rwsem.rs | `name` | 1082 | 8 | yes |
| sync/rwsem.rs | `pid_hash_lookup_pinned` | 1090 | 8 | yes |
| sync/rwsem.rs | `task_put` | 1098 | 5 | yes |
| sync/rwsem.rs | `owner_pid_on_cpu` | 1103 | 12 |  |
| sync/rwsem.rs | `preempt_disable_at` | 1115 | 8 | yes |
| sync/rwsem.rs | `preempt_enable` | 1123 | 5 | yes |
| sync/rwsem.rs | `preempt_disable` | 1128 | 22 | yes |
| sync/rwsem.rs | `fmt` | 1150 | 9 | yes |
| sync/rwsem.rs | `new` | 1159 | 10 | yes |
| sync/rwsem.rs | `read` | 1169 | 8 | yes |
| sync/rwsem.rs | `write` | 1177 | 8 | yes |
| sync/rwsem.rs | `try_read` | 1185 | 11 | yes |
| sync/rwsem.rs | `try_write` | 1196 | 9 | yes |
| sync/rwsem.rs | `is_locked` | 1205 | 5 | yes |
| sync/rwsem.rs | `is_contended` | 1210 | 9 |  |
| sync/rwsem.rs | `diag` | 1219 | 19 |  |
| sync/rwsem.rs | `deref` | 1238 | 8 | yes |
| sync/rwsem.rs | `drop` | 1246 | 15 | yes |
| sync/rwsem.rs | `deref` | 1261 | 8 | yes |
| sync/rwsem.rs | `deref_mut` | 1269 | 8 | yes |
| sync/rwsem.rs | `drop` | 1277 | 4 | yes |
| sync/semaphore.rs | `cas_take` | 24 | 46 |  |
| sync/semaphore.rs | `new` | 70 | 11 | yes |
| sync/semaphore.rs | `init` | 81 | 23 | yes |
| sync/semaphore.rs | `down` | 104 | 89 |  |
| sync/semaphore.rs | `down_interruptible` | 193 | 92 |  |
| sync/semaphore.rs | `down_trylock` | 285 | 28 |  |
| sync/semaphore.rs | `up` | 313 | 19 |  |
| sync/semaphore.rs | `count` | 332 | 31 | yes |
| sync/semaphore.rs | `new` | 363 | 19 | yes |
| sync/semaphore.rs | `lock` | 382 | 27 | yes |
| sync/semaphore.rs | `try_lock` | 409 | 15 | yes |
| sync/semaphore.rs | `unlock` | 424 | 28 | yes |
| sync/semaphore.rs | `new` | 452 | 7 | yes |
| sync/semaphore.rs | `drop` | 459 | 23 | yes |
| sync/semaphore.rs | `guard` | 482 | 4 | yes |
| sync/seqlock.rs | `new` | 44 | 8 | yes |
| sync/seqlock.rs | `write_lock` | 52 | 19 | yes |
| sync/seqlock.rs | `write_unlock` | 71 | 6 | yes |
| sync/seqlock.rs | `try_write_lock` | 77 | 12 | yes |
| sync/seqlock.rs | `read_begin` | 89 | 7 | yes |
| sync/seqlock.rs | `read_retry` | 96 | 6 | yes |
| sync/seqlock.rs | `is_locked` | 102 | 24 | yes |
| sync/seqlock.rs | `new` | 126 | 9 | yes |
| sync/seqlock.rs | `write` | 135 | 22 | yes |
| sync/seqlock.rs | `try_write` | 157 | 18 | yes |
| sync/seqlock.rs | `read` | 175 | 12 | yes |
| sync/seqlock.rs | `read_preempt_disabled` | 187 | 6 | yes |
| sync/seqlock.rs | `is_locked` | 193 | 6 | yes |
| sync/seqlock.rs | `read_inner` | 199 | 37 | yes |
| sync/seqlock.rs | `deref` | 236 | 7 | yes |
| sync/seqlock.rs | `deref_mut` | 243 | 7 | yes |
| sync/seqlock.rs | `drop` | 250 | 15 | yes |
| sync/seqlock.rs | `preempt_disable` | 265 | 5 | yes |
| sync/seqlock.rs | `preempt_enable` | 270 | 8 | yes |
| sync/seqlock.rs | `preempt_disable_bh` | 278 | 5 | yes |
| sync/seqlock.rs | `preempt_enable_bh` | 283 | 3 | yes |
| sync/spinlock.rs | `note_acquire_evidence` | 136 | 25 | yes |
| sync/spinlock.rs | `acquire_evidence_fresh` | 161 | 15 | yes |
| sync/spinlock.rs | `caller_return_address` | 176 | 65 | yes |
| sync/spinlock.rs | `locked_word` | 241 | 5 | yes |
| sync/spinlock.rs | `new` | 246 | 17 | yes |
| sync/spinlock.rs | `record_owner` | 263 | 14 | yes |
| sync/spinlock.rs | `clear_owner` | 277 | 19 | yes |
| sync/spinlock.rs | `held_word` | 296 | 14 | yes |
| sync/spinlock.rs | `holder_cpu` | 310 | 20 | yes |
| sync/spinlock.rs | `held_by` | 330 | 6 | yes |
| sync/spinlock.rs | `record_owner` | 336 | 4 | yes |
| sync/spinlock.rs | `clear_owner` | 340 | 42 | yes |
| sync/spinlock.rs | `spin_may_preempt` | 382 | 12 | yes |
| sync/spinlock.rs | `lock` | 394 | 73 | yes |
| sync/spinlock.rs | `deadlock_warn` | 467 | 329 | yes |
| sync/spinlock.rs | `puthex` | 796 | 11 | yes |
| sync/spinlock.rs | `putdec` | 807 | 18 | yes |
| sync/spinlock.rs | `try_lock` | 825 | 16 | yes |
| sync/spinlock.rs | `unlock` | 841 | 42 | yes |
| sync/spinlock.rs | `force_unlock_terminal` | 883 | 6 | yes |
| sync/spinlock.rs | `is_locked` | 889 | 5 | yes |
| sync/spinlock.rs | `reset` | 894 | 12 | yes |
| sync/spinlock.rs | `data_offset` | 906 | 19 | yes |
| sync/spinlock.rs | `new` | 925 | 33 | yes |
| sync/spinlock.rs | `lock` | 958 | 10 | yes |
| sync/spinlock.rs | `lock_irq` | 968 | 11 | yes |
| sync/spinlock.rs | `lock_irqsave` | 979 | 12 | yes |
| sync/spinlock.rs | `lock_bh` | 991 | 8 | yes |
| sync/spinlock.rs | `try_lock` | 999 | 12 | yes |
| sync/spinlock.rs | `try_lock_irqsave` | 1011 | 13 | yes |
| sync/spinlock.rs | `is_locked` | 1024 | 8 | yes |
| sync/spinlock.rs | `get_ref` | 1032 | 9 | yes |
| sync/spinlock.rs | `get_mut_unchecked` | 1041 | 9 | yes |
| sync/spinlock.rs | `into_inner` | 1050 | 16 | yes |
| sync/spinlock.rs | `deref` | 1066 | 8 | yes |
| sync/spinlock.rs | `deref_mut` | 1074 | 8 | yes |
| sync/spinlock.rs | `drop` | 1082 | 37 | yes |
| sync/spinlock.rs | `deref` | 1119 | 8 | yes |
| sync/spinlock.rs | `deref_mut` | 1127 | 8 | yes |
| sync/spinlock.rs | `drop` | 1135 | 28 | yes |
| sync/spinlock.rs | `unlock_irqretain` | 1163 | 22 | yes |
| sync/spinlock.rs | `deref` | 1185 | 8 | yes |
| sync/spinlock.rs | `deref_mut` | 1193 | 8 | yes |
| sync/spinlock.rs | `drop` | 1201 | 10 | yes |
| sync/spinlock.rs | `preempt_disable` | 1211 | 11 | yes |
| sync/spinlock.rs | `preempt_disable_at` | 1222 | 9 | yes |
| sync/spinlock.rs | `preempt_enable` | 1231 | 9 | yes |
| sync/spinlock.rs | `irq_save` | 1240 | 8 | yes |
| sync/spinlock.rs | `irq_restore` | 1248 | 6 | yes |
| sync/spinlock.rs | `bh_disable` | 1254 | 8 | yes |
| sync/spinlock.rs | `bh_enable` | 1262 | 10 | yes |
| sync/spinlock.rs | `raw_spin_lock` | 1272 | 2 | yes |
| sync/spinlock.rs | `raw_spin_unlock` | 1274 | 2 | yes |
| sync/spinlock.rs | `raw_spin_is_locked` | 1276 | 2 | yes |
| sync/spinlock.rs | `raw_spin_trylock` | 1278 | 1 | yes |
