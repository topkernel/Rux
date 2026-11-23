# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# timer — hot 619 lines (stay) / cold 932 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| timer/nohz.rs | `ts_of` | 74 | 7 |  |
| timer/nohz.rs | `check_tick_dependency` | 81 | 26 |  |
| timer/nohz.rs | `tick_nohz_dep_set` | 107 | 8 |  |
| timer/nohz.rs | `tick_nohz_dep_clear` | 115 | 6 |  |
| timer/nohz.rs | `tick_nohz_dep_set_cpu` | 121 | 9 |  |
| timer/nohz.rs | `tick_nohz_dep_clear_cpu` | 130 | 13 |  |
| timer/nohz.rs | `sched_update_tick_dependency_on_load_change` | 143 | 19 |  |
| timer/nohz.rs | `tick_nohz_full_kick_cpu` | 162 | 12 |  |
| timer/nohz.rs | `tick_nohz_full_kick_stopped` | 174 | 9 |  |
| timer/nohz.rs | `tick_nohz_full_kick_all` | 183 | 15 |  |
| timer/nohz.rs | `can_stop_full_tick` | 198 | 30 |  |
| timer/nohz.rs | `tick_nohz_next_event` | 228 | 55 |  |
| timer/nohz.rs | `tick_do_timer_cpu` | 283 | 8 |  |
| timer/nohz.rs | `tick_nohz_stop_tick` | 291 | 25 |  |
| timer/nohz.rs | `tick_nohz_stop_sched_tick` | 316 | 15 |  |
| timer/nohz.rs | `tick_nohz_restart_sched_tick` | 331 | 11 |  |
| timer/nohz.rs | `tick_nohz_full_update_tick_at` | 342 | 18 |  |
| timer/nohz.rs | `tick_nohz_full_cpu` | 360 | 7 |  |
| timer/nohz.rs | `nohz_full_setup` | 367 | 33 |  |
| timer/nohz.rs | `tick_nohz_full_update_tick` | 400 | 18 |  |
| timer/nohz.rs | `tick_nohz_irq_exit` | 418 | 13 | yes |
| timer/nohz.rs | `tick_nohz_idle_enter` | 431 | 6 | yes |
| timer/nohz.rs | `tick_nohz_idle_exit` | 437 | 8 | yes |
| timer/nohz.rs | `grid_period` | 445 | 10 |  |
| timer/nohz.rs | `tick_nohz_rearm_local` | 455 | 17 |  |
| timer/oneshot.rs | `arm_jiffy` | 79 | 9 |  |
| timer/oneshot.rs | `arm_hres` | 88 | 8 |  |
| timer/oneshot.rs | `wrap` | 96 | 11 |  |
| timer/oneshot.rs | `pending` | 107 | 12 |  |
| timer/oneshot.rs | `rearm_if_spent` | 119 | 21 |  |
| timer/oneshot.rs | `cancel` | 140 | 11 |  |
| timer/oneshot.rs | `id` | 151 | 10 |  |
| timer/oneshot.rs | `drop` | 161 | 17 | yes |
| timer/oneshot.rs | `recipe` | 178 | 11 |  |
| timer/wheel.rs | `clone` | 106 | 19 | yes |
| timer/wheel.rs | `clone` | 125 | 8 | yes |
| timer/wheel.rs | `from_jiffies` | 133 | 7 |  |
| timer/wheel.rs | `from_time` | 140 | 8 | yes |
| timer/wheel.rs | `as_u64` | 148 | 9 | yes |
| timer/wheel.rs | `is_due` | 157 | 6 |  |
| timer/wheel.rs | `abs` | 163 | 13 | yes |
| timer/wheel.rs | `is_due` | 176 | 6 |  |
| timer/wheel.rs | `abs` | 182 | 27 | yes |
| timer/wheel.rs | `is_due` | 209 | 10 |  |
| timer/wheel.rs | `hres_abs` | 219 | 10 |  |
| timer/wheel.rs | `jiffy_bookkeeping` | 229 | 22 |  |
| timer/wheel.rs | `new` | 251 | 32 | yes |
| timer/wheel.rs | `delivers` | 283 | 35 |  |
| timer/wheel.rs | `from_legacy_oneshot` | 318 | 10 | yes |
| timer/wheel.rs | `action` | 328 | 12 |  |
| timer/wheel.rs | `from_legacy_periodic` | 340 | 10 | yes |
| timer/wheel.rs | `action` | 350 | 46 |  |
| timer/wheel.rs | `new` | 396 | 79 | yes |
| timer/wheel.rs | `new` | 475 | 16 | yes |
| timer/wheel.rs | `route_for` | 491 | 22 |  |
| timer/wheel.rs | `link_tail` | 513 | 19 |  |
| timer/wheel.rs | `unlink` | 532 | 21 |  |
| timer/wheel.rs | `unlink_from` | 553 | 27 |  |
| timer/wheel.rs | `splice_to_due` | 580 | 29 |  |
| timer/wheel.rs | `pop_bucket` | 609 | 10 |  |
| timer/wheel.rs | `find_free_slot` | 619 | 12 |  |
| timer/wheel.rs | `occupy` | 631 | 7 |  |
| timer/wheel.rs | `free` | 638 | 7 | yes |
| timer/wheel.rs | `find_by_id` | 645 | 18 |  |
| timer/wheel.rs | `len` | 663 | 9 | yes |
| timer/wheel.rs | `arm_oneshot_jiffy` | 672 | 21 | yes |
| timer/wheel.rs | `arm_periodic_jiffy` | 693 | 23 | yes |
| timer/wheel.rs | `arm_oneshot_hres` | 716 | 25 | yes |
| timer/wheel.rs | `arm_periodic_hres` | 741 | 25 | yes |
| timer/wheel.rs | `arm_entry` | 766 | 32 |  |
| timer/wheel.rs | `remove` | 798 | 14 | yes |
| timer/wheel.rs | `contains` | 812 | 6 | yes |
| timer/wheel.rs | `state_of` | 818 | 11 | yes |
| timer/wheel.rs | `min_hres` | 829 | 21 | yes |
| timer/wheel.rs | `overdue_hres` | 850 | 22 | yes |
| timer/wheel.rs | `push_snapshots` | 872 | 33 | yes |
| timer/wheel.rs | `advance` | 905 | 24 | yes |
| timer/wheel.rs | `cascade` | 929 | 26 |  |
| timer/wheel.rs | `rescan_far` | 955 | 34 |  |
| timer/wheel.rs | `collect` | 989 | 84 | yes |
| timer/wheel.rs | `reset` | 1073 | 19 | yes |
| timer/wheel.rs | `test_shared` | 1092 | 8 |  |
| timer/wheel.rs | `probe_route` | 1100 | 17 |  |
| timer/wheel.rs | `bucket_of` | 1117 | 5 |  |
| timer/wheel.rs | `cursor` | 1122 | 5 |  |
| timer/wheel.rs | `due_len` | 1127 | 22 |  |
