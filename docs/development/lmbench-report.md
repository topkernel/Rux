# lmbench baseline — Rux x86_64 vs the reference kernel

**Status:** the optimization baseline for Rux. Every performance change
from here on is judged against this table, one case at a time.

- **Rux column:** the Rux x86_64 kernel under QEMU **KVM**
  (`-accel kvm -cpu host`), booting the shared x86_64 Ubuntu rootfs —
  the same image the LTP x86 leg uses — with the lmbench binaries
  injected under /test.
- **Reference column:** the **same static binaries** executed natively
  on the host (the reference kernel), on the same CPU, in the same
  session. Same code, same machine, one variable: the kernel.
- **Rig:** 9950X host, QEMU 10.2.1, `-smp 2 -m 2G`, Rux built with the
  strong profile (LTO, codegen-units=1, stripped).
- **Method:** one benchmark per boot (the serial console interleaves
  concurrent kernel prints with a long sweep, so a single-case boot is
  the only clean collection); the case travels on the kernel cmdline
  (`lmbench=<bench>,<args>` — the cmdline parser does not preserve
  quotes), each case has a 20 s in-guest timeout, and the runner is
  PID 1 (`init=/test/lmbench_run`), the same shape as the LTP lane's
  `test/ltpscan` runner.

## Reproduce

```bash
userspace/lmbench/extract-lmbench.sh     # unpack the tarball (sources are not in git)
userspace/lmbench/build-lmbench.sh       # 24 static musl benchmarks
userspace/lmbench/build-rootfs.sh work/lmbench-x86.img   # inject into a copy of the Ubuntu rootfs
userspace/lmbench/run-rux.sh             # the Rux column  -> run/lmbench-rux.txt
userspace/lmbench/run-linux.sh           # the reference column -> run/lmbench-linux.txt
python3 userspace/lmbench/make-report.py run/lmbench-rux.txt run/lmbench-linux.txt
```

## Results

<!--RESULTS-->

## Reading the table

- **Ratio** = Rux value / reference value. Above 1.00 means Rux is
  slower on that case.
- **TIMEOUT** = the benchmark did not finish inside the in-guest
  budget. On Rux this is a **functional gap** (the case needs a
  syscall or device semantic the kernel does not implement yet), not a
  slow number — those are the first optimization targets, ahead of any
  micro-optimization.
- **n/a** = the case produced no parseable measurement on that side
  (both sides use the same binary and the same arguments, so a missing
  line usually means the case needs an environment the rig does not
  provide, e.g. a fifo reader or a multi-process handshake).

## Optimization queue

Ordered by expected payoff (the queue is the working list for the
per-case optimization work):

1. **Cases that time out** — each is a missing semantic, not a slow
   path. Fixing one turns a case into a measurable number.
2. **lat_syscall family** — the cheapest, most direct measure of the
   syscall entry/exit path; the Rux column is 1.5–3.3x the reference
   here, which is the per-call overhead the syscall dispatcher carries.
3. **lat_proc fork/exec** — the process lifecycle path (page tables,
   task teardown, the mm drop path).
4. **lat_mem_rd / bw_mem** — the memory path (page allocation, the
   per-CPU pagesets, page-table walks).
5. **lat_ctx / lat_pipe / lat_unix** — the scheduler and the wake
   paths.

## Notes and caveats

- The reference column runs **natively**, not under KVM; the guest
  adds one virtualization layer to the Rux column. That layer is not
  free (the guest pays for it on every syscall and every fault), so
  the Rux numbers include a cost the reference does not. The ratios
  are therefore conservative in Rux's favour.
- The first sweep of each case is a **cold** boot: caches are empty,
  the page cache is cold, and the first fault of each kind pays the
  demand-fault path in full. The reference column runs warm after the
  first case. Read the numbers as "cold-ish guest vs warm host" and
  re-run a case twice before drawing conclusions.
- lmbench's `lat_syscall` stat/fstat/open cases stat
  `/usr/include/linux/types.h`; the Ubuntu rootfs ships that header
  under the multiarch subdirectory, so `build-rootfs.sh` places a
  placeholder at the plain path. Without it those three cases fail
  with "No such file or directory" and report nothing.
