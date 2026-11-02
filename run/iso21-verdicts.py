#!/usr/bin/env python3
"""Build results/iso21-verdicts.txt + iso21-summary.txt from the two iso passes.

Classification (r21 charter, per the 2x isolation rule):
  a = same non-PASS class in BOTH iso runs (consistent; treat as real for
      the x86_64 baseline) — for gains: PASS in both runs.
  b = differs between runs / PASS at least once (flaky under load or
      poison-only in the fleet scan).
  c = no usable verdict in isolation (POISON/wedge both passes).
"""
import sys
from collections import Counter

RES_LEN = 180

def load(path):
    out = {}
    try:
        for line in open(path):
            line = line.rstrip('\n')
            if not line or line.startswith('#'):
                continue
            p = line.split('|', 4)
            if len(p) < 3:
                continue
            out[p[1]] = (p[2], p[4] if len(p) > 4 else '')
    except FileNotFoundError:
        pass
    return out

def resline(name, entry):
    if not entry:
        return ''
    cls, tail = entry
    return f'RES|{name}|0|{tail}'[:RES_LEN] if False else f'{cls}: {tail}'[:RES_LEN]

def main():
    losses = [l.strip() for l in open('run/iso-losses.txt') if l.strip()]
    gains = [l.strip() for l in open('run/iso-gains.txt') if l.strip()]
    A, B = load('run/iso21-a.txt'), load('run/iso21-b.txt')
    out = []
    cls_count = Counter()
    flaky, real, inconc = [], [], []
    for name in losses + gains:
        ea, eb = A.get(name), B.get(name)
        la, lb = resline(name, ea), resline(name, eb)
        ca = ea[0] if ea else 'NONE'
        cb = eb[0] if eb else 'NONE'
        if name in losses:
            if ca == 'NONE' and cb == 'NONE':
                v = 'c'; inconc.append(name)
            elif 'PASS' in (ca, cb):
                v = 'b'; flaky.append(name)
            elif ca == cb:
                v = 'a'; real.append(name)
            else:
                v = 'b'; flaky.append(name)
        else:  # gain
            if ca == 'PASS' and cb == 'PASS':
                v = 'a'; real.append(name)
            elif 'PASS' in (ca, cb):
                v = 'b'; flaky.append(name)
            elif ca == 'NONE' and cb == 'NONE':
                v = 'c'; inconc.append(name)
            else:
                v = 'b'; flaky.append(name)
        cls_count[v] += 1
        out.append(f'ISO21 {name} a: {la}')
        out.append(f'ISO21 {name} b: {lb}')
    out.append('ISO21 ALL DONE')
    with open('results/iso21-verdicts.txt', 'w') as f:
        f.write('\n'.join(out) + '\n')
    with open('results/iso21-summary.txt', 'w') as f:
        f.write('r21 isolation (2x) over the 47 losses + 29 gains vs r20 (riscv64)\n')
        f.write(f'class a (consistent, real for the x86_64 baseline): {len(real)}\n')
        f.write(f'class b (flaky / poison-only / divergent):          {len(flaky)}\n')
        f.write(f'class c (no isolation verdict):                     {len(inconc)}\n\n')
        f.write(f'a-real: {" ".join(real)}\n\n')
        f.write(f'b-flaky: {" ".join(flaky)}\n\n')
        f.write(f'c-inconc: {" ".join(inconc)}\n')
    print(f'a={len(real)} b={len(flaky)} c={len(inconc)}')
    print('b-flaky:', ' '.join(flaky))
    print('c-inconc:', ' '.join(inconc))

if __name__ == '__main__':
    main()
