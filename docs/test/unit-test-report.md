# Unit test report — in-kernel guest suite

**Rig**: `make test` boots the kernel under QEMU with the minimal
rootfs; `kernel/src/tests/` runs as the guest harness (the kernel crate
cannot host `#[cfg(test)]` — it collides with the std panic handler, so
every kernel assertion runs in-guest).

## Standing (2026-11-01, main 09d2969c)

| Architecture | Result | Suite size |
|---|---|---|
| riscv64 (TCG, 4 CPU) | **ALL PASSED** | 1267 |
| x86_64 (KVM) | **ALL PASSED** | 1263 |
| aarch64 (TCG, virt) | all green (last recorded count 1119; grown since) | — |

Known standing red: none. The last long-standing row
(`sys_prlimit64 kernel new_rlim returns -EFAULT`) went green when the
bootstrap identity map was dropped from the kernel CR3 (x86_64) — bad
user pointers now fault through the exception table instead of reading
physical memory through the PML4[0] window.

## Recent coverage additions (2026-11-01)

- `tests/tcp_semantics.rs` (44 rows): all seven TCP fix families from
  the RFC-semantics sweep (RST validation incl. byte order, FIN gating,
  single-shot fast retransmit, Karn + partial-ACK trim, SND.WL1/WL2
  window gate, OOO caps, close-drains-before-FIN, handshake-only
  option adoption, header alignment).
- `tests/mm_pcp.rs` (10 rows): per-CPU pageset conservation
  (INV-PCP-2), cache bound, no-leak realloc.
- semid64_ds field-offset pins (the 88-byte asm-generic contract);
  mprotect hole-span ENOMEM rows; 8 tautological signal rows replaced
  with exact errno contracts.

## Where the counts come from

The suite prints `Total / Passed / Failed` on the serial console at the
end of a run; the CI smoke gate (`.github/workflows/smoke.yml`) runs
the riscv64 leg on every push to main, so a silent regression shows up
as a red check within minutes of landing.
