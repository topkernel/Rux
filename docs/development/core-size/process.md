# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# process — hot 8348 lines (stay) / cold 9435 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| process/coredump.rs | `signal_makes_core` | 67 | 18 |  |
| process/coredump.rs | `push_u32` | 85 | 3 |  |
| process/coredump.rs | `push_u64` | 88 | 6 |  |
| process/coredump.rs | `push_note` | 94 | 14 |  |
| process/coredump.rs | `push_phdr` | 108 | 26 |  |
| process/coredump.rs | `build_prstatus` | 134 | 45 |  |
| process/coredump.rs | `build_prpsinfo` | 179 | 42 |  |
| process/coredump.rs | `expand_core_pattern` | 221 | 64 |  |
| process/coredump.rs | `do_coredump` | 285 | 222 |  |
| process/exec.rs | `new` | 26 | 21 | yes |
| process/exec.rs | `do_execve_elf` | 47 | 217 |  |
| process/exec.rs | `drop` | 264 | 1215 | yes |
| process/exit.rs | `exit_robust_list` | 49 | 36 |  |
| process/exit.rs | `handle_robust_entry` | 85 | 27 |  |
| process/exit.rs | `handle_pending_entry` | 112 | 18 |  |
| process/exit.rs | `walk_robust_chain` | 130 | 41 |  |
| process/exit.rs | `exit_clear_child_tid` | 171 | 43 |  |
| process/exit.rs | `kill_other_threads` | 214 | 23 |  |
| process/exit.rs | `pd` | 237 | 195 | yes |
| process/exit.rs | `wait_for_thread_group_death` | 432 | 48 |  |
| process/exit.rs | `de_thread` | 480 | 40 |  |
| process/exit.rs | `free_dead_member` | 520 | 184 |  |
| process/exit.rs | `sweep_dead_threads` | 704 | 138 |  |
| process/exit.rs | `release_task` | 842 | 298 |  |
| process/exit.rs | `release_task_storage` | 1140 | 169 |  |
| process/exit.rs | `do_exit` | 1309 | 1220 | yes |
| process/exit.rs | `do_exit_group` | 2529 | 59 | yes |
| process/exit.rs | `reparent_children_to_init` | 2588 | 272 |  |
| process/exit.rs | `wake_group_chldexit` | 2860 | 59 | yes |
| process/exit.rs | `wl_entry_syscall` | 2919 | 38 |  |
| process/exit.rs | `wait4_park_audit` | 2957 | 199 | yes |
| process/exit.rs | `wait_pid_matches` | 3156 | 25 |  |
| process/exit.rs | `do_wait` | 3181 | 329 |  |
| process/exit.rs | `do_wait_nonblock` | 3510 | 212 |  |
| process/exit.rs | `write_siginfo` | 3722 | 35 |  |
| process/exit.rs | `do_waitid` | 3757 | 224 |  |
| process/fork.rs | `do_fork` | 69 | 30 |  |
| process/fork.rs | `copy_thread` | 99 | 136 |  |
| process/fork.rs | `ret_from_fork` | 235 | 67 | yes |
| process/fork.rs | `einval` | 302 | 1 |  |
| process/fork.rs | `eagain` | 303 | 1 |  |
| process/fork.rs | `enomem` | 304 | 1 |  |
| process/fork.rs | `efault` | 305 | 8 |  |
| process/fork.rs | `clone_warn_budgeted` | 313 | 18 |  |
| process/fork.rs | `do_clone` | 331 | 353 |  |
| process/fork.rs | `write_child_settid` | 684 | 523 |  |
| process/kthread.rs | `kernel_thread_create` | 59 | 55 |  |
| process/kthread.rs | `ret_from_fork_kernel_asm` | 114 | 8 |  |
| process/kthread.rs | `ret_from_fork_kernel` | 122 | 5 | yes |
| process/kthread.rs | `ret_from_fork_kernel_asm` | 127 | 76 |  |
| process/kthread.rs | `kthread_enqueue` | 203 | 35 |  |
| process/kthread.rs | `kernel_thread` | 238 | 17 |  |
| process/kthread.rs | `kthread_run_on_cpu` | 255 | 15 | yes |
| process/kthread.rs | `kthread_run` | 270 | 16 | yes |
| process/kthread.rs | `kthread_should_stop` | 286 | 11 | yes |
| process/kthread.rs | `kthread_name` | 297 | 14 |  |
| process/kthread.rs | `kthread_stop` | 311 | 36 |  |
| process/kthread.rs | `kthread_bind` | 347 | 5 |  |
| process/mod.rs | `thread_clear_callee_saved` | 38 | 13 |  |
| process/mod.rs | `thread_set_entry` | 51 | 27 |  |
| process/mod.rs | `set_user_tls` | 78 | 22 |  |
| process/mod.rs | `current_pid` | 100 | 5 | yes |
| process/mod.rs | `current_ppid` | 105 | 5 |  |
| process/mod.rs | `current_pgid` | 110 | 8 |  |
| process/mod.rs | `current_task` | 118 | 8 | yes |
| process/mod.rs | `find_task_by_pid` | 126 | 10 | yes |
| process/ns.rs | `new` | 72 | 27 | yes |
| process/ns.rs | `uts_set` | 99 | 12 |  |
| process/ns.rs | `uts_get` | 111 | 5 |  |
| process/ns.rs | `set_hostname` | 116 | 4 |  |
| process/ns.rs | `get_hostname` | 120 | 4 |  |
| process/ns.rs | `set_domainname` | 124 | 4 |  |
| process/ns.rs | `get_domainname` | 128 | 36 |  |
| process/ns.rs | `alloc_mount_id` | 164 | 31 |  |
| process/ns.rs | `drop` | 195 | 13 | yes |
| process/ns.rs | `new` | 208 | 12 | yes |
| process/ns.rs | `ns_track_vfsmount` | 220 | 8 | yes |
| process/ns.rs | `in_init_mnt_ns` | 228 | 24 | yes |
| process/ns.rs | `new` | 252 | 13 | yes |
| process/ns.rs | `alloc_local_pid` | 265 | 52 |  |
| process/ns.rs | `alloc_ns_inum` | 317 | 12 |  |
| process/ns.rs | `init_uts_ns` | 329 | 13 |  |
| process/ns.rs | `init_mnt_ns` | 342 | 10 |  |
| process/ns.rs | `init_pid_ns` | 352 | 10 |  |
| process/ns.rs | `init_net_ns` | 362 | 12 |  |
| process/ns.rs | `init_ipc_ns` | 374 | 12 |  |
| process/ns.rs | `init_user_ns` | 386 | 14 |  |
| process/ns.rs | `is_init_pid_ns` | 400 | 9 |  |
| process/ns.rs | `upid_in_ns` | 409 | 18 |  |
| process/ns.rs | `resolve_local_pid_to_global` | 427 | 25 | yes |
| process/ns.rs | `translate_sender_pid_ns` | 452 | 21 | yes |
| process/ns.rs | `current_uts_ns` | 473 | 6 |  |
| process/ns.rs | `current_mnt_ns` | 479 | 7 | yes |
| process/ns.rs | `current_pid_ns` | 486 | 7 |  |
| process/ns.rs | `current_net_ns` | 493 | 7 |  |
| process/ns.rs | `current_ipc_ns` | 500 | 7 |  |
| process/ns.rs | `current_user_ns` | 507 | 12 |  |
| process/ns.rs | `boot_mount_entries` | 519 | 17 |  |
| process/ns.rs | `ns_register_mount` | 536 | 21 |  |
| process/ns.rs | `ns_unregister_mount` | 557 | 7 |  |
| process/ns.rs | `ns_get_mounts` | 564 | 5 |  |
| process/ns.rs | `ns_is_mountpoint` | 569 | 13 |  |
| process/ns.rs | `task_pid_ns_for_children` | 582 | 10 |  |
| process/ns.rs | `resolve_vpid` | 592 | 27 |  |
| process/ns.rs | `resolve_vtid` | 619 | 26 |  |
| process/ns.rs | `task_vpid` | 645 | 9 |  |
| process/ns.rs | `task_same_pid_ns` | 654 | 11 |  |
| process/ns.rs | `wait_vpid` | 665 | 26 |  |
| process/ns.rs | `check_ns_capability` | 691 | 15 |  |
| process/ns.rs | `copy_namespaces` | 706 | 115 |  |
| process/ns.rs | `unshare_namespaces` | 821 | 68 |  |
| process/ns.rs | `task_uts_of` | 889 | 5 |  |
| process/ns.rs | `task_mnt_table_of` | 894 | 23 |  |
| process/ns.rs | `name` | 917 | 11 | yes |
| process/ns.rs | `from_name` | 928 | 13 |  |
| process/ns.rs | `clone_flag` | 941 | 22 |  |
| process/ns.rs | `index` | 963 | 12 | yes |
| process/ns.rs | `from_index` | 975 | 25 | yes |
| process/ns.rs | `kind` | 1000 | 11 |  |
| process/ns.rs | `inum` | 1011 | 16 |  |
| process/ns.rs | `ns_fd_register` | 1027 | 8 |  |
| process/ns.rs | `ns_fd_lookup` | 1035 | 10 |  |
| process/ns.rs | `task_ns_handle` | 1045 | 12 |  |
| process/ns.rs | `ns_link_target` | 1057 | 9 |  |
| process/ns.rs | `fallback_inum` | 1066 | 4 |  |
| process/ns.rs | `task_ns_handle_init` | 1070 | 12 |  |
| process/ns.rs | `ns_file_content` | 1082 | 9 |  |
| process/ns.rs | `open_ns_file` | 1091 | 10 |  |
| process/ns.rs | `build_ns_file` | 1101 | 40 | yes |
| process/ns.rs | `ns_file_read` | 1141 | 19 |  |
| process/ns.rs | `ns_file_close` | 1160 | 15 |  |
| process/ns.rs | `try_open_ns_path` | 1175 | 39 |  |
| process/ns.rs | `setns` | 1214 | 64 |  |
| process/ns.rs | `reanchor_root_for_mnt_ns` | 1278 | 52 |  |
| process/pid.rs | `pid_max_effective` | 44 | 7 |  |
| process/pid.rs | `set_pid_max_live` | 51 | 5 |  |
| process/pid.rs | `pid_max_live` | 56 | 25 |  |
| process/pid.rs | `new` | 81 | 14 | yes |
| process/pid.rs | `scan_range` | 95 | 30 |  |
| process/pid.rs | `find_next_zero` | 125 | 31 |  |
| process/pid.rs | `alloc_pid` | 156 | 24 | yes |
| process/pid.rs | `allocated_count` | 180 | 7 |  |
| process/pid.rs | `free_pid` | 187 | 26 | yes |
| process/pid.rs | `last_pid` | 213 | 10 |  |
| process/pid.rs | `set_last_pid` | 223 | 8 |  |
| process/pid_hash.rs | `bucket_index` | 39 | 17 |  |
| process/pid_hash.rs | `lock_bucket` | 56 | 22 |  |
| process/pid_hash.rs | `try_lock_bucket` | 78 | 14 |  |
| process/pid_hash.rs | `unlock_bucket` | 92 | 10 |  |
| process/pid_hash.rs | `init` | 102 | 11 | yes |
| process/pid_hash.rs | `pid_hash_insert` | 113 | 22 | yes |
| process/pid_hash.rs | `pid_hash_remove` | 135 | 47 |  |
| process/pid_hash.rs | `pid_hash_lookup_pinned` | 182 | 10 | yes |
| process/pid_hash.rs | `pid_hash_lookup` | 192 | 30 | yes |
| process/pid_hash.rs | `pid_hash_for_each_task` | 222 | 29 | yes |
| process/pid_hash.rs | `pid_hash_for_each_task_try` | 251 | 41 | yes |
| process/pid_hash.rs | `pid_hash_collect_all` | 292 | 19 |  |
| process/ptrace.rs | `ptrace_may_access` | 85 | 21 |  |
| process/ptrace.rs | `read_target_word` | 106 | 49 |  |
| process/ptrace.rs | `write_target_word` | 155 | 60 | yes |
| process/ptrace.rs | `process_vm_copy` | 215 | 92 |  |
| process/ptrace.rs | `ptrace_stop` | 307 | 34 | yes |
| process/ptrace.rs | `sys_ptrace` | 341 | 48 |  |
| process/ptrace.rs | `dispatch_request` | 389 | 321 |  |
| process/reboot.rs | `ctrl_alt_del_latch` | 31 | 9 |  |
| process/reboot.rs | `cad_deliver_pending` | 40 | 48 |  |
| process/reboot.rs | `pid1_exited` | 88 | 36 |  |
| process/reboot.rs | `init_exited_fallback_cascade` | 124 | 19 |  |
| process/reboot.rs | `panic_reboot` | 143 | 36 |  |
| process/reboot.rs | `force_reboot` | 179 | 11 |  |
| process/reboot.rs | `force_power_off` | 190 | 12 |  |
| process/reboot.rs | `shutdown_cascade` | 202 | 107 |  |
| process/reboot.rs | `machine_park` | 309 | 8 |  |
| process/reboot.rs | `sbi_legacy_shutdown` | 317 | 14 |  |
| process/reboot.rs | `sleep_one_tick` | 331 | 27 |  |
| process/reboot.rs | `set_cad_immediate` | 358 | 3 |  |
| process/task.rs | `new` | 53 | 11 | yes |
| process/task.rs | `pop` | 64 | 20 | yes |
| process/task.rs | `push` | 84 | 111 | yes |
| process/task.rs | `kstack_slab_alloc` | 195 | 40 |  |
| process/task.rs | `kstack_slab_free` | 235 | 50 |  |
| process/task.rs | `kstack_slab_stats` | 285 | 12 |  |
| process/task.rs | `kstack_slab_stats` | 297 | 16 |  |
| process/task.rs | `kstack_anat` | 313 | 8 |  |
| process/task.rs | `plain_kstack_alloc` | 321 | 7 |  |
| process/task.rs | `stack_cache_alloc` | 328 | 120 |  |
| process/task.rs | `note_kstack_freed` | 448 | 12 |  |
| process/task.rs | `freed_stack_containing` | 460 | 16 | yes |
| process/task.rs | `stack_cache_free` | 476 | 132 |  |
| process/task.rs | `new` | 608 | 6 | yes |
| process/task.rs | `bits` | 614 | 6 | yes |
| process/task.rs | `contains` | 620 | 6 | yes |
| process/task.rs | `is_running` | 626 | 6 | yes |
| process/task.rs | `is_sleeping` | 632 | 6 | yes |
| process/task.rs | `is_dead` | 638 | 6 | yes |
| process/task.rs | `is_interruptible` | 644 | 6 |  |
| process/task.rs | `default` | 650 | 106 | yes |
| process/task.rs | `new_init` | 756 | 34 | yes |
| process/task.rs | `keeps_caps_on_setuid` | 790 | 8 |  |
| process/task.rs | `new_user` | 798 | 22 | yes |
| process/task.rs | `in_group` | 820 | 955 | yes |
| process/task.rs | `default_rlimits` | 1775 | 40 |  |
| process/task.rs | `clone` | 1815 | 26 | yes |
| process/task.rs | `new` | 1841 | 231 | yes |
| process/task.rs | `new_idle_at` | 2072 | 39 | yes |
| process/task.rs | `register_slot_deferred_sizes` | 2111 | 22 |  |
| process/task.rs | `new_task_at` | 2133 | 55 | yes |
| process/task.rs | `state` | 2188 | 6 | yes |
| process/task.rs | `set_state` | 2194 | 6 | yes |
| process/task.rs | `is_state` | 2200 | 37 |  |
| process/task.rs | `sleep` | 2237 | 51 |  |
| process/task.rs | `is_plausible_task_ptr` | 2288 | 38 |  |
| process/task.rs | `wake_up` | 2326 | 8 | yes |
| process/task.rs | `wake_up_sync` | 2334 | 12 | yes |
| process/task.rs | `wake_up_in_futex_bucket` | 2346 | 12 |  |
| process/task.rs | `wake_up_impl` | 2358 | 110 |  |
| process/task.rs | `ph` | 2468 | 410 | yes |
| process/task.rs | `pid` | 2878 | 11 | yes |
| process/task.rs | `rlimit` | 2889 | 8 | yes |
| process/task.rs | `cpu_time_last_sigxcpu` | 2897 | 5 | yes |
| process/task.rs | `set_cpu_time_last_sigxcpu` | 2902 | 13 | yes |
| process/task.rs | `tick_time_slice` | 2915 | 13 |  |
| process/task.rs | `reset_time_slice` | 2928 | 6 | yes |
| process/task.rs | `time_slice_expired` | 2934 | 6 |  |
| process/task.rs | `get_time_slice` | 2940 | 6 |  |
| process/task.rs | `set_time_slice` | 2946 | 10 | yes |
| process/task.rs | `sched_entity` | 2956 | 6 | yes |
| process/task.rs | `sched_entity_mut` | 2962 | 6 |  |
| process/task.rs | `policy` | 2968 | 6 | yes |
| process/task.rs | `set_policy` | 2974 | 6 |  |
| process/task.rs | `static_prio` | 2980 | 6 |  |
| process/task.rs | `prio` | 2986 | 6 | yes |
| process/task.rs | `set_prio` | 2992 | 6 |  |
| process/task.rs | `rt_priority` | 2998 | 6 |  |
| process/task.rs | `set_rt_priority` | 3004 | 6 |  |
| process/task.rs | `rt_entity` | 3010 | 6 | yes |
| process/task.rs | `rt_entity_mut` | 3016 | 6 |  |
| process/task.rs | `dl_entity` | 3022 | 6 | yes |
| process/task.rs | `dl_entity_mut` | 3028 | 6 |  |
| process/task.rs | `sched_reset_on_fork` | 3034 | 7 |  |
| process/task.rs | `set_sched_reset_on_fork` | 3041 | 9 |  |
| process/task.rs | `nice` | 3050 | 7 |  |
| process/task.rs | `set_nice` | 3057 | 17 |  |
| process/task.rs | `ppid` | 3074 | 22 | yes |
| process/task.rs | `is_fork_child` | 3096 | 6 | yes |
| process/task.rs | `set_fork_child` | 3102 | 7 |  |
| process/task.rs | `fork_pt_regs` | 3109 | 8 | yes |
| process/task.rs | `clear_fork_child` | 3117 | 6 | yes |
| process/task.rs | `tgid` | 3123 | 6 | yes |
| process/task.rs | `set_tgid` | 3129 | 8 |  |
| process/task.rs | `group_leader_ptr` | 3137 | 15 |  |
| process/task.rs | `group_leader_raw` | 3152 | 6 |  |
| process/task.rs | `set_group_leader` | 3158 | 6 |  |
| process/task.rs | `next_thread_ptr` | 3164 | 10 |  |
| process/task.rs | `nr_threads` | 3174 | 6 |  |
| process/task.rs | `is_thread_group_leader` | 3180 | 15 |  |
| process/task.rs | `thread_group_join` | 3195 | 24 |  |
| process/task.rs | `thread_group_leave` | 3219 | 54 |  |
| process/task.rs | `for_each_group_member` | 3273 | 22 |  |
| process/task.rs | `cred` | 3295 | 6 | yes |
| process/task.rs | `cred_mut` | 3301 | 6 |  |
| process/task.rs | `access_token` | 3307 | 5 |  |
| process/task.rs | `set_access_token` | 3312 | 6 |  |
| process/task.rs | `access_ftoken` | 3318 | 5 |  |
| process/task.rs | `set_access_ftoken` | 3323 | 6 |  |
| process/task.rs | `address_space_mut` | 3329 | 12 |  |
| process/task.rs | `address_space` | 3341 | 13 | yes |
| process/task.rs | `set_address_space` | 3354 | 31 |  |
| process/task.rs | `address_space_arc` | 3385 | 13 | yes |
| process/task.rs | `mm_ptr_peek` | 3398 | 6 |  |
| process/task.rs | `active_mm` | 3404 | 14 |  |
| process/task.rs | `clear_active_mm` | 3418 | 5 |  |
| process/task.rs | `set_vfork_parent` | 3423 | 5 |  |
| process/task.rs | `clear_vfork_parent` | 3428 | 5 |  |
| process/task.rs | `vfork_parent_ptr` | 3433 | 16 |  |
| process/task.rs | `vfork_wake_parent` | 3449 | 13 |  |
| process/task.rs | `preempt_underflow_report` | 3462 | 20 |  |
| process/task.rs | `ph` | 3482 | 63 | yes |
| process/task.rs | `thread` | 3545 | 5 | yes |
| process/task.rs | `thread_mut` | 3550 | 8 | yes |
| process/task.rs | `ti_flags` | 3558 | 6 | yes |
| process/task.rs | `set_ti_flags` | 3564 | 6 |  |
| process/task.rs | `test_ti_flag` | 3570 | 6 | yes |
| process/task.rs | `set_ti_flag` | 3576 | 6 | yes |
| process/task.rs | `clear_ti_flag` | 3582 | 6 | yes |
| process/task.rs | `need_resched` | 3588 | 6 | yes |
| process/task.rs | `set_need_resched_flag` | 3594 | 6 |  |
| process/task.rs | `clear_need_resched_flag` | 3600 | 6 |  |
| process/task.rs | `has_pending_signal` | 3606 | 6 |  |
| process/task.rs | `set_pending_signal_flag` | 3612 | 6 |  |
| process/task.rs | `preempt_count` | 3618 | 6 | yes |
| process/task.rs | `inc_preempt_count` | 3624 | 6 |  |
| process/task.rs | `dec_preempt_count` | 3630 | 6 |  |
| process/task.rs | `preemptible` | 3636 | 6 | yes |
| process/task.rs | `add_preempt_count` | 3642 | 19 | yes |
| process/task.rs | `sub_preempt_count` | 3661 | 7 |  |
| process/task.rs | `sub_preempt_count_at` | 3668 | 52 | yes |
| process/task.rs | `task_pid_of` | 3720 | 15 | yes |
| process/task.rs | `release_hardirq_base_on` | 3735 | 23 | yes |
| process/task.rs | `release_hardirq_base_remote` | 3758 | 15 |  |
| process/task.rs | `sub_preempt_count2_at` | 3773 | 32 | yes |
| process/task.rs | `ti_preempt_count_zero` | 3805 | 24 | yes |
| process/task.rs | `ti_preempt_count_clear_base` | 3829 | 20 | yes |
| process/task.rs | `ti_preempt_count_keep_tick_nesting` | 3849 | 16 | yes |
| process/task.rs | `ti_kernel_sp` | 3865 | 6 | yes |
| process/task.rs | `set_ti_kernel_sp` | 3871 | 6 |  |
| process/task.rs | `ti_user_sp` | 3877 | 6 |  |
| process/task.rs | `set_ti_user_sp` | 3883 | 6 |  |
| process/task.rs | `ti_cpu` | 3889 | 10 | yes |
| process/task.rs | `ti_pc_zero_jif` | 3899 | 7 |  |
| process/task.rs | `ti_pc_sirq_present_run` | 3906 | 6 | yes |
| process/task.rs | `ti_pc_hirq_present_run` | 3912 | 6 | yes |
| process/task.rs | `ti_pc_pirq_present_run` | 3918 | 10 | yes |
| process/task.rs | `ti_pc_present_runs_tick` | 3928 | 30 | yes |
| process/task.rs | `ti_pc_present_runs_reset_base` | 3958 | 9 |  |
| process/task.rs | `ti_pc_present_runs_reset_all` | 3967 | 9 |  |
| process/task.rs | `set_ti_cpu` | 3976 | 7 | yes |
| process/task.rs | `cpus_allowed` | 3983 | 16 | yes |
| process/task.rs | `default_cpus_allowed` | 3999 | 19 |  |
| process/task.rs | `refresh_default_affinity_to_online` | 4018 | 24 | yes |
| process/task.rs | `task_put` | 4042 | 29 | yes |
| process/task.rs | `try_pin` | 4071 | 25 | yes |
| process/task.rs | `on_cpu` | 4096 | 5 | yes |
| process/task.rs | `set_on_cpu` | 4101 | 21 |  |
| process/task.rs | `publish_pick_ownership` | 4122 | 19 |  |
| process/task.rs | `release_pick_ownership` | 4141 | 14 | yes |
| process/task.rs | `try_affirm_pick` | 4155 | 32 | yes |
| process/task.rs | `pick_state` | 4187 | 10 | yes |
| process/task.rs | `revoke_pending_pick` | 4197 | 28 | yes |
| process/task.rs | `ownership_pickable` | 4225 | 24 |  |
| process/task.rs | `set_cpus_allowed` | 4249 | 6 |  |
| process/task.rs | `cpu_allowed` | 4255 | 8 |  |
| process/task.rs | `wake_cpu` | 4263 | 13 |  |
| process/task.rs | `alloc_kernel_stack` | 4276 | 27 | yes |
| process/task.rs | `free_kernel_stack` | 4303 | 244 |  |
| process/task.rs | `get_kernel_stack` | 4547 | 6 | yes |
| process/task.rs | `kernel_stack_bottom` | 4553 | 6 | yes |
| process/task.rs | `kernel_stack_top` | 4559 | 18 | yes |
| process/task.rs | `owns_kernel_stack` | 4577 | 22 |  |
| process/task.rs | `kernel_stack_top_checked` | 4599 | 15 | yes |
| process/task.rs | `is_in_kernel_stack` | 4614 | 16 | yes |
| process/task.rs | `is_stack_overflow` | 4630 | 17 | yes |
| process/task.rs | `pt_regs` | 4647 | 21 |  |
| process/task.rs | `pt_regs_mut` | 4668 | 25 |  |
| process/task.rs | `pt_regs_ref` | 4693 | 14 | yes |
| process/task.rs | `has_address_space` | 4707 | 6 |  |
| process/task.rs | `oom_score_adj` | 4713 | 6 |  |
| process/task.rs | `set_oom_score_adj` | 4719 | 7 |  |
| process/task.rs | `ioprio` | 4726 | 6 |  |
| process/task.rs | `set_ioprio` | 4732 | 6 |  |
| process/task.rs | `has_fdtable` | 4738 | 6 |  |
| process/task.rs | `try_fdtable` | 4744 | 10 | yes |
| process/task.rs | `fdtable_unshared` | 4754 | 11 | yes |
| process/task.rs | `parent_task_ptr` | 4765 | 14 |  |
| process/task.rs | `fdtable` | 4779 | 7 |  |
| process/task.rs | `try_fdtable_mut` | 4786 | 7 |  |
| process/task.rs | `fdtable_mut` | 4793 | 6 |  |
| process/task.rs | `set_fdtable` | 4799 | 5 |  |
| process/task.rs | `fdtable_arc` | 4804 | 5 |  |
| process/task.rs | `signal_arc` | 4809 | 5 |  |
| process/task.rs | `set_signal` | 4814 | 5 |  |
| process/task.rs | `set_parent` | 4819 | 10 | yes |
| process/task.rs | `parent_ptr` | 4829 | 7 |  |
| process/task.rs | `get_exit_signal` | 4836 | 6 | yes |
| process/task.rs | `set_exit_signal` | 4842 | 4 |  |
| process/task.rs | `exit_code` | 4846 | 6 | yes |
| process/task.rs | `set_exit_code` | 4852 | 6 |  |
| process/task.rs | `stop_signal` | 4858 | 6 |  |
| process/task.rs | `set_stop_signal` | 4864 | 8 |  |
| process/task.rs | `tracer_pid` | 4872 | 6 | yes |
| process/task.rs | `set_tracer_pid` | 4878 | 6 |  |
| process/task.rs | `set_ptrace_options` | 4884 | 7 |  |
| process/task.rs | `set_ptrace_sigdeliver` | 4891 | 6 |  |
| process/task.rs | `take_ptrace_sigdeliver` | 4897 | 6 |  |
| process/task.rs | `ptrace_sigdeliver_peek` | 4903 | 5 |  |
| process/task.rs | `set_ptrace_siginfo` | 4908 | 5 |  |
| process/task.rs | `ptrace_siginfo` | 4913 | 5 |  |
| process/task.rs | `clear_ptrace_siginfo` | 4918 | 7 |  |
| process/task.rs | `arm_single_step` | 4925 | 8 |  |
| process/task.rs | `clear_single_step` | 4933 | 6 | yes |
| process/task.rs | `single_step_active` | 4939 | 6 | yes |
| process/task.rs | `single_step_addr` | 4945 | 6 | yes |
| process/task.rs | `single_step_saved_insn` | 4951 | 6 | yes |
| process/task.rs | `stop_reported` | 4957 | 6 |  |
| process/task.rs | `set_core_dumped` | 4963 | 6 |  |
| process/task.rs | `core_dumped` | 4969 | 6 | yes |
| process/task.rs | `comm` | 4975 | 12 | yes |
| process/task.rs | `comm_str` | 4987 | 8 |  |
| process/task.rs | `set_comm` | 4995 | 8 |  |
| process/task.rs | `pgid` | 5003 | 6 | yes |
| process/task.rs | `set_pgid` | 5009 | 6 |  |
| process/task.rs | `sid` | 5015 | 6 |  |
| process/task.rs | `set_sid` | 5021 | 6 |  |
| process/task.rs | `has_execed` | 5027 | 6 |  |
| process/task.rs | `mark_execed` | 5033 | 12 |  |
| process/task.rs | `from_sibling_node` | 5045 | 9 |  |
| process/task.rs | `first_child` | 5054 | 17 |  |
| process/task.rs | `next_sibling` | 5071 | 21 |  |
| process/task.rs | `has_children` | 5092 | 11 |  |
| process/task.rs | `add_child` | 5103 | 52 | yes |
| process/task.rs | `remove_child` | 5155 | 39 | yes |
| process/task.rs | `collect_children_pinned` | 5194 | 39 |  |
| process/task.rs | `move_child` | 5233 | 77 |  |
| process/task.rs | `claim_child_locked` | 5310 | 25 |  |
| process/task.rs | `for_each_child` | 5335 | 34 |  |
| process/task.rs | `for_each_group_child` | 5369 | 42 |  |
| process/task.rs | `find_child_by_pid` | 5411 | 20 |  |
| process/task.rs | `count_children` | 5431 | 14 |  |
| process/task.rs | `pending` | 5445 | 10 |  |
| process/task.rs | `set_clear_child_tid` | 5455 | 6 |  |
| process/task.rs | `clear_child_tid` | 5461 | 8 |  |
| process/task.rs | `set_robust_list` | 5469 | 7 |  |
| process/task.rs | `robust_list_head` | 5476 | 6 |  |
| process/task.rs | `robust_list_len` | 5482 | 6 |  |
| process/task.rs | `get_brk` | 5488 | 6 |  |
| process/task.rs | `set_brk` | 5494 | 5 |  |
| process/task.rs | `get_cwd` | 5499 | 9 | yes |
| process/task.rs | `set_cwd` | 5508 | 8 |  |
| process/task.rs | `get_root` | 5516 | 10 | yes |
| process/task.rs | `set_root` | 5526 | 7 |  |
| process/task.rs | `fs_arc` | 5533 | 7 | yes |
| process/task.rs | `process_rooted_cheap` | 5540 | 8 | yes |
| process/task.rs | `set_fs` | 5548 | 5 |  |
| process/task.rs | `get_umask` | 5553 | 9 | yes |
| process/task.rs | `set_umask` | 5562 | 11 |  |
| process/task.rs | `seccomp_mode` | 5573 | 5 |  |
| process/task.rs | `set_seccomp_mode` | 5578 | 5 |  |
| process/task.rs | `no_new_privs` | 5583 | 4 |  |
| process/task.rs | `set_no_new_privs` | 5587 | 8 |  |
| process/task.rs | `seccomp_kill` | 5595 | 28 |  |
| process/task.rs | `ns_pid_local` | 5623 | 6 |  |
| process/task.rs | `cgroup_ptr` | 5629 | 8 |  |
| process/task.rs | `set_cgroup_ptr` | 5637 | 7 |  |
| process/task.rs | `cgroup_mem_charged` | 5644 | 6 |  |
| process/task.rs | `add_cgroup_mem_charged` | 5650 | 7 |  |
| process/task.rs | `sub_cgroup_mem_charged` | 5657 | 17 |  |
| process/task.rs | `get_exe_path` | 5674 | 5 | yes |
| process/task.rs | `set_user_sp` | 5679 | 5 |  |
| process/task.rs | `set_exe_path` | 5684 | 80 |  |
| process/task.rs | `get_current_fdtable` | 5764 | 6 | yes |
| process/task_ref.rs | `lookup` | 51 | 11 | yes |
| process/task_ref.rs | `as_ptr` | 62 | 16 | yes |
| process/task_ref.rs | `from_raw` | 78 | 8 | yes |
| process/task_ref.rs | `deref` | 86 | 10 | yes |
| process/task_ref.rs | `deref_mut` | 96 | 30 | yes |
| process/task_ref.rs | `lookup` | 126 | 7 | yes |
| process/task_ref.rs | `from_pinned` | 133 | 7 | yes |
| process/task_ref.rs | `into_raw` | 140 | 6 | yes |
| process/task_ref.rs | `as_ptr` | 146 | 8 | yes |
| process/task_ref.rs | `deref` | 154 | 9 | yes |
| process/task_ref.rs | `deref_mut` | 163 | 8 | yes |
| process/task_ref.rs | `drop` | 171 | 6 | yes |
| process/unsafe_edges.rs | `borrow_task` | 50 | 17 | yes |
| process/unsafe_edges.rs | `borrow_task_mut` | 67 | 34 | yes |
| process/unsafe_edges.rs | `place_task` | 101 | 32 | yes |
| process/unsafe_edges.rs | `dump_read_page` | 133 | 38 | yes |
| process/unsafe_edges.rs | `file_private_data_ref` | 171 | 15 |  |
| process/unsafe_edges.rs | `file_private_data_take_boxed` | 186 | 16 |  |
| process/unsafe_edges.rs | `file_private_data_mut` | 202 | 14 |  |
| process/unsafe_edges.rs | `read_volatile_u64` | 216 | 13 | yes |
| process/unsafe_edges.rs | `pt_user_pc` | 229 | 23 | yes |
| process/unsafe_edges.rs | `journal_handle` | 252 | 18 |  |
| process/unsafe_edges.rs | `clear_journal_handle` | 270 | 11 | yes |
| process/unsafe_edges.rs | `free_fork_pt_regs` | 281 | 25 | yes |
| process/unsafe_edges.rs | `switch_to_kernel_root_on_exit` | 306 | 19 | yes |
| process/unsafe_edges.rs | `apppark_rets` | 325 | 16 | yes |
| process/unsafe_edges.rs | `freelist_next` | 341 | 10 | yes |
| process/unsafe_edges.rs | `freelist_link` | 351 | 13 | yes |
| process/unsafe_edges.rs | `heap_alloc` | 364 | 10 | yes |
| process/unsafe_edges.rs | `heap_dealloc` | 374 | 10 | yes |
| process/unsafe_edges.rs | `slot_ptr` | 384 | 11 | yes |
| process/unsafe_edges.rs | `zero_bytes` | 395 | 10 | yes |
| process/unsafe_edges.rs | `write_volatile_u64` | 405 | 20 | yes |
| process/unsafe_edges.rs | `read_volatile_u32` | 425 | 14 | yes |
| process/unsafe_edges.rs | `borrow_active_mm` | 439 | 20 |  |
| process/unsafe_edges.rs | `list_add_tail` | 459 | 11 | yes |
| process/unsafe_edges.rs | `list_del` | 470 | 11 | yes |
| process/unsafe_edges.rs | `list_for_each` | 481 | 10 |  |
| process/unsafe_edges.rs | `task_pid_of` | 491 | 12 | yes |
| process/unsafe_edges.rs | `store_pick_qword` | 503 | 14 | yes |
| process/unsafe_edges.rs | `paint_bytes` | 517 | 17 | yes |
| process/unsafe_edges.rs | `frame_mut` | 534 | 14 | yes |
| process/unsafe_edges.rs | `frame_ref` | 548 | 19 | yes |
| process/unsafe_edges.rs | `exec_copy_user_image` | 567 | 36 | yes |
| process/unsafe_edges.rs | `exec_phdr_at` | 603 | 14 | yes |
| process/unsafe_edges.rs | `exec_ehdr_from` | 617 | 12 | yes |
| process/unsafe_edges.rs | `exec_map_user_range` | 629 | 11 | yes |
| process/unsafe_edges.rs | `exec_map_user_region_static` | 640 | 18 | yes |
| process/unsafe_edges.rs | `exec_probe_read_u64` | 658 | 19 | yes |
| process/unsafe_edges.rs | `exec_free_user_page_trees` | 677 | 19 | yes |
| process/unsafe_edges.rs | `exec_write_sigtramp` | 696 | 14 | yes |
| process/unsafe_edges.rs | `exec_mm_new_user` | 710 | 12 | yes |
| process/unsafe_edges.rs | `exec_switch_address_space` | 722 | 22 | yes |
| process/unsafe_edges.rs | `exec_set_leaf_flags_range` | 744 | 140 | yes |
| process/unsafe_edges.rs | `fork_save_parent_fpu` | 884 | 11 | yes |
| process/unsafe_edges.rs | `fork_save_parent_vstate` | 895 | 11 | yes |
| process/unsafe_edges.rs | `fork_save_parent_fpu` | 906 | 9 | yes |
| process/unsafe_edges.rs | `fork_fpu_save_for_switch` | 915 | 11 | yes |
| process/unsafe_edges.rs | `fork_pt_walk` | 926 | 9 | yes |
| process/unsafe_edges.rs | `fork_handle_cow_fault` | 935 | 24 | yes |
| process/unsafe_edges.rs | `fork_write_child_tid` | 959 | 11 | yes |
| process/unsafe_edges.rs | `read_user_tp_register` | 970 | 19 | yes |
| process/unsafe_edges.rs | `page_refcount` | 989 | 33 | yes |
| process/unsafe_edges.rs | `pid_hash_unlink_store` | 1022 | 17 | yes |
| process/unsafe_edges.rs | `ptrace_translate` | 1039 | 15 | yes |
| process/unsafe_edges.rs | `ptrace_read_word` | 1054 | 11 | yes |
| process/unsafe_edges.rs | `ptrace_write_word` | 1065 | 12 | yes |
| process/unsafe_edges.rs | `ptrace_copy_chunk` | 1077 | 18 | yes |
| process/unsafe_edges.rs | `ptrace_flush_icache` | 1095 | 38 | yes |
| process/unsafe_edges.rs | `ptrace_read_gregset` | 1133 | 17 | yes |
| process/unsafe_edges.rs | `ptrace_write_gregset` | 1150 | 19 | yes |
| process/unsafe_edges.rs | `zeroed_siginfo` | 1169 | 5 | yes |
| process/wait.rs | `new` | 59 | 19 | yes |
| process/wait.rs | `registered_pid` | 78 | 5 |  |
| process/wait.rs | `is_woken` | 83 | 5 |  |
| process/wait.rs | `set_woken` | 88 | 20 |  |
| process/wait.rs | `clear_woken` | 108 | 5 |  |
| process/wait.rs | `task` | 113 | 5 |  |
| process/wait.rs | `is_exclusive` | 118 | 48 |  |
| process/wait.rs | `wq_regs_dec` | 166 | 20 |  |
| process/wait.rs | `new` | 186 | 8 | yes |
| process/wait.rs | `init` | 194 | 8 | yes |
| process/wait.rs | `add` | 202 | 40 | yes |
| process/wait.rs | `remove_count` | 242 | 32 |  |
| process/wait.rs | `remove` | 274 | 21 | yes |
| process/wait.rs | `wake_up` | 295 | 223 | yes |
| process/wait.rs | `wake_up_all` | 518 | 7 |  |
| process/wait.rs | `debug_entries` | 525 | 22 |  |
| process/wait.rs | `wake_up_one` | 547 | 16 | yes |
| process/wait.rs | `wake_up_one_hiprio` | 563 | 60 |  |
| process/wait.rs | `debug_entries_ptr` | 623 | 25 |  |
| process/wait.rs | `has_sleepers` | 648 | 7 | yes |
| process/wait.rs | `vw_queue_census` | 655 | 11 |  |
| process/wait.rs | `vw_queue_census_try` | 666 | 17 |  |
| process/wait.rs | `prepare_to_wait` | 683 | 35 |  |
| process/wait.rs | `finish_wait` | 718 | 131 |  |
