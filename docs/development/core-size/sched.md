# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# sched — hot 12542 lines (stay) / cold 2960 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| sched/cgroup.rs | `new` | 112 | 73 | yes |
| sched/cgroup.rs | `new_root` | 184 | 21 |  |
| sched/cgroup.rs | `new_child` | 204 | 24 |  |
| sched/cgroup.rs | `path` | 227 | 30 |  |
| sched/cgroup.rs | `for_each_ancestor` | 256 | 32 |  |
| sched/cgroup.rs | `chain_throttled` | 287 | 13 |  |
| sched/cgroup.rs | `populated` | 299 | 6 |  |
| sched/cgroup.rs | `subtree_task_count` | 304 | 28 |  |
| sched/cgroup.rs | `init_cgroup` | 331 | 10 |  |
| sched/cgroup.rs | `cgroup_root` | 340 | 6 |  |
| sched/cgroup.rs | `alloc_ino_base` | 345 | 12 |  |
| sched/cgroup.rs | `valid_cgroup_name` | 356 | 10 |  |
| sched/cgroup.rs | `cgroup_create` | 365 | 26 |  |
| sched/cgroup.rs | `cgroup_rmdir` | 390 | 34 |  |
| sched/cgroup.rs | `cgroup_lookup_child` | 423 | 23 |  |
| sched/cgroup.rs | `task_cgroup` | 445 | 42 |  |
| sched/class.rs | `name` | 57 | 9 |  |
| sched/class.rs | `enqueue_task` | 65 | 12 | yes |
| sched/class.rs | `dequeue_task` | 76 | 4 | yes |
| sched/class.rs | `yield_task` | 79 | 9 |  |
| sched/class.rs | `wakeup_preempt` | 87 | 11 |  |
| sched/class.rs | `pick_next_task` | 97 | 11 | yes |
| sched/class.rs | `put_prev_task` | 107 | 11 |  |
| sched/class.rs | `set_next_task` | 117 | 13 |  |
| sched/class.rs | `balance` | 129 | 12 |  |
| sched/class.rs | `select_task_rq` | 140 | 11 |  |
| sched/class.rs | `task_tick` | 150 | 7 |  |
| sched/class.rs | `update_curr` | 156 | 11 | yes |
| sched/class.rs | `get_rr_interval` | 166 | 10 |  |
| sched/class.rs | `has_runnable` | 175 | 6 |  |
| sched/class.rs | `next_class` | 180 | 22 |  |
| sched/class.rs | `to_class` | 201 | 16 |  |
| sched/class.rs | `task_sched_class` | 216 | 19 | yes |
| sched/class.rs | `new` | 234 | 11 | yes |
| sched/class.rs | `next` | 244 | 15 | yes |
| sched/class.rs | `from` | 258 | 11 |  |
| sched/class.rs | `next` | 268 | 7 | yes |
| sched/edges.rs | `idle_task_ptr` | 48 | 13 | yes |
| sched/edges.rs | `idle_task_storage_base` | 60 | 28 | yes |
| sched/edges.rs | `task_mut` | 87 | 19 | yes |
| sched/edges.rs | `task_shared` | 105 | 23 | yes |
| sched/edges.rs | `grq_is_ready` | 127 | 7 | yes |
| sched/edges.rs | `grq_claim_init` | 133 | 15 | yes |
| sched/edges.rs | `grq_stage` | 147 | 13 | yes |
| sched/edges.rs | `grq_publish_init` | 159 | 7 | yes |
| sched/edges.rs | `grq_ref` | 165 | 13 | yes |
| sched/edges.rs | `grq_mut` | 177 | 14 | yes |
| sched/edges.rs | `task_poison_pair_volatile` | 190 | 21 | yes |
| sched/edges.rs | `task_pid_volatile` | 210 | 19 | yes |
| sched/edges.rs | `task_ownership_qword` | 228 | 18 | yes |
| sched/edges.rs | `current_trap_ip` | 245 | 18 | yes |
| sched/edges.rs | `current_task_mut` | 262 | 29 | yes |
| sched/edges.rs | `arch_context_switch` | 290 | 11 | yes |
| sched/edges.rs | `task_borrow` | 300 | 12 | yes |
| sched/edges.rs | `cgroup_node_ref` | 311 | 19 | yes |
| sched/edges.rs | `pt_regs_at_top` | 329 | 19 | yes |
| sched/edges.rs | `list_next` | 347 | 10 | yes |
| sched/edges.rs | `task_of_rt_list` | 356 | 10 | yes |
| sched/edges.rs | `rt_list_add` | 365 | 9 | yes |
| sched/edges.rs | `rt_list_add_tail` | 373 | 9 | yes |
| sched/edges.rs | `rt_list_del` | 381 | 10 | yes |
| sched/edges.rs | `rt_list_linked` | 390 | 39 | yes |
| sched/edges.rs | `new` | 428 | 61 | yes |
| sched/edges.rs | `linked_at` | 488 | 14 | yes |
| sched/edges.rs | `detach_at` | 501 | 12 | yes |
| sched/edges.rs | `node_ok_shared` | 512 | 5 | yes |
| sched/edges.rs | `unlinked` | 516 | 20 | yes |
| sched/edges.rs | `rb_nil` | 535 | 9 | yes |
| sched/edges.rs | `node_of_task` | 543 | 9 | yes |
| sched/edges.rs | `task_of_node` | 551 | 10 | yes |
| sched/edges.rs | `node_key` | 560 | 28 | yes |
| sched/edges.rs | `new` | 587 | 9 | yes |
| sched/edges.rs | `nil` | 595 | 6 | yes |
| sched/edges.rs | `is_empty` | 600 | 8 | yes |
| sched/edges.rs | `node_ok` | 607 | 6 | yes |
| sched/edges.rs | `subtree_min` | 612 | 15 | yes |
| sched/edges.rs | `next` | 626 | 22 | yes |
| sched/edges.rs | `rotate_left` | 647 | 23 | yes |
| sched/edges.rs | `rotate_right` | 669 | 26 | yes |
| sched/edges.rs | `insert` | 694 | 47 | yes |
| sched/edges.rs | `insert_fixup` | 740 | 52 | yes |
| sched/edges.rs | `erase` | 791 | 124 | yes |
| sched/edges.rs | `erase_fixup` | 914 | 6 | yes |
| sched/edges.rs | `kid` | 919 | 125 | yes |
| sched/edges.rs | `debug_insert` | 1043 | 6 | yes |
| sched/edges.rs | `debug_erase` | 1048 | 6 | yes |
| sched/edges.rs | `debug_erase_leftmost` | 1053 | 8 | yes |
| sched/edges.rs | `debug_leftmost_key` | 1060 | 13 | yes |
| sched/edges.rs | `debug_walk_keys` | 1072 | 16 | yes |
| sched/edges.rs | `debug_black_height` | 1087 | 3 | yes |
| sched/edges.rs | `bh` | 1089 | 45 | yes |
| sched/edges.rs | `grq_ptr_shared` | 1133 | 9 | yes |
| sched/edges.rs | `grq_ptr_mut` | 1141 | 19 | yes |
| sched/edges.rs | `per_cpu_shared` | 1159 | 11 | yes |
| sched/edges.rs | `per_cpu_mut` | 1169 | 20 | yes |
| sched/edges.rs | `secondary_idle_publish` | 1188 | 8 | yes |
| sched/edges.rs | `secondary_idle_table_base` | 1195 | 7 | yes |
| sched/edges.rs | `secondary_idle_task` | 1201 | 5 | yes |
| sched/entities.rs | `sched_clock` | 68 | 12 | yes |
| sched/entities.rs | `sched_slice_to_ms` | 79 | 6 | yes |
| sched/entities.rs | `ms_to_ns` | 84 | 83 |  |
| sched/entities.rs | `new` | 166 | 17 | yes |
| sched/entities.rs | `new_const` | 182 | 12 |  |
| sched/entities.rs | `init` | 193 | 9 | yes |
| sched/entities.rs | `nr_running` | 201 | 7 | yes |
| sched/entities.rs | `bitmap_word` | 207 | 12 |  |
| sched/entities.rs | `highest_linked_user_prio` | 218 | 40 |  |
| sched/entities.rs | `walk_prio` | 257 | 27 |  |
| sched/entities.rs | `is_empty` | 283 | 10 | yes |
| sched/entities.rs | `rt_nr_dec` | 292 | 18 |  |
| sched/entities.rs | `rr_nr_dec` | 309 | 15 |  |
| sched/entities.rs | `find_highest_prio` | 323 | 21 |  |
| sched/entities.rs | `enqueue` | 343 | 68 | yes |
| sched/entities.rs | `dequeue` | 410 | 64 | yes |
| sched/entities.rs | `is_linked` | 473 | 20 | yes |
| sched/entities.rs | `force_unlink` | 492 | 61 | yes |
| sched/entities.rs | `pick_next` | 552 | 25 |  |
| sched/entities.rs | `peek_next` | 576 | 36 |  |
| sched/entities.rs | `new` | 611 | 14 | yes |
| sched/entities.rs | `from_nice` | 624 | 13 |  |
| sched/entities.rs | `update_inv_weight` | 636 | 12 |  |
| sched/entities.rs | `default` | 647 | 66 |  |
| sched/entities.rs | `new` | 712 | 18 | yes |
| sched/entities.rs | `from_nice` | 729 | 18 |  |
| sched/entities.rs | `set_nice` | 746 | 7 |  |
| sched/entities.rs | `get_vruntime` | 752 | 7 |  |
| sched/entities.rs | `set_vruntime` | 758 | 7 |  |
| sched/entities.rs | `add_vruntime` | 764 | 12 |  |
| sched/entities.rs | `update_exec_runtime` | 775 | 40 | yes |
| sched/entities.rs | `calc_delta_fair` | 814 | 26 |  |
| sched/entities.rs | `update_vruntime` | 839 | 9 | yes |
| sched/entities.rs | `is_on_rq` | 847 | 7 | yes |
| sched/entities.rs | `set_on_rq` | 853 | 8 |  |
| sched/entities.rs | `timeline_key` | 860 | 9 | yes |
| sched/entities.rs | `set_timeline_key` | 868 | 7 |  |
| sched/entities.rs | `get_slice` | 874 | 7 |  |
| sched/entities.rs | `set_slice` | 880 | 7 |  |
| sched/entities.rs | `get_exec_start` | 886 | 7 |  |
| sched/entities.rs | `set_exec_start` | 892 | 6 | yes |
| sched/entities.rs | `default` | 897 | 6 |  |
| sched/entities.rs | `clone` | 902 | 34 |  |
| sched/entities.rs | `new` | 935 | 10 | yes |
| sched/entities.rs | `is_on_rq` | 944 | 7 | yes |
| sched/entities.rs | `set_on_rq` | 950 | 7 |  |
| sched/entities.rs | `get_time_slice` | 956 | 7 |  |
| sched/entities.rs | `set_time_slice` | 962 | 7 | yes |
| sched/entities.rs | `dec_time_slice` | 968 | 13 | yes |
| sched/entities.rs | `reset_time_slice` | 980 | 6 | yes |
| sched/entities.rs | `default` | 985 | 48 |  |
| sched/entities.rs | `new` | 1032 | 18 | yes |
| sched/entities.rs | `is_on_rq` | 1049 | 6 | yes |
| sched/entities.rs | `get_bw` | 1054 | 13 |  |
| sched/entities.rs | `update_deadline` | 1066 | 15 |  |
| sched/entities.rs | `replenish_runtime` | 1080 | 14 |  |
| sched/entities.rs | `consume_runtime` | 1093 | 16 | yes |
| sched/entities.rs | `default` | 1108 | 54 |  |
| sched/entities.rs | `fmt` | 1161 | 43 |  |
| sched/entities.rs | `new` | 1203 | 12 | yes |
| sched/entities.rs | `is_empty` | 1214 | 7 | yes |
| sched/entities.rs | `nr_running` | 1220 | 8 | yes |
| sched/entities.rs | `dl_nr_dec` | 1227 | 19 |  |
| sched/entities.rs | `enqueue` | 1245 | 62 | yes |
| sched/entities.rs | `dequeue` | 1306 | 57 | yes |
| sched/entities.rs | `is_linked` | 1362 | 9 | yes |
| sched/entities.rs | `pick_next` | 1370 | 27 |  |
| sched/entities.rs | `peek_next` | 1396 | 15 |  |
| sched/entities.rs | `pick_next_cpu` | 1410 | 83 |  |
| sched/entities.rs | `new` | 1492 | 17 | yes |
| sched/entities.rs | `leftmost_entry` | 1508 | 31 |  |
| sched/entities.rs | `avg_vruntime` | 1538 | 31 |  |
| sched/entities.rs | `capture_sleep_vlag` | 1568 | 42 | yes |
| sched/entities.rs | `timeline_insert` | 1609 | 15 |  |
| sched/entities.rs | `ph` | 1623 | 58 | yes |
| sched/entities.rs | `timeline_remove` | 1680 | 24 |  |
| sched/entities.rs | `ph` | 1703 | 31 | yes |
| sched/entities.rs | `nr_running` | 1733 | 15 | yes |
| sched/entities.rs | `nr_running_dec` | 1747 | 71 |  |
| sched/entities.rs | `load_weight_sub` | 1817 | 30 |  |
| sched/entities.rs | `is_empty` | 1846 | 7 | yes |
| sched/entities.rs | `get_min_vruntime` | 1852 | 8 |  |
| sched/entities.rs | `leftmost_vruntime` | 1859 | 22 |  |
| sched/entities.rs | `ww_tie_mass` | 1880 | 27 |  |
| sched/entities.rs | `update_min_vruntime` | 1906 | 44 | yes |
| sched/entities.rs | `enqueue` | 1949 | 9 | yes |
| sched/entities.rs | `enqueue_migrate` | 1957 | 5 |  |
| sched/entities.rs | `enqueue_inner` | 1961 | 121 |  |
| sched/entities.rs | `dequeue` | 2081 | 77 | yes |
| sched/entities.rs | `is_linked` | 2157 | 20 | yes |
| sched/entities.rs | `resync_nr_running_from_timeline` | 2176 | 26 | yes |
| sched/entities.rs | `timeline_head_diag` | 2201 | 46 | yes |
| sched/entities.rs | `strand_wall_probe` | 2246 | 135 |  |
| sched/entities.rs | `heal_report_budget_ok` | 2380 | 16 |  |
| sched/entities.rs | `stale_heal_line` | 2395 | 37 |  |
| sched/entities.rs | `stale_claim_healed` | 2431 | 20 |  |
| sched/entities.rs | `pdec5` | 2450 | 65 |  |
| sched/entities.rs | `stale_oncpu_healed` | 2514 | 64 |  |
| sched/entities.rs | `put_dec` | 2577 | 29 |  |
| sched/entities.rs | `claim_inject_scan` | 2605 | 34 |  |
| sched/entities.rs | `oncpu_inject_scan` | 2638 | 62 |  |
| sched/entities.rs | `claim_sweep` | 2699 | 125 | yes |
| sched/entities.rs | `pick_next` | 2823 | 47 |  |
| sched/entities.rs | `peek_next` | 2869 | 14 |  |
| sched/entities.rs | `update_curr` | 2882 | 34 | yes |
| sched/entities.rs | `sched_slice` | 2915 | 67 | yes |
| sched/entities.rs | `check_preempt` | 2981 | 20 |  |
| sched/entities.rs | `set_curr` | 3000 | 7 | yes |
| sched/entities.rs | `get_curr` | 3006 | 6 | yes |
| sched/entities.rs | `clear` | 3011 | 120 |  |
| sched/reapdrain.rs | `mint` | 131 | 9 |  |
| sched/reapdrain.rs | `as_ptr` | 139 | 9 |  |
| sched/reapdrain.rs | `task` | 147 | 15 |  |
| sched/reapdrain.rs | `into_parked` | 161 | 10 |  |
| sched/reapdrain.rs | `into_free` | 170 | 89 |  |
| sched/reapdrain.rs | `defer_reap_storage` | 258 | 34 |  |
| sched/reapdrain.rs | `switch_stage_done` | 291 | 46 |  |
| sched/reapdrain.rs | `reap_drain_tick` | 336 | 215 |  |
| sched/reapdrain.rs | `notify_parent_if_any` | 550 | 10 |  |
| sched/reapdrain.rs | `reap_storage_free` | 559 | 17 |  |
| sched/sched.rs | `need_resched` | 40 | 14 | yes |
| sched/sched.rs | `set_need_resched` | 53 | 11 | yes |
| sched/sched.rs | `clear_need_resched` | 63 | 8 | yes |
| sched/sched.rs | `resched_curr` | 70 | 6 | yes |
| sched/sched.rs | `resched_cpu` | 75 | 39 | yes |
| sched/sched.rs | `ph` | 113 | 34 | yes |
| sched/sched.rs | `wake_up_process` | 146 | 9 | yes |
| sched/sched.rs | `wake_up_process_sync` | 154 | 18 | yes |
| sched/sched.rs | `enqueue_task` | 171 | 71 | yes |
| sched/sched.rs | `wake_up_enqueue` | 241 | 10 | yes |
| sched/sched.rs | `wake_up_enqueue_nested` | 250 | 12 | yes |
| sched/sched.rs | `wake_up_enqueue_opt` | 261 | 828 | yes |
| sched/sched.rs | `ph` | 1088 | 43 | yes |
| sched/sched.rs | `pd2` | 1130 | 59 | yes |
| sched/sched.rs | `pdec` | 1188 | 154 | yes |
| sched/sched.rs | `dequeue_task` | 1341 | 58 | yes |
| sched/sched.rs | `dequeue_if_enqueued` | 1398 | 19 | yes |
| sched/sched.rs | `context_switch` | 1416 | 41 | yes |
| sched/sched.rs | `ph` | 1456 | 81 | yes |
| sched/sched.rs | `current` | 1536 | 13 | yes |
| sched/sched.rs | `current_is_dying` | 1548 | 26 | yes |
| sched/sched.rs | `class_nonempty_set` | 1573 | 7 | yes |
| sched/sched.rs | `class_nonempty_clear` | 1579 | 7 | yes |
| sched/sched.rs | `class_nonempty` | 1585 | 6 | yes |
| sched/sched.rs | `load_balance` | 1590 | 4 | yes |
| sched/sched.rs | `psledger_note_publish` | 1593 | 4 | yes |
| sched/sched.rs | `psledger_peek` | 1596 | 8 | yes |
| sched/sched.rs | `sched_hist_armed` | 1603 | 6 | yes |
| sched/sched.rs | `sched_hist_ring` | 1608 | 4 | yes |
| sched/sched.rs | `sched_hist_refuse` | 1611 | 4 | yes |
| sched/sched.rs | `sched_hist_link` | 1614 | 4 | yes |
| sched/sched.rs | `sched_hist_pick` | 1617 | 4 | yes |
| sched/sched.rs | `ww_cfs_geometry` | 1620 | 4 | yes |
| sched/sched.rs | `ww_cfs_tie_mass` | 1623 | 4 | yes |
| sched/sched.rs | `ww_cfs_tie_mass_vr` | 1626 | 4 | yes |
| sched/sched.rs | `ww_tail_fill` | 1629 | 4 | yes |
| sched/sched.rs | `ww_append_dump` | 1632 | 11 | yes |
| sched/sched.rs | `enabled` | 1642 | 8 | yes |
| sched/sched.rs | `set_enabled` | 1649 | 7 | yes |
| sched/sched.rs | `r56_print_pid` | 1655 | 5 | yes |
| sched/sched.rs | `sched_clock` | 1659 | 7 | yes |
| sched/sched.rs | `pick_trace` | 1665 | 44 | yes |
| sched/sched.rs | `pick_gen_supersede` | 1708 | 8 | yes |
| sched/sched.rs | `change_task_policy` | 1715 | 5 | yes |
| sched/sched.rs | `ww_record` | 1719 | 5 | yes |
| sched/sched.rs | `orphan_watch_confirm` | 1723 | 6 | yes |
| sched/sched.rs | `pick_trace_peek` | 1728 | 18 | yes |
| sched/sched.rs | `yield_cpu` | 1745 | 5 | yes |
| sched/sched.rs | `enqueue_task_locked` | 1749 | 5 | yes |
| sched/sched.rs | `ww_watch_add` | 1753 | 5 | yes |
| sched/sched.rs | `abort_next_release_orphans` | 1757 | 5 | yes |
| sched/sched.rs | `r56_report_switch_abort` | 1761 | 5 | yes |
| sched/sched.rs | `sched_renice_locked` | 1765 | 7 | yes |
| sched/sched.rs | `pick_gen_retire` | 1771 | 11 | yes |
| sched/sched.rs | `psledger_consume` | 1781 | 5 | yes |
| sched/sched.rs | `steer_task_cpu` | 1785 | 5 | yes |
| sched/sched.rs | `sched_can_stop_tick` | 1789 | 5 | yes |
| sched/sched.rs | `dl_throttle` | 1793 | 5 | yes |
| sched/sched.rs | `ensure_linked_locked` | 1797 | 5 | yes |
| sched/sched.rs | `check_cfs_preempt` | 1801 | 5 | yes |
| sched/sched.rs | `check_rt_preempt` | 1805 | 5 | yes |
| sched/sched.rs | `check_dl_preempt` | 1809 | 5 | yes |
| sched/sched.rs | `orphan_watch_clear` | 1813 | 5 | yes |
| sched/sched.rs | `pick_giveups` | 1817 | 5 | yes |
| sched/sched.rs | `ww_slot_mismatch_counters` | 1821 | 5 | yes |
| sched/sched.rs | `ww_sched_snapshot` | 1825 | 5 | yes |
| sched/sched.rs | `ww_stalled_cpu_state` | 1829 | 5 | yes |
| sched/sched.rs | `debug_claim_inject` | 1833 | 5 | yes |
| sched/sched.rs | `debug_oncpu_inject` | 1837 | 6 | yes |
| sched/sched.rs | `ww_pick_walks` | 1842 | 5 | yes |
| sched/sched.rs | `ww_pick_walk_inc` | 1846 | 5 | yes |
| sched/sched.rs | `ww_oncpu_heals` | 1850 | 5 | yes |
| sched/sched.rs | `ww_oncpu_heal_inc` | 1854 | 5 | yes |
| sched/sched.rs | `ww_claim_heals` | 1858 | 5 | yes |
| sched/sched.rs | `ww_claim_heal_inc` | 1862 | 5 | yes |
| sched/sched.rs | `tl_poison_inserts` | 1866 | 5 | yes |
| sched/sched.rs | `tl_poison_insert_inc` | 1870 | 5 | yes |
| sched/sched.rs | `tl_poison_erases` | 1874 | 5 | yes |
| sched/sched.rs | `tl_poison_erase_inc` | 1878 | 8 | yes |
| sched/sched.rs | `sched_cs_phase` | 1885 | 9 | yes |
| sched/sched.rs | `sched_cs_phase_set` | 1893 | 7 | yes |
| sched/sched.rs | `sched_calls` | 1899 | 9 | yes |
| sched/sched.rs | `sched_calls_inc` | 1907 | 7 | yes |
| sched/sched.rs | `ww_idle_phase` | 1913 | 9 | yes |
| sched/sched.rs | `ww_idle_phase_set` | 1921 | 7 | yes |
| sched/sched.rs | `wake_disp` | 1927 | 5 | yes |
| sched/sched.rs | `wake_disp_set` | 1931 | 5 | yes |
| sched/sched.rs | `wake_idle_mask` | 1935 | 5 | yes |
| sched/sched.rs | `wake_idle_mask_set` | 1939 | 7 | yes |
| sched/sched.rs | `ww_heals` | 1945 | 5 | yes |
| sched/sched.rs | `ww_subgate_counts` | 1949 | 5 | yes |
| sched/sched.rs | `ww_rt_pick_stats` | 1953 | 5 | yes |
| sched/sched.rs | `heartbeat` | 1957 | 5 | yes |
| sched/sched.rs | `stallwatch_tick` | 1961 | 5 | yes |
| sched/sched.rs | `preempt_check_wake` | 1965 | 5 | yes |
| sched/sched.rs | `preempt_check_wake_sync` | 1969 | 5 | yes |
| sched/sched.rs | `w2_refusal_census` | 1973 | 5 | yes |
| sched/sched.rs | `task_sched_class` | 1977 | 5 | yes |
| sched/sched.rs | `ww_note_slot_mismatch` | 1981 | 5 | yes |
| sched/sched.rs | `monopolist_census` | 1985 | 5 | yes |
| sched/sched.rs | `runq_strand_census` | 1989 | 5 | yes |
| sched/sched.rs | `ffrt_wait_census` | 1993 | 5 | yes |
| sched/sched.rs | `rt_wedge_dump_tick` | 1997 | 5 | yes |
| sched/sched.rs | `rt_bandwidth_tick` | 2001 | 5 | yes |
| sched/sched.rs | `emit_grq_reports` | 2005 | 8 | yes |
| sched/sched.rs | `pick_gen_publish` | 2012 | 21 | yes |
| sched/sched.rs | `pick_next_task` | 2032 | 5 | yes |
| sched/sched.rs | `requeue_prev_locked` | 2036 | 5 | yes |
| sched/sched.rs | `g_diag_idle_bit` | 2040 | 5 | yes |
| sched/sched.rs | `cpu_tick_stale_ms` | 2044 | 5 | yes |
| sched/sched.rs | `lp_idle_lie_report` | 2048 | 5 | yes |
| sched/sched.rs | `sched_slice_to_ms` | 2052 | 5 | yes |
| sched/sched.rs | `sched_ev_drain` | 2056 | 5 | yes |
| sched/sched.rs | `note_task_freed` | 2060 | 5 | yes |
| sched/sched.rs | `report_poisoned_rq_entry` | 2064 | 72 | yes |
| sched/sched.rs | `new` | 2135 | 19 | yes |
| sched/sched.rs | `lock_irqsave` | 2153 | 15 | yes |
| sched/sched.rs | `lock_irqsave_site` | 2167 | 32 | yes |
| sched/sched.rs | `lock_irqsave_nested` | 2198 | 15 | yes |
| sched/sched.rs | `lock_irqsave_nested_site` | 2212 | 14 | yes |
| sched/sched.rs | `lock_plain` | 2225 | 16 | yes |
| sched/sched.rs | `try_lock_irqsave` | 2240 | 24 | yes |
| sched/sched.rs | `mark_idle` | 2263 | 8 | yes |
| sched/sched.rs | `clear_idle` | 2270 | 16 | yes |
| sched/sched.rs | `find_idle_cpu` | 2285 | 20 | yes |
| sched/sched.rs | `claim_idle_cpu` | 2304 | 17 | yes |
| sched/sched.rs | `claim_idle_cpu_prefer` | 2320 | 12 | yes |
| sched/sched.rs | `grq_nr_running` | 2331 | 6 | yes |
| sched/sched.rs | `rq_load` | 2336 | 64 | yes |
| sched/sched.rs | `unlock_irqretain` | 2399 | 20 | yes |
| sched/sched.rs | `deref` | 2418 | 8 | yes |
| sched/sched.rs | `deref_mut` | 2425 | 8 | yes |
| sched/sched.rs | `drop` | 2432 | 26 | yes |
| sched/sched.rs | `deref` | 2457 | 8 | yes |
| sched/sched.rs | `deref_mut` | 2464 | 8 | yes |
| sched/sched.rs | `drop` | 2471 | 23 | yes |
| sched/sched.rs | `new` | 2493 | 113 | yes |
| sched/sched.rs | `grq_site` | 2605 | 35 | yes |
| sched/sched.rs | `grq_current_fp` | 2639 | 23 | yes |
| sched/sched.rs | `grq_site_mark_at` | 2661 | 11 | yes |
| sched/sched.rs | `grq_site_mark_fp_at` | 2671 | 9 | yes |
| sched/sched.rs | `grq_site_name` | 2679 | 47 | yes |
| sched/sched.rs | `grq_dump_holder` | 2725 | 166 | yes |
| sched/sched.rs | `grq_wedge_arm` | 2890 | 8 | yes |
| sched/sched.rs | `grq_wedge_inject` | 2897 | 26 | yes |
| sched/sched.rs | `grq_held_here` | 2922 | 11 | yes |
| sched/sched.rs | `grq_init` | 2932 | 37 | yes |
| sched/sched.rs | `grq` | 2968 | 18 | yes |
| sched/sched.rs | `kernel_ptr_canonical` | 2985 | 16 | yes |
| sched/sched.rs | `kernel_text_window` | 3000 | 31 | yes |
| sched/sched.rs | `kernel_ptr_in_physmap` | 3030 | 40 | yes |
| sched/sched.rs | `kernel_task_ptr_range_ok` | 3069 | 21 | yes |
| sched/sched.rs | `try_grq_lock` | 3089 | 81 | yes |
| sched/sched.rs | `task_is_mid_switch_out` | 3169 | 39 | yes |
| sched/sched.rs | `pick_owner_live` | 3207 | 42 | yes |
| sched/sched.rs | `owner_slot_mask` | 3248 | 15 | yes |
| sched/sched.rs | `switching_out_mask` | 3262 | 58 | yes |
| sched/sched.rs | `member_release_stale_ownership` | 3319 | 96 | yes |
| sched/sched.rs | `switching_out_names` | 3414 | 21 | yes |
| sched/sched.rs | `stale_mark_confirm` | 3434 | 19 | yes |
| sched/sched.rs | `sched_cs_stamp` | 3452 | 19 | yes |
| sched/sched.rs | `slot_tag_write` | 3470 | 58 | yes |
| sched/sched.rs | `pack_notify` | 3527 | 6 | yes |
| sched/sched.rs | `unpack_notify` | 3532 | 12 | yes |
| sched/sched.rs | `defer_exit_notify` | 3543 | 24 | yes |
| sched/sched.rs | `deferred_exit_notify_slots` | 3566 | 14 | yes |
| sched/sched.rs | `build_sigchld_info` | 3579 | 24 | yes |
| sched/sched.rs | `process_deferred_exit_pid` | 3602 | 74 | yes |
| sched/sched.rs | `process_deferred_exit_notify_cpu` | 3675 | 79 | yes |
| sched/sched.rs | `cond_resched` | 3753 | 17 | yes |
| sched/sched.rs | `claim_idle_cpu_for` | 3769 | 32 | yes |
| sched/sched.rs | `claim_idle_cpu_for_prefer` | 3800 | 24 | yes |
| sched/sched.rs | `idle_task_ptr` | 3823 | 16 | yes |
| sched/sched.rs | `this_cpu` | 3838 | 14 | yes |
| sched/sched.rs | `this_cpu_mut` | 3851 | 9 | yes |
| sched/sched.rs | `cpu_state` | 3859 | 8 | yes |
| sched/sched.rs | `cpu_state_mut` | 3866 | 8 | yes |
| sched/sched.rs | `cpu_online` | 3873 | 16 | yes |
| sched/sched.rs | `cpu_running_idle` | 3888 | 13 | yes |
| sched/sched.rs | `init_per_cpu_rq` | 3900 | 15 | yes |
| sched/sched.rs | `init_secondary` | 3914 | 46 | yes |
| sched/sched.rs | `clear_trap_scratch` | 3959 | 9 | yes |
| sched/sched.rs | `init` | 3967 | 81 | yes |
| sched/sched.rs | `task_page_index` | 4047 | 19 | yes |
| sched/sched.rs | `task_page_bit` | 4065 | 7 | yes |
| sched/sched.rs | `task_page_mark` | 4071 | 7 | yes |
| sched/sched.rs | `task_page_unmark` | 4077 | 45 | yes |
| sched/sched.rs | `task_page_is_owned` | 4121 | 23 | yes |
| sched/sched.rs | `alloc_task_slot` | 4143 | 84 | yes |
| sched/sched.rs | `free_task_slot` | 4226 | 53 | yes |
| sched/sched.rs | `pdec_fts` | 4278 | 102 | yes |
| sched/sched.rs | `pdec3` | 4379 | 260 | yes |
| sched/sched.rs | `schedule` | 4638 | 135 | yes |
| sched/sched.rs | `terminal_release_grq_if_held` | 4772 | 58 | yes |
| sched/sched.rs | `grq_raw_puts` | 4829 | 7 | yes |
| sched/sched.rs | `grq_raw_putdec` | 4835 | 18 | yes |
| sched/sched.rs | `grq_raw_puthex` | 4852 | 11 | yes |
| sched/sched.rs | `schedule_dead_never_returns` | 4862 | 173 | yes |
| sched/sched.rs | `swapper_comm` | 5034 | 10 | yes |
| sched/sched.rs | `__schedule` | 5043 | 370 | yes |
| sched/sched.rs | `pdec3` | 5412 | 781 | yes |
| sched/sched.rs | `grq_diag_cfs_linked` | 6192 | 15 | yes |
| sched/sched.rs | `grq_diag_counts` | 6206 | 14 | yes |
| sched/sched.rs | `grq_idle_mask` | 6219 | 6 | yes |
| sched/sched.rs | `grq_nr_running_now` | 6224 | 9 | yes |
| sched/sched.rs | `grq_diag_cpu_currents` | 6232 | 28 | yes |
| sched/sched.rs | `scheduler_tick` | 6259 | 295 | yes |
| sched/sched.rs | `ph` | 6553 | 176 | yes |
| sched/sched.rs | `ph` | 6728 | 119 | yes |
| sched/sched.rs | `ph` | 6846 | 133 | yes |
| sched/sched.rs | `ph` | 6978 | 309 | yes |
| sched/sched.rs | `ph` | 7286 | 37 | yes |
| sched/sched.rs | `ph` | 7322 | 50 | yes |
| sched/sched.rs | `ph` | 7371 | 64 | yes |
| sched/sched.rs | `pdec` | 7434 | 519 | yes |
| sched/sched.rs | `grq_cfs_adjust` | 7952 | 27 | yes |
| sched/sched.rs | `for_each_task` | 7978 | 31 | yes |
| sched/sched.rs | `for_each_task_try` | 8008 | 9 | yes |
| sched/sched.rs | `get_current_pid` | 8016 | 11 | yes |
| sched/sched.rs | `get_current_ppid` | 8026 | 11 | yes |
| sched/sched.rs | `find_task_by_pid` | 8036 | 7 | yes |
| sched/sched.rs | `this_cpu_rq` | 8042 | 5 | yes |
| sched/sched.rs | `cpu_rq` | 8046 | 10 | yes |
| sched/sched.rs | `task_onrq_flag` | 8055 | 22 | yes |
| sched/sched.rs | `harvest_print_parked` | 8076 | 16 | yes |
| sched/sched.rs | `pdec` | 8091 | 19 | yes |
| sched/sched.rs | `phex` | 8109 | 100 | yes |
| sched/sched.rs | `task_is_dying` | 8208 | 16 | yes |
| sched/sched.rs | `harvest_relink` | 8223 | 252 | yes |
| sched/sched.rs | `republish_current_after_abort` | 8474 | 54 | yes |
| sched/sched.rs | `harvest_orphan_tasks` | 8527 | 184 | yes |
| sched/sched.rs | `idle_heal_own_slot` | 8710 | 107 | yes |
| sched/sched.rs | `relink_unlinked_running_phantoms` | 8816 | 255 | yes |
| sched/sched.rs | `cpu_idle_loop` | 9070 | 27 | yes |
| sched/sched.rs | `idle_guard_sp` | 9096 | 8 | yes |
| sched/sched.rs | `idle_guard_sp` | 9103 | 8 | yes |
| sched/sched.rs | `idle_guard_sp` | 9110 | 7 | yes |
| sched/sched.rs | `idle_guard_report` | 9116 | 3 | yes |
| sched/sched.rs | `ph` | 9118 | 310 | yes |
| sched/sched_hooks.rs | `sched_register_hooks` | 249 | 6 |  |
| sched/sched_hooks.rs | `hooks` | 254 | 16 |  |
| sched/sched_hooks.rs | `r56_print_pid` | 269 | 4 | yes |
| sched/sched_hooks.rs | `sched_clock` | 272 | 4 | yes |
| sched/sched_hooks.rs | `change_task_policy` | 275 | 11 | yes |
| sched/sched_hooks.rs | `ww_record` | 285 | 9 | yes |
| sched/sched_hooks.rs | `orphan_watch_confirm` | 293 | 4 | yes |
| sched/sched_hooks.rs | `yield_cpu` | 296 | 4 | yes |
| sched/sched_hooks.rs | `enqueue_task_locked` | 299 | 4 | yes |
| sched/sched_hooks.rs | `ww_watch_add` | 302 | 4 | yes |
| sched/sched_hooks.rs | `abort_next_release_orphans` | 305 | 4 | yes |
| sched/sched_hooks.rs | `r56_report_switch_abort` | 308 | 4 | yes |
| sched/sched_hooks.rs | `sched_renice_locked` | 311 | 8 | yes |
| sched/sched_hooks.rs | `psledger_consume` | 318 | 9 | yes |
| sched/sched_hooks.rs | `steer_task_cpu` | 326 | 4 | yes |
| sched/sched_hooks.rs | `sched_can_stop_tick` | 329 | 4 | yes |
| sched/sched_hooks.rs | `dl_throttle` | 332 | 4 | yes |
| sched/sched_hooks.rs | `ensure_linked_locked` | 335 | 4 | yes |
| sched/sched_hooks.rs | `check_cfs_preempt` | 338 | 4 | yes |
| sched/sched_hooks.rs | `check_rt_preempt` | 341 | 4 | yes |
| sched/sched_hooks.rs | `check_dl_preempt` | 344 | 4 | yes |
| sched/sched_hooks.rs | `orphan_watch_clear` | 347 | 4 | yes |
| sched/sched_hooks.rs | `pick_giveups` | 350 | 4 | yes |
| sched/sched_hooks.rs | `ww_slot_mismatch_counters` | 353 | 4 | yes |
| sched/sched_hooks.rs | `ww_sched_snapshot` | 356 | 4 | yes |
| sched/sched_hooks.rs | `ww_stalled_cpu_state` | 359 | 4 | yes |
| sched/sched_hooks.rs | `debug_claim_inject` | 362 | 4 | yes |
| sched/sched_hooks.rs | `debug_oncpu_inject` | 365 | 4 | yes |
| sched/sched_hooks.rs | `ww_heals` | 368 | 4 | yes |
| sched/sched_hooks.rs | `ww_subgate_counts` | 371 | 4 | yes |
| sched/sched_hooks.rs | `ww_rt_pick_stats` | 374 | 4 | yes |
| sched/sched_hooks.rs | `heartbeat` | 377 | 4 | yes |
| sched/sched_hooks.rs | `stallwatch_tick` | 380 | 4 | yes |
| sched/sched_hooks.rs | `preempt_check_wake` | 383 | 4 | yes |
| sched/sched_hooks.rs | `preempt_check_wake_sync` | 386 | 4 | yes |
| sched/sched_hooks.rs | `w2_refusal_census` | 389 | 4 | yes |
| sched/sched_hooks.rs | `task_sched_class` | 392 | 4 | yes |
| sched/sched_hooks.rs | `ww_note_slot_mismatch` | 395 | 4 | yes |
| sched/sched_hooks.rs | `monopolist_census` | 398 | 4 | yes |
| sched/sched_hooks.rs | `runq_strand_census` | 401 | 4 | yes |
| sched/sched_hooks.rs | `ffrt_wait_census` | 404 | 4 | yes |
| sched/sched_hooks.rs | `rt_wedge_dump_tick` | 407 | 4 | yes |
| sched/sched_hooks.rs | `rt_bandwidth_tick` | 410 | 4 | yes |
| sched/sched_hooks.rs | `emit_grq_reports` | 413 | 4 | yes |
| sched/sched_hooks.rs | `pick_next_task` | 416 | 4 | yes |
| sched/sched_hooks.rs | `requeue_prev_locked` | 419 | 4 | yes |
| sched/sched_hooks.rs | `g_diag_idle_bit` | 422 | 4 | yes |
| sched/sched_hooks.rs | `cpu_tick_stale_ms` | 425 | 4 | yes |
| sched/sched_hooks.rs | `lp_idle_lie_report` | 428 | 4 | yes |
| sched/sched_hooks.rs | `sched_slice_to_ms` | 431 | 4 | yes |
| sched/sched_hooks.rs | `sched_ev_drain` | 434 | 4 | yes |
| sched/sched_hooks.rs | `note_task_freed` | 437 | 4 | yes |
| sched/sched_hooks.rs | `report_poisoned_rq_entry` | 440 | 4 | yes |
| sched/sched_hooks.rs | `psledger_task` | 443 | 4 |  |
| sched/sched_hooks.rs | `psledger_cpu` | 446 | 4 |  |
| sched/sched_hooks.rs | `psledger_jif` | 449 | 4 |  |
| sched/sched_hooks.rs | `psledger_pk` | 452 | 4 |  |
| sched/sched_hooks.rs | `ww_watch` | 455 | 3 |  |
| sched/sched_hooks.rs | `ww_watch_slots` | 457 | 4 |  |
| sched/sched_hooks.rs | `load_balance` | 460 | 6 | yes |
| sched/sched_hooks.rs | `psledger_note_publish` | 465 | 9 | yes |
| sched/sched_hooks.rs | `psledger_peek` | 473 | 4 | yes |
| sched/sched_hooks.rs | `sched_hist_ring` | 476 | 4 | yes |
| sched/sched_hooks.rs | `sched_hist_refuse` | 479 | 4 | yes |
| sched/sched_hooks.rs | `sched_hist_link` | 482 | 4 | yes |
| sched/sched_hooks.rs | `sched_hist_pick` | 485 | 4 | yes |
| sched/sched_hooks.rs | `ww_cfs_geometry` | 488 | 4 | yes |
| sched/sched_hooks.rs | `ww_cfs_tie_mass` | 491 | 4 | yes |
| sched/sched_hooks.rs | `ww_cfs_tie_mass_vr` | 494 | 4 | yes |
| sched/sched_hooks.rs | `ww_tail_fill` | 497 | 4 | yes |
| sched/sched_hooks.rs | `ww_append_dump` | 500 | 4 | yes |
| sched/sched_hooks.rs | `fair_sched_class` | 503 | 3 |  |
| sched/sched_hooks.rs | `rt_sched_class` | 505 | 3 |  |
| sched/sched_hooks.rs | `dl_sched_class` | 507 | 3 |  |
| sched/sched_hooks.rs | `idle_sched_class` | 509 | 3 |  |
| sched/sched_hooks.rs | `stop_sched_class` | 511 | 4 |  |
| sched/sched_hooks.rs | `sched_ev` | 514 | 3 |  |
| sched/sched_hooks.rs | `stale_oncpu_healed` | 516 | 9 |  |
| sched/sched_hooks.rs | `vr_canary` | 524 | 7 |  |
| sched/side_edges.rs | `evring_slot_mut` | 32 | 16 |  |
| sched/side_edges.rs | `evring_slot_ref` | 47 | 19 |  |
| sched/side_edges.rs | `new` | 65 | 9 | yes |
