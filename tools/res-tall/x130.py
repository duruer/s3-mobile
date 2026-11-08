#!/usr/bin/env python3
"""Find `mov R,S; shl R,6; add R,S; ... lea X,[Y+R*2]` sequences (Y + S*130 = snapshot grid index) in an objdump
listing (intel, no raw bytes). Prints the shl address, the lea, and whether Y == S (x131, not the grid).
Usage: python -I tools/res-tall/x130.py s3.asm"""
import re, sys
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
ins = []
for l in open(sys.argv[1]):
    m = pat.match(l)
    if m:
        ins.append((int(m.group(1), 16), m.group(2).strip()))
for i, (a, s) in enumerate(ins):
    m = re.match(r'shl\s+(\w+),0x6$', s)
    if not m:
        continue
    r = m.group(1)
    add = None
    for j in range(i + 1, min(i + 4, len(ins))):
        m2 = re.match(rf'add\s+{r},(\w+)$', ins[j][1])
        if m2:
            add = (j, m2.group(1))
            break
    if not add:
        print(f'{a:x}: NOADD {s}')
        continue
    src = add[1]
    lea = None
    for j in range(add[0] + 1, min(add[0] + 8, len(ins))):
        m3 = re.match(rf'lea\s+(\w+),\[(\w+)\+{r}\*2\]$', ins[j][1])
        if m3:
            lea = (j, m3.group(1), m3.group(2))
            break
        if re.match(rf'\w+\s+{r},', ins[j][1]) and not ins[j][1].startswith('lea'):
            pass
    if not lea:
        nxt = ' | '.join(x[1] for x in ins[add[0] + 1: add[0] + 4])
        print(f'{a:x}: x65 src {src} NO-LEA*2  next: {nxt}')
        continue
    kind = 'x131' if lea[2] == src else 'GRID?'
    print(f'{a:x}: {kind} {lea[1]} = {lea[2]} + {src}*130   (lea at {ins[lea[0]][0]:x})')
