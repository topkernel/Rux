#!/bin/bash
# asm_identity_diff.sh — the merge-gate hard bar (design §7 row 1):
# per-symbol disassembly comparison of the lock paths between two
# kernel ELFs built with the SAME default config (cargo feature
# debug-lockdep OFF). Absolute addresses are normalized; the
# instruction stream (mnemonic + symbolic target) must be IDENTICAL
# for every lock-path symbol present in both ELFs.
# Usage: asm_identity_diff.sh <elf A> <elf B> <arch: x86_64|riscv64>
set -u
A="$1"; B="$2"; ARCH="${3:-x86_64}"
case "$ARCH" in
  x86_64) OD=objdump ;;
  riscv64) OD=riscv64-linux-gnu-objdump ;;
  *) echo "bad arch"; exit 2 ;;
esac

# Lock-path surface: the keyed lock types (every method), the raw
# spinlock acquire/release, the SleepMutex funnel, and the drop glue
# of the lock guards. Substring match on mangled names (the default
# dev profile keeps these as distinct symbols).
WANT='KeyedRawSpinlock|KeyedSpinlock|RawSpinlock(4lock|6unlock|8try_lock)|SleepMutex(4lock|7release|11lock_inner|16release_if_owner|16optimistic_probe)|drop_glue.*(SpinlockGuard|SpinlockIrqGuard|SpinlockBhGuard|KeyedGuard|KeyedIrqGuard|SleepMutexGuard)|(SpinlockGuard|SpinlockIrqGuard|SpinlockBhGuard|KeyedGuard|KeyedIrqGuard|SleepMutexGuard).*drop'

extract() {
  local elf="$1"
  $OD -d --no-show-raw-insn "$elf" | awk -v want="$WANT" '
    /^[0-9a-f]+ <.*>:$/ {
      sym = $0; sub(/^[0-9a-f]+ </, "", sym); sub(/>:$/, "", sym)
      keep = (sym ~ want) ? 1 : 0; next
    }
    keep && /^[0-9a-f]+:/ {
      # PC-relative pair folding (riscv64): the linker splits one
      # effective target between the auipc immediate and the consumer
      # offset differently depending on 4KiB page alignment of the
      # alignment — a whole-binary layout artifact, not a codegen
      # choice. The symbolic comment on the consumer line carries the
      # effective target (compared strictly below); the base line
      # alone pins no codegen fact, so drop it and fold the consumer
      # to mnemonic + resolved symbol.
      if ($0 ~ /auipc/) next
      line = $0; sub(/^[0-9a-f]+:\t?/, "", line)
      # Normalize hex immediates (0x-prefixed) and branch/call target
      # addresses (raw-hex followed by the symbolic annotation).
      gsub(/0x[0-9a-f]+/, "0xX", line)
      gsub(/[ \t][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+/, " ADDR", line)
      gsub(/,[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+/, ",ADDR", line)
      gsub(/# [0-9a-f][0-9a-f]*/, "# ADDR", line)
      # riscv: jalr/branch relative offsets are decimal signed.
      gsub(/-?[0-9]+\(/, "X(", line)
      gsub(/,-?[0-9]+\)/, ",X)", line)
      gsub(/-?[0-9]+%(ra|sp|s0|t0|t1|a0)/, "X%REG", line)
      if (match(line, /# ADDR <[^>]+>/)) {
        op = line; sub(/[ \t].*$/, "", op)
        tgt = substr(line, RSTART + 7)
        line = op " " tgt
      }
      print sym "\t" line
    }
  '
}

extract "$A" | sort > /tmp/asmA.txt
extract "$B" | sort > /tmp/asmB.txt

cut -f1 /tmp/asmA.txt | sort -u > /tmp/symsA.txt
cut -f1 /tmp/asmB.txt | sort -u > /tmp/symsB.txt
na=$(wc -l < /tmp/symsA.txt); nb=$(wc -l < /tmp/symsB.txt)
echo "lock-path symbols: A(main)=$na B(branch)=$nb"
comm -12 /tmp/symsA.txt /tmp/symsB.txt > /tmp/common.txt
echo "symbols in both: $(wc -l < /tmp/common.txt)"
only_a=$(comm -23 /tmp/symsA.txt /tmp/symsB.txt | head -5)
only_b=$(comm -13 /tmp/symsA.txt /tmp/symsB.txt | head -5)
if [ -n "$only_a" ]; then echo "symbols ONLY in A(main):"; echo "$only_a"; fi
if [ -n "$only_b" ]; then echo "symbols ONLY in B(branch):"; echo "$only_b"; fi

fail=0
while IFS= read -r sym; do
  if ! diff <(grep -F "$sym"$'\t' /tmp/asmA.txt) <(grep -F "$sym"$'\t' /tmp/asmB.txt) > /tmp/asm_sym_diff.txt 2>&1; then
    echo "DIFF in $sym:"
    head -8 /tmp/asm_sym_diff.txt
    fail=1
  fi
done < /tmp/common.txt
if [ "$fail" = 0 ]; then
  echo "RESULT: IDENTICAL — $(wc -l < /tmp/common.txt) common lock-path symbols instruction-identical"
else
  echo "RESULT: DIFFERENCES FOUND"
fi
exit $fail
