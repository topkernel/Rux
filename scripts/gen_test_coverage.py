#!/usr/bin/env python3
"""Generate the test-coverage ledger (docs/development/test-coverage.md).

The ledger answers one question per subsystem: what actually exercises
this code, and does it run unattended? It is assembled from the tree,
not from memory:

  * guest probes        every test/*.c is listed with its boot driver,
                        its rootfs presence and its harness wiring (a
                        test/<name>-smoke.sh / -probe.sh wrapper, or a
                        named row in test/suite.sh)
  * suite legs          the matrix rows of test/suite.sh (parsed) plus
                        every auto-discovered probe wrapper
  * in-kernel units     kernel/src/tests/*.rs (count per subsystem)
  * host verification   kernel/verify/**.rs (count per subsystem)
  * LTP                 the newest in-tree test/ltp-results-r*.txt scan
                        (bucket totals + TCONF families). The TCONF
                        family classification in TRIAGE below is manual
                        and ranked; the counts are parsed.
  * formal tools        spin / kani / miri availability is probed on
                        the host at generation time

Manual content (the subsystem matrix, the TCONOF triage table, the gap
notes) lives in this file so it is versioned next to the code that
changed it; the counts and inventories around it are recomputed.

Usage: scripts/gen_test_coverage.py [--check]

  --check   regenerate to a temp file and diff against the committed
            ledger; exit non-zero on drift
"""

import glob
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "development", "test-coverage.md")

# --------------------------------------------------------------------------
# subsystem classification of the guest probes (test/*.c)
# --------------------------------------------------------------------------
# name -> (subsystem, note). A probe not listed here falls into "misc"
# with an empty note; the ledger lists it all the same.

PROBE_SUBSYSTEM = {
    "biglock_fair": ("sched", "global lock fairness under fork churn"),
    "binder_probe": ("binder", "closed-loop ioctl transaction sweep"),
    "binder2_probe": ("binder", "binder threads + death recursion"),
    "binder4_probe": ("binder", "binder stress/transactions"),
    "bindmkdir": ("fs", "bind-mount + mkdir visibility"),
    "brksegv_churn": ("mm", "brk/segv fault churn"),
    "cfi_shadow": ("arch-trap", "CFI shadow mapping behavior"),
    "chrootcwd": ("fs", "chroot cwd/getcwd semantics"),
    "coredump_probe": ("process", "coredump file production"),
    "decode_probe": ("arch-trap", "trap-decode of faulting instructions"),
    "decshim": ("misc", "decode shim helper"),
    "dfx_x86_probe": ("dfx", "x86 DFX surfaces (needs x86 kernel)"),
    "drm_probe": ("drivers-gpu", "DRM device probe"),
    "egl_painter": ("drivers-gpu", "EGL painting loop"),
    "egl_probe": ("drivers-gpu", "EGL initialization"),
    "epollexam": ("ipc", "epoll semantics exam"),
    "execloop": ("process", "exec loop churn"),
    "exitwalk_churn": ("process", "exit-path walk churn"),
    "faultaround": ("mm", "fault-around page population"),
    "faultlat": ("mm", "page-fault latency"),
    "faultlat2": ("mm", "page-fault latency variant 2"),
    "faultlat3": ("mm", "page-fault latency variant 3"),
    "fbdraw": ("drivers-gpu", "framebuffer drawing"),
    "fbterm": ("drivers-gpu", "framebuffer terminal"),
    "filedrain": ("fs", "page-cache drain behavior"),
    "filelock_probe": ("fs", "flock/POSIX lock semantics"),
    "filemap": ("fs", "file-backed mmap coherence"),
    "forkcold": ("process", "fork cold-start cost"),
    "forkwait_leak": ("process", "fork/wait resource leaks"),
    "fsfuzz": ("fs", "filesystem fuzz workload (fleet image)"),
    "futex_lost": ("sync", "lost futex wakeup"),
    "futex_min": ("sync", "minimal futex wait/wake"),
    "gbm_probe": ("drivers-gpu", "GBM buffer allocation"),
    "gwp_churn": ("mm", "guard-page write churn"),
    "inotify_probe": ("fs", "inotify event delivery"),
    "jchurn": ("mm", "jmalloc churn"),
    "loader_vma": ("mm", "loader VMA placement"),
    "mdprobe": ("fs", "multidisk probe"),
    "megapage_mmap": ("mm", "megapage mmap"),
    "mmapfix": ("mm", "mmap edge cases"),
    "modchurn": ("module", "module load/unload churn"),
    "modchurn_lib": ("module", "modchurn payload library"),
    "monostat": ("time", "monotonic clock stats"),
    "mrpar": ("ipc", "mr par client/server"),
    "munmapwrap": ("mm", "munmap wrap edge cases"),
    "nettest": ("net", "network stack smoke"),
    "oh2_probe": ("oh", "OpenHarmony S2 bringup probe"),
    "oomchurn": ("mm", "OOM pressure churn (suite stress leg)"),
    "oomtouch": ("mm", "OOM touch workload"),
    "orphan_storm": ("process", "orphan reparenting storm"),
    "p5long": ("ipc", "pbench P5 long-run variant"),
    "pbench": ("perf", "the perf-gate benchmark (P1-P7)"),
    "pbench_arm": ("perf", "pbench arm variant"),
    "perf_probe": ("perf", "perf subsystem probe"),
    "permcheck": ("fs", "permission checks"),
    "pollburn": ("ipc", "poll busy-loop"),
    "printk_flood": ("printk", "printk flood pressure"),
    "pstore_x86test": ("pstore", "x86 pstore init probe"),
    "pthread_min": ("sync", "minimal pthread test"),
    "pthread_staged": ("sync", "staged pthread test"),
    "pthread_test": ("sync", "pthread futex test"),
    "reboot_probe": ("process", "reboot syscall probe"),
    "reclaimwb": ("mm", "reclaim writeback behavior"),
    "selinux_mmap": ("security", "selinux mmap policy"),
    "sleep_probe": ("time", "sleep precision"),
    "slirp_test": ("net", "slirp networking test"),
    "subreap_churn": ("process", "subreaper churn"),
    "sysfs_drm_probe": ("drivers-gpu", "sysfs drm class probe"),
    "taskcomm_probe": ("process", "task comm semantics"),
    "tgsig": ("signal", "thread-group signal"),
    "timer_probe": ("time", "timer wheel probe"),
    "timer_runit": ("time", "timer runit helper"),
    "tokprobe": ("ipc", "token ipc probe"),
    "tokwrap": ("ipc", "token ipc wrapper"),
    "unixstress": ("ipc", "unix socket stress"),
    "unixwake": ("ipc", "unix socket wakeup"),
    "vma_storm": ("mm", "VMA churn storm"),
    "watchdog_probe": ("dfx", "watchdog detector probe"),
    "writestorm": ("fs", "write storm"),
    "xinput_probe": ("drivers-input", "xinput device probe"),
}

# --------------------------------------------------------------------------
# in-kernel unit family mapping (kernel/src/tests/<file>.rs -> subsystem)
# --------------------------------------------------------------------------

UNIT_FAMILY = [
    ("syscall_file", "fs"), ("syscall_io", "fs"), ("syscall_memory", "mm"),
    ("syscall_misc", "misc"), ("syscall_network", "net"),
    ("syscall_process", "process"), ("syscall_sched", "sched"),
    ("syscall_signal", "signal"), ("syscall_time", "time"),
    ("mem_cow", "mm"), ("mem_mmap", "mm"), ("page_allocator", "mm"),
    ("heap_allocator", "mm"), ("small_alloc", "mm"),
    ("ipc_epoll", "ipc"), ("ipc_eventfd", "ipc"), ("ipc_poll", "ipc"),
    ("ipc_sysv", "ipc"),
    ("scheduler", "sched"), ("preemptive_scheduler", "sched"),
    ("smp", "sched"), ("smp_schedule", "sched"), ("sleep_wakeup", "sched"),
    ("ext4_allocator", "fs"), ("ext4_file_write", "fs"),
    ("dcache", "fs"), ("path", "fs"), ("link", "fs"), ("mkdir_unlink", "fs"),
    ("mount_flags", "fs"), ("file_open", "fs"), ("file_flags", "fs"),
    ("fileops_caps", "fs"), ("fcntl", "fs"), ("fstat", "fs"),
    ("fdtable", "fs"), ("getdents_cache", "fs"), ("loop_flags", "fs"),
    ("tmpfs_mknod", "fs"), ("sgid_mode", "fs"),
    ("pipe2", "ipc"), ("network", "net"), ("virtio_net", "net"),
    ("tcp_handshake", "net"), ("virtio_queue", "drivers"),
    ("framebuffer", "drivers"), ("signal", "signal"),
    ("signal_procmask", "signal"), ("fork", "process"),
    ("process_tree", "process"), ("reparent_safety", "process"),
    ("wait4", "process"), ("getpid", "process"), ("pid_test", "process"),
    ("execve", "process"), ("futex_test", "sync"), ("semaphore", "sync"),
    ("listhead", "lib"), ("checksum", "lib"), ("errno_test", "lib"),
    ("dev_t", "lib"), ("boundary", "lib"), ("buffer_state", "lib"),
    ("config_test", "lib"), ("icache", "arch-trap"),
    ("initrd", "boot"), ("percpu", "smp"),
]

# The subsystem matrix rows: subsystem -> short description of what
# exercises it today. The wired/total probe counts are computed; the
# free-text columns are maintained by hand here.
SUBSYSTEM_NOTES = {
    "sched": "GRQ/sched classes: unit tests + biglock_fair probe; perf gate P2",
    "mm": "buddy/small_alloc/COW/fault: unit tests + the faultlat family, vma_storm, reclaimwb; oomchurn stress leg",
    "fs": "ext4/vfs: unit tests + filemap/filedrain/bindmkdir probes; pbench fs rows; fuzz-fs leg (fleet image)",
    "ipc": "pipes/unix/eventfd: unit tests; pbench P5; LTP syscalls family",
    "binder": "binder.rs: binder/binder2 probes (riscv), binder4 probe (x86 leg) — all wired",
    "net": "loopback/virtio-net: unit tests + nettest probe (ABI-correct per-arch rt_sigaction check after the 2026-10-28 adjudication), slirp_test needs user networking",
    "dfx": "syscost/trace/watchdog: dfx-smoke, tracefs-smoke, kallsyms-smoke, dfx-x86 probe",
    "arch-trap": "trap paths: decode/cfi probes (cfi wired on riscv), oops-smoke; KERNPANIC printer",
    "x86-port": "x86_64: pstore-x86, dfx-x86, binder4-x86, faultlat/2/3, tgsig, monostat, decode, subreap, exitwalk, watchdog, taskcomm, selinux-mmap, loader-vma, mmapfix, heapwatch-x86, ohlite-x86 legs (bzImage via PLATFORM=x86_64 make build; monkey-x86 also needs work/ubuntu-x86.img; several drivers need -mem 2G: the x86 heap starts at the 1GiB mark)",
    "pstore": "pstore/ramoops: pstore-smoke (riscv), pstore-x86-smoke (x86)",
    "process": "fork/exec/wait/reaper: unit tests + execloop/orphan_storm/subreap churns",
    "signal": "unit tests + tgsig probe",
    "time": "timers/clock: unit tests + timer_probe, sleep_probe (x86 driver), monostat",
    "sync": "futex/semaphore: unit tests + futex_min/futex_lost, pthread probes",
    "printk": "printk ring: printk_flood (rootfs)",
    "security": "selinux/caps: selinux_mmap probe; LTP caps family",
    "drivers-gpu": "drm/gpu/fb: drm/egl/gbm/fb probes (image-dependent)",
    "drivers-input": "input: xinput_probe",
    "module": "module loader: modchurn (needs module build env)",
    "perf": "perf subsystem: perf_probe; run/perf-gate.sh tier legs",
    "oh": "oh2-probe wired (riscv; currently FAILs 15/68 checks on stale expectations — memfd NULL-name EFAULT and the access-token registry are kernel-correct per the OH feature work; OH lane to adjudicate); ohlite-x86 wired (blocked on the MS_MOVE relative-source kernel gap)",
    "boot": "initrd/boot path: initrd suite (subdir)",
    "lib": "lib-level units only",
    "misc": "uncategorized",
    "drivers": "virtio/block drivers",
    "smp": "SMP bringup",
}

# --------------------------------------------------------------------------
# LTP TCONF triage (manual classification, counts computed from the
# newest in-tree scan). Rows: (family label, test-name regex, class, note)
# classification: CHEAP (small kernel feature, bounded work) |
# BIG (subsystem-sized) | ENV (environment artifact: missing guest
# tools/config, not a kernel gap)

TCONF_TRIAGE = [
    ("namespaces (user/net/pid/time)",
     r"^(userns|netns|pidns|timens)", "BIG",
     "clone(CLONE_NEW*) namespaces + nsproxy infrastructure; the single "
     "largest tst_kconfig constraint group"),
    ("hugemmap", r"^hugemmap", "CHEAP",
     "MAP_HUGETLB refused; a hugetlb pool + mmap flag path serves most "
     "of the family (no THP needed)"),
    ("keys (add_key/keyctl/request_key)",
     r"^(add_key|keyctl|request_key)", "BIG",
     "keyring subsystem; large API surface"),
    ("move_pages", r"^move_pages", "CHEAP",
     "TCONF is 'NUMA support' detection — a single-node "
     "/sys/devices/system/node topology plus a UMA move_pages(2) stub "
     "converts the family to real runs"),
    ("wqueue (posix mq)", r"^wqueue", "CHEAP",
     "message-queue syscall coverage; posix_mq exists, needs the "
     "missing knob/flag edges"),
    ("quotactl", r"^quotactl", "BIG",
     "quota ioctls + per-fs quota accounting"),
    ("setsockopt edges", r"^setsockopt", "CHEAP",
     "individual missing sockopt options on top of the existing net "
     "stack"),
    ("ksm", r"^ksm", "BIG",
     "kernel same-page merging"),
    ("af_alg", r"^af_alg", "BIG",
     "crypto API (AF_ALG sockets)"),
    ("input (evdev)", r"^input", "CHEAP",
     "evdev ioctls on top of the existing input layer"),
    ("fanotify", r"^fanotify", "BIG",
     "fanotify(2) group API, mount-mark notification subsystem (the "
     "newest scan grew this family to ~23)"),
    ("fcntl family", r"^fcntl", "CHEAP",
     "OFD locks (F_OFD_SETLK) + lease ioctls on the existing locks.rs "
     "(the newest scan grew this family to ~37)"),
    ("guest tooling (useradd etc.)",
     r"__never_matches__", "ENV",
     "~10 tests TCONF on missing guest tools (useradd, mkfs variants) — "
     "rootfs content, not kernel"),
    ("tst_kconfig CONFIG ids",
     r"__never_matches__", "ENV",
     "/proc/config.gz exists but names differ from Linux CONFIG_* ids; "
     "aligning ids un-hides tests but must reflect reality (advertising "
     "a CONFIG the kernel lacks converts TCONF into FAIL)"),
]

# headline LTP numbers from the newest coordinator scan (2026-10-28).
# The per-family TCONF counts above are re-derived from the newest
# in-tree scan file; if the two disagree the file is the older one.
LTP_HEADLINE = ("2026-10-28 coordinator scan: 1018/1869 PASS, 525 TCONF, "
                "231 FAIL, 32 CRASH, 32 EXECFAIL, 23 TIMEOUT")

# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------


def sh(cmd, timeout=20):
    try:
        r = subprocess.run(cmd, shell=True, capture_output=True, text=True,
                           timeout=timeout, cwd=ROOT)
        return r.returncode, (r.stdout + r.stderr).strip()
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return 124, ""


def boot_driver_for(base):
    """The boot driver script for a probe base name, if any."""
    for cand in (base + "_boot.py", re.sub(r"_probe$", "", base) + "_boot.py",
                 base.replace("_probe", "") + "_boot.py"):
        if os.path.isfile(os.path.join(ROOT, "test", cand)):
            return cand
    return None


# Probes whose suite wrapper has a name that does not derive from the
# .c base name (historical naming): base -> wrapper file, or
# "matrix:<leg>" when the probe runs inside a named suite matrix row.
WRAPPER_ALIAS = {
    "pstore_x86test": "pstore-x86-smoke.sh",
    "perf_probe": "perf-smoke.sh",
    "binder4_probe": "binder4-x86-probe.sh",
    "pbench": "matrix:perf-gate (via run/perf-gate.sh tier 2)",
    # x86-driver family: wrappers carry the -x86- name so the suite
    # routes them to the x86_64 leg
    "faultlat": "faultlat-x86-probe.sh",
    "faultlat2": "faultlat2-x86-probe.sh",
    "faultlat3": "faultlat3-x86-probe.sh",
    "tgsig": "tgsig-x86-probe.sh",
    "monostat": "monostat-x86-probe.sh",
    "subreap_churn": "subreap-x86-probe.sh",
    "exitwalk_churn": "exitwalk-x86-probe.sh",
    "watchdog_probe": "watchdog-x86-probe.sh",
    "taskcomm_probe": "taskcomm-x86-probe.sh",
    "selinux_mmap": "selinux-mmap-x86-probe.sh",
    "loader_vma": "loader-vma-x86-probe.sh",
    "mmapfix": "mmapfix-x86-probe.sh",
    "decode_probe": "decode-x86-probe.sh",
    "sleep_probe": "sleep-x86-probe.sh",
    "oh2_probe": "oh2-probe.sh",
    "brksegv_churn": "brksegv-x86-probe.sh",
}


def wrapper_candidates(base):
    """Wrapper file names a probe's suite leg may carry: the plain base,
    the hyphen-normalized base (binder_probe -> binder-probe.sh), and
    any explicit alias. Every candidate ends in -smoke.sh/-probe.sh —
    those are the only names test/suite.sh auto-discovers."""
    hyph = base.replace("_", "-")
    cands = [WRAPPER_ALIAS.get(base), base + "-smoke.sh", base + "-probe.sh"]
    if hyph != base:
        cands += [hyph + "-smoke.sh", hyph + "-probe.sh"]
        # a *_probe base absorbs its suffix: binder_probe's wrapper is
        # binder-probe.sh (not binder-probe-probe.sh)
        if base.endswith("_probe") or base.endswith("_smoke"):
            cands.append(hyph + ".sh")
    return [c for c in cands if c]


def collect_probes():
    """Every test/*.c with its driver/wiring facts."""
    out = []
    mkrootfs = ""
    with open(os.path.join(ROOT, "test", "mkrootfs.sh")) as f:
        mkrootfs = f.read()
    suite = ""
    with open(os.path.join(ROOT, "test", "suite.sh")) as f:
        suite = f.read()
    for c in sorted(glob.glob(os.path.join(ROOT, "test", "*.c"))):
        base = os.path.basename(c)[:-2]
        driver = boot_driver_for(base)
        wrapper = None
        for w in wrapper_candidates(base):
            if w.startswith("matrix:") or \
                    os.path.isfile(os.path.join(ROOT, "test", w)):
                wrapper = w
                break
        in_matrix = re.search(r"\b%s\b" % re.escape(base), suite) is not None
        baked = ("STAGING/test/" + base) in mkrootfs
        subsystem, note = PROBE_SUBSYSTEM.get(base, ("misc", ""))
        out.append({
            "base": base, "subsystem": subsystem, "note": note,
            "driver": driver, "wrapper": wrapper, "matrix": in_matrix,
            "baked": baked,
            "wired": bool(wrapper) or in_matrix,
        })
    return out


def collect_suite_legs():
    """The named rows of the suite matrix (static part) + probe count."""
    with open(os.path.join(ROOT, "test", "suite.sh")) as f:
        text = f.read()
    m = re.search(r"matrix_rows\(\) \{\n    cat <<'ROWS'\n(.*?)\nROWS", text,
                  re.S)
    rows = []
    if m:
        for line in m.group(1).splitlines():
            parts = line.split("|")
            if len(parts) >= 5:
                rows.append(parts)
    n_probes = len(glob.glob(os.path.join(ROOT, "test", "*-smoke.sh"))) + \
        len(glob.glob(os.path.join(ROOT, "test", "*-probe.sh")))
    return rows, n_probes


def collect_units():
    """kernel/src/tests/*.rs grouped into families."""
    counts = {}
    total = 0
    for f in glob.glob(os.path.join(ROOT, "kernel", "src", "tests", "*.rs")):
        name = os.path.basename(f)[:-3]
        if name == "mod":
            continue
        total += 1
        fam = "misc"
        for prefix, sub in UNIT_FAMILY:
            if name == prefix or name.startswith(prefix):
                fam = sub
                break
        counts[fam] = counts.get(fam, 0) + 1
    return counts, total


def collect_verify():
    """kernel/verify/**/*.rs grouped by top module."""
    counts = {}
    total = 0
    for f in glob.glob(os.path.join(ROOT, "kernel", "verify", "src", "**",
                                    "*.rs"), recursive=True):
        rel = os.path.relpath(f, os.path.join(ROOT, "kernel", "verify",
                                              "src"))
        top = rel.split(os.sep)[0].replace(".rs", "")
        total += 1
        counts[top] = counts.get(top, 0) + 1
    return counts, total


def collect_ltp():
    """The newest in-tree LTP scan: totals + TCONF families."""
    files = sorted(glob.glob(os.path.join(ROOT, "test",
                                          "ltp-results-r*.txt")))
    if not files:
        return None
    newest = files[-1]
    totals = {}
    tconf = []
    tconf_tails = []
    with open(newest, errors="replace") as f:
        for line in f:
            if line.startswith("# total tests:"):
                for m in re.finditer(r"(\w+)=([0-9]+)", line):
                    totals[m.group(1)] = int(m.group(2))
            parts = line.rstrip("\n").split("|")
            if len(parts) >= 4 and parts[3] == "TCONF":
                tconf.append(parts[1])
                tconf_tails.append("|".join(parts[4:]))
    n_tooling = sum(1 for t in tconf_tails if "Couldn't find" in t)
    n_kconfig = sum(1 for t in tconf_tails
                    if "tst_kconfig" in t or "Constraint" in t)
    families = {}
    for t in tconf:
        fam = re.sub(r"[0-9]+$", "", t)
        families[fam] = families.get(fam, 0) + 1
    top = sorted(families.items(), key=lambda kv: -kv[1])
    return {
        "file": os.path.relpath(newest, ROOT),
        "totals": totals,
        "tconf_tests": tconf,
        "families": top,
        "n_tooling": n_tooling,
        "n_kconfig": n_kconfig,
    }


def formal_status():
    rc, out = sh("spin -V")
    spin = out if rc == 0 else "NOT INSTALLED"
    rc, out = sh("cargo +nightly miri --version")
    miri = out if rc == 0 else "NOT INSTALLED (no nightly/miri component)"
    rc, out = sh("cargo kani --version")
    kani = out if rc == 0 else "NOT INSTALLED (no kani cargo plugin)"
    return spin, miri, kani


def probe_counts_by_subsystem(probes):
    by = {}
    for p in probes:
        e = by.setdefault(p["subsystem"], {"wired": 0, "total": 0})
        e["total"] += 1
        if p["wired"]:
            e["wired"] += 1
    return by


# --------------------------------------------------------------------------
# document assembly
# --------------------------------------------------------------------------


def main():
    check = "--check" in sys.argv
    probes = collect_probes()
    rows, n_probe_legs = collect_suite_legs()
    units, unit_total = collect_units()
    verify, verify_total = collect_verify()
    ltp = collect_ltp()
    spin, miri, kani = formal_status()
    by_sub = probe_counts_by_subsystem(probes)
    wired = sum(1 for p in probes if p["wired"])

    o = []
    o.append("# Test Coverage Ledger")
    o.append("")
    o.append("What exercises each part of the kernel, and whether it runs")
    o.append("unattended. Generated by `scripts/gen_test_coverage.py` —")
    o.append("counts and inventories are recomputed from the tree; the")
    o.append("manual tables (triage, notes) live in that script. Refresh")
    o.append("with `python3 scripts/gen_test_coverage.py` after changing")
    o.append("any test wiring; CI-style drift check:")
    o.append("`scripts/gen_test_coverage.py --check`.")
    o.append("")
    o.append("Generated: %s" % subprocess.run(
        ["date", "-u", "+%Y-%m-%dT%H:%MZ"], capture_output=True, text=True
    ).stdout.strip())
    o.append("")

    # ---- headline -----------------------------------------------------
    o.append("## Headline")
    o.append("")
    o.append("| what | count |")
    o.append("|---|---:|")
    o.append("| guest probes in test/*.c | %d |" % len(probes))
    o.append("| probes wired to the unattended suite | %d |" % wired)
    o.append("| probes unwired | %d |" % (len(probes) - wired))
    o.append("| probe wrapper legs auto-discovered by suite.sh | %d |" %
             n_probe_legs)
    o.append("| named suite matrix rows | %d |" % len(rows))
    o.append("| in-kernel unit test files | %d |" % unit_total)
    o.append("| host verify-crate sources | %d |" % verify_total)
    o.append("")

    # ---- subsystem matrix ----------------------------------------------
    o.append("## Subsystem matrix")
    o.append("")
    o.append("Columns: unit = in-kernel test files, verify = host verify")
    o.append("crate sources, probes = guest probes wired/total (wired =")
    o.append("runs unattended via a suite wrapper or matrix row). The")
    o.append("notes column names the suite legs, LTP families, perf-gate")
    o.append("rows and stress coverage that apply.")
    o.append("")
    subs = sorted(set(list(by_sub.keys()) + list(units.keys()) +
                      list(SUBSYSTEM_NOTES.keys())))
    o.append("| subsystem | unit | verify | probes (wired/total) | coverage notes |")
    o.append("|---|---:|---:|---|---|")
    for s in subs:
        p = by_sub.get(s, {"wired": 0, "total": 0})
        n_unit = units.get(s, 0)
        n_ver = verify.get(s, 0)
        o.append("| %s | %s | %s | %d/%d | %s |" % (
            s, n_unit or "-", n_ver or "-", p["wired"], p["total"],
            SUBSYSTEM_NOTES.get(s, "")))
    o.append("")

    # ---- suite legs -----------------------------------------------------
    o.append("## Unattended suite legs (test/suite.sh)")
    o.append("")
    o.append("Static matrix rows:")
    o.append("")
    o.append("| name | kind | arch |")
    o.append("|---|---|---|")
    for parts in rows:
        o.append("| %s | %s | %s |" % (parts[0], parts[1], parts[2]))
    o.append("")
    o.append("Plus every `test/*-smoke.sh` / `test/*-probe.sh` (%d today),"
             % n_probe_legs)
    o.append("auto-discovered as probe legs. x86 legs need"
             " `target/x86_64-unknown-none/debug/bzImage` and"
             " `work/ubuntu-x86.img`; the fuzz leg needs"
             " `run/fleet/images/fleet-fsfuzz-rootfs.img`.")
    o.append("")

    # ---- probe inventory -------------------------------------------------
    o.append("## Guest probe inventory (test/*.c)")
    o.append("")
    o.append("wired = has a suite wrapper (`test/<name>-smoke.sh` or"
             " `-probe.sh`) or a named suite matrix row. driver = the"
             " boot driver builds and boots the probe itself; rootfs ="
             " the binary is baked into test/rootfs.img by mkrootfs.sh.")
    o.append("")
    o.append("| probe | subsystem | driver | rootfs | wrapper | wired | note |")
    o.append("|---|---|---|---|---|---|---|")
    for p in probes:
        o.append("| %s | %s | %s | %s | %s | %s | %s |" % (
            p["base"], p["subsystem"],
            p["driver"] or "-", "yes" if p["baked"] else "-",
            p["wrapper"] or ("matrix" if p["matrix"] else "-"),
            "WIRED" if p["wired"] else "UNWIRED", p["note"]))
    o.append("")

    # ---- subdir suites ---------------------------------------------------
    o.append("## Sub-directory suites")
    o.append("")
    o.append("| suite | entry point | wired | note |")
    o.append("|---|---|---|---|")
    subdirs = [
        ("test/cowrace", "cowrace-probe.sh", "suite",
         "wired: musl pthread build, tiny-image init boot, asserts "
         "COWRACE-PASS (the fake-OOM COW race repro)"),
        ("test/heapwatch", "run-boot.sh", "suite",
         "wired on x86 (heapwatch-x86-smoke.sh: prebuilt initrd cpio, "
         "HEAPWATCH-START + beats, no damage report); riscv64 path "
         "needs a musl recipe in mkinitrd-hw.py (plain static glibc "
         "SIGILLs via vector ifuncs)"),
        ("test/hotplug", "run.py", "manual",
         "CPU/PCI hotplug uevent probe; build-img.sh needs the shared "
         "gate rootfs at /home/william/rux-gate/ubuntu/rootfs (absent)"),
        ("test/initrd", "run-initrd.sh", "suite",
         "wired as test/initrd-boot-probe.sh (packs the cpio, requires "
         "all 11 markers — passing; the reported fault was adjudicated "
         "a build artifact: gcc-15 default march emits zcb the boot CPU "
         "lacks, and without -fno-builtin gcc folds the memset shim "
         "into infinite recursion, walking sp to the stack floor)"),
        ("test/ohlite", "mkohlite.sh + run-oh.sh", "suite",
         "wired on x86 (ohlite-x86-smoke.sh: builds the 4-image set, "
         "boots via run-oh.sh, asserts the 60s BOOT GATE PASS)"),
        ("test/stress", "monkey.py (suite monkey legs)", "suite",
         "randomised command churn; riscv leg runs, x86 needs "
         "work/ubuntu-x86.img"),
        ("test/ltpscan", "run1.sh (suite integration leg)", "suite",
         "bounded LTP sample on a private image copy"),
    ]
    for name, entry, wired_state, note in subdirs:
        o.append("| %s | %s | %s | %s |" % (name, entry, wired_state, note))
    o.append("")

    # ---- LTP -------------------------------------------------------------
    o.append("## LTP")
    o.append("")
    o.append(LTP_HEADLINE + ".")
    o.append("")
    if ltp:
        o.append("Per-family data below is parsed from the newest in-tree"
                 " scan (%s, older than the headline scan — TCONF causes"
                 " are structural, so the family shapes carry over)."
                 % ltp["file"])
        o.append("")
        o.append("Bucket totals of that scan: " +
                 ", ".join("%s=%d" % kv for kv in
                           sorted(ltp["totals"].items())))
        o.append("")
        o.append("Top TCONF families (tests | family):")
        o.append("")
        o.append("| tests | family |")
        o.append("|---:|---|")
        for fam, n in ltp["families"][:15]:
            o.append("| %d | %s |" % (n, fam))
        o.append("")
    o.append("### TCONF triage (ranked enablement backlog)")
    o.append("")
    o.append("CHEAP = bounded kernel feature; BIG = subsystem-sized;")
    o.append("ENV = guest environment artifact, not a kernel gap.")
    o.append("Counts are from the parsed in-tree scan; rank orders by")
    o.append("value (tests unlocked per unit of work). This lane's")
    o.append("deliverable is the list; implementation belongs to the")
    o.append("feature lanes.")
    o.append("")
    o.append("| rank | family | tests | class | what it takes |")
    o.append("|---:|---|---:|---|---|")
    if ltp:
        ranked = []
        for label, rx, cls, note in TCONF_TRIAGE:
            n = sum(1 for t in ltp["tconf_tests"] if re.match(rx, t))
            ranked.append((label, n, cls, note))
        ranked.sort(key=lambda r: -(r[1] if r[2] != "ENV" else r[1] / 1000.0))
        for i, (label, n, cls, note) in enumerate(ranked, 1):
            if n == 0 and cls == "ENV":
                if "tooling" in label:
                    shown = "~%d" % ltp["n_tooling"]
                else:
                    shown = "~%d" % ltp["n_kconfig"]
            else:
                shown = str(n)
            o.append("| %d | %s | %s | %s | %s |" % (i, label, shown, cls,
                                                    note))
    o.append("")

    # ---- formal ----------------------------------------------------------
    o.append("## Formal verification")
    o.append("")
    o.append("Miri runs the whole kernel/verify crate under"
             " interpretation: >28 min on this host (the suite leg"
             " carries a 4200 s budget; every test completed so far"
             " was ok). Kani's cargo plugin is installed but its CBMC"
             " bundle download (github.com releases) hangs on this"
             " host — honest SKIP until the bundle is fetched from a"
             " reachable mirror.")
    o.append("")
    o.append("| tool | host status | suite leg |")
    o.append("|---|---|---|")
    o.append("| SPIN | %s | formal-spin (runs) |" % spin)
    o.append("| Kani | %s | formal-kani (SKIP until the bundle exists) |" % kani)
    o.append("| Miri | %s | formal-miri (runs, 4200 s budget) |" % miri)
    o.append("")

    # ---- gaps ------------------------------------------------------------
    unwired = [p for p in probes if not p["wired"]]
    o.append("## Current gaps (recomputed each refresh)")
    o.append("")
    o.append("1. %d of %d guest probes are unwired (no suite wrapper);"
             " the driver-based ones are one thin wrapper away."
             % (len(unwired), len(probes)))
    o.append("2. x86 legs need target/x86_64-unknown-none/debug/bzImage"
             " in the repo they run from (untracked build artifact —"
             " rebuilt via PLATFORM=x86_64 make build); monkey-x86 also"
             " needs work/ubuntu-x86.img (built by"
             " test/ubuntu-x86/build-img.sh).")
    o.append("3. fuzz-fs and fleet stress legs SKIP without"
             " run/fleet/images/* (stress-lane artifacts, gitignored).")
    o.append("4. Sub-directory suites (cowrace, hotplug, initrd,"
             " heapwatch, ohlite) have no suite discovery.")
    o.append("5. Kani/Miri depend on nightly toolchain components being"
             " installed on the host.")
    o.append("")
    o.append("Unwired probes right now: " +
             ", ".join(p["base"] for p in unwired) if unwired else
             "none")
    o.append("")

    doc = "\n".join(o) + "\n"

    if check:
        with open(OUT) as f:
            committed = f.read()
        if committed == doc:
            print("ledger in sync")
            return 0
        print("ledger DRIFTED from the tree (rerun "
              "scripts/gen_test_coverage.py and commit)")
        return 1

    with open(OUT, "w") as f:
        f.write(doc)
    print("wrote %s (%d probes, %d wired)" % (OUT, len(probes), wired))
    return 0


if __name__ == "__main__":
    sys.exit(main())
