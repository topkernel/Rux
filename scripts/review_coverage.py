#!/usr/bin/env python3
"""Generate the kernel review coverage map.

The map is the bookkeeping for the line-by-line review: every source
file under kernel/src carries a status, so nothing can be silently
skipped.

Status vocabulary (kept in STATUS below):

  done    the file was read end to end, no defect found
  fixed   the file was read end to end, a defect was found and fixed
  part    the file was read in part (the reviewed range is recorded)
  todo    the file has not been read yet

Usage: scripts/review_coverage.py [--check]

  --check   verify the map is in sync with the tree (no new files, no
            status for an unknown file) and exit non-zero on drift
"""

import fnmatch
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "kernel", "src")
OUT = os.path.join(ROOT, "docs", "development", "review-coverage.md")

# --- the status of every file -----------------------------------------
#
# done  = read end to end, nothing found
# fixed = read end to end, a defect was found and fixed (see BUGS)
# part  = read in part; the value is the reviewed line ranges
# todo  = not read yet
STATUS = {
    "kernel/src/arch/riscv64/mm/memory_layout.rs": ("part", "s2c: PAGE_OFFSET/WAKE_PTR_FLOOR constants consumed by task.rs wake guard (riscv64 floor comment block verified)"),
    "kernel/src/arch/riscv64/mm/mmu_init.rs": ("part", "kernel image mapping"),
    "kernel/src/arch/uaccess_safe.rs": ("part", "safe uaccess capsules over the arch primitives"),
    "kernel/src/arch/riscv64/trap.rs": ("part", "s2c: trap_handler dispatch, RCU user-return hook (one relaxed load verified), timer/soft/ext irq brackets + leak forensics, syscall entry (SIE/sscratch), FP/RVV lazy-enable incl. the FS/VS hoist, WFI skip, bad-frame gate; panic dump tail not re-read"),
    "kernel/src/arch/riscv64/vext.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/memory_layout.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/mmu_init.rs": ("part", "kernel image mapping"),
    "kernel/src/arch/x86_64/trap.rs": ("part", "s2c: vector dispatch + irq brackets, LOCAL_TIMER EOI-first discipline, RCU user-return twin, irq_exit_maybe_preempt_kernel guards (TIF_IN_SCHED, preemptible, user skip)"),
    "kernel/src/cmdline.rs": ("part", "parameter parsing"),
    "kernel/src/dfx/backtrace.rs": ("part", "symbolized frame printing"),
    "kernel/src/dfx/bug.rs": ("part", "WARN/BUG banners"),
    "kernel/src/dfx/driftwatch.rs": ("part", "release-site attribution"),
    "kernel/src/dfx/hexdump.rs": ("part", "hex dump helper"),
    "kernel/src/dfx/hung_task.rs": ("part", "D-state scan"),
    "kernel/src/dfx/filerd.rs": ("todo", "new file (perf-path lane) — awaiting its owner lane's review"),
    "kernel/src/dfx/forkprof.rs": ("todo", ""),
    "kernel/src/dfx/lostpark.rs": ("todo", ""),
    "kernel/src/dfx/ksyms.rs": ("done", "written by this review (symbol table)"),
    "kernel/src/dfx/ledger.rs": ("part", "page census"),
    "kernel/src/dfx/allocwatch.rs": ("todo", "r3b bookkeeping: file present on main without a map entry (pre-existing --check drift); awaiting its owner lane"),
    "kernel/src/dfx/cctx.rs": ("todo", "r3b bookkeeping: file present on main without a map entry (pre-existing --check drift); awaiting its owner lane"),
    "kernel/src/dfx/leakorigin.rs": ("done", "cr3 closeout: read end to end — diagnostics-only (dfx=leakhunt), every entry point ENABLED-gated (note_acquire via preempt_count_add's caller check; note_release/check_user_return/report_* internal), prints budgeted (400/200/40) direct-console (GRQ/IRQ-laden discipline), outermost-acquire tap (88dd4b82) records base 0->1 stably across inner pairs; no failure-path semantics touched — PASS"),
    "kernel/src/dfx/memwatch.rs": ("part", "heap census"),
    "kernel/src/dfx/metawatch.rs": ("part", "buddy shadow scan"),
    "kernel/src/dfx/mod.rs": ("part", "init, debugfs knob registration"),
    "kernel/src/dfx/monowatch.rs": ("part", "monotonic jump tripwire"),
    "kernel/src/dfx/oops.rs": ("todo", ""),
    "kernel/src/dfx/scmtap.rs": ("part", "cr3: session-fifo hunt tap — every print budgeted, dfx=scmtap gated; no failure-path semantics touched"),
    "kernel/src/dfx/r83hunt.rs": ("part", "task header audit"),
    "kernel/src/dfx/filerd.rs": ("todo", ""),
    "kernel/src/dfx/forkprof.rs": ("todo", ""),
    "kernel/src/dfx/lostpark.rs": ("todo", ""),
    "kernel/src/sync/mutex.rs": ("part", "SleepMutex primitive (sync-realign: EXT4_BIG_LOCK/VFS_MUTATION_LOCK converted); reviewed from the fs/ lock-audit side for the sleeping-section contract"),
    "kernel/src/dfx/pathstash.rs": ("todo", ""),
    "kernel/src/dfx/pstore.rs": ("todo", ""),
    "kernel/src/dfx/sbi_debug.rs": ("part", "raw SBI printers"),
    "kernel/src/dfx/scribble.rs": ("part", "task scribble hunter"),
    "kernel/src/dfx/softlockup.rs": ("part", "soft lockup detector"),
    "kernel/src/dfx/switches.rs": ("done", "switch table refactored to one macro; cr2: ohhunt switch added (95094dd2)"),
    "kernel/src/dfx/syscost.rs": ("part", "per-syscall cost table"),
    "kernel/src/dfx/taint.rs": ("done", "the reference kernel taint letters"),
    "kernel/src/dfx/taskdump.rs": ("part", "task dump format"),
    "kernel/src/dfx/treehunt.rs": ("todo", ""),
    "kernel/src/dfx/wildwatch.rs": ("part", "RO-guard tripwire"),
    "kernel/src/drivers/blkdev/iostat.rs": ("done", "written by this review"),
    "kernel/src/drivers/blkdev/mod.rs": ("part", "request path, register_disk"),
    "kernel/src/drivers/drm/mod.rs": ("fixed", "cr2: GEM lifecycle reviewed against drm_gem_handle_delete — create/close/map dumb, prime export/import (Box<Arc> geometry re-derived), strong_count retention proxy sound (2 residual proxy limitations ledgered in run/review-findings-cr2.md), addfb/addfb2, hunt taps gated; r3b: gem_close re-read — BUG FIXED: shared-object GEM_CLOSE returned -EINVAL (None conflated keep-with-not-found; reference close never fails because another file shares the OBJECT) — outcome split, drop still outside the state lock (2b2a4a52)"),
    "kernel/src/drivers/gpu/*": ("todo", ""),
    "kernel/src/drivers/input/*": ("todo", ""),
    "kernel/src/drivers/intc/*": ("todo", ""),
    "kernel/src/drivers/net/*": ("todo", ""),
    "kernel/src/drivers/pci/*": ("todo", ""),
    "kernel/src/drivers/timer/mod.rs": ("part", "jiffies/monotonic"),
    "kernel/src/drivers/timer/riscv64.rs": ("part", "time CSR timebase"),
    "kernel/src/drivers/timer/x86_64.rs": ("part", "TSC/HPET timebase; cr3: set_next_trigger min(grid,hres) arming + re-arm-first placement in handle_timer_tick; idempotent jiffies derive makes PIT/LAPIC double-tick structurally impossible — PASS"),
    "kernel/src/drivers/virtio/mod.rs": ("part", "s2c: completion walker (collect-under-BLK-lock, deliver outside — INV-LOCK-6/VIRTIO-WQ-1 intact), softirq lock-deferral, assert_no_virtio_lock fence at wake sites; r3b: sync read/write_block paths (R8-M2/R22-6/late-scan), walker positional+device-truthed arms, ABANDON-vs-DELIVERY, lossless re-raise, vw_lag_watchdog re-read — R contracts honored, PASS"),
    "kernel/src/drivers/virtio/offset.rs": ("todo", ""),
    "kernel/src/drivers/virtio/probe.rs": ("todo", ""),
    "kernel/src/drivers/virtio/queue.rs": ("part", "s2c: wait_for_used_interruptible + wait_for_desc_completion (prepare/recheck/deadline/dequeue-compensation discipline, poll_only for early boot and lock holders), submit_quiet publish via avail_shadow"),
    "kernel/src/drivers/virtio/virtio_pci.rs": ("part", "device-ID match arms + per-arch BAR self-assignment windows (riscv 0x40000000, aarch64 DTB 0x10000000, x86 firmware values) reviewed"),
    "kernel/src/fs/bio.rs": ("part", "sync_buffers, ordered publications; cr3: publication_batch_drain (PUB_BATCH=64, journal-stop boundary, D-13), getblk_overwrite (full-4096 skip-read, hit requires uptodate), drop_freed_block_forgotten (dirty-unpinned-only drop + jbd2 forget, pinned/clean left), cache 8192/2048 sizing — all three gates PASS; r3b: bread/get eviction + publication queue re-read in the D-13/ENOSPC context — PASS (dirty-storm wedge filed F-r3b-1/2)"),
    "kernel/src/fs/buffer.rs": ("part", "FileBuffer chunked read/write"),
    "kernel/src/fs/cgroup.rs": ("todo", ""),
    "kernel/src/fs/char_dev.rs": ("todo", ""),
    "kernel/src/fs/vt.rs": ("todo", ""),
    "kernel/src/fs/debugfs.rs": ("done", "written by this review (debugfs)"),
    "kernel/src/fs/dentry.rs": ("fixed", "r3b: read end to end — reader banks + deferred retirement (692e7623) writer side audited: SeqCst claim/fence pairing re-derived sound, all child/inode mutators retire+bump, drain lock order consistent, DENTRY_DRAIN_DEFERS safety-neutral; BUG FIXED: drain dropped retired vecs under DENTRY_SLOW_READ irqsave and the cascade runs ext4_destroy_inode disk I/O with IRQs+preempt off (virtio poll-only = cli bounded only by device latency) — drops moved outside the lock scope (f62a4df6)"),
    "kernel/src/fs/devfs/mod.rs": ("fixed", "s2b: read end to end — entry model, devtmpfs populate, mknod/remove/mkdir/lookup walkers, inode ops (lookup/iget/getattr/setattr/open/get_file_ops), reg-file read/write/read_at, SharedFilePages coherent MAP_SHARED backing, null/zero/full/random/con devices. DEFECT FIXED (d1275bce): getattr/lseek sized regular files from the stale entry.data while a shared backing is active — now the authoritative entry.size; A/B devfsmap probe 5->110 both cases. S_IFMT-vs-S_IFCHR classification and cross-open size coherence verified sound"),
    "kernel/src/fs/ext4/dir.rs": ("part", "directory ops"),
    "kernel/src/fs/ext4/extent.rs": ("fixed", "read end to end (this review): mapping walkers, multi-level append/split/rebuild engine, merge guards, materialize; 3 defects fixed (b70a5deb)"),
    "kernel/src/fs/ext4/file.rs": ("fixed", "read/fill machinery + read_at + read(2) paths + write_at_core/flush_dirty re-read this review (Err-vs-hole fixed, atime parity 1db98ff2); redundant op-level IN_ACCESS self-notifies removed; write body/alloc/lseek/fsync from earlier sessions; r3b: ext4_file_write per-block loop re-read in full — BUG FIXED: mid-loop allocation failure discarded partial progress (Err(ENOSPC) after bytes landed; reference reports the partial count) — partial_or_err arms + partial-safe size tail (6a83a090), host-mirror proptest added in kernel/verify"),
    "kernel/src/fs/ext4/inode.rs": ("fixed", "read end to end (this review): on-disk struct, read/write_inode clamp family, write_inode_disk, inode_table_block_of; s_inode_size clamp fixed (1db98ff2)"),
    "kernel/src/fs/ext4/journal.rs": ("part", "jbd2 transaction start/stop"),
    "kernel/src/fs/ext4/mod.rs": ("part", "superblock, mount; cr2 re-read: mount-time csum verify region (32-bit sb compare) + the add_dir_entry/append/delete seal call sites; r3b: ext4_destroy_inode + PENDING_REAPS/flush_pending_reaps re-read (read_inode in Drop context is the irqsave-drain hazard fixed on the dentry side; comment drift filed F-r3b-4)"),
    "kernel/src/fs/ext4/namei.rs": ("part", "cr2: dir-write paths (ext4_add_entry/add_entry_to_block/find_entry_space/create_initial_entry/dir_block_seal — tail-reserve threading + write_group_descriptor converted to read-modify-write, a6b01fd9), mark_inode_used bitmap-csum refresh, alloc/free-inode counter paths; remaining lookup/unlink/rename bodies pending; cr3: in-place dir scans (scan_dir_block_for_name), tail-first add, bidirectional find (unique names — order is speed-only), fresh-block full init verified, write_block_from_vec getblk_overwrite routing (full-block check >=4096 correct for sub-4K fs blocks via the bread RMW arm) — PASS; r3b: tail_reserve 12-byte tail threading re-verified (deleted-slot + split arms bounded by usable_end; legacy chains fall through on Err) + D-13 publication ordering — PASS"),
    "kernel/src/fs/fifo.rs": ("todo", ""),
    "kernel/src/fs/file.rs": ("part", "reg_file_read/write/lseek、f_pos_lock；O_APPEND 原子性已修（327b4a52）；cr3: arch-gated O_DIRECTORY/O_NOFOLLOW/O_DIRECT/O_LARGEFILE bit positions (aarch64 uapi vs asm-generic) verified against both reference tables; raw literals replaced with the shared constants — PASS"),
    "kernel/src/fs/inode.rs": ("part", "read_data/write_data/append_data；O_APPEND 原子性已修（327b4a52）"),
    "kernel/src/fs/inotify.rs": ("part", "event notification"),
    "kernel/src/fs/jbd2/checkpoint.rs": ("todo", ""),
    "kernel/src/fs/jbd2/commit.rs": ("todo", ""),
    "kernel/src/fs/jbd2/mod.rs": ("todo", ""),
    "kernel/src/fs/jbd2/recovery.rs": ("todo", ""),
    "kernel/src/fs/jbd2/transaction.rs": ("todo", ""),
    "kernel/src/fs/locks.rs": ("part", "flock/POSIX record locks"),
    "kernel/src/fs/memfd.rs": ("fixed", "memfd_create 的参数校验与 seal 语义（参数校验已对齐 879328f3）"),
    "kernel/src/fs/page_cache.rs": ("done", "end to end (pcache Option B assumption audit; no defect)"),
    "kernel/src/fs/path.rs": ("todo", ""),
    "kernel/src/fs/pipe.rs": ("todo", ""),
    "kernel/src/fs/procfs/cmdline.rs": ("todo", ""),
    "kernel/src/fs/procfs/configgz.rs": ("todo", ""),
    "kernel/src/fs/procfs/cpuinfo.rs": ("fixed", "x86 块重复 processor 行已修（真实 x86 每 CPU 块仅块首一次）；kvtab_pad pins 自洽"),
    "kernel/src/fs/procfs/diskstats.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/interrupts.rs": ("part", "per-IRQ counters"),
    "kernel/src/fs/procfs/kallsyms.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/loadavg.rs": ("part", "load average"),
    "kernel/src/fs/procfs/locks.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/meminfo.rs": ("part", "meminfo fields"),
    "kernel/src/fs/procfs/mod.rs": ("part", "节点树、read/write 分发、/proc/<pid> 路径解析; r3b: ino scheme (100..2524 ranges collision-free: PidFileKind 100..200, fd-dir 200, fd-links 300..1324, ns 410..416, attr 510..516, task 620, fdinfo 1400/1500..2524), node types + root pid-dir fabrication + refcount get/put — PASS (init trees/generators not re-derived)"),
    "kernel/src/fs/procfs/mounts.rs": ("todo", ""),
    "kernel/src/fs/procfs/net.rs": ("todo", ""),
    "kernel/src/fs/procfs/pid.rs": ("part", "status/stat generators"),
    "kernel/src/fs/procfs/procfile.rs": ("todo", ""),
    "kernel/src/fs/procfs/schedstat.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/self_proc.rs": ("todo", ""),
    "kernel/src/fs/procfs/softirqs.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/stat.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/swaps.rs": ("todo", ""),
    "kernel/src/fs/procfs/sysctl.rs": ("part", "sysctl tree"),
    "kernel/src/fs/procfs/sysrq.rs": ("done", "written by this review"),
    "kernel/src/fs/procfs/uptime.rs": ("part", "uptime/idle accounting"),
    "kernel/src/fs/procfs/vmstat.rs": ("done", "written by this review"),
    "kernel/src/fs/pty.rs": ("todo", ""),
    "kernel/src/fs/rootfs.rs": ("part", "symlink resolution, MAX_SYMLINKS"),
    "kernel/src/fs/selinuxfs.rs": ("todo", ""),
    "kernel/src/fs/stat.rs": ("todo", ""),
    "kernel/src/fs/sysfs.rs": ("part", "sysfs tree, writable attributes"),
    "kernel/src/fs/tmpfs.rs": ("part", "tmpfs read/write; read_at op reviewed (IN_ACCESS self-notify removed, wrapper owns it)"),
    "kernel/src/fs/vfs.rs": ("part", "file_read/file_write 分发、目录权限与 sticky 位、chroot 下的路径归一化; r3b: path_lookup walk + follow_symlink_hinted + dentry_walk_no_symlink read in full (claim discipline on every arm verified), all Dentry mutator call sites grep-audited, vfs_rename_locked/rmdir/unlink publication vs the reader-bank protocol — PASS; walk-side icache-refresh set_inode under claim = drain deferral only"),
    "kernel/src/fs/xattr.rs": ("todo", ""),
    "kernel/src/interrupt/domain.rs": ("todo", ""),
    "kernel/src/interrupt/irqchip.rs": ("todo", ""),
    "kernel/src/interrupt/irqdesc.rs": ("part", "descriptor registry"),
    "kernel/src/interrupt/ksoftirqd.rs": ("part", "drain loop"),
    "kernel/src/interrupt/mod.rs": ("todo", ""),
    "kernel/src/interrupt/preempt.rs": ("done", "preempt_count 位布局与参考内核一致，irq_enter/exit 的软中断派发条件"),
    "kernel/src/interrupt/softirq.rs": ("fixed", "2ms 预算改为逐 vector 检查（ddfa8abc）"),
    "kernel/src/interrupt/tasklet.rs": ("todo", ""),
    "kernel/src/io_uring/mod.rs": ("todo", ""),
    "kernel/src/ipc/access_token.rs": ("done", "read end to end (s2-a): registry keyed by tgid, synthetic native-token layout, perm-ioctl return-value ABI; all per-ioctl traces dfx-gated"),
    "kernel/src/ipc/binder.rs": ("fixed", "read end to end (s2-a): WORLD-spinlock model audited (no user copies/sleeps under it; fd installs outside; SLOW-TXN ConsoleWriter discipline; wake paths WORLD->GRQ one-way only); alloc engine, BWR write/read loops, txn fixup/translation, death notifications, sweeps; FIXED 3 defects (0e96200e): offsets-entry wraparound OOB (obj_off/parent_off/rel+length now checked_add), sync-recycle blocks reset reply_done/txn (sweep_stale_async would reclaim an in-flight txn buffer; unit row test_binder_alloc_recycle), CLOSE probe budget fetch_sub underflow; perf filed: staged-Vec per read + hist WORLD acquire per BWR (see run/review-findings-s2a.md)"),
    "kernel/src/ipc/mod.rs": ("done", "re-export layer (s2-a)"),
    "kernel/src/ipc/posix_mq.rs": ("done", "read end to end (s2-a): mq_open R36-B3 create race, timedsend/receive prepare_to_wait + R24 entry cleanup, mq_notify SIGEV_* family incl. netlink SIGEV_THREAD, absolute-timeout conversion; filed: mq_timedreceive copy_to_user ret ignored (success returned on partial copy)"),
    "kernel/src/ipc/sysv_msg.rs": ("done", "read end to end (s2-a): msgsnd/msgrcv E2BIG-before-unlink, MSG_COPY ordinals, EIDRM/seq revalidation loops, IPC_SET qbytes CAP_SYS_RESOURCE; minor filed: IPC_SET updates perm before the qbytes EPERM (partial update on failure), msgrcv put_user/copy_to_user return ignored (consume-on-fault matches reference outcome but ret value shape differs)"),
    "kernel/src/ipc/sysv_sem.rs": ("done", "read end to end (s2-a): semget/semctl/semop contracts (errno shapes match reference: E2BIG/EFBIG/ERANGE/EIDRM, GETPID per-sem, SEM_STAT_ANY no permcheck), R39/R7-B6/R9-11 wait discipline sound (slots->sems->wq->GRQ one-way); perf filed: try_apply_semops allocates 3 Vecs per semop (hot IPC path)"),
    "kernel/src/ipc/sysv_shm.rs": ("done", "read end to end (s2-a): shmat ATOMIC-reserve + IPC-C4 nattch discipline, RMID/deferred-destroy, SHM_LOCK bits, sysctl limits; SHMTRAC print budgeted(60) pr_info"),
    "kernel/src/ipc/util.rs": ("done", "idr allocation, permission checks"),
    "kernel/src/main.rs": ("part", "boot sequence, panic handler, layout constants"),
    "kernel/src/mm/buddy_allocator.rs": ("part", "heap allocator, dealloc dispatch"),
    "kernel/src/mm/deferred_free.rs": ("todo", ""),
    "kernel/src/mm/memblock.rs": ("todo", ""),
    "kernel/src/mm/mm_struct.rs": ("part", "total_vm/rss 记账、VMA 增删；记账只增不减已修（e32cdfef）"),
    "kernel/src/mm/mod.rs": ("todo", ""),
    "kernel/src/mm/pcp.rs": ("done", "written by this review (per-CPU pagesets, INV-PCP-1..3)"),
    "kernel/src/mm/page_alloc.rs": ("part", "分配快路径、OOM 判定、直接回收闸门、free 的各路防护"),
    "kernel/src/mm/page_desc.rs": ("fixed", "stale Page.mapping/index field-contract docs corrected (mapping is the cache eviction key, never rmap slot 0); Chinese comment translated"),
    "kernel/src/mm/pglist.rs": ("todo", ""),
    "kernel/src/mm/rmap.rs": ("done", "read end to end (mm review); cr2: try_to_unmap_inner FD-1 reroute re-read (VMA lock discipline + broadcast shootdown arms) — clean. Original note: no code defect. AnonVma/page_add_anon_rmap/page_add_file_rmap/page_get_mappings/rmap_stats are exported scaffolding with zero callers (documented). try_to_unmap correctness rests on ppn revalidation under the PTE lock + caller-held refcounts; it is an O(all tasks) scan per page (Linux anon_vma chains jump straight to the mms) — degraded-but-safe, acknowledged in-code. rmap_alt slots are written only by the legacy non-pcache alias arm: every primary fault site set_index()es slot 0 first, making page_record_mapping a no-op there, so the 4.10 multi-map mechanism is effectively slot-0-only in practice"),
    "kernel/src/mm/slab.rs": ("todo", ""),
    "kernel/src/mm/small_alloc.rs": ("part", "size classes、每 CPU magazine、alloc/free 的快慢路径与 D-7 护栏"),
    "kernel/src/mm/vdso.rs": ("part", "per-arch e_machine arm + ELF header emit reviewed; clock_gettime payload unchanged"),
    "kernel/src/mm/vma.rs": ("fixed", "re-read end to end after main moved (origin stamps are the last functional change): split/merge offset continuity, chain merges, max_end, expand_downwards, find_stack_vma all verified against the reference kernel; the mmap-after-merge metadata-clobber hypothesis disproven by construction (file VMAs are added with fd=-1, so install-time merges are anon-only and the fd/offset writes find the merged VMA harmlessly). Fixed: VMA-ADD-DUP tripwire message claimed 'replacing incumbent' while the code rejects the add with Overlap — message now states the real outcome"),
    "kernel/src/mm/vmscan.rs": ("todo", ""),
    "kernel/src/mm/vmstat.rs": ("done", "written by this review (page counters)"),
    "kernel/src/mm/zone.rs": ("part", "buddy alloc/free、split/merge、水位线；水位公式与参考内核的 6.6 形式有量级很小的差异（更早唤醒 kswapd）"),
    "kernel/src/module/mod.rs": ("todo", ""),
    "kernel/src/net/*": ("part", "s2b: mod.rs/arp/ethernet/icmp/raw/buffer/ipv4/ipv6 spot-read for the lock audit and the F-fix call paths (ethernet_send loopback framing, ip_rcv gates, udp_rcv delivery); full line-by-line pending"),
    "kernel/src/panic_policy.rs": ("done", "written by this review (panic= parameter)"),
    "kernel/src/perf/mod.rs": ("todo", ""),
    "kernel/src/printk.rs": ("part", "ring buffer, console level"),
    "kernel/src/process/coredump.rs": ("todo", ""),
    "kernel/src/process/exec.rs": ("fixed", "read end to end (abi-review s1): image/stack/interp load, auxv, entry frames; redundant zeroing fixed, linear stack/interp writers audited safe-by-single-block"),
    "kernel/src/process/exit.rs": ("todo", ""),
    "kernel/src/process/fork.rs": ("part", "task creation paths; TLS inheritance arms (x86_64 fs/gs bases, aarch64 TPIDR_EL0 live read, CLONE_SETTLS) reviewed this session"),
    "kernel/src/process/kthread.rs": ("todo", ""),
    "kernel/src/process/mod.rs": ("todo", ""),
    "kernel/src/process/pid.rs": ("todo", ""),
    "kernel/src/process/pid_hash.rs": ("part", "task iteration"),
    "kernel/src/process/ptrace.rs": ("todo", ""),
    "kernel/src/process/task.rs": ("part", "task layout, counters, comm; WAKE_PTR_FLOOR per-arch floors verified against each arch's image link base + linear map (riscv floor admits its vmalloc band — looser than needed, not a defect); compat-review: cpus_allowed default + default_cpus_allowed/refresh_default_affinity_to_online read (affinity fix 5252505e/a9cbdac2 family)"),
    "kernel/src/process/wait.rs": ("todo", ""),
    "kernel/src/sched/cgroup.rs": ("done", "cpu/pids/memory controller core: simplified v2 model read clean — rmdir/pids-fork/charge-rollback semantics match the reference contract surface; containment guards (R90/b61) documented"),
    "kernel/src/sched/class.rs": ("done", "sched_class trait 层：类序 stop>dl>rt>fair>idle、SCHED_IDLE 归 fair(weight 3) 均与参考一致；WF_*/ENQUEUE_* 数值与 Linux 不同但纯内部量，非 ABI"),
    "kernel/src/sched/deadline.rs": ("done", "EDF/CBS 结构与实体：双入队守卫(R7-B4)/dequeue 返回契约(R32-DL2)/attr_deadline 读回契约正确；注意 get_bw 为死代码、无全局带宽准入(__dl_overflow)——留待 syscall/sched.rs 轮次；dl 计数已改 checked-sub 防回绕"),
    "kernel/src/sched/fair.rs": ("fixed", "CFS 核心：vruntime 权重方向、min_vruntime、pick 跳过路径（min_vruntime 越界已修 b433fb13）；基线 188d844d 已重读 main 增量（dfx 埋点 SKIPHIST/普查计数/ww_tie_mass、inv_weight 预计算=参考 __update_inv_weight），无新缺陷；类计数 fetch_sub 回绕→sched_slice 乘法溢出 panic 已修（nr_running_dec/load_weight_sub checked-sub + saturating）；cr3: claim_sweep (position-independent stale-claim/oncpu sweep, owner_live + 50-jiffy exec-frozen confirm, full-qword release matches __switch_to store, lying-slot shapes census-only) — PASS"),
    "kernel/src/sched/idle.rs": ("done", ""),
    "kernel/src/sched/mod.rs": ("done", "纯 re-export 层"),
    "kernel/src/sched/rt.rs": ("done", "全文读完：位图反相(bit0=prio99)、head/tail 入队、P08 空转 dequeue、force_unlink 防御、RR timeslice=100ms 均与参考一致；RT 带宽控制在 sched.rs（全局 950ms×MAX_CPUS/1s，比参考的 per-root-domain 粗，pick 时跳过 RT 类）；rt/rr 计数已改 checked-sub 防回绕"),
    "kernel/src/sched/sched.rs": ("fixed", "188d844d 全文读完（见前注）；cr3 (d608a9b7..5bf31528): WAKE-HEAL (wake_up_enqueue_opt, 3-skip adjudication vs sc265 conviction — preempt-discipline refusal + GRQ-release-before-switch make a parked nonzero count phantom 100%; full-zero correct for a no-CPU task), CLAIM-SWEEP hook (scheduler_tick -> fair::claim_sweep), claim_idle_cpu_for + preempt_check_wake (measured-wait CFS gate, D-14), PEREMPT-HEAL cumulative per-task window re-read — all three gates PASS;2b092c49 GRQ 锁交换已撤回（623c4736，13-boot 配对判决= d2a58ccb panic 家族触发器；机制为 codegen/时序揭示的潜在 GRQ 协议竞争，R74-DOUBLE-SEED 为线索，OPEN）"),
    "kernel/src/sched/stats.rs": ("done", "sched_schedstats 运行时门：默认关、Relaxed 读，与参考 sysctl 语义一致"),
    "kernel/src/sched/stop_task.rs": ("done", "stop 类为 PerCpuState 直管的 stub，next_class 链正确"),
    "kernel/src/sched/wakewatch.rs": ("done", "DFX wake-latency instrument: no contract surface; side-table validity/free-invalidation sound, sc207/sc215 softirq deadlock discipline in place"),
    "kernel/src/security/*": ("todo", ""),
    "kernel/src/sync/condvar.rs": ("done", "s2c read end to end: C8 wake-ordering invariant holds on both sides, prepare_to_wait/unlock/schedule ordering clean, restore_irq-before-schedule convention consistent"),
    "kernel/src/sync/lockkey.rs": ("done", "s2c read end to end: proof minting only by live guards, must_use on both guard types (2b092c49), CanNestInside table matches INV-LOCK-5/6"),
    "kernel/src/sync/futex.rs": ("part", "s2c: wait/wake cores, hash (uaddr-only bucketing + Fibonacci spread), waiter pool alloc/free, pool-full backoff, R12-2 compile-checked wake-under-bucket; requeue/PI arms not re-read this round"),
    "kernel/src/sync/rcu.rs": ("done", "s2c read end to end: RCU_WAITERS fast path is one relaxed load on user-return/context-switch; QS reporting + softirq drain + synchronize_rcu wait loop sound"),
    "kernel/src/sync/rwlock.rs": ("fixed", "s2c read end to end: reader-preferring write() could starve a preempt-disabled writer under a continuous cross-CPU reader stream (post-realign hazard) — fixed with the reference qrwlock pending-writer discipline (WRITER_PENDING bit gates new readers; e7616c98); verify mirror protocol tests added"),
    "kernel/src/sync/semaphore.rs": ("done", "s2c read end to end: R77 CAS-take discipline, register-then-recheck, NEW-C2 dequeue compensation on every exit arm"),
    "kernel/src/sync/seqlock.rs": ("fixed", "s2c read end to end: try_write() added PREEMPT_OFFSET while Drop subtracts SOFTIRQ_OFFSET — every successful call left preempt_count at -255; bracket aligned with write()/Drop (a54bf5aa); in-kernel sync_locks rows added"),
    "kernel/src/sync/spinlock.rs": ("done", "s2c read end to end post-realign: pure spinning acquire, no schedule/state change anywhere, TTAS + DEADLOCK_WARN print-only, guard preempt pairing balanced on all variants"),
    "kernel/src/sync/mutex.rs": ("done", "s2c read end to end: SleepMutex matches the reference mutex recipe (state spinlock instructions-only, queue-based FIFO park, wake-one handoff token, R84/R85 fairness + epoch self-heal, R73 exit force-drop); no sleep reachable with a spinlock held"),

    "kernel/src/sync/condvar.rs": ("todo", ""),
    "kernel/src/sync/mutex.rs": ("todo", "sync lane owns (SleepMutex)"),
    "kernel/src/dfx/filerd.rs": ("todo", ""),
    "kernel/src/dfx/forkprof.rs": ("todo", ""),
    "kernel/src/dfx/lostpark.rs": ("todo", ""),
    "kernel/src/config.rs": ("todo", "generated from Kernel.toml"),
    "kernel/src/dfx/ksyms_generated.rs": ("todo", "generated"),
    "kernel/src/sync/lockkey.rs": ("todo", "typed lock keys (D-11, refactor lane 2026-10-28)"),
    "kernel/src/sync/rcu.rs": ("todo", ""),
    "kernel/src/sync/rwlock.rs": ("todo", ""),
    "kernel/src/sync/semaphore.rs": ("todo", ""),
    "kernel/src/sync/seqlock.rs": ("todo", ""),
    "kernel/src/sync/spinlock.rs": ("todo", ""),    "kernel/src/syscall/dispatch.rs": ("todo", ""),
    "kernel/src/syscall/file.rs": ("part", "futex2 wrappers, fsync; compat-review: xattr syscall family read end to end (set/get/list/remove + *at variants) — found and fixed the xattr_read_name errno double-negation (LTP setxattr01/fsetxattr01/fremovexattr02 EFAULT contracts); readlinkat read (ring/NOFOLLOW trailing-slash divergence recorded in run/review-findings-compat.md)"),
    "kernel/src/syscall/io.rs": ("part", "read/write/pread64 的分块与用户指针校验; cr2: drm/prime ioctl dispatch region + ohhunt tap gating (95094dd2)"),
    "kernel/src/syscall/memory.rs": ("part", "mmap/munmap/mprotect/mremap/brk；msync 回绕已修（ff6fc39b）、mmap 偏移未页对齐已修（b433fb13）、brk 记账已修（e32cdfef）; cr2: drm GEM mmap region + ohhunt tap gating (95094dd2)"),
    "kernel/src/syscall/misc.rs": ("part", "timerfd family aligned v6.6 (4e1f6110); s2-a: poll/ppoll/pselect/select/epoll_wait head + eventfd2 read (poll fd<0 slots, revents mask, event-driven P3 wait round, maxevents truncation rule, eventfd flag validation); perf filed: per-iteration Vec<PollFd> alloc in sys_poll and ready_events alloc under epoll.entries lock; signalfd4_impl/eventfd write path remain"),
    "kernel/src/syscall/mod.rs": ("todo", ""),
    "kernel/src/syscall/network.rs": ("fixed", "read end to end (this review): socket/bind/listen/accept/connect/send/recv family + get/setsockopt + mmsg batchers; 4 defects fixed — recvmsg msg_namelen VALUE field was dereferenced as a pointer (write-back gate ate by garbage pointer, msg_namelen never updated), put_sockaddr_* ignored the caller's buffer length (move_addr_to_user clamp), v6 write-backs ran 28B through a 16B-validated range, unix dgram sendto/recvfrom silently truncated at RW_CHUNK 64KB (all-or-nothing datagram contract); A/B probe test/sockaddr_clamp.c (sockaddr_clamp_boot.py): 6 fixable cases FAIL->PASS; findings F5-F11 filed (run/review-findings-func.md) incl. loopback TCP connect wedge vs accept"),
    "kernel/src/syscall/process.rs": ("part", "exec ladder + credentials + shebang + EXECCOST instrument (abi-review s1); reboot cascade, clone/wait/ptrace ladders still pending"),
    "kernel/src/syscall/time.rs": ("part", "nanosleep, realtime clock"),
    "kernel/src/syscall/x86_compat.rs": ("done", "every table entry audited against the host x86_64 + riscv64 asm-generic unistd headers (0 mismatches); targets cross-checked against dispatch arms; legacy block (poll/select/dup2/pause/epoll_create/epoll_wait/arch_prctl) covers the no-generic numbers"),
    "kernel/src/tests/*": ("todo", ""),
    "kernel/src/trace/files.rs": ("done", "written by this review (tracefs files)"),
    "kernel/src/trace/mod.rs": ("done", "written by this review (tracepoint core)"),
    "kernel/src/arch/aarch64/boot.rs": ("done", "DTB-pointer extern + get_dtb_pointer; boot.S publishes the pointer pre-MMU and re-publishes post-MMU"),
    "kernel/src/arch/aarch64/boot/boot.S": ("fixed", "read end to end (this review): EL2 drop, early TTBR0/TTBR1 tables, MAIR/TCR, secondary_start PSCI path; stray 'str x0, [x1]' stored the vector-table base into dtb_pointer (latent — the post-MMU re-publish repaired it before any consumer), store removed"),
    "kernel/src/arch/aarch64/dtb_blob.S": ("done", "7-line .incbin of the QEMU virt DTB"),
    "kernel/src/arch/aarch64/trap.S": ("done", "read end to end: VBAR vectors, TRAP_BODY save protocol, SPSR-based EL0/EL1 sp select, FPSIMD capture/restore, exit loop preempt gate, restore_and_eret slot map, ret_from_fork trampolines"),
    "kernel/src/drivers/timer/aarch64.rs": ("todo", ""),
    "kernel/src/arch/aarch64/context.rs": ("part", "switch_mm/TTBR0/ASID paths, context_switch step order, R56 switch_abort twin — mirrors riscv64"),
    "kernel/src/arch/aarch64/cpu.rs": ("done", "DAIF/timer/TPIDR/barrier primitives; local_flush_tlb_all vmalle1"),
    "kernel/src/arch/aarch64/ipi.rs": ("part", "IPI surface mirrors the riscv64 twin (request/send/flush/handle); GIC SGI wiring from the aarch64 campaign"),
    "kernel/src/arch/aarch64/mm/asid.rs": ("fixed", "read end to end (this review): tlbi operand encodings, composed satp; the bump-only ASID counter exhausted permanently after 254 mm creations (later mms ran ASID 0 = TLB-miss-per-access) — riscv64 twin's reuse bitmap ported, AsidContext::free gained the twin's flush-before-free"),
    "kernel/src/arch/aarch64/mm/exception.rs": ("fixed", "read end to end (this review): fixup table search, bad_area/no_context, kernel demand-fill-before-fixup (R82/R78 parity), non-canonical gate — the in_interrupt guard both twins carry was MISSING (an IRQ-context fault would run the sleeping mm path); guard added, x86_64 form"),
    "kernel/src/arch/aarch64/mm/fixmap.rs": ("done", "slot map anchored at VMEMMAP_START, PL011 earlycon slot"),
    "kernel/src/arch/aarch64/mm/memory_layout.rs": ("done", "TTBR1 bands, VA_PA_OFFSET identity-from-zero linear map, mmio_alias/ECAM alias, VirtAddr sign extension, user_addr block (256GB parity)"),
    "kernel/src/arch/aarch64/mm/mm_ops.rs": ("done", "read end to end: walker, mmap/MAP_FIXED split surgery, munmap/zap, unmap_pages device/foreign guards, fork COW walk + SMP shootdown, handle_cow_fault (S7 pre-alloc, tri-state, shootdown), mprotect range + PROT_NONE markers; note: mprotect_pte_range doc says aside1e1is but emits all-ASID vaae1is (conservative, correct)"),
    "kernel/src/arch/aarch64/mm/mmu_init.rs": ("done", "read end to end: staged PT allocator + ledger-guarded frees, TTBR1 adoption from boot.S, linear-map block sizing, device windows, free_user_page_tables teardown"),
    "kernel/src/arch/aarch64/mm/mod.rs": ("done", "module re-export tree"),
    "kernel/src/arch/aarch64/mm/page_fault.rs": ("done", "read end to end: stack expansion, PROT_NONE marker re-presentation, handle_mm_fault fill arms (file SIGBUS at EOF, effective-size re-derivation, race-lost free), PTE section + R7-C3 re-checks"),
    "kernel/src/arch/aarch64/mm/pagetable.rs": ("done", "descriptor model: hardware attrs synthesized from software R/W/X/U/G bits, AP/AP_RO/XN/nG derivation, PROT_NONE marker constructor, no table-XN bits, Satp abstraction"),
    "kernel/src/arch/aarch64/mod.rs": ("todo", ""),
    "kernel/src/arch/aarch64/process.rs": ("part", "start_thread PSTATE, copy_thread parity, flush_thread (FPSIMD + tp_value + live TPIDR_EL0 clear) reviewed"),
    "kernel/src/arch/aarch64/pt_regs.rs": ("done", "Linux arm64 frame layout (320B 16-aligned), ESR decode, pseudo-cause IRQ/FIQ/SError mapping"),
    "kernel/src/arch/aarch64/smp.rs": ("part", "mpidr mapping, boot-complete park/signl shape reviewed against the riscv64 twin"),
    "kernel/src/arch/aarch64/thread.rs": ("done", "FPSIMD eager model: live fpu image + user_fpsimd entry-capture buffer, offset exports"),
    "kernel/src/arch/aarch64/trap.rs": ("fixed", "read end to end (mm/ + boot remainder of the earlier routing pass): dispatch, fault-flag derivation (WnR ISS bit 6), CURRENT_PT_REGS swap, bad-vector dumper; dfsc_class rows contradicted the ARM DFSC encoding (AF 0x09-0x0C / permission 0x0D-0x0F|0x13 vs truth AF 0x08-0x0B, permission 0x0C-0x0F; dead diagnostics) — rows corrected"),
    "kernel/src/arch/aarch64/uaccess.rs": ("part", "access_ok/copy_from_user/clear_user semantics match the twins (zero-fill tails); byte-loop extable copies; NOTE: no explicit fault_in_write retry in copy_to_user — covered by do_page_fault's kernel-uaccess demand-fill-first (R82), a belt-layer divergence from the riscv64/x86_64 second belt, not a defect"),
    "kernel/src/arch/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/boot.S": ("todo", ""),
    "kernel/src/arch/riscv64/boot.rs": ("todo", ""),
    "kernel/src/arch/riscv64/context.rs": ("todo", ""),
    "kernel/src/arch/riscv64/cpu.rs": ("todo", ""),
    "kernel/src/arch/riscv64/ipi.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/asid.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/exception.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/fixmap.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/mm_ops.rs": ("part", "COW 故障快/慢路径、PTE 权限检查、mmap/MAP_FIXED/munmap/fork 的 VMA 手术与记账（e32cdfef）; cr2: FD-1 leaf primitives (leaf_pte_replace/install, pte_flag_bits) reviewed — Sv39 walk shape, expect_pfn re-validation, 0x3FF flag mask all correct; s2c: PTE lock-mode audit (pte_lock_for split-mode + stale GLOBAL-mode doc fixed 344ce32c), COW fault alloc-before-lock/S7 + TLB shootdowns re-verified"),
    "kernel/src/arch/riscv64/mm/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mm/page_fault.rs": ("fixed", "R-SC-RV64: file-fault path re-derived (vma_start, file, effective_size) in 4 separately locked lookups (shared-alias site took TWO back-to-back vma_read acquisitions) - mismatched generations could pair one file with another VMA's start and pin a foreign file's cache page; single entry-time snapshot ported from the x86_64 twin (3 finds + up to 4 pin-table lookups per file fault -> 1) + devfs live-size (b74) parity"),
    "kernel/src/arch/riscv64/mm/pagetable.rs": ("todo", ""),
    "kernel/src/arch/riscv64/mod.rs": ("todo", ""),
    "kernel/src/arch/riscv64/process.rs": ("todo", ""),
    "kernel/src/arch/riscv64/pt_regs.rs": ("todo", ""),
    "kernel/src/arch/riscv64/smp.rs": ("part", "compat-review: start_secondaries flow + num_started_cpus semantics read (affinity default-mask fix wired here); CPU_STARTED accounting verified"),
    "kernel/src/arch/riscv64/thread.rs": ("todo", ""),
    "kernel/src/arch/riscv64/trap.S": ("todo", ""),
    "kernel/src/arch/riscv64/uaccess.S": ("todo", ""),
    "kernel/src/arch/riscv64/uaccess.rs": ("part", "cr3: strncpy_from_user chunked rewrite reviewed (same verdict as the x86_64 twin); duplicate shadowed limit pair removed"),
    "kernel/src/arch/x86_64/boot.S": ("todo", ""),
    "kernel/src/arch/x86_64/boot.rs": ("todo", ""),
    "kernel/src/arch/x86_64/context.rs": ("todo", ""),
    "kernel/src/arch/x86_64/cpu.rs": ("todo", ""),
    "kernel/src/arch/x86_64/ipi.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/asid.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/exception.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/fixmap.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/mm_ops.rs": ("part", "mmap/MAP_FIXED/munmap/fork 的 VMA 手术与记账（与 riscv64 同构，e32cdfef）; cr2: FD-1 leaf primitives reviewed — 4-level walk (39/30/21/12) with correct root-not-leaf handling, re-validation, flag mask correct; s2c: brk eager-map path (alloc outside PTE section, locked re-check, PT-fail anti-leak), pte_lock_for split-mode, COW twin spot-check"),
    "kernel/src/arch/x86_64/mm/mod.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/page_fault.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mm/pagetable.rs": ("todo", ""),
    "kernel/src/arch/x86_64/mod.rs": ("todo", ""),
    "kernel/src/arch/x86_64/process.rs": ("todo", ""),
    "kernel/src/arch/x86_64/pt_regs.rs": ("todo", ""),
    "kernel/src/arch/x86_64/setup.S": ("todo", ""),
    "kernel/src/arch/x86_64/smp.rs": ("todo", ""),
    "kernel/src/arch/x86_64/smp_trampoline.S": ("todo", ""),
    "kernel/src/arch/x86_64/thread.rs": ("todo", ""),
    "kernel/src/arch/x86_64/trap.S": ("todo", ""),
    "kernel/src/arch/x86_64/uaccess.rs": ("part", "cr3: strncpy_from_user chunked rewrite reviewed (64B chunks, mid-chunk fault zero-fills tail, Ok/EFAULT shapes byte-loop-identical, NUL-at-0 success kept); duplicate shadowed limit pair removed; s2c: SMAP runtime gate verified end to end (CPUID.7.0.EBX[20] -> CR4.SMAP -> SMAP_ENABLED byte read by the asm brackets; stac/clac at every site incl. fixup labels)"),
    "kernel/src/config.rs": ("todo", ""),

    "kernel/src/arch/x86_64/uaccess.rs": ("part", "cr3: strncpy_from_user chunked rewrite reviewed (64B chunks, mid-chunk fault zero-fills tail, Ok/EFAULT shapes byte-loop-identical, NUL-at-0 success kept); duplicate shadowed limit pair removed"),    "kernel/src/console.rs": ("part", "arch UART arms: x86_64 16550 THRE/stall machinery, riscv64 raw sb, aarch64 PL011 (FR 0x18/IMSC 0x38/ICR 0x44) — PL011 init_irq follows the twins' ownership discipline (pre-arm drain, IRQ-owned RX, armed-flag fallback); cr3: batched TX family (puts_bytes lock-once, fmt::Write coalescing with \r\n expansion, ConsoleWriter staging with Drop flush, uart_write_kernel 32B chunk+cond_resched) — byte-identical streams, per-slice irqsave span is the one ledger note (ms-class at boot volumes) — PASS"),
    "kernel/src/dfx/fsw.rs": ("todo", ""),
    "kernel/src/dfx/schb.rs": ("todo", ""),
    "kernel/src/dfx/slowtask.rs": ("todo", ""),
    "kernel/src/drivers/access_tokenid.rs": ("todo", ""),
    "kernel/src/drivers/ashmem.rs": ("todo", ""),
    "kernel/src/drivers/gpu/fb_simple.rs": ("todo", ""),
    "kernel/src/drivers/gpu/fbdev.rs": ("todo", ""),
    "kernel/src/drivers/gpu/framebuffer.rs": ("todo", ""),
    "kernel/src/drivers/gpu/mod.rs": ("todo", ""),
    "kernel/src/drivers/gpu/virtio_cmd.rs": ("todo", ""),
    "kernel/src/drivers/gpu/virtio_gpu.rs": ("todo", ""),
    "kernel/src/drivers/input/evdev.rs": ("todo", ""),
    "kernel/src/drivers/input/event.rs": ("todo", ""),
    "kernel/src/drivers/input/mod.rs": ("todo", ""),
    "kernel/src/drivers/input/ps2.rs": ("todo", ""),
    "kernel/src/drivers/input/virtio_input.rs": ("todo", ""),
    "kernel/src/drivers/intc/apic.rs": ("part", "cr3: timer_arm_oneshot (LVT one-shot + TMICT, u128 conversion, past-deadline clamp to 1 count, freq==0/!READY refuse keeps periodic) — PASS"),
    "kernel/src/drivers/intc/clint.rs": ("todo", ""),
    "kernel/src/drivers/intc/mod.rs": ("todo", ""),
    "kernel/src/drivers/intc/plic.rs": ("todo", ""),
    "kernel/src/drivers/loop_dev.rs": ("todo", ""),
    "kernel/src/drivers/mod.rs": ("todo", ""),
    "kernel/src/drivers/net/loopback.rs": ("todo", ""),
    "kernel/src/drivers/net/mod.rs": ("todo", ""),
    "kernel/src/drivers/net/space.rs": ("todo", ""),
    "kernel/src/drivers/net/virtio_net.rs": ("todo", ""),
    "kernel/src/drivers/pci/mod.rs": ("fixed", "device-ID table + ECAM scan walk: 04e23b45 shifted the modern virtio IDs (blk 0x1042->0x1041), losing every disable-legacy=on root disk; restored to spec IDs 0x1040+dev (net 1/blk 2), QEMU-verified"),
    "kernel/src/drivers/rtc.rs": ("todo", ""),
    "kernel/src/drivers/timer/hpet.rs": ("todo", ""),
    "kernel/src/drivers/watchdog.rs": ("todo", ""),
    "kernel/src/errno.rs": ("todo", ""),
    "kernel/src/fs/dev_t.rs": ("todo", ""),
    "kernel/src/fs/devfs/registry.rs": ("todo", ""),
    "kernel/src/fs/elf.rs": ("fixed", "read end to end (abi-review s1); this review: machine-check cfg hole on x86_64 fixed (validate now checks e_machine on every platform)"),
    "kernel/src/fs/ext4/allocator.rs": ("part", "geometry/addressing sites + alloc/free paths around them (EXT4-64BIT hi-half fix, 070770d6); full read pending"),
    "kernel/src/fs/ext4/csum.rs": ("fixed", "read end to end (cr2): crc32c + all six checksum classes; 3 write-path divergences vs e2fsprogs fixed (a6b01fd9) — sb le32 full-crc seal, 12-byte dir tail with markers + generation-chained seed + blocksize-12 span, extent seed contract; proven with the run/cr2-csum-harness e2fsck oracle (reseal bit-identical, dirfix clean)"),
    "kernel/src/fs/ext4/indirect.rs": ("todo", ""),
    "kernel/src/fs/ext4/superblock.rs": ("part", "cr2: Ext4GroupDesc/Ext4SuperBlock field layout vs on-disk offsets (write_group_descriptor RMW fix)"),
    "kernel/src/fs/fs_struct.rs": ("todo", ""),
    "kernel/src/fs/io_completion.rs": ("todo", ""),
    "kernel/src/fs/jbd2/journal.rs": ("todo", ""),
    "kernel/src/fs/jbd2/revoke.rs": ("todo", ""),
    "kernel/src/fs/jbd2/types.rs": ("todo", ""),
    "kernel/src/fs/mod.rs": ("todo", ""),
    "kernel/src/fs/mount.rs": ("todo", ""),
    "kernel/src/fs/permission.rs": ("todo", ""),
    "kernel/src/fs/procfs/timer_list.rs": ("todo", ""),
    "kernel/src/fs/procfs/version.rs": ("todo", ""),
    "kernel/src/fs/readahead.rs": ("todo", ""),
    "kernel/src/fs/superblock.rs": ("todo", ""),
    "kernel/src/fs/tty.rs": ("todo", ""),
    "kernel/src/init.rs": ("todo", ""),
    "kernel/src/initrd.rs": ("todo", ""),
    "kernel/src/list.rs": ("todo", ""),
    "kernel/src/mm/allocator.rs": ("todo", ""),
    "kernel/src/mm/compact.rs": ("part", "cr2: remap_page FD-1 reroute through leaf_pte_install reviewed (migration window + Migrating-flag wait) — clean"),
    "kernel/src/mm/hugepage.rs": ("todo", ""),
    "kernel/src/mm/kswapd.rs": ("todo", ""),
    "kernel/src/mm/layout.rs": ("todo", ""),
    "kernel/src/mm/lru.rs": ("todo", ""),
    "kernel/src/mm/meminfo.rs": ("todo", ""),
    "kernel/src/mm/oom_kill.rs": ("todo", ""),
    "kernel/src/mm/page.rs": ("todo", ""),
    "kernel/src/mm/pagemap.rs": ("todo", ""),
    "kernel/src/mm/swap.rs": ("part", "cr2: entry-encoding section (swap bit 62 / migration bit 61 signatures, type/offset codec, pending-write tracking) — internally consistent, clean"),
    "kernel/src/mm/vmemmap.rs": ("todo", ""),
    "kernel/src/net/arp.rs": ("todo", ""),
    "kernel/src/net/buffer.rs": ("todo", ""),
    "kernel/src/net/ethernet.rs": ("todo", ""),
    "kernel/src/net/icmp.rs": ("todo", ""),
    "kernel/src/net/ipv4/checksum.rs": ("todo", ""),
    "kernel/src/net/ipv4/defrag.rs": ("todo", ""),
    "kernel/src/net/ipv4/mod.rs": ("todo", ""),
    "kernel/src/net/ipv4/route.rs": ("todo", ""),
    "kernel/src/net/ipv6/icmpv6.rs": ("todo", ""),
    "kernel/src/net/ipv6/mod.rs": ("todo", ""),
    "kernel/src/net/mod.rs": ("todo", ""),
    "kernel/src/net/netlink.rs": ("fixed", "s2b: read socket model, exec request dispatch (rtnl), send/recv engines, uevent broadcast + monitor multicast, sockaddr helpers, ioctl stubs. DEFECTS FIXED: b8a38043 (put_sockaddr_nl* clamp to caller's length), 922e158b (F11 portid-routed unicast: NETLINK_PORTID_TABLE, bind adopts nl_pid with EADDRINUSE, unicast sends deliver/ECONNREFUSED)"),
    "kernel/src/net/raw.rs": ("todo", ""),
    "kernel/src/net/socket.rs": ("fixed", "cr3: socket_file_stat (S_IFSOCK|0777 + per-description st_ino, ops-ptr dispatch for inet/unix/netlink/raw) — closes the EBADF fstat hole; reference sockfs behavior — PASS. s2b re-read: connect engine (loopback drain + blocking wait), send/recv engines, shutdown path, close/fin paths. DEFECTS FIXED: F7 peek plumbed through socket_recv_ctl/Socket::recv (4edca91e); F10 socket_shutdown_check ENOTCONN discipline (219fce00); F8 UDP zero-length datagram no longer EAGAIN-after-consume (6a1e7eb7)"),
    "kernel/src/net/tcp.rs": ("done", "s2b: read end to end — headers/options, TcpTxBatch (R35 deferred emit), RTT/congestion/timers, full state machine (handle_packet per state, RST validation, SYN spawn/pass dispatch, window-update snd_wl1/wl2 test, OOO trim+drain, partial-ACK trim, fast retransmit-once, persist/keepalive probes, TIME_WAIT reclaim), tcp_rcv/6 checksum gates, tcp_v4_err. No new defects; F7 MSG_PEEK added to TcpSocket::recv (4edca91e), prior rounds' findings 1-10 pinned by comments"),
    "kernel/src/net/tcp_timer.rs": ("todo", ""),
    "kernel/src/net/udp.rs": ("fixed", "s2b: read end to end — socket/table model, bind/connect, send engines v4+v6, recvfrom_ext (F7 peek arm), rcv/rcv6 delivery scans, checksum. DEFECTS FIXED: (1) 70a1af5d — sends emitted inline under UDP_TABLE_LOCK (virtio xmit wait under irqsave spinlock, R34/R35 wedge class); build/emit split via BuiltUdp; (2) F7/F8 — peek_packet head view; a real zero-length datagram was consumed then reported EAGAIN (6a1e7eb7)"),
    "kernel/src/net/unix.rs": ("done", "s2b: read end to end — locking model (UNIX_TABLE + per-socket spinlocks, Weak peer links), name registry (abstract + fs inode-identity keys), bind/listen/connect (backlog wait discipline, autobind, epoll edge notify), send engine (flow-control charge, taps), recv engine (stream/seqpacket/dgram shapes, peek, first-touch ancillary), shutdown/close, file ops, setsockopt. DEFECT FIXED: put_sockaddr_un clamps to the caller's length (b8a38043). No other defects; leg-34 fast paths verified sound"),
    "kernel/src/print.rs": ("todo", ""),
    "kernel/src/process/ns.rs": ("done", "compat-review: read end to end (UTS namespace) — found and fixed the empty default hostname (reference default is \"(none)\", LTP gethostname02)"),
    "kernel/src/sbi.rs": ("todo", ""),
    "kernel/src/security/cap_lsm.rs": ("todo", ""),
    "kernel/src/security/capability.rs": ("todo", ""),
    "kernel/src/security/lsm.rs": ("todo", ""),
    "kernel/src/security/mod.rs": ("todo", ""),
    "kernel/src/signal.rs": ("fixed", "read end to end (s2-a, prior rounds covered flush_handlers + per-arch cfg arms): do_signal selection loop + sigmask_restore discipline, unkillable-init filter, ptrace intercept, restart_syscall_no_handler on all no-frame paths, per-arch frame build/restore (M-04 user-copy-wins, SPP/SPIE/IF privilege stripping), default actions, send path (pinned lookups, ns pid translation, RLIMIT_SIGPENDING, sig_ignored ordering); pre-existing budgeted prints noted in ledger (KSIG/TKSIG 4000, DEQUEUE 120, SWI 120)"),
    "kernel/src/sync/mod.rs": ("todo", ""),
    "kernel/src/sync/percpu.rs": ("todo", ""),
    "kernel/src/syscall/sched.rs": ("todo", "s2-a lane did not reach; sched-class side reviewed in sched/ rounds"),
    "kernel/src/syscall/signal.rs": ("done", "read end to end (s2-a): rt_sigaction per-arch user ABI structs, procmask NULL-set query semantics, sigreturn sp-derived frame + forged-frame SIGSEGV, sigpending pending&blocked, sigaltstack, sigsuspend/pause state-first loops, tkill EINVAL/permission order"),
    "kernel/src/tests/boundary.rs": ("todo", ""),
    "kernel/src/tests/buffer_state.rs": ("todo", ""),
    "kernel/src/tests/checksum.rs": ("todo", ""),
    "kernel/src/tests/config_test.rs": ("todo", ""),
    "kernel/src/tests/dcache.rs": ("todo", ""),
    "kernel/src/tests/dev_t.rs": ("todo", ""),
    "kernel/src/tests/errno_test.rs": ("todo", ""),
    "kernel/src/tests/execve.rs": ("todo", ""),
    "kernel/src/tests/ext4_allocator.rs": ("todo", ""),
    "kernel/src/tests/ext4_file_write.rs": ("todo", ""),
    "kernel/src/tests/fcntl.rs": ("todo", ""),
    "kernel/src/tests/fileops_caps.rs": ("todo", ""),
    "kernel/src/tests/fdtable.rs": ("todo", ""),
    "kernel/src/tests/file_flags.rs": ("todo", ""),
    "kernel/src/tests/file_open.rs": ("todo", ""),
    "kernel/src/tests/fork.rs": ("todo", ""),
    "kernel/src/tests/framebuffer.rs": ("todo", ""),
    "kernel/src/tests/fstat.rs": ("todo", ""),
    "kernel/src/tests/futex_test.rs": ("todo", ""),
    "kernel/src/tests/getpid.rs": ("todo", ""),
    "kernel/src/tests/getdents_cache.rs": ("todo", ""),
    "kernel/src/tests/heap_allocator.rs": ("todo", ""),
    "kernel/src/tests/icache.rs": ("todo", ""),
    "kernel/src/tests/initrd.rs": ("todo", ""),
    "kernel/src/tests/ipc_epoll.rs": ("todo", ""),
    "kernel/src/tests/ipc_eventfd.rs": ("todo", ""),
    "kernel/src/tests/ipc_poll.rs": ("todo", ""),
    "kernel/src/tests/ipc_sysv.rs": ("todo", ""),
    "kernel/src/tests/link.rs": ("todo", ""),
    "kernel/src/tests/listhead.rs": ("todo", ""),
    "kernel/src/tests/loop_flags.rs": ("todo", ""),
    "kernel/src/tests/mem_cow.rs": ("todo", ""),
    "kernel/src/tests/mem_mmap.rs": ("todo", ""),
    "kernel/src/tests/mkdir_unlink.rs": ("todo", ""),
    "kernel/src/tests/mod.rs": ("todo", ""),
    "kernel/src/tests/mount_flags.rs": ("todo", ""),
    "kernel/src/tests/network.rs": ("todo", ""),
    "kernel/src/tests/page_allocator.rs": ("todo", ""),
    "kernel/src/tests/path.rs": ("todo", ""),
    "kernel/src/tests/percpu.rs": ("todo", ""),
    "kernel/src/tests/pid_test.rs": ("todo", ""),
    "kernel/src/tests/pipe2.rs": ("todo", ""),
    "kernel/src/tests/preemptive_scheduler.rs": ("todo", ""),
    "kernel/src/tests/process_tree.rs": ("todo", ""),
    "kernel/src/tests/reparent_safety.rs": ("todo", ""),
    "kernel/src/tests/scheduler.rs": ("part", "guest 侧调度断言：vruntime 公式与权重方向（b433fb13 新增）"),
    "kernel/src/tests/semaphore.rs": ("todo", ""),
    "kernel/src/tests/sgid_mode.rs": ("todo", ""),
    "kernel/src/tests/signal.rs": ("todo", ""),
    "kernel/src/tests/signal_procmask.rs": ("todo", ""),
    "kernel/src/tests/sleep_wakeup.rs": ("todo", ""),
    "kernel/src/tests/small_alloc.rs": ("todo", ""),
    "kernel/src/tests/smp.rs": ("todo", ""),
    "kernel/src/tests/smp_schedule.rs": ("todo", ""),
    "kernel/src/tests/syscall_file.rs": ("todo", ""),
    "kernel/src/tests/syscall_io.rs": ("todo", ""),
    "kernel/src/tests/syscall_memory.rs": ("part", "guest 侧内存 syscall 断言：mmap 偏移非页对齐返回 EINVAL（b433fb13 新增）"),
    "kernel/src/tests/syscall_misc.rs": ("todo", ""),
    "kernel/src/tests/syscall_network.rs": ("todo", ""),
    "kernel/src/tests/syscall_process.rs": ("todo", ""),
    "kernel/src/tests/syscall_sched.rs": ("todo", ""),
    "kernel/src/tests/syscall_signal.rs": ("todo", ""),
    "kernel/src/tests/syscall_time.rs": ("todo", ""),
    "kernel/src/tests/tcp_handshake.rs": ("todo", ""),
    "kernel/src/tests/tmpfs_mknod.rs": ("todo", ""),
    "kernel/src/tests/virtio_net.rs": ("todo", ""),
    "kernel/src/tests/virtio_queue.rs": ("todo", ""),
    "kernel/src/tests/vma_flags.rs": ("todo", ""),
    "kernel/src/tests/wait4.rs": ("todo", ""),
    "kernel/src/timer.rs": ("todo", ""),}

# Bugs found and fixed by this review line.
BUGS = [
    ("f62a4df6", "kernel/src/fs/dentry.rs",
     "The reader-bank drain (692e7623) dropped the retired dentry/inode vecs while holding "
     "DENTRY_SLOW_READ under lock_irqsave. The drop cascade frees whole dentry subtrees and "
     "runs Inode::drop -> ext4_destroy_inode -> read_inode -> bio::bread — synchronous disk "
     "I/O with IRQs and preemption off, where the virtio completion wait degrades to the "
     "poll-only MMIO spin (1e6 iterations plus read_block's 5e7-iteration late scan): a cli "
     "segment bounded only by device latency, past the 3-7ms IRQ-gate class, tick/IPI loss, "
     "watchdog bait. Reachable from every unlink/rmdir whose retired inode is the last "
     "reference (they icache_remove BEFORE clear_inode) and from walk-side icache-refresh "
     "set_inode retires. Fix: take the vecs under the slow lock, drop AFTER the lock scope — "
     "the quiescence proof (banks zero at scan; fresh readers cannot reach the Arcs) never "
     "needed the lock held during the drop."),
    ("6a83a090", "kernel/src/fs/ext4/file.rs",
     "ext4_file_write returned Err(ENOSPC) from the per-block loop even after earlier blocks "
     "of the same request had landed in dirty buffers — a write into an interior hole of a "
     "sparse file on a full disk discarded its own partial progress. write(2) and the "
     "reference kernel report the partial count when bytes landed; only a request that wrote "
     "nothing fails. Every mid-loop failure arm (hole/block allocation, extent materialize, "
     "buffer fetch) now degrades to partial success once total_written > 0, and the size tail "
     "tracks only what landed (offset + written). Host-mirror proptest added in kernel/verify "
     "(fs::ext4::write_partial_test): the old body fails 3/4 tests, the fixed body 4/4."),
    ("2b2a4a52", "kernel/src/drivers/drm/mod.rs",
     "gem_close conflated its two None outcomes: 'handle not found' and 'entry kept because "
     "another holder still references the object' — the shared case returned -EINVAL although "
     "the caller's close succeeded. The reference's per-file handle tables make a close never "
     "fail because another file shares the OBJECT, so Mesa's GEM_CLOSE on shared buffers (the "
     "oh-glm5 EGLImage shape the shared-keep proxy exists for) must see 0. Outcome split into "
     "Removed/SharedKeep/NotFound; the object drop still runs outside the state lock."),
    ("4ee3e430", "kernel/src/arch/aarch64/mm/asid.rs",
     "The aarch64 ASID allocator bumped NEXT_ASID monotonically and never consulted the in-use "
     "table: after 254 mm creations (any process-churning workload — LTP forks thousands) "
     "alloc_asid returned None FOREVER and every later mm fell back to ASID 0 via context.rs's "
     "unwrap_or(0) — functionally correct (every leaf write invalidates all-ASID, every switch "
     "full-flushes) but every user access TLB-misses for the rest of the boot. The riscv64 twin "
     "scans a reuse bitmap and only exhausts under genuine concurrency. Twin divergence fixed: "
     "the twin's bitmap allocator ported over the 256-entry pool (TCR_EL1.AS = 0); reuse is safe "
     "because stale entries die at every all-ASID leaf invalidation and every full-flush switch."),
    ("cfc04591", "kernel/src/arch/aarch64/mm/exception.rs",
     "aarch64 do_page_fault ran handle_mm_fault unconditionally — the in_interrupt guard both "
     "twins carry was missing, so a fault taken from an IRQ handler would run the sleeping "
     "mm fault path (allocator + PTE section) from interrupt context. Ported the x86_64 guard "
     "form: in_interrupt() routes to no_context, which still unwinds a uaccess copy through "
     "the exception table before giving up (the riscv64 arm additionally heals preempt-count "
     "underflow — no aarch64 counterpart engine, not ported)."),
    ("6e808eea", "kernel/src/arch/aarch64/boot/boot.S",
     "The 'Save the DTB pointer' step stored x0 into dtb_pointer, but x0 held the "
     "aarch64_vector_table address from the VBAR_EL1 setup immediately above — it clobbered "
     "the DTB pointer published at .Lat_el1 entry. Latent only: nothing reads dtb_pointer "
     "between the clobber and the post-MMU re-publish that repaired it, so the boot worked; "
     "the stray store contradicted its own comment and would feed the vector table to the "
     "first pre-MMU-republish reader if one ever appeared. Store removed."),
    ("00338fd2", "kernel/src/arch/aarch64/trap.rs",
     "dfsc_class rows contradicted the ARM DDI 0487 DFSC encoding (the rows Linux arm64 names "
     "FSC_FAULT/FSC_ACCESS/FSC_PERM): access-flag faults are 0x08-0x0B, permission faults "
     "0x0C-0x0F — the draft read AF as 0x09-0x0C (misclassifying permission level 0) and "
     "permission as 0x0D-0x0F|0x13 (0x13 is not a permission row; AF level 0 was missed). "
     "Dead diagnostics today (zero consumers in-tree); fixed so the first consumer inherits "
     "the right interface."),
    ("eaefd06f", "kernel/src/drivers/pci/mod.rs",
     "04e23b45 (aarch64 campaign) shifted the modern virtio PCI device IDs to net=0x1040/blk="
     "0x1041, reading the reserved base 0x1040 as the net id. Modern IDs are 0x1040 + virtio "
     "device id (net=1 -> 0x1041, blk=2 -> 0x1042): with the shifted table a virtio-blk-pci,"
     "disable-legacy=on device (1af4:1042, QEMU-verified via info pci) matched NOTHING — the "
     "riscv64 suite/probe boots (which attach the rootfs exactly that way) enumerated zero block "
     "devices and died at 'init: no init program loadable'. A modern-only net device (0x1041) "
     "would have bound the BLK probe. The commit's aarch64 motivation was a misdiagnosis: the "
     "GPEX boot used transitional devices (1af4:1001) and was actually fixed by the BAR "
     "self-assignment commit 096c4eca. Restored to spec IDs; the suite itself (test/run.sh "
     "disable-legacy=on) is now the regression guard. How the merge gate went green with the "
     "broken table is a stale-binary question for the coordinator."),
    ("76b92e71", "kernel/src/fs/elf.rs",
     "ElfLoader::validate's machine check was cfg-gated to aarch64/riscv64 (written before the "
     "x86_64 port): on x86_64 builds a foreign-architecture ELF passed validation, its PT_LOAD "
     "segments were mapped, and execve jumped into garbage instead of returning ENOEXEC. The "
     "gate is validate() alone on the exec path (the PT_INTERP image is never machine-checked). "
     "cfg dropped — check_machine has a per-feature arm on every platform."),
    ("f73a13d8", "kernel/src/fs/ext4/file.rs, kernel/src/fs/tmpfs.rs, test/inoaccess.c",
     "IN_ACCESS double-fire: the ext4 read_at op (both arms), the ext4 ops.read O_DIRECT arm, "
     "and the tmpfs read_at op self-notified while running behind File::read/File::read_at whose "
     "wrappers already notify — one watched pread(2) queued the event twice. The inotify "
     "last-event merge (fsnotify_add_event coalescing) masked the visible count, so no test "
     "failed; the duplicate surfaced only when another watcher event interleaved between the two "
     "fires. Op-level self-notify now lives only in the ext4 read(2) fast path that genuinely "
     "bypasses the wrappers; test/inoaccess-probe.sh pins one event per read/pread/O_DIRECT "
     "pread."),

    ("b70a5deb", "kernel/src/fs/ext4/extent.rs",
     "Three defects in the extent engine. (1) ext4_ext_insert_written's external-leaf merge had "
     "NEITHER the unwritten-state check NOR the u15 headroom check append_node has: a written "
     "insert landing right after a trailing UNWRITTEN extent (physically contiguous) cleared bit "
     "15 of ee_len, marking the whole never-materialized fallocate preallocation WRITTEN — reads "
     "then served disk garbage instead of zeros (Linux ext4_can_extents_be_merged rejects mixed "
     "states); the same site's '(len + len) & 0x7FFF' silently TRUNCATED an overflowing length "
     "instead of refusing the merge. (2) insert_written_inline's 'ee_len += len' without the "
     "headroom check wraps a max-length (0x7FFF) written extent into a ZERO-length unwritten "
     "one. (3) find_covering_leaf_blk indexed idxs[0] of an EMPTY index root (eh_entries=0, "
     "depth>=1, corrupt/stale tree) — kernel panic; now Ok(0) (hole)."),
    ("070770d6", "kernel/src/fs/ext4/file.rs, kernel/src/fs/ext4/mod.rs, kernel/src/fs/ext4/allocator.rs",
     "Three line-review fixes in the read/fill region. (1) ext4_file_read_cached_dst's sync "
     "fallback folded get_data_block's Err into the hole arm via unwrap_or(0): a failing "
     "extent/bitmap read served ZEROES for real data — Err is now EIO (Linux propagates "
     "ext4_map_blocks errors). (2) Ext4FileSystem::read_inode addressed the inode table with "
     "bg_inode_table_lo only — every inode of a >2^32-block (64BIT) filesystem resolved to the "
     "wrong disk location; now lo|hi<<32 like the four inode.rs twins. (3) allocator.rs's six "
     "bitmap-addressing sites (bg_block_bitmap_lo x4, bg_inode_bitmap_lo x2) same class, now "
     "lo|hi<<32 (the mount parse already populates the hi halves). Note: no write path "
     "recomputes metadata_csum checksums — fenced, not a live defect: KNOWN_RO_COMPAT refuses "
     "metadata_csum rw mounts; the OH policy path mounts such fs read-only."),
    ("92377595", "kernel/src/arch/riscv64/mm/page_fault.rs",
     "R-SC-RV64: the riscv64 file-fault path re-derived (vma_start, file, effective_size) in FOUR "
     "separately locked lookups (SIGBUS gate, shared-alias resolution, private cache pin, fill arm); "
     "the shared-alias site took TWO back-to-back vma_read() acquisitions (file from one, vma_start "
     "from the other). A CLONE_VM sibling mutating the VMA tree between the lookups could pair one "
     "file with another VMA's start: wrong-offset fill, or a page-cache pin taken under ANOTHER "
     "file's (fs_id, ino, index) key. Fixed with a single entry-time snapshot (x86_64 twin parity) "
     "plus the devfs live-size consult (b74) the twin has had since its session-14; 3 VMA finds and "
     "up to 4 pin-table lookups per file fault become 1 each."),
    ("6c56c0b6", "kernel/src/syscall/process.rs",
     "The shebang optional argument was truncated at the next whitespace after the "
     "first word; the reference kernel (binfmt_script) passes the WHOLE rest of the "
     "interpreter line as one argument, so '#!/usr/bin/env -S python3 -u' lost "
     "'python3 -u'. A 255-byte interpreter line without a newline was also rejected "
     "instead of parsed. Fixed to binfmt_script parity with 14 unit rows."),
    ("2fbb5a64", "kernel/src/syscall/process.rs, kernel/src/process/exec.rs, kernel/src/fs/elf.rs",
     "execve of a directory returned ENOENT (failed read) instead of EACCES (may_open "
     "S_ISDIR); the PT_INTERP interpreter was opened without the MAY_EXEC gate; and "
     "the exec ladder walked the exe path four times plus double-zeroed every freshly "
     "mapped page (the arch allocators already zero before mapping). Fixed with "
     "resolve_exec_file (one walk feeds gate/stat/read/vm_file pin) and removal of "
     "the redundant zero passes."),
    ("ddfa8abc", "kernel/src/interrupt/softirq.rs",
     "The 2ms softirq budget was checked once per batch of pending vectors, so a batch of "
     "slow handlers could overrun it by the time of every vector left in the batch. the reference kernel "
     "checks the budget before every vector and leaves the unreached ones pending."),
    ("327b4a52", "kernel/src/fs/inode.rs, kernel/src/fs/file.rs",
     "The generic O_APPEND write read the file size, released the lock and only then took "
     "the data lock to write — two appenders on different open descriptions could pick the "
     "same offset, and the second write overwrote the first. the reference kernel takes the append "
     "position under the inode lock."),
    ("ff6fc39b", "kernel/src/syscall/memory.rs",
     "msync had no 'addr + len' wrap check (the reference kernel's mm/msync.c has one). A near-max length "
     "wrapped the page-aligned length to 0, so msync reported success without syncing "
     "anything; the reference kernel returns EINVAL."),
    ("879328f3", "kernel/src/fs/memfd.rs, kernel/src/syscall/process.rs",
     "memfd_create accepted a NULL name (the reference reads the name with strnlen_user, which faults on a NULL pointer and yields EFAULT), accepted MFD_EXEC together with MFD_NOEXEC_SEAL (the reference rejects the combination with EINVAL - the two flags carry opposite meaning), and allowed a name of up to NAME_MAX bytes where the reference's limit is MFD_NAME_MAX_LEN (NAME_MAX minus the length of the 'memfd:' prefix it prepends)."),
    ("e32cdfef", "kernel/src/mm/mm_struct.rs, kernel/src/arch/*/mm/mm_ops.rs, kernel/src/syscall/memory.rs, kernel/src/process/exec.rs, kernel/src/ipc/binder.rs",
     "mm->total_vm only ever grew: the VMA removal paths (munmap, the MAP_FIXED replacement) never released the pages that left the mapping, and the VMA-installing paths that do not go through mmap (the brk heap, the exec image, the forked child's copy, the binder zone) never charged theirs. The reference kernel keeps the counter equal to the live mapping size - every installed VMA is charged and every removed one gives its pages back - and checks RLIMIT_AS against that live value. The drift made /proc/<pid>/status VmSize and /proc/<pid>/stat vsize describe every mapping the process ever made, and refused mappings that fit once the counter had grown past a finite RLIMIT_AS."),
    ("b433fb13", "kernel/src/sched/fair.rs",
     "The CFS pick lifts the entries a CPU cannot run (affinity, cgroup throttle, ownership) out of "
     "the timeline and re-inserts them after the pick, but min_vruntime was advanced while they "
     "were out: the leftmost looked further right than it is. min_vruntime only ever grows, so the "
     "drift was permanent, and it is the reference point new tasks and waking sleepers are placed "
     "against - a drifted value silently erased their sleep credit. The reference kernel computes "
     "min_vruntime from the leftmost entry of the complete tree (kernel/sched/fair.c: "
     "update_min_vruntime)."),
    ("b433fb13", "kernel/src/syscall/memory.rs",
     "mmap accepted a file offset that is not a multiple of the page size. The reference kernel's "
     "mmap wrapper rejects it with EINVAL before anything else, and the check is unconditional - it "
     "applies to anonymous mappings too, where the offset is ignored. Without it a misaligned offset "
     "silently shifted the mapping's file extent: the VMA stored the raw offset and the fault path "
     "then looked up a page-cache entry at a non-page-aligned offset."),
    ("e47b155a", "kernel/src/ipc/posix_mq.rs",
     "mq_timedreceive/mq_timedsend take an ABSOLUTE CLOCK_REALTIME deadline (abs_timeout), "
     "but Rux read the value as RELATIVE — a caller passing an absolute deadline waited "
     "twice as long, and an already-expired deadline blocked for the full timeout instead "
     "of returning ETIMEDOUT at once."),
    ("623c4736", "kernel/src/sched/sched.rs, kernel/src/sync/lockkey.rs",
     "The 2b092c49 GRQ typed-lock-key swap (KeyedRawSpinlock<GrqKey> wrapping "
     "RawSpinlock as a pure delegate) is CONVICTED as the trigger of the d2a58ccb "
     "stochastic panic family by a 13-boot paired adjudication: identical tree "
     "differing only by the 3-line swap. WITH the swap: 6/6 x86 exitwalk boots "
     "panic in the CFS timeline btree walk (navigate.rs:552 next_unchecked "
     "unwrap; stack: pick_next_cpu -> Iter::next -> pick_next_task), and the "
     "riscv64 orphan-storm churn panics on a torn-context wild epc immediately "
     "after an R74-DOUBLE-SEED print (two harts executing one task). WITHOUT it "
     "(revert arm 3/3 + all pre-swap kernels 4/4): every boot runs the full "
     "churn clean on both arches. fdtable lever B (ac53dd6e), NJ deferred "
     "writes (306d7cf6), the exec one-path (2fbb5a64), mkdir-flush deletion "
     "(e3c1ec35) and the code-review round's fixes (b433fb13 family) are all "
     "EXONERATED - they are in the clean pre-swap midpoints. MECHANISM "
     "CORRECTED same day by the refactor lane (75dbf637): the initial "
     "'latent GRQ race' theory was WRONG - KeyedRawSpinlock::lock() returns "
     "a guard while RawSpinlock::lock() returns (), and the swap kept the "
     "bare 'self.lock.lock();' statement, so the guard was DISCARDED at "
     "end-of-statement and its Drop released the GRQ lock immediately: "
     "every critical section ran unexcluded. The R74-DOUBLE-SEED shape was "
     "the expected downstream of a lock that never held; there is NO latent "
     "GRQ pick/ownership race and the protocol is exonerated. Guards are "
     "#[must_use] now (the discard class is dead) and protocol-wrapping "
     "subsystems use lock_claim(); the NRUN-DOUBLE-DEC/LWEIGHT/garbage "
     "tripwires stay as plain regression guards. The withdrawal itself "
     "stands until the corrected re-adoption passes its own paired "
     "adjudication."),
    ("4e1f6110", "kernel/src/syscall/misc.rs",
     "sys_timerfd_settime broke the reference contract (kernel/fs/timerfd.c) four ways: "
     "the old timer was cancelled BEFORE the itimerspec validation (the reference validates "
     "itimerspec64_valid() first, so an EINVAL call must leave the running timer armed — Rux "
     "silently disarmed it); old_value was copied out BEFORE the reprogramming (the reference "
     "reads the new value first, reprograms, then copies the old snapshot out — a faulting "
     "copy is EFAULT with the NEW timer already programmed); a disarm (it_value == 0) "
     "returned early keeping the OLD interval and the stale expiration count (timerfd_setup() "
     "sets ctx->tintv and ctx->ticks = 0 in both the arming and disarming paths); and the "
     "arming call ran even for total_nsec == 0, arming a dead timer slot."),
    ("a6b01fd9", "kernel/src/fs/ext4/csum.rs, kernel/src/fs/ext4/namei.rs",
     "cr2 review of the review-sprint delta (82059759..40e925fd). Three metadata_csum "
     "WRITE-path divergences vs e2fsprogs, all e2fsck-proven with a host harness that "
     "compiles the kernel's own csum.rs against real mke2fs images: (1) s_checksum is "
     "the FULL 32-bit crc32c (le32 at 0x3FC) — we sealed only the low u16, leaving "
     "[1022..1024) stale; a u16 read passes on mkfs images, which hid it (e2fsck "
     "compares all 32 bits and refused the primary sb). (2) the dir tail is 12 bytes "
     "(0, rec_len=12, name_len=0xDE00, csum) over a blocksize-12 span with the seed "
     "crc32c(crc32c(seed,ino),generation) — we wrote an 8-byte markerless tail with an "
     "ino-only seed over blocksize-8; dir_block_seal now normalizes the dirent chain "
     "to end at blocksize-12 and the three entry-space sites reserve the tail. (3) "
     "write_group_descriptor serialized the whole in-memory (mount-time-view) "
     "Ext4GroupDesc over the disk descriptor, reverting freshly written bitmap "
     "checksums and counter updates — now a read-modify-write of the free-inode "
     "fields. Bitmap/inode/GD formulas verified correct as-is."),
    ("95094dd2", "kernel/src/dfx/switches.rs, kernel/src/syscall/io.rs, kernel/src/syscall/memory.rs, kernel/src/drivers/drm/mod.rs, kernel/src/signal.rs",
     "Known-gap-4 sweep: the oh-lane hunt instruments (MAP_DUMB + failing-ioctl traces, "
     "drm mmap resolve/placement prints — one unbudgeted — the prime-import "
     "compose-content probe walking the whole GEM per import, and the SIGABRT "
     "first-raise dump) moved behind a new dfx=ohhunt switch, off by default; "
     "standard and perf-gate boots pay nothing."),
]

MARKS = {"done": "✅", "fixed": "🔧", "part": "🔶", "todo": "⬜"}

# The mainline commit the statuses above were recorded against. A file
# that mainline has changed since then keeps its mark but gains a "needs
# re-read" flag: the code I read is no longer the code that is there.
# The mainline commit the reviews were done against (the branch base at
# the time). Everything mainline touched since then needs a re-read.
REVIEW_BASE = "47d96961"


def status_for(rel):
    """The map entry for a file: an exact key wins, else the best glob.

    The exact key always wins so a specific file can be lifted out of a
    directory-wide entry (e.g. kernel/src/sched/idle.rs out of
    kernel/src/sched/*). Among globs the one with the longest literal
    prefix wins, so a narrow pattern beats a broad one.
    """
    if rel in STATUS:
        return STATUS[rel]
    best = None
    best_prefix = -1
    for key, value in STATUS.items():
        if "*" not in key:
            continue
        if fnmatch.fnmatchcase(rel, key):
            prefix = len(key.split("*")[0])
            if prefix > best_prefix:
                best, best_prefix = value, prefix
    return best if best is not None else ("todo", "")


def changed_since_base():
    """The kernel/src files mainline touched after REVIEW_BASE."""
    import subprocess
    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", REVIEW_BASE + "..main", "--", "kernel/src"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return set()
    return {line for line in out.split() if line}


def collect():
    """Every source file under kernel/src with its line count."""
    out = []
    for dirpath, _dirs, files in os.walk(SRC):
        for name in sorted(files):
            if not (name.endswith(".rs") or name.endswith(".S")):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, ROOT)
            with open(path, "rb") as handle:
                lines = sum(1 for _ in handle)
            out.append((rel, lines))
    return sorted(out)


def main():
    check = "--check" in sys.argv
    files = collect()
    by_path = {rel: lines for rel, lines in files}

    # Drift detection: a file in the tree that the map does not know.
    unknown = [rel for rel, _ in files if rel not in STATUS and status_for(rel)[0] == "todo"
               and not any("*" in k and fnmatch.fnmatchcase(rel, k) for k in STATUS)]
    # A map entry that no longer exists.
    stale = [rel for rel in STATUS if "*" not in rel and rel not in by_path]

    if check:
        if unknown:
            print("map is missing entries for:")
            for rel in unknown:
                print("  " + rel)
        if stale:
            print("map has entries for files that no longer exist:")
            for rel in stale:
                print("  " + rel)
        # Hot-path print ratchet (print-audit lane, 2026-11): one more
        # un-annotated pr_err/pr_warn/pr_info in a hot-path file fails
        # the check — see scripts/hotpath_print_check.py.
        import hotpath_print_check
        save_argv = sys.argv
        sys.argv = [sys.argv[0]]
        try:
            hot_rc = hotpath_print_check.main()
        finally:
            sys.argv = save_argv
        return 1 if (unknown or stale or hot_rc) else 0

    # Per-directory tables.
    groups = {}
    for rel, lines in files:
        d = os.path.dirname(rel)
        groups.setdefault(d, []).append((rel, lines))

    changed = changed_since_base()
    counts = {"done": 0, "part": 0, "todo": 0, "fixed": 0}
    stale = 0
    for rel, _lines in files:
        status = status_for(rel)[0]
        counts[status] = counts.get(status, 0) + 1
        if status in ("done", "fixed", "part") and rel in changed:
            stale += 1

    total_lines = sum(lines for _rel, lines in files)
    reviewed_lines = 0
    for rel, lines in files:
        if status_for(rel)[0] in ("done", "fixed"):
            reviewed_lines += lines

    out = []
    out.append("# Kernel Review Coverage Map")
    out.append("")
    out.append("Bookkeeping for the line-by-line kernel review: every source file")
    out.append("under `kernel/src` has a status, so nothing is silently skipped.")
    out.append("")
    out.append("Generated by `scripts/review_coverage.py` — edit the STATUS table")
    out.append("there, not this file. `scripts/review_coverage.py --check` fails")
    out.append("when the tree gains a file the map does not know about.")
    out.append("")
    out.append("## Legend")
    out.append("")
    out.append("| mark | meaning |")
    out.append("|---|---|")
    out.append("| ✅ done | read end to end, no defect found |")
    out.append("| 🔧 fixed | read end to end, a defect was found and fixed (see below) |")
    out.append("| 🔶 part | read in part; the note says what was covered |")
    out.append("| ⬜ todo | not read yet |")
    out.append("| 🔁 | reviewed, but mainline changed the file since the review base — re-read needed |")
    out.append("")
    out.append("## Progress")
    out.append("")
    out.append("| metric | value |")
    out.append("|---|---|")
    out.append(f"| source files in kernel/src | {len(files)} |")
    out.append(f"| source lines in kernel/src | {total_lines:,} |")
    out.append(f"| files fully reviewed | {counts.get('done', 0) + counts.get('fixed', 0)} |")
    out.append(f"| files partially reviewed | {counts.get('part', 0)} |")
    out.append(f"| files not yet reviewed | {counts.get('todo', 0)} |")
    out.append(f"| lines in fully reviewed files | {reviewed_lines:,} |")
    out.append(f"| reviewed files mainline changed since `{REVIEW_BASE}` (need re-read) | {stale} |")
    out.append("")
    out.append("## Defects found and fixed")
    out.append("")
    out.append("| commit | file | defect |")
    out.append("|---|---|---|")
    for commit, path, text in BUGS:
        out.append(f"| `{commit}` | `{path}` | {text} |")
    out.append("")
    out.append("## Per-file status")
    out.append("")
    for d in sorted(groups):
        out.append(f"### {d} ({sum(l for _r, l in groups[d])} lines)")
        out.append("")
        out.append("| file | lines | status | note |")
        out.append("|---|---:|---|---|")
        for rel, lines in groups[d]:
            status, note = status_for(rel)
            mark = MARKS.get(status, "⬜")
            if status in ("done", "fixed", "part") and rel in changed:
                mark += "🔁"
                note = (note + "; " if note else "") + "主线已改动，需重读"
            out.append(f"| `{os.path.basename(rel)}` | {lines} | {mark} | {note} |")
        out.append("")

    with open(OUT, "w") as handle:
        handle.write("\n".join(out))
    print("wrote %s (%d files tracked)" % (OUT, len(files)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
