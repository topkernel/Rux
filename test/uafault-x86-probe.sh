#!/bin/bash
# uafault-x86-probe.sh — suite wrapper: uafault_boot.py (x86_64).
# Bad-user-pointer EFAULT contract across a syscall set: prlimit64
# new/old, setrlimit/getrlimit, write, read, fstat, getcwd — every one
# must return -EFAULT (not success) now that CR4.SMAP is on and the
# bootstrap identity map is gone; valid-buffer controls prove the
# uaccess brackets still copy. Materializes the musl loader the
# driver's fixed /tmp/ww-loader path expects.

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_need_x86_kernel
leg_x86_musl_loader || leg_result SKIP "musl loader source run/oh-x/rd/lib/ld-musl-x86_64.so.1 absent (loader-dependent driver cannot build)"

leg_run_x86_driver test/uafault_boot.py --platform x86_64 --kernel "$LEG_X86_KERNEL"
