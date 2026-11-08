#!/usr/bin/env python3
"""Print an address range of an objdump listing (`--no-show-raw-insn -M intel`).
Usage: python -I tools/res-tall/dis.py s3.asm START END"""
import re, sys
a0, a1 = int(sys.argv[2], 16), int(sys.argv[3], 16)
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
for l in open(sys.argv[1]):
    m = pat.match(l)
    if not m:
        continue
    a = int(m.group(1), 16)
    if a0 <= a < a1:
        print(f'{a:x}: {m.group(2)}')
    elif a >= a1:
        break
