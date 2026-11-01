#!/bin/bash
# pa115p2fk — attribution A/B for the khog 10ms quantization: same strong
# kernel, hook ARMED via cmdline (kpreempt=1). If khog w2r collapses to
# the ~10us class, the quantization is preemption policy (hook off +
# wake-preemption gating); if it stays 10ms, something else broke.
cd "$(dirname "$0")/.."
python3 test/pbench_boot.py --p2f \
    --bzimage run/pa115-main-afc41b8b-strong-bzImage \
    --accel kvm --tag pa115p2fk --timeout 900 \
    --append-extra kpreempt=1 \
    > run/pa115p2fk-boot.log 2>&1
echo "p2fk rc=$?" >> run/pa115p2fk-boot.log
