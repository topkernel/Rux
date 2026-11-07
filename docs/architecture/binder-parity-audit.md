# Binder 1:1 parity audit — reference 6.6 vs Rux (2026-11-07)

Mandate: the binder layer has produced the campaign's recurring bugs
(the proc-todo steal 68th, the thread-selection divergence 65th, the
release_task spin's binder-facing half 69th). This audit enumerates the
REFERENCE's semantic functions against ours, function by function, to
close the class instead of the instances.

Sizes: reference binder.c 6955 lines / 106 functions; ours 11696 lines
/ 88 functions (instruments included in ours).

## A. Reference functions with NO counterpart in ours (the gap list)

| Reference fn | Reference semantics (line) | Ours | Risk |
|---|---|---|---|
| binder_can_update_transaction (2734) | TF_UPDATE_TXN: a new oneway txn may REPLACE a matching pending one (same proc/code/flags/pid/node ptr+cookie) instead of queueing | NOT IMPLEMENTED (no TF_UPDATE_TXN anywhere) | oneway storms queue where the reference collapses them — buffer pressure + ordering divergence |
| binder_deferred_fd_close (1962) | fd closed via task_work AFTER ioctl return (TWA_RESUME) | fdreap (our own shape, grace-based) | semantic overlap but different lifetime contract — audit required |
| binder_flush (ioctl) | flushes pending async descriptors for the fd | ? | API completeness (BC_REGISTER_LOOPER path) |
| binder_fixup_parent / binder_validate_fixup | BINDER_TYPE_PTR parent fixup chain validation | partial (fail_txn_after_fixups) | SG buffer corruption class |
| binder_wakeup_poll_threads_ilocked | wakes POLL threads on proc work | binder_poll_wait exists | verify wake fan-out parity |
| binder_wait_for_work (list_add head LIFO) | DONE 65th (park_seq) | ✓ fixed | — |
| binder_available_for_proc_work_ilocked (3 sites) | DONE 68th (looper gate) | ✓ fixed | — |

## B. Our functions with NO reference counterpart (invention risk)

bwr_park_audit, ams46_* (6 fns), clear_live_bwr, latch_park_walk_*,
sc161 child routing tiers, TXN_PQUEUED drain-arm shape — instruments
are dfx-gated (acceptable); the sc161 tiers and the drain-arm are
BEHAVIORAL inventions: each must be re-derived from the reference
text or documented as a divergence with evidence (design-divergences
register rule).

## C. Fix protocol for this class

Every future binder change cites the reference function+line it
mirrors; every gap-list row lands as its own reference-parity commit
with an LTP/instrument check. The class closes when this table is
empty.
