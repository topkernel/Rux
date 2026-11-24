# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# timer — hot 503 lines (stay) / cold 1134 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| timer/nohz.rs | `ts_of` | 74 | 8 |  |
| timer/nohz.rs | `check_tick_dependency` | 81 | 27 |  |
| timer/nohz.rs | `tick_nohz_dep_set` | 107 | 9 |  |
| timer/nohz.rs | `tick_nohz_dep_clear` | 115 | 7 |  |
| timer/nohz.rs | `tick_nohz_dep_set_cpu` | 121 | 10 |  |
| timer/nohz.rs | `tick_nohz_dep_clear_cpu` | 130 | 14 |  |
| timer/nohz.rs | `sched_update_tick_dependency_on_load_change` | 143 | 20 | yes |
| timer/nohz.rs | `tick_nohz_full_kick_cpu` | 162 | 13 |  |
| timer/nohz.rs | `tick_nohz_full_kick_stopped` | 174 | 10 |  |
| timer/nohz.rs | `tick_nohz_full_kick_all` | 183 | 16 |  |
| timer/nohz.rs | `can_stop_full_tick` | 198 | 31 |  |
| timer/nohz.rs | `tick_nohz_next_event` | 228 | 56 |  |
| timer/nohz.rs | `tick_do_timer_cpu` | 283 | 9 |  |
| timer/nohz.rs | `tick_nohz_stop_tick` | 291 | 26 |  |
| timer/nohz.rs | `tick_nohz_stop_sched_tick` | 316 | 16 |  |
| timer/nohz.rs | `tick_nohz_restart_sched_tick` | 331 | 12 |  |
| timer/nohz.rs | `tick_nohz_full_update_tick_at` | 342 | 19 |  |
| timer/nohz.rs | `tick_nohz_full_cpu` | 360 | 8 |  |
| timer/nohz.rs | `nohz_full_setup` | 367 | 34 |  |
| timer/nohz.rs | `tick_nohz_full_update_tick` | 400 | 19 |  |
| timer/nohz.rs | `tick_nohz_irq_exit` | 418 | 14 | yes |
| timer/nohz.rs | `tick_nohz_idle_enter` | 431 | 7 | yes |
| timer/nohz.rs | `tick_nohz_idle_exit` | 437 | 9 | yes |
| timer/nohz.rs | `grid_period` | 445 | 11 |  |
| timer/nohz.rs | `tick_nohz_rearm_local` | 455 | 18 |  |
| timer/oneshot.rs | `arm_jiffy` | 79 | 10 |  |
| timer/oneshot.rs | `arm_hres` | 88 | 9 | yes |
| timer/oneshot.rs | `wrap` | 96 | 12 |  |
| timer/oneshot.rs | `pending` | 107 | 13 | yes |
| timer/oneshot.rs | `rearm_if_spent` | 119 | 22 | yes |
| timer/oneshot.rs | `cancel` | 140 | 12 | yes |
| timer/oneshot.rs | `id` | 151 | 11 | yes |
| timer/oneshot.rs | `drop` | 161 | 18 | yes |
| timer/oneshot.rs | `recipe` | 178 | 12 |  |
| timer/wheel.rs | `clone` | 106 | 20 | yes |
| timer/wheel.rs | `clone` | 125 | 9 | yes |
| timer/wheel.rs | `from_jiffies` | 133 | 8 |  |
| timer/wheel.rs | `from_time` | 140 | 9 |  |
| timer/wheel.rs | `as_u64` | 148 | 10 |  |
| timer/wheel.rs | `is_due` | 157 | 7 |  |
| timer/wheel.rs | `abs` | 163 | 14 | yes |
| timer/wheel.rs | `is_due` | 176 | 7 |  |
| timer/wheel.rs | `abs` | 182 | 28 | yes |
| timer/wheel.rs | `is_due` | 209 | 11 |  |
| timer/wheel.rs | `hres_abs` | 219 | 11 |  |
| timer/wheel.rs | `jiffy_bookkeeping` | 229 | 23 |  |
| timer/wheel.rs | `new` | 251 | 33 | yes |
| timer/wheel.rs | `delivers` | 283 | 36 |  |
| timer/wheel.rs | `from_legacy_oneshot` | 318 | 11 |  |
| timer/wheel.rs | `action` | 328 | 13 |  |
| timer/wheel.rs | `from_legacy_periodic` | 340 | 11 |  |
| timer/wheel.rs | `action` | 350 | 47 |  |
| timer/wheel.rs | `new` | 396 | 80 | yes |
| timer/wheel.rs | `new` | 475 | 17 | yes |
| timer/wheel.rs | `route_for` | 491 | 23 |  |
| timer/wheel.rs | `link_tail` | 513 | 20 |  |
| timer/wheel.rs | `unlink` | 532 | 22 | yes |
| timer/wheel.rs | `unlink_from` | 553 | 28 |  |
| timer/wheel.rs | `splice_to_due` | 580 | 30 |  |
| timer/wheel.rs | `pop_bucket` | 609 | 11 |  |
| timer/wheel.rs | `find_free_slot` | 619 | 13 |  |
| timer/wheel.rs | `occupy` | 631 | 8 |  |
| timer/wheel.rs | `free` | 638 | 8 | yes |
| timer/wheel.rs | `find_by_id` | 645 | 19 |  |
| timer/wheel.rs | `len` | 663 | 10 | yes |
| timer/wheel.rs | `arm_oneshot_jiffy` | 672 | 22 |  |
| timer/wheel.rs | `arm_periodic_jiffy` | 693 | 24 |  |
| timer/wheel.rs | `arm_oneshot_hres` | 716 | 26 |  |
| timer/wheel.rs | `arm_periodic_hres` | 741 | 26 |  |
| timer/wheel.rs | `arm_entry` | 766 | 33 |  |
| timer/wheel.rs | `remove` | 798 | 15 | yes |
| timer/wheel.rs | `contains` | 812 | 7 | yes |
| timer/wheel.rs | `state_of` | 818 | 12 |  |
| timer/wheel.rs | `min_hres` | 829 | 22 |  |
| timer/wheel.rs | `overdue_hres` | 850 | 23 |  |
| timer/wheel.rs | `push_snapshots` | 872 | 34 |  |
| timer/wheel.rs | `advance` | 905 | 25 |  |
| timer/wheel.rs | `cascade` | 929 | 27 |  |
| timer/wheel.rs | `rescan_far` | 955 | 35 |  |
| timer/wheel.rs | `collect` | 989 | 85 | yes |
| timer/wheel.rs | `reset` | 1073 | 20 | yes |
| timer/wheel.rs | `test_shared` | 1092 | 9 |  |
| timer/wheel.rs | `probe_route` | 1100 | 18 |  |
| timer/wheel.rs | `bucket_of` | 1117 | 6 |  |
| timer/wheel.rs | `cursor` | 1122 | 6 |  |
| timer/wheel.rs | `due_len` | 1127 | 23 |  |
