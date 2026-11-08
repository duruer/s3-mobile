#!/usr/bin/env python3
"""Group instruction addresses (first word of each line of a file, hex, `addr:` ok) by enclosing function
(= the nearest preceding direct-call target in the listing) and print start, end (next call target) and count.
Usage: python -I tools/res-tall/funcs.py s3.asm sites.txt"""
import bisect, re, sys
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
call = re.compile(r'^call\s+0x([0-9a-f]+)$')
starts = set()
for l in open(sys.argv[1]):
    m = pat.match(l)
    if m:
        c = call.match(m.group(2))
        if c:
            starts.add(int(c.group(1), 16))
starts = sorted(starts)
groups = {}
for l in open(sys.argv[2]):
    w = l.split()
    if not w:
        continue
    a = int(w[0].rstrip(':'), 16)
    i = bisect.bisect_right(starts, a) - 1
    s = starts[i]
    e = starts[i + 1] if i + 1 < len(starts) else 0
    groups.setdefault((s, e), []).append(a)
for (s, e), v in sorted(groups.items()):
    print(f'{s:x} {e:x} {len(v)}')
