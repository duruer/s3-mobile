#!/usr/bin/env python3
"""Scan an objdump listing (intel, no raw bytes) for constants that look like render-snapshot grid strides
(130 rows per column, 0x30 bytes per cell, grid at +0x500): column displacements k*0x1860 (+ small row offsets,
+ optionally the 0x500 grid base) and index multiples k*130 (+-1..3), plus `shl r,6` (x65, part of x130).
Usage: python -I tools/res-tall/stride_scan.py s3.asm [lo hi] ... (address ranges, hex; default whole .text)"""
import re, sys

pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
num = re.compile(r'(-?)0x([0-9a-f]+)\b')
ranges = []
args = sys.argv[2:]
for i in range(0, len(args), 2):
    ranges.append((int(args[i], 16), int(args[i + 1], 16)))
if not ranges:
    ranges = [(0x401000, 0x630000)]

TAB = {}
for sign in (1, -1):
    for base in (0, 0x500):
        for k in range(1, 8):
            for d in range(-0x90, 0x91):
                v = sign * (k * 0x1860 + d) + base
                if abs(v) > 0x100:
                    TAB.setdefault(v, []).append(f'{"-" if sign < 0 else ""}{k}col{d:+#x}{"+grid" if base else ""}')
    for k in range(1, 8):
        for d in (-3, -2, -1, 0, 1, 2, 3):
            TAB.setdefault(sign * (k * 130 + d), []).append(f'{"-" if sign < 0 else ""}{k}*130{d:+d}')

def interesting(v):
    return TAB.get(v, [])

for l in open(sys.argv[1]):
    m = pat.match(l)
    if not m:
        continue
    a = int(m.group(1), 16)
    if not any(lo <= a < hi for lo, hi in ranges):
        continue
    txt = m.group(2)
    if re.match(r'(call|j[a-z]+)\s', txt):
        continue
    tags = []
    for s, h in num.findall(txt):
        v = int(h, 16)
        if v >= 0x80000000:
            v -= 1 << 32
        if s:
            v = -v
        tags += interesting(v)
    if re.search(r'\bshl\s+\w+,0x6$', txt):
        tags.append('shl6')
    if re.search(r'\bimul\b', txt):
        tags.append('imul')
    if tags:
        print(f'{a:x}: {txt:50s} {" ".join(sorted(set(tags)))}')
