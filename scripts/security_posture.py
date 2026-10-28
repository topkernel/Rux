#!/usr/bin/env python3
"""Generate the kernel security posture ledger.

The ledger is the living artifact of the security review track: every
isolation/privilege surface carries a row with current state, reference
behavior, gap class, and the test that guards regressions.

Usage: scripts/security_posture.py [--check]

  --check  exit non-zero if the generated doc is stale relative to ROWS
           (drift guard, same pattern as review_coverage.py --check)

Gap classes:
  ok        matches the reference kernel (or our documented divergence)
  quick     small contained fix, implementable in one session
  project   multi-session engineering
  accepted  deliberate divergence, registered in design-divergences.md
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "development", "security-posture.md")

# ---------------------------------------------------------------------------
# The ledger rows. Each row: (area, title, state, reference, gap_class, guard)
# ---------------------------------------------------------------------------

ROWS = [
    # --- user/kernel isolation: the syscall boundary ---
    ("user/kernel isolation",
     "syscall entry validation",
     "Every syscall enters one dispatch (syscall/dispatch.rs, 1214 lines); "
     "numbers are per-arch (riscv64 native; x86_64 translated in "
     "syscall/x86_compat.rs with explicit no-fallthrough for unknown x86 "
     "numbers — several x86 numbers collide with DIFFERENT generic syscalls).",
     "Reference: do_syscall_64 -> syscall table indexed by nr; unknown nr "
     "returns -ENOSYS.",
     "ok",
     "LTP syscall family (unknown-nr rows); dispatch unit rows"),

    ("user/kernel isolation",
     "uaccess: bounds-checked copies",
     "Every accessor (get_user/put_user/copy_from_user/copy_to_user/"
     "clear_user/strncpy_from_user/strnlen_user) routes through arch "
     "extable copy cores: riscv64 SUM-bracketed asm with per-byte and "
     "word paths (arch/riscv64/uaccess.S), x86_64 restartable rep-movsb "
     "with __ex_table entries (arch/x86_64/uaccess.rs); access_ok(addr, n) "
     "checks [addr, addr+n) <= USER_END on every accessor; a fault returns "
     "short/EFAULT, never panics (LTP x86-r2 N1 evidence: 37 EFAULT-family "
     "tests pass through the fixup). ~150 call sites across syscall/*.",
     "Reference: same shape (extable + access_ok).",
     "ok",
     "LTP EFAULT family; copy stratagem unit rows"),

    ("user/kernel isolation",
     "uaccess TOCTOU on user pointers",
     "No wholesale audit exists. Known-good pattern: args copied ONCE into "
     "kernel memory at the ladder head (exec argv/envp via get_user loop "
     "into String; sock_fprog copied wholesale). Known-risk pattern to "
     "audit: syscalls that copy a struct containing embedded user POINTERS "
     "and later deref those pointers with the user's remaining lifetime "
     "(iovec chains, msghdr, sigaction handlers, clone args, bpf_attr — "
     "the latter ENOSYS). No kernel thread ever runs with a user mm that "
     "can be concurrently mutated by a sibling during the copy window "
     "except CLONE_VM threads sharing the mm.",
     "Reference: copy_struct_from_user once + user_access_begin/end "
     "regions; unsafe_user_pointer pointers never live across copies.",
     "project",
     "New: LTP io_uring/msg fault-injection rows + a grep audit pass in "
     "this lane's review coverage (per-file marks)"),

    ("user/kernel isolation",
     "SMAP/SMEP (x86_64)",
     "NOT enabled. uaccess.rs header states it; CR0.WP IS enabled from "
     "earliest boot (F1b fix, boot.rs:95) so supervisor writes to "
     "read-only PTEs fault — half the protection exists. SMEP equivalent "
     "on riscv64 is architectural (U-mode cannot execute supervisor "
     "pages; sstatus.SUM=0 keeps user pages unreadable in S-mode, SUM "
     "convergence review SEC).",
     "Reference: CR4.SMAP+SMEP set at boot; stac/clac around uaccess.",
     "project",
     "Boot tripwire + LTP full-family sweep (any missed raw user deref "
     "panics at runtime)"),

    ("user/kernel isolation",
     "riscv64 SUM discipline",
     "sstatus.SUM deliberately kept 0 (exec entry comment, review SEC): "
     "kernel reaches user memory only via uaccess exception paths. "
     "sret to U-mode with SUM=0 is the ABI.",
     "Reference: equivalent (no SUM on most ports; SMAP-like via PMP).",
     "ok",
     "exec entry unit rows; any SUM=1 boot print would be a regression"),

    # --- privilege model ---
    ("privilege model",
     "capability set breadth (41 caps)",
     "All 41 CAP_* constants present with exact Linux UAPI numbering "
     "(security/capability.rs); Cap(u64) with valid-mask enforcement; "
     "capget/capset ABI via lo/hi halves. Enforcement coverage is "
     "PARTIAL: checked at setuid/setgid exec transition (cap drop rules), "
     "kill permission, seccomp attach, mount/umount, chroot, mknod, "
     "syslog, reboot, clock_set, iopl/ioperm (x86), IPC owner override. "
     "NOT enforced (no-op or absent): CAP_NET_ADMIN/NET_RAW class gates "
     "(raw socket create unprivileged — audit pending), CAP_IPC_LOCK "
     "(mlock unbounded), CAP_SYS_RESOURCE (rlimits bypass unprivileged?).",
     "Reference: every capable() site enumerated in "
     "Documentation/security; ~120 check sites.",
     "project",
     "LTP capability family (capset01-08, execve cap drops); new: "
     "per-syscall capability audit pass in this lane"),

    ("privilege model",
     "setuid/setgid exec transitions",
     "Implemented with rollback-on-failed-load (cred snapshot restored); "
     "AT_SECURE=1 exactly when ids change (root AT_SECURE false-positive "
     "fixed in session 1); ambient caps cleared on exec; x-bit required "
     "for setuid to apply; kernel-side LD_* scrub (D-9).",
     "Reference: bprm_fill_uid + cap_bset intersection rules.",
     "ok",
     "LTP execve/setuid family; shebang + secure-exec unit rows"),

    ("privilege model",
     "no_new_privs (PR_SET_NO_NEW_PRIVS)",
     "ABSENT. prctl handles options 1-8, 15, 21, 22 (+ others); option "
     "39/40 missing. seccomp filter attach therefore requires CAP_SYS_ADMIN "
     "(recorded in sys_seccomp) — unprivileged real-world filter users "
     "(systemd, OH appspawn, gVisor-class sandboxes) cannot attach.",
     "Reference: prctl(39) sets per-thread flag; inherited across fork "
     "and exec; exec refuses privilege GAIN when set (EPERM); makes "
     "unprivileged seccomp attach legal.",
     "quick",
     "New unit rows + LTP prctl02/seccomp01 shape"),

    ("privilege model",
     "seccomp (engine)",
     "REAL and substantially complete (U1c landed, coordinator's 'likely "
     "no engine' was stale): classic-BPF subset interpreter "
     "(LD|W|ABS/IMM, JMP|JEQ/JGT/JGE/JSET|K, RET|K; max 4096 insns; "
     "oob-load and fall-off-end kill), gate called from BOTH trap paths "
     "BEFORE dispatch, STRICT and FILTER modes, kill actions queue "
     "SIGKILL, fork inheritance (fork.rs seccomp_dup).",
     "Reference: full classic BPF + cBPF-to-eBPF, filter STACKING, "
     "TSYNC, RET_TRACE, USER_NOTIF, SECCOMP_RET_KILL_PROCESS vs "
     "KILL_THREAD distinction, filter-LOG.",
     "quick",  # for the two bugs below; stacking etc = accepted divergence
     "LTP seccomp01/02/prctl02; new STRICT arch unit rows"),

    ("privilege model",
     "seccomp BUG: STRICT_ALLOWED is riscv64-only on BOTH arches",
     "syscall/process.rs:4528 const STRICT_ALLOWED = [63 read, 64 write, "
     "93 exit, 94 exit_group, 139 rt_sigreturn] with no arch gate; the "
     "x86_64 trap gate passes the RAW x86_64 number (regs.rax, BEFORE "
     "x86_compat translation). Consequence on x86_64: native read(0)/"
     "write(1)/exit(60)/exit_group(231)/rt_sigreturn(15) are all KILLED; "
     "uname(63) is ALLOWED as if it were read. A task entering STRICT on "
     "x86_64 dies on its first read. FILTER mode is incidentally CORRECT "
     "on x86_64 (native numbers + AUDIT_ARCH token = 0xC000003E even "
     "though the constant is misnamed AUDIT_ARCH_RISCV64).",
     "Reference: strict mode allows read/write/exit/exit_group/"
     "sigreturn per the RUNNING arch's numbers.",
     "quick",
     "New unit rows (per-arch allowed sets); a STRICT-mode smoke on x86"),

    ("privilege model",
     "seccomp divergence: single filter, no TSYNC/TRACE/NOTIF",
     "One filter per task, new replaces old (documented in code); TSYNC/"
     "LOG/SPEC_ALLOW/TSYNC_ESRCH flag bits accepted-and-ignored (TSYNC "
     "semantics NOT applied despite being in the accept mask — a thread "
     "group calling TSYNC gets the filter only on the caller); KILL_"
     "PROCESS and KILL_THREAD both kill the current task (seccomp_kill "
     "queues SIGKILL on current only).",
     "Reference: filters stack (all evaluated, most restrictive wins); "
     "TSYNC applies to all sharing threads with E2BIG/ESRCH shapes; "
     "KILL_PROCESS kills the thread group.",
     "accepted",
     "documented divergence — register in design-divergences.md when the "
     "stacking work lands"),

    # --- MAC ---
    ("MAC",
     "SELinux enforcement depth",
     "LABEL-CARRYING ONLY, permissive by construction (fs/selinuxfs.rs): "
     "selinuxfs is a minimal libselinux-compat mount (enforce reads 0, "
     "load accepts-and-discards, context node world-rw for samgr's "
     "CheckPerm, static selinux_status page). No policy is parsed, no "
     "AVC decisions are made, no xattr contexts are read from inodes. "
     "The OH image's SELinux userspace boots and gates services through "
     "this compat surface; nothing is enforced.",
     "Reference: policy load, SID table, AVC with permissive/enforce "
     "modes, xattr label fetch on inodes.",
     "project",
     "documented: this IS the registered state; any future policy engine "
     "starts from this ledger row"),

    # --- memory safety at the ABI boundary ---
    ("memory safety",
     "kernel text/rodata W^X",
     "NOT enforced. riscv64: whole kernel image mapped V|R|W|X (both "
     "identity and virtual, mmu_init.rs kernel_flags). x86_64: image "
     "mapped P|RW without NX (executable by absence of NX bit). No "
     "post-boot RO/ROX tightening pass exists. No kernel module loader "
     "(no runtime text allocation). No JIT in-tree today (OH ArkTS "
     "runs in USERSPACE and manages its own W^X via mprotect).",
     "Reference: mark_rodata_ro() at init, NX on data, "
     "CONFIG_STRICT_KERNEL_RWX.",
     "project",
     "New tripwire: page-table walk asserting text pages RO after init "
     "(gated dfx knob), any W+X page print"),

    ("memory safety",
     "user-pointer struct copies with embedded pointers",
     "Covered under the TOCTOU row; highest-risk call sites named there.",
     "Reference: same.", "project", "see TOCTOU row"),

    # --- namespaces ---
    ("namespace/isolation surface",
     "PID namespace",
     "REAL: PidNamespace with CLONE_NEWPID (fork sets ns_local_pid, "
     "ns_pid_for_children for unshare), setns/unshare paths in ns.rs "
     "(1179 lines); /proc wiring partial.",
     "Reference: full pid_ns including reaping, /proc/pid per-ns view.",
     "project",
     "LTP pidns family (runs in fleet)"),

    ("namespace/isolation surface",
     "NET/USER/MOUNT/IPC namespaces",
     "STUBS: task.rs carries ns_net/ns_user Arcs marked '(stub)' in the "
     "comments; CLONE_NEWNET/NEWUSER return shapes unverified — audit "
     "row for this lane.",
     "Reference: full implementations.",
     "project",
     "LTP namespaces family (expected FAIL — record baseline)"),

    # --- information leaks ---
    ("information leaks",
     "/proc/kallsyms addresses",
     "REAL ADDRESSES to every reader (fs/procfs/kallsyms.rs header says "
     "so: 'Linux masks with kptr_restrict — Rux has no such knob'). "
     "Defeats KASLR by reading one file (and KASLR is off anyway).",
     "Reference: kptr_restrict=1 zeroes addresses for unprivileged "
     "readers; kallsyms shows 0000000000000000.",
     "quick",
     "grep test: unprivileged read of /proc/kallsyms yields zeros when "
     "the knob lands"),

    ("information leaks",
     "KASLR",
     "DISABLED (memory_layout.rs KernelMapping.kaslr_offset: '0 if KASLR "
     "disabled'; boot.S loads fixed KERNEL_ENTRY). Field and plumbing "
     "exist; no entropy source is wired at boot.",
     "Reference: CONFIG_RANDOMIZE_BASE, offset from RDRAND/trng.",
     "project",
     "boot print of kaslr_offset twice must differ; PMP/phys window "
     "constraints on riscv64 make this a design task"),

    ("information leaks",
     "kernel pointers in other /proc nodes",
     "NOT audited. /proc/[pid]/stat has kstkeip/kstkesp fields "
     "(pid.rs) — check they are 0; /proc/[pid]/wchan, stack (dfx) "
     "unaudited. DFX files under debugfs are root-gated by DAC only.",
     "Reference: kptr_restrict governs all of these.",
     "quick",
     "audit pass over procfs/pid.rs generators in this lane"),

    # --- attack surface ---
    ("attack surface",
     "syscalls reachable with zero caps",
     "Everything except the capable()-gated set (mount/umount, chroot, "
     "mknod, setuid-family, reboot, swapon, syslog, seccomp-attach, "
     "clock_set). Raw sockets (AF_PACKET if present), bpf(ENOSYS), "
     "kexec(N/A), io_uring(present? io_uring/ dir exists — audit), "
     "userfaultfd audit pending.",
     "Reference: distro lockdown lists; same shape otherwise.",
     "project",
     "the per-syscall capability audit pass (privilege model row)"),

    ("attack surface",
     "debugfs/dfx exposure",
     "debugfs mounted only when requested (dfx= cmdline gates the "
     "instruments themselves; debugfs.rs reviewed by the coverage "
     "lane). DFX switches default OFF. Root-only by DAC.",
     "Reference: debugfs root-only; lockdown limits in lockdown modes.",
     "ok",
     "dfx switch table unit rows"),
]


def render() -> str:
    lines = []
    w = lines.append
    w("# Kernel Security Posture Ledger")
    w("")
    w("Living artifact of the security review track (user directive,")
    w("2026-10-29: security hardening for a monolithic kernel). Every")
    w("isolation/privilege surface carries a row: current state, the")
    w("reference kernel's behavior, a gap class, and the test that guards")
    w("regressions. Generated by `scripts/security_posture.py` — edit the")
    w("ROWS table there, not this file. `--check` fails on drift.")
    w("")
    w("**Framing (user directive):** Rux is a MONOLITHIC kernel — there is")
    w("no microkernel message boundary to hide behind. The isolation story")
    w("IS the syscall boundary (uaccess + entry validation) plus the")
    w("privilege model (capabilities + seccomp + exec transitions). Every")
    w("row below is one of those two things, or feeds them (W^X, KASLR,")
    w("and leak hygiene protect the boundary's enforcement machinery).")
    w("")
    w("## Gap classes")
    w("")
    w("| class | meaning |")
    w("|---|---|")
    w("| ok | matches the reference kernel or a documented, gated divergence |")
    w("| quick | small contained fix (one session) |")
    w("| project | multi-session engineering |")
    w("| accepted | deliberate divergence, registered in design-divergences.md |")
    w("")
    w("## Summary")
    w("")
    counts = {}
    for r in ROWS:
        counts[r[4]] = counts.get(r[4], 0) + 1
    total = len(ROWS)
    order = ["ok", "quick", "project", "accepted"]
    w("| class | rows |")
    w("|---|---|")
    for k in order:
        if k in counts:
            w(f"| {k} | {counts[k]} |")
    w(f"| **total** | **{total}** |")
    w("")
    w("## The ledger")
    w("")
    cur_area = None
    for area, title, state, ref, gap, guard in ROWS:
        if area != cur_area:
            w(f"### {area}")
            w("")
            cur_area = area
        w(f"#### {title} — {gap.upper()}")
        w("")
        w(f"- **State:** {state}")
        w(f"- **Reference:** {ref}")
        w(f"- **Gap class:** {gap}")
        w(f"- **Regression guard:** {guard}")
        w("")
    w("## Session history")
    w("")
    w("| session | changes |")
    w("|---|---|")
    w("| 1 (2026-10-29) | Ledger created. First hardening item: seccomp "
      "STRICT-mode x86_64 number bug + PR_SET_NO_NEW_PRIVS (quick rows). |")
    w("")
    return "\n".join(lines) + "\n"


def main() -> int:
    content = render()
    if "--check" in sys.argv[1:]:
        if os.path.exists(OUT):
            with open(OUT) as f:
                if f.read() == content:
                    print("security-posture: up to date")
                    return 0
        print("security-posture: STALE — rerun scripts/security_posture.py",
              file=sys.stderr)
        return 1
    with open(OUT, "w") as f:
        f.write(content)
    print(f"wrote {OUT} ({len(ROWS)} rows)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
