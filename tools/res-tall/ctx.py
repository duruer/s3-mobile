#!/usr/bin/env python3
"""Print context (B lines before, A after) around addresses in an objdump listing.
Usage: python -I tools/res-tall/ctx.py s3.asm B A addr [addr ...]"""
import re, sys
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
ins = []
for l in open(sys.argv[1]):
    m = pat.match(l)
    if m:
        ins.append((int(m.group(1), 16), m.group(2)))
idx = {a: i for i, (a, _) in enumerate(ins)}
B, A = int(sys.argv[2]), int(sys.argv[3])
for t in sys.argv[4:]:
    i = idx[int(t, 16)]
    print(f'---- {t}')
    for a, s in ins[max(0, i - B): i + A + 1]:
        print(f'{"=>" if a == int(t, 16) else "  "}{a:x}: {s}')
