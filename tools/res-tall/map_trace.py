#!/usr/bin/env python3
"""Map a memtrace dump (lines `next_eip last_addr`, hex) to the instructions of an objdump listing and classify
the last address against snapshot bases (offset from the base that contains it).
Usage: python -I tools/res-tall/map_trace.py s3.asm trace.txt [size_hex base_hex ...]"""
import re, sys
lines = open(sys.argv[1]).read().splitlines()
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
insns = []
for l in lines:
    m = pat.match(l)
    if m:
        insns.append((int(m.group(1), 16), m.group(2)))
nxt = {}
for i in range(len(insns) - 1):
    nxt[insns[i + 1][0]] = insns[i]
size = int(sys.argv[3], 16) if len(sys.argv) > 3 else 0
bases = [int(b, 16) for b in sys.argv[4:]]
for l in open(sys.argv[2]).read().splitlines():
    w = l.split()
    n = int(w[0], 16)
    addr = int(w[1], 16) if len(w) > 1 else 0
    a, txt = nxt.get(n, (0, '?'))
    txt = re.sub(r'^([0-9a-f]{2} )+\s*', '', txt)
    cls = 'other'
    for b in bases:
        if b <= addr < b + size:
            off = addr - b
            cls = 'hdr  %05x' % off if off < 0x500 else ('grid %05x' % off if off < size - 0x1160 else 'tail %05x' % off)
    print(f'{a:x}: {cls:12s} {txt}')
