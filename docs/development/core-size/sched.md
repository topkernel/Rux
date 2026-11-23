# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# sched — hot 12731 lines (stay) / cold 2232 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| sched/cgroup.rs | `new` | 112 | 72 | yes |
| sched/cgroup.rs | `new_root` | 184 | 20 |  |
| sched/cgroup.rs | `new_child` | 204 | 23 |  |
| sched/cgroup.rs | `path` | 227 | 29 | yes |
| sched/cgroup.rs | `for_each_ancestor` | 256 | 31 |  |
| sched/cgroup.rs | `chain_throttled` | 287 | 12 |  |
| sched/cgroup.rs | `populated` | 299 | 5 |  |
| sched/cgroup.rs | `subtree_task_count` | 304 | 27 |  |
| sched/cgroup.rs | `init_cgroup` | 331 | 9 |  |
| sched/cgroup.rs | `cgroup_root` | 340 | 5 |  |
| sched/cgroup.rs | `alloc_ino_base` | 345 | 11 |  |
| sched/cgroup.rs | `valid_cgroup_name` | 356 | 9 |  |
| sched/cgroup.rs | `cgroup_create` | 365 | 25 |  |
| sched/cgroup.rs | `cgroup_rmdir` | 390 | 33 |  |
| sched/cgroup.rs | `cgroup_lookup_child` | 423 | 22 |  |
| sched/cgroup.rs | `task_cgroup` | 445 | 41 |  |
| sched/class.rs | `name` | 57 | 8 | yes |
| sched/class.rs | `enqueue_task` | 65 | 11 | yes |
| sched/class.rs | `dequeue_task` | 76 | 3 | yes |
| sched/class.rs | `yield_task` | 79 | 8 |  |
| sched/class.rs | `wakeup_preempt` | 87 | 10 |  |
| sched/class.rs | `pick_next_task` | 97 | 10 | yes |
| sched/class.rs | `put_prev_task` | 107 | 10 |  |
| sched/class.rs | `set_next_task` | 117 | 12 |  |
| sched/class.rs | `balance` | 129 | 11 |  |
| sched/class.rs | `select_task_rq` | 140 | 10 |  |
| sched/class.rs | `task_tick` | 150 | 6 |  |
| sched/class.rs | `update_curr` | 156 | 10 | yes |
| sched/class.rs | `get_rr_interval` | 166 | 9 |  |
| sched/class.rs | `has_runnable` | 175 | 5 |  |
| sched/class.rs | `next_class` | 180 | 21 |  |
| sched/class.rs | `to_class` | 201 | 15 |  |
| sched/class.rs | `task_sched_class` | 216 | 18 | yes |
| sched/class.rs | `new` | 234 | 10 | yes |
| sched/class.rs | `next` | 244 | 14 | yes |
| sched/class.rs | `from` | 258 | 10 | yes |
| sched/class.rs | `next` | 268 | 6 | yes |
| sched/edges.rs | `idle_task_ptr` | 48 | 12 | yes |
| sched/edges.rs | `idle_task_storage_base` | 60 | 27 | yes |
| sched/edges.rs | `task_mut` | 87 | 18 | yes |
| sched/edges.rs | `task_shared` | 105 | 22 | yes |
| sched/edges.rs | `grq_is_ready` | 127 | 6 | yes |
| sched/edges.rs | `grq_claim_init` | 133 | 14 | yes |
| sched/edges.rs | `grq_stage` | 147 | 12 | yes |
| sched/edges.rs | `grq_publish_init` | 159 | 6 | yes |
| sched/edges.rs | `grq_ref` | 165 | 12 | yes |
| sched/edges.rs | `grq_mut` | 177 | 13 | yes |
| sched/edges.rs | `task_poison_pair_volatile` | 190 | 20 | yes |
| sched/edges.rs | `task_pid_volatile` | 210 | 18 | yes |
| sched/edges.rs | `task_ownership_qword` | 228 | 17 | yes |
| sched/edges.rs | `current_trap_ip` | 245 | 17 | yes |
| sched/edges.rs | `current_task_mut` | 262 | 28 | yes |
| sched/edges.rs | `arch_context_switch` | 290 | 10 | yes |
| sched/edges.rs | `task_borrow` | 300 | 11 | yes |
| sched/edges.rs | `cgroup_node_ref` | 311 | 18 | yes |
| sched/edges.rs | `pt_regs_at_top` | 329 | 18 | yes |
| sched/edges.rs | `list_next` | 347 | 9 | yes |
| sched/edges.rs | `task_of_rt_list` | 356 | 9 | yes |
| sched/edges.rs | `rt_list_add` | 365 | 8 | yes |
| sched/edges.rs | `rt_list_add_tail` | 373 | 8 | yes |
| sched/edges.rs | `rt_list_del` | 381 | 9 | yes |
| sched/edges.rs | `rt_list_linked` | 390 | 38 | yes |
| sched/edges.rs | `new` | 428 | 60 | yes |
| sched/edges.rs | `linked_at` | 488 | 13 | yes |
| sched/edges.rs | `detach_at` | 501 | 11 | yes |
| sched/edges.rs | `node_ok_shared` | 512 | 4 | yes |
| sched/edges.rs | `unlinked` | 516 | 19 | yes |
| sched/edges.rs | `rb_nil` | 535 | 8 | yes |
| sched/edges.rs | `node_of_task` | 543 | 8 | yes |
| sched/edges.rs | `task_of_node` | 551 | 9 | yes |
| sched/edges.rs | `node_key` | 560 | 27 | yes |
| sched/edges.rs | `new` | 587 | 8 | yes |
| sched/edges.rs | `nil` | 595 | 5 | yes |
| sched/edges.rs | `is_empty` | 600 | 7 | yes |
| sched/edges.rs | `node_ok` | 607 | 5 | yes |
| sched/edges.rs | `subtree_min` | 612 | 14 | yes |
| sched/edges.rs | `next` | 626 | 21 | yes |
| sched/edges.rs | `rotate_left` | 647 | 22 | yes |
| sched/edges.rs | `rotate_right` | 669 | 25 | yes |
| sched/edges.rs | `insert` | 694 | 46 | yes |
| sched/edges.rs | `insert_fixup` | 740 | 51 | yes |
| sched/edges.rs | `erase` | 791 | 123 | yes |
| sched/edges.rs | `erase_fixup` | 914 | 5 | yes |
| sched/edges.rs | `kid` | 919 | 124 | yes |
| sched/edges.rs | `debug_insert` | 1043 | 5 | yes |
| sched/edges.rs | `debug_erase` | 1048 | 5 | yes |
| sched/edges.rs | `debug_erase_leftmost` | 1053 | 7 | yes |
| sched/edges.rs | `debug_leftmost_key` | 1060 | 12 | yes |
| sched/edges.rs | `debug_walk_keys` | 1072 | 15 | yes |
| sched/edges.rs | `debug_black_height` | 1087 | 2 | yes |
| sched/edges.rs | `bh` | 1089 | 44 | yes |
| sched/edges.rs | `grq_ptr_shared` | 1133 | 8 | yes |
| sched/edges.rs | `grq_ptr_mut` | 1141 | 18 | yes |
| sched/edges.rs | `per_cpu_shared` | 1159 | 10 | yes |
| sched/edges.rs | `per_cpu_mut` | 1169 | 19 | yes |
| sched/edges.rs | `secondary_idle_publish` | 1188 | 7 | yes |
| sched/edges.rs | `secondary_idle_table_base` | 1195 | 6 | yes |
| sched/edges.rs | `secondary_idle_task` | 1201 | 4 | yes |
| sched/entities.rs | `sched_clock` | 68 | 11 | yes |
| sched/entities.rs | `sched_slice_to_ms` | 79 | 5 | yes |
| sched/entities.rs | `ms_to_ns` | 84 | 82 |  |
| sched/entities.rs | `new` | 166 | 16 | yes |
| sched/entities.rs | `new_const` | 182 | 11 |  |
| sched/entities.rs | `init` | 193 | 8 | yes |
| sched/entities.rs | `nr_running` | 201 | 6 | yes |
| sched/entities.rs | `bitmap_word` | 207 | 11 |  |
| sched/entities.rs | `highest_linked_user_prio` | 218 | 39 |  |
| sched/entities.rs | `walk_prio` | 257 | 26 |  |
| sched/entities.rs | `is_empty` | 283 | 9 | yes |
| sched/entities.rs | `rt_nr_dec` | 292 | 17 |  |
| sched/entities.rs | `rr_nr_dec` | 309 | 14 |  |
| sched/entities.rs | `find_highest_prio` | 323 | 20 |  |
| sched/entities.rs | `enqueue` | 343 | 67 | yes |
| sched/entities.rs | `dequeue` | 410 | 63 | yes |
| sched/entities.rs | `is_linked` | 473 | 19 | yes |
| sched/entities.rs | `force_unlink` | 492 | 60 | yes |
| sched/entities.rs | `pick_next` | 552 | 24 |  |
| sched/entities.rs | `peek_next` | 576 | 35 |  |
| sched/entities.rs | `new` | 611 | 13 | yes |
| sched/entities.rs | `from_nice` | 624 | 12 |  |
| sched/entities.rs | `update_inv_weight` | 636 | 11 |  |
| sched/entities.rs | `default` | 647 | 65 | yes |
| sched/entities.rs | `new` | 712 | 17 | yes |
| sched/entities.rs | `from_nice` | 729 | 17 |  |
| sched/entities.rs | `set_nice` | 746 | 6 |  |
| sched/entities.rs | `get_vruntime` | 752 | 6 |  |
| sched/entities.rs | `set_vruntime` | 758 | 6 |  |
| sched/entities.rs | `add_vruntime` | 764 | 11 |  |
| sched/entities.rs | `update_exec_runtime` | 775 | 39 | yes |
| sched/entities.rs | `calc_delta_fair` | 814 | 25 |  |
| sched/entities.rs | `update_vruntime` | 839 | 8 | yes |
| sched/entities.rs | `is_on_rq` | 847 | 6 | yes |
| sched/entities.rs | `set_on_rq` | 853 | 7 |  |
| sched/entities.rs | `timeline_key` | 860 | 8 | yes |
| sched/entities.rs | `set_timeline_key` | 868 | 6 |  |
| sched/entities.rs | `get_slice` | 874 | 6 |  |
| sched/entities.rs | `set_slice` | 880 | 6 |  |
| sched/entities.rs | `get_exec_start` | 886 | 6 |  |
| sched/entities.rs | `set_exec_start` | 892 | 5 | yes |
| sched/entities.rs | `default` | 897 | 5 | yes |
| sched/entities.rs | `clone` | 902 | 33 | yes |
| sched/entities.rs | `new` | 935 | 9 | yes |
| sched/entities.rs | `is_on_rq` | 944 | 6 | yes |
| sched/entities.rs | `set_on_rq` | 950 | 6 |  |
| sched/entities.rs | `get_time_slice` | 956 | 6 |  |
| sched/entities.rs | `set_time_slice` | 962 | 6 | yes |
| sched/entities.rs | `dec_time_slice` | 968 | 12 | yes |
| sched/entities.rs | `reset_time_slice` | 980 | 5 | yes |
| sched/entities.rs | `default` | 985 | 47 | yes |
| sched/entities.rs | `new` | 1032 | 17 | yes |
| sched/entities.rs | `is_on_rq` | 1049 | 5 | yes |
| sched/entities.rs | `get_bw` | 1054 | 12 |  |
| sched/entities.rs | `update_deadline` | 1066 | 14 |  |
| sched/entities.rs | `replenish_runtime` | 1080 | 13 |  |
| sched/entities.rs | `consume_runtime` | 1093 | 15 | yes |
| sched/entities.rs | `default` | 1108 | 53 | yes |
| sched/entities.rs | `fmt` | 1161 | 42 | yes |
| sched/entities.rs | `new` | 1203 | 11 | yes |
| sched/entities.rs | `is_empty` | 1214 | 6 | yes |
| sched/entities.rs | `nr_running` | 1220 | 7 | yes |
| sched/entities.rs | `dl_nr_dec` | 1227 | 18 |  |
| sched/entities.rs | `enqueue` | 1245 | 61 | yes |
| sched/entities.rs | `dequeue` | 1306 | 56 | yes |
| sched/entities.rs | `is_linked` | 1362 | 8 | yes |
| sched/entities.rs | `pick_next` | 1370 | 26 |  |
| sched/entities.rs | `peek_next` | 1396 | 14 |  |
| sched/entities.rs | `pick_next_cpu` | 1410 | 82 |  |
| sched/entities.rs | `new` | 1492 | 16 | yes |
| sched/entities.rs | `leftmost_entry` | 1508 | 30 |  |
| sched/entities.rs | `avg_vruntime` | 1538 | 30 |  |
| sched/entities.rs | `capture_sleep_vlag` | 1568 | 41 | yes |
| sched/entities.rs | `timeline_insert` | 1609 | 14 |  |
| sched/entities.rs | `ph` | 1623 | 57 | yes |
| sched/entities.rs | `timeline_remove` | 1680 | 23 |  |
| sched/entities.rs | `ph` | 1703 | 30 | yes |
| sched/entities.rs | `nr_running` | 1733 | 14 | yes |
| sched/entities.rs | `nr_running_dec` | 1747 | 70 |  |
| sched/entities.rs | `load_weight_sub` | 1817 | 29 |  |
| sched/entities.rs | `is_empty` | 1846 | 6 | yes |
| sched/entities.rs | `get_min_vruntime` | 1852 | 7 |  |
| sched/entities.rs | `leftmost_vruntime` | 1859 | 21 |  |
| sched/entities.rs | `ww_tie_mass` | 1880 | 26 |  |
| sched/entities.rs | `update_min_vruntime` | 1906 | 43 | yes |
| sched/entities.rs | `enqueue` | 1949 | 8 | yes |
| sched/entities.rs | `enqueue_migrate` | 1957 | 4 |  |
| sched/entities.rs | `enqueue_inner` | 1961 | 120 |  |
| sched/entities.rs | `dequeue` | 2081 | 76 | yes |
| sched/entities.rs | `is_linked` | 2157 | 19 | yes |
| sched/entities.rs | `resync_nr_running_from_timeline` | 2176 | 25 | yes |
| sched/entities.rs | `timeline_head_diag` | 2201 | 45 | yes |
| sched/entities.rs | `strand_wall_probe` | 2246 | 134 |  |
| sched/entities.rs | `heal_report_budget_ok` | 2380 | 15 |  |
| sched/entities.rs | `stale_heal_line` | 2395 | 36 |  |
| sched/entities.rs | `stale_claim_healed` | 2431 | 19 |  |
| sched/entities.rs | `pdec5` | 2450 | 64 |  |
| sched/entities.rs | `stale_oncpu_healed` | 2514 | 63 |  |
| sched/entities.rs | `put_dec` | 2577 | 28 |  |
| sched/entities.rs | `claim_inject_scan` | 2605 | 33 |  |
| sched/entities.rs | `oncpu_inject_scan` | 2638 | 61 |  |
| sched/entities.rs | `claim_sweep` | 2699 | 124 | yes |
| sched/entities.rs | `pick_next` | 2823 | 46 |  |
| sched/entities.rs | `peek_next` | 2869 | 13 |  |
| sched/entities.rs | `update_curr` | 2882 | 33 | yes |
| sched/entities.rs | `sched_slice` | 2915 | 66 | yes |
| sched/entities.rs | `check_preempt` | 2981 | 19 |  |
| sched/entities.rs | `set_curr` | 3000 | 6 | yes |
| sched/entities.rs | `get_curr` | 3006 | 5 | yes |
| sched/entities.rs | `clear` | 3011 | 119 | yes |
| sched/reapdrain.rs | `mint` | 131 | 8 |  |
| sched/reapdrain.rs | `as_ptr` | 139 | 8 | yes |
| sched/reapdrain.rs | `task` | 147 | 14 |  |
| sched/reapdrain.rs | `into_parked` | 161 | 9 |  |
| sched/reapdrain.rs | `into_free` | 170 | 88 |  |
| sched/reapdrain.rs | `defer_reap_storage` | 258 | 33 |  |
| sched/reapdrain.rs | `switch_stage_done` | 291 | 45 |  |
| sched/reapdrain.rs | `reap_drain_tick` | 336 | 214 | yes |
| sched/reapdrain.rs | `notify_parent_if_any` | 550 | 9 |  |
| sched/reapdrain.rs | `reap_storage_free` | 559 | 16 |  |
| sched/sched.rs | `need_resched` | 40 | 13 | yes |
| sched/sched.rs | `set_need_resched` | 53 | 10 | yes |
| sched/sched.rs | `clear_need_resched` | 63 | 7 | yes |
| sched/sched.rs | `resched_curr` | 70 | 5 | yes |
| sched/sched.rs | `resched_cpu` | 75 | 38 | yes |
| sched/sched.rs | `ph` | 113 | 33 | yes |
| sched/sched.rs | `wake_up_process` | 146 | 8 | yes |
| sched/sched.rs | `wake_up_process_sync` | 154 | 17 | yes |
| sched/sched.rs | `enqueue_task` | 171 | 70 | yes |
| sched/sched.rs | `wake_up_enqueue` | 241 | 9 | yes |
| sched/sched.rs | `wake_up_enqueue_nested` | 250 | 11 |  |
| sched/sched.rs | `wake_up_enqueue_opt` | 261 | 827 | yes |
| sched/sched.rs | `ph` | 1088 | 42 | yes |
| sched/sched.rs | `pd2` | 1130 | 58 | yes |
| sched/sched.rs | `pdec` | 1188 | 153 | yes |
| sched/sched.rs | `dequeue_task` | 1341 | 57 | yes |
| sched/sched.rs | `dequeue_if_enqueued` | 1398 | 18 | yes |
| sched/sched.rs | `context_switch` | 1416 | 40 | yes |
| sched/sched.rs | `ph` | 1456 | 80 | yes |
| sched/sched.rs | `current` | 1536 | 12 | yes |
| sched/sched.rs | `current_is_dying` | 1548 | 25 | yes |
| sched/sched.rs | `class_nonempty_set` | 1573 | 6 | yes |
| sched/sched.rs | `class_nonempty_clear` | 1579 | 6 | yes |
| sched/sched.rs | `class_nonempty` | 1585 | 5 | yes |
| sched/sched.rs | `load_balance` | 1590 | 3 | yes |
| sched/sched.rs | `psledger_note_publish` | 1593 | 3 | yes |
| sched/sched.rs | `psledger_peek` | 1596 | 7 | yes |
| sched/sched.rs | `sched_hist_armed` | 1603 | 5 | yes |
| sched/sched.rs | `sched_hist_ring` | 1608 | 3 | yes |
| sched/sched.rs | `sched_hist_refuse` | 1611 | 3 | yes |
| sched/sched.rs | `sched_hist_link` | 1614 | 3 | yes |
| sched/sched.rs | `sched_hist_pick` | 1617 | 3 | yes |
| sched/sched.rs | `ww_cfs_geometry` | 1620 | 3 | yes |
| sched/sched.rs | `ww_cfs_tie_mass` | 1623 | 3 | yes |
| sched/sched.rs | `ww_cfs_tie_mass_vr` | 1626 | 3 | yes |
| sched/sched.rs | `ww_tail_fill` | 1629 | 3 | yes |
| sched/sched.rs | `ww_append_dump` | 1632 | 10 | yes |
| sched/sched.rs | `enabled` | 1642 | 7 | yes |
| sched/sched.rs | `set_enabled` | 1649 | 6 | yes |
| sched/sched.rs | `r56_print_pid` | 1655 | 4 | yes |
| sched/sched.rs | `sched_clock` | 1659 | 6 | yes |
| sched/sched.rs | `pick_trace` | 1665 | 43 | yes |
| sched/sched.rs | `pick_gen_supersede` | 1708 | 7 | yes |
| sched/sched.rs | `change_task_policy` | 1715 | 4 | yes |
| sched/sched.rs | `ww_record` | 1719 | 4 | yes |
| sched/sched.rs | `orphan_watch_confirm` | 1723 | 5 | yes |
| sched/sched.rs | `pick_trace_peek` | 1728 | 17 | yes |
| sched/sched.rs | `yield_cpu` | 1745 | 4 | yes |
| sched/sched.rs | `enqueue_task_locked` | 1749 | 4 | yes |
| sched/sched.rs | `ww_watch_add` | 1753 | 4 | yes |
| sched/sched.rs | `abort_next_release_orphans` | 1757 | 4 | yes |
| sched/sched.rs | `r56_report_switch_abort` | 1761 | 4 | yes |
| sched/sched.rs | `sched_renice_locked` | 1765 | 6 | yes |
| sched/sched.rs | `pick_gen_retire` | 1771 | 10 | yes |
| sched/sched.rs | `psledger_consume` | 1781 | 4 | yes |
| sched/sched.rs | `steer_task_cpu` | 1785 | 4 | yes |
| sched/sched.rs | `sched_can_stop_tick` | 1789 | 4 | yes |
| sched/sched.rs | `dl_throttle` | 1793 | 4 | yes |
| sched/sched.rs | `ensure_linked_locked` | 1797 | 4 | yes |
| sched/sched.rs | `check_cfs_preempt` | 1801 | 4 | yes |
| sched/sched.rs | `check_rt_preempt` | 1805 | 4 | yes |
| sched/sched.rs | `check_dl_preempt` | 1809 | 4 | yes |
| sched/sched.rs | `orphan_watch_clear` | 1813 | 4 | yes |
| sched/sched.rs | `pick_giveups` | 1817 | 4 | yes |
| sched/sched.rs | `ww_slot_mismatch_counters` | 1821 | 4 | yes |
| sched/sched.rs | `ww_sched_snapshot` | 1825 | 4 | yes |
| sched/sched.rs | `ww_stalled_cpu_state` | 1829 | 4 | yes |
| sched/sched.rs | `debug_claim_inject` | 1833 | 4 | yes |
| sched/sched.rs | `debug_oncpu_inject` | 1837 | 5 | yes |
| sched/sched.rs | `ww_pick_walks` | 1842 | 4 | yes |
| sched/sched.rs | `ww_pick_walk_inc` | 1846 | 4 | yes |
| sched/sched.rs | `ww_oncpu_heals` | 1850 | 4 | yes |
| sched/sched.rs | `ww_oncpu_heal_inc` | 1854 | 4 | yes |
| sched/sched.rs | `ww_claim_heals` | 1858 | 4 | yes |
| sched/sched.rs | `ww_claim_heal_inc` | 1862 | 4 | yes |
| sched/sched.rs | `tl_poison_inserts` | 1866 | 4 | yes |
| sched/sched.rs | `tl_poison_insert_inc` | 1870 | 4 | yes |
| sched/sched.rs | `tl_poison_erases` | 1874 | 4 | yes |
| sched/sched.rs | `tl_poison_erase_inc` | 1878 | 7 | yes |
| sched/sched.rs | `sched_cs_phase` | 1885 | 8 | yes |
| sched/sched.rs | `sched_cs_phase_set` | 1893 | 6 | yes |
| sched/sched.rs | `sched_calls` | 1899 | 8 | yes |
| sched/sched.rs | `sched_calls_inc` | 1907 | 6 | yes |
| sched/sched.rs | `ww_idle_phase` | 1913 | 8 | yes |
| sched/sched.rs | `ww_idle_phase_set` | 1921 | 6 | yes |
| sched/sched.rs | `wake_disp` | 1927 | 4 | yes |
| sched/sched.rs | `wake_disp_set` | 1931 | 4 | yes |
| sched/sched.rs | `wake_idle_mask` | 1935 | 4 | yes |
| sched/sched.rs | `wake_idle_mask_set` | 1939 | 6 | yes |
| sched/sched.rs | `ww_heals` | 1945 | 4 | yes |
| sched/sched.rs | `ww_subgate_counts` | 1949 | 4 | yes |
| sched/sched.rs | `ww_rt_pick_stats` | 1953 | 4 | yes |
| sched/sched.rs | `heartbeat` | 1957 | 4 | yes |
| sched/sched.rs | `stallwatch_tick` | 1961 | 4 | yes |
| sched/sched.rs | `preempt_check_wake` | 1965 | 4 | yes |
| sched/sched.rs | `preempt_check_wake_sync` | 1969 | 4 | yes |
| sched/sched.rs | `w2_refusal_census` | 1973 | 4 | yes |
| sched/sched.rs | `task_sched_class` | 1977 | 4 | yes |
| sched/sched.rs | `ww_note_slot_mismatch` | 1981 | 4 | yes |
| sched/sched.rs | `monopolist_census` | 1985 | 4 | yes |
| sched/sched.rs | `runq_strand_census` | 1989 | 4 | yes |
| sched/sched.rs | `ffrt_wait_census` | 1993 | 4 | yes |
| sched/sched.rs | `rt_wedge_dump_tick` | 1997 | 4 | yes |
| sched/sched.rs | `rt_bandwidth_tick` | 2001 | 4 | yes |
| sched/sched.rs | `emit_grq_reports` | 2005 | 7 | yes |
| sched/sched.rs | `pick_gen_publish` | 2012 | 20 | yes |
| sched/sched.rs | `pick_next_task` | 2032 | 4 | yes |
| sched/sched.rs | `requeue_prev_locked` | 2036 | 4 | yes |
| sched/sched.rs | `g_diag_idle_bit` | 2040 | 4 | yes |
| sched/sched.rs | `cpu_tick_stale_ms` | 2044 | 4 | yes |
| sched/sched.rs | `lp_idle_lie_report` | 2048 | 4 | yes |
| sched/sched.rs | `sched_slice_to_ms` | 2052 | 4 | yes |
| sched/sched.rs | `sched_ev_drain` | 2056 | 4 | yes |
| sched/sched.rs | `note_task_freed` | 2060 | 4 | yes |
| sched/sched.rs | `report_poisoned_rq_entry` | 2064 | 71 | yes |
| sched/sched.rs | `new` | 2135 | 18 | yes |
| sched/sched.rs | `lock_irqsave` | 2153 | 14 | yes |
| sched/sched.rs | `lock_irqsave_site` | 2167 | 31 | yes |
| sched/sched.rs | `lock_irqsave_nested` | 2198 | 14 |  |
| sched/sched.rs | `lock_irqsave_nested_site` | 2212 | 13 | yes |
| sched/sched.rs | `lock_plain` | 2225 | 15 | yes |
| sched/sched.rs | `try_lock_irqsave` | 2240 | 23 | yes |
| sched/sched.rs | `mark_idle` | 2263 | 7 | yes |
| sched/sched.rs | `clear_idle` | 2270 | 15 | yes |
| sched/sched.rs | `find_idle_cpu` | 2285 | 19 | yes |
| sched/sched.rs | `claim_idle_cpu` | 2304 | 16 | yes |
| sched/sched.rs | `claim_idle_cpu_prefer` | 2320 | 11 | yes |
| sched/sched.rs | `grq_nr_running` | 2331 | 5 | yes |
| sched/sched.rs | `rq_load` | 2336 | 63 | yes |
| sched/sched.rs | `unlock_irqretain` | 2399 | 19 | yes |
| sched/sched.rs | `deref` | 2418 | 7 | yes |
| sched/sched.rs | `deref_mut` | 2425 | 7 | yes |
| sched/sched.rs | `drop` | 2432 | 25 | yes |
| sched/sched.rs | `deref` | 2457 | 7 | yes |
| sched/sched.rs | `deref_mut` | 2464 | 7 | yes |
| sched/sched.rs | `drop` | 2471 | 22 | yes |
| sched/sched.rs | `new` | 2493 | 112 | yes |
| sched/sched.rs | `grq_site` | 2605 | 34 | yes |
| sched/sched.rs | `grq_current_fp` | 2639 | 22 | yes |
| sched/sched.rs | `grq_site_mark_at` | 2661 | 10 | yes |
| sched/sched.rs | `grq_site_mark_fp_at` | 2671 | 8 | yes |
| sched/sched.rs | `grq_site_name` | 2679 | 46 | yes |
| sched/sched.rs | `grq_dump_holder` | 2725 | 165 | yes |
| sched/sched.rs | `grq_wedge_arm` | 2890 | 7 | yes |
| sched/sched.rs | `grq_wedge_inject` | 2897 | 25 | yes |
| sched/sched.rs | `grq_held_here` | 2922 | 10 | yes |
| sched/sched.rs | `grq_init` | 2932 | 36 | yes |
| sched/sched.rs | `grq` | 2968 | 17 | yes |
| sched/sched.rs | `kernel_ptr_canonical` | 2985 | 15 | yes |
| sched/sched.rs | `kernel_text_window` | 3000 | 30 | yes |
| sched/sched.rs | `kernel_ptr_in_physmap` | 3030 | 39 | yes |
| sched/sched.rs | `kernel_task_ptr_range_ok` | 3069 | 20 | yes |
| sched/sched.rs | `try_grq_lock` | 3089 | 80 | yes |
| sched/sched.rs | `task_is_mid_switch_out` | 3169 | 38 | yes |
| sched/sched.rs | `pick_owner_live` | 3207 | 41 | yes |
| sched/sched.rs | `owner_slot_mask` | 3248 | 14 | yes |
| sched/sched.rs | `switching_out_mask` | 3262 | 57 | yes |
| sched/sched.rs | `member_release_stale_ownership` | 3319 | 95 | yes |
| sched/sched.rs | `switching_out_names` | 3414 | 20 | yes |
| sched/sched.rs | `stale_mark_confirm` | 3434 | 18 | yes |
| sched/sched.rs | `sched_cs_stamp` | 3452 | 18 | yes |
| sched/sched.rs | `slot_tag_write` | 3470 | 57 | yes |
| sched/sched.rs | `pack_notify` | 3527 | 5 | yes |
| sched/sched.rs | `unpack_notify` | 3532 | 11 | yes |
| sched/sched.rs | `defer_exit_notify` | 3543 | 23 | yes |
| sched/sched.rs | `deferred_exit_notify_slots` | 3566 | 13 | yes |
| sched/sched.rs | `build_sigchld_info` | 3579 | 23 | yes |
| sched/sched.rs | `process_deferred_exit_pid` | 3602 | 73 | yes |
| sched/sched.rs | `process_deferred_exit_notify_cpu` | 3675 | 78 | yes |
| sched/sched.rs | `cond_resched` | 3753 | 16 | yes |
| sched/sched.rs | `claim_idle_cpu_for` | 3769 | 31 | yes |
| sched/sched.rs | `claim_idle_cpu_for_prefer` | 3800 | 23 | yes |
| sched/sched.rs | `idle_task_ptr` | 3823 | 15 | yes |
| sched/sched.rs | `this_cpu` | 3838 | 13 | yes |
| sched/sched.rs | `this_cpu_mut` | 3851 | 8 | yes |
| sched/sched.rs | `cpu_state` | 3859 | 7 | yes |
| sched/sched.rs | `cpu_state_mut` | 3866 | 7 | yes |
| sched/sched.rs | `cpu_online` | 3873 | 15 | yes |
| sched/sched.rs | `cpu_running_idle` | 3888 | 12 | yes |
| sched/sched.rs | `init_per_cpu_rq` | 3900 | 14 | yes |
| sched/sched.rs | `init_secondary` | 3914 | 45 | yes |
| sched/sched.rs | `clear_trap_scratch` | 3959 | 8 | yes |
| sched/sched.rs | `init` | 3967 | 80 | yes |
| sched/sched.rs | `task_page_index` | 4047 | 18 | yes |
| sched/sched.rs | `task_page_bit` | 4065 | 6 | yes |
| sched/sched.rs | `task_page_mark` | 4071 | 6 | yes |
| sched/sched.rs | `task_page_unmark` | 4077 | 44 | yes |
| sched/sched.rs | `task_page_is_owned` | 4121 | 22 | yes |
| sched/sched.rs | `alloc_task_slot` | 4143 | 83 | yes |
| sched/sched.rs | `free_task_slot` | 4226 | 52 | yes |
| sched/sched.rs | `pdec_fts` | 4278 | 101 | yes |
| sched/sched.rs | `pdec3` | 4379 | 259 | yes |
| sched/sched.rs | `schedule` | 4638 | 134 | yes |
| sched/sched.rs | `terminal_release_grq_if_held` | 4772 | 57 | yes |
| sched/sched.rs | `grq_raw_puts` | 4829 | 6 | yes |
| sched/sched.rs | `grq_raw_putdec` | 4835 | 17 | yes |
| sched/sched.rs | `grq_raw_puthex` | 4852 | 10 | yes |
| sched/sched.rs | `schedule_dead_never_returns` | 4862 | 172 | yes |
| sched/sched.rs | `swapper_comm` | 5034 | 9 | yes |
| sched/sched.rs | `__schedule` | 5043 | 369 | yes |
| sched/sched.rs | `pdec3` | 5412 | 780 | yes |
| sched/sched.rs | `grq_diag_cfs_linked` | 6192 | 14 | yes |
| sched/sched.rs | `grq_diag_counts` | 6206 | 13 | yes |
| sched/sched.rs | `grq_idle_mask` | 6219 | 5 | yes |
| sched/sched.rs | `grq_nr_running_now` | 6224 | 8 | yes |
| sched/sched.rs | `grq_diag_cpu_currents` | 6232 | 27 | yes |
| sched/sched.rs | `scheduler_tick` | 6259 | 294 | yes |
| sched/sched.rs | `ph` | 6553 | 175 | yes |
| sched/sched.rs | `ph` | 6728 | 118 | yes |
| sched/sched.rs | `ph` | 6846 | 132 | yes |
| sched/sched.rs | `ph` | 6978 | 308 | yes |
| sched/sched.rs | `ph` | 7286 | 36 | yes |
| sched/sched.rs | `ph` | 7322 | 49 | yes |
| sched/sched.rs | `ph` | 7371 | 63 | yes |
| sched/sched.rs | `pdec` | 7434 | 518 | yes |
| sched/sched.rs | `grq_cfs_adjust` | 7952 | 26 | yes |
| sched/sched.rs | `for_each_task` | 7978 | 30 | yes |
| sched/sched.rs | `for_each_task_try` | 8008 | 8 | yes |
| sched/sched.rs | `get_current_pid` | 8016 | 10 | yes |
| sched/sched.rs | `get_current_ppid` | 8026 | 10 | yes |
| sched/sched.rs | `find_task_by_pid` | 8036 | 6 | yes |
| sched/sched.rs | `this_cpu_rq` | 8042 | 4 | yes |
| sched/sched.rs | `cpu_rq` | 8046 | 9 | yes |
| sched/sched.rs | `task_onrq_flag` | 8055 | 21 | yes |
| sched/sched.rs | `harvest_print_parked` | 8076 | 15 | yes |
| sched/sched.rs | `pdec` | 8091 | 18 | yes |
| sched/sched.rs | `phex` | 8109 | 99 | yes |
| sched/sched.rs | `task_is_dying` | 8208 | 15 | yes |
| sched/sched.rs | `harvest_relink` | 8223 | 251 | yes |
| sched/sched.rs | `republish_current_after_abort` | 8474 | 53 | yes |
| sched/sched.rs | `harvest_orphan_tasks` | 8527 | 183 | yes |
| sched/sched.rs | `idle_heal_own_slot` | 8710 | 106 | yes |
| sched/sched.rs | `relink_unlinked_running_phantoms` | 8816 | 254 | yes |
| sched/sched.rs | `cpu_idle_loop` | 9070 | 26 | yes |
| sched/sched.rs | `idle_guard_sp` | 9096 | 7 | yes |
| sched/sched.rs | `idle_guard_sp` | 9103 | 7 | yes |
| sched/sched.rs | `idle_guard_sp` | 9110 | 6 | yes |
| sched/sched.rs | `idle_guard_report` | 9116 | 2 | yes |
| sched/sched.rs | `ph` | 9118 | 309 | yes |
| sched/sched_hooks.rs | `sched_register_hooks` | 249 | 5 |  |
| sched/sched_hooks.rs | `hooks` | 254 | 15 |  |
| sched/sched_hooks.rs | `r56_print_pid` | 269 | 3 | yes |
| sched/sched_hooks.rs | `sched_clock` | 272 | 3 | yes |
| sched/sched_hooks.rs | `change_task_policy` | 275 | 10 | yes |
| sched/sched_hooks.rs | `ww_record` | 285 | 8 | yes |
| sched/sched_hooks.rs | `orphan_watch_confirm` | 293 | 3 | yes |
| sched/sched_hooks.rs | `yield_cpu` | 296 | 3 | yes |
| sched/sched_hooks.rs | `enqueue_task_locked` | 299 | 3 | yes |
| sched/sched_hooks.rs | `ww_watch_add` | 302 | 3 | yes |
| sched/sched_hooks.rs | `abort_next_release_orphans` | 305 | 3 | yes |
| sched/sched_hooks.rs | `r56_report_switch_abort` | 308 | 3 | yes |
| sched/sched_hooks.rs | `sched_renice_locked` | 311 | 7 | yes |
| sched/sched_hooks.rs | `psledger_consume` | 318 | 8 | yes |
| sched/sched_hooks.rs | `steer_task_cpu` | 326 | 3 | yes |
| sched/sched_hooks.rs | `sched_can_stop_tick` | 329 | 3 | yes |
| sched/sched_hooks.rs | `dl_throttle` | 332 | 3 | yes |
| sched/sched_hooks.rs | `ensure_linked_locked` | 335 | 3 | yes |
| sched/sched_hooks.rs | `check_cfs_preempt` | 338 | 3 | yes |
| sched/sched_hooks.rs | `check_rt_preempt` | 341 | 3 | yes |
| sched/sched_hooks.rs | `check_dl_preempt` | 344 | 3 | yes |
| sched/sched_hooks.rs | `orphan_watch_clear` | 347 | 3 | yes |
| sched/sched_hooks.rs | `pick_giveups` | 350 | 3 | yes |
| sched/sched_hooks.rs | `ww_slot_mismatch_counters` | 353 | 3 | yes |
| sched/sched_hooks.rs | `ww_sched_snapshot` | 356 | 3 | yes |
| sched/sched_hooks.rs | `ww_stalled_cpu_state` | 359 | 3 | yes |
| sched/sched_hooks.rs | `debug_claim_inject` | 362 | 3 | yes |
| sched/sched_hooks.rs | `debug_oncpu_inject` | 365 | 3 | yes |
| sched/sched_hooks.rs | `ww_heals` | 368 | 3 | yes |
| sched/sched_hooks.rs | `ww_subgate_counts` | 371 | 3 | yes |
| sched/sched_hooks.rs | `ww_rt_pick_stats` | 374 | 3 | yes |
| sched/sched_hooks.rs | `heartbeat` | 377 | 3 | yes |
| sched/sched_hooks.rs | `stallwatch_tick` | 380 | 3 | yes |
| sched/sched_hooks.rs | `preempt_check_wake` | 383 | 3 | yes |
| sched/sched_hooks.rs | `preempt_check_wake_sync` | 386 | 3 | yes |
| sched/sched_hooks.rs | `w2_refusal_census` | 389 | 3 | yes |
| sched/sched_hooks.rs | `task_sched_class` | 392 | 3 | yes |
| sched/sched_hooks.rs | `ww_note_slot_mismatch` | 395 | 3 | yes |
| sched/sched_hooks.rs | `monopolist_census` | 398 | 3 | yes |
| sched/sched_hooks.rs | `runq_strand_census` | 401 | 3 | yes |
| sched/sched_hooks.rs | `ffrt_wait_census` | 404 | 3 | yes |
| sched/sched_hooks.rs | `rt_wedge_dump_tick` | 407 | 3 | yes |
| sched/sched_hooks.rs | `rt_bandwidth_tick` | 410 | 3 | yes |
| sched/sched_hooks.rs | `emit_grq_reports` | 413 | 3 | yes |
| sched/sched_hooks.rs | `pick_next_task` | 416 | 3 | yes |
| sched/sched_hooks.rs | `requeue_prev_locked` | 419 | 3 | yes |
| sched/sched_hooks.rs | `g_diag_idle_bit` | 422 | 3 | yes |
| sched/sched_hooks.rs | `cpu_tick_stale_ms` | 425 | 3 | yes |
| sched/sched_hooks.rs | `lp_idle_lie_report` | 428 | 3 | yes |
| sched/sched_hooks.rs | `sched_slice_to_ms` | 431 | 3 | yes |
| sched/sched_hooks.rs | `sched_ev_drain` | 434 | 3 | yes |
| sched/sched_hooks.rs | `note_task_freed` | 437 | 3 | yes |
| sched/sched_hooks.rs | `report_poisoned_rq_entry` | 440 | 3 | yes |
| sched/sched_hooks.rs | `psledger_task` | 443 | 3 |  |
| sched/sched_hooks.rs | `psledger_cpu` | 446 | 3 |  |
| sched/sched_hooks.rs | `psledger_jif` | 449 | 3 |  |
| sched/sched_hooks.rs | `psledger_pk` | 452 | 3 |  |
| sched/sched_hooks.rs | `ww_watch` | 455 | 2 |  |
| sched/sched_hooks.rs | `ww_watch_slots` | 457 | 3 |  |
| sched/sched_hooks.rs | `load_balance` | 460 | 5 | yes |
| sched/sched_hooks.rs | `psledger_note_publish` | 465 | 8 | yes |
| sched/sched_hooks.rs | `psledger_peek` | 473 | 3 | yes |
| sched/sched_hooks.rs | `sched_hist_ring` | 476 | 3 | yes |
| sched/sched_hooks.rs | `sched_hist_refuse` | 479 | 3 | yes |
| sched/sched_hooks.rs | `sched_hist_link` | 482 | 3 | yes |
| sched/sched_hooks.rs | `sched_hist_pick` | 485 | 3 | yes |
| sched/sched_hooks.rs | `ww_cfs_geometry` | 488 | 3 | yes |
| sched/sched_hooks.rs | `ww_cfs_tie_mass` | 491 | 3 | yes |
| sched/sched_hooks.rs | `ww_cfs_tie_mass_vr` | 494 | 3 | yes |
| sched/sched_hooks.rs | `ww_tail_fill` | 497 | 3 | yes |
| sched/sched_hooks.rs | `ww_append_dump` | 500 | 3 | yes |
| sched/sched_hooks.rs | `fair_sched_class` | 503 | 2 |  |
| sched/sched_hooks.rs | `rt_sched_class` | 505 | 2 |  |
| sched/sched_hooks.rs | `dl_sched_class` | 507 | 2 |  |
| sched/sched_hooks.rs | `idle_sched_class` | 509 | 2 |  |
| sched/sched_hooks.rs | `stop_sched_class` | 511 | 3 |  |
| sched/sched_hooks.rs | `sched_ev` | 514 | 2 | yes |
| sched/sched_hooks.rs | `stale_oncpu_healed` | 516 | 8 |  |
| sched/sched_hooks.rs | `vr_canary` | 524 | 6 |  |
| sched/side_edges.rs | `evring_slot_mut` | 32 | 15 |  |
| sched/side_edges.rs | `evring_slot_ref` | 47 | 18 |  |
| sched/side_edges.rs | `new` | 65 | 8 | yes |
