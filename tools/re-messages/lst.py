#!/usr/bin/env python3
"""Print N lines of a local objdump listing from a VA, or grep it for a pattern (local analysis aid).
usage: python -I tools/re-messages/lst.py [ASM] ADDR [N]        (ADDR hex, no 0x)
       python -I tools/re-messages/lst.py ASM -g REGEX [MAX]  (lines matching REGEX)
The listing (scratch/re-ui/s3.asm, `objdump -d -M intel` of S3.EXE) is game-derived: keep it in scratch/."""
import sys, re
if not sys.argv[1].endswith('.asm'):
    sys.argv.insert(1, 'C:/Users/<user>/Projects/settlers-3/scratch/re-ui/s3.asm')
asm = sys.argv[1]
if sys.argv[2] == '-g':
    rx = re.compile(sys.argv[3]); mx = int(sys.argv[4]) if len(sys.argv) > 4 else 200
    n = 0
    with open(asm, errors='replace') as f:
        for l in f:
            if rx.search(l):
                print(l.rstrip()[:140]); n += 1
                if n >= mx: break
    sys.exit()
key = '  %s:' % sys.argv[2].lower().lstrip('0x') if not sys.argv[2].startswith('0x') else '  %s:' % sys.argv[2][2:]
tgt = int(key.strip().rstrip(':'), 16)
cnt = int(sys.argv[3]) if len(sys.argv) > 3 else 80
out = None
with open(asm, errors='replace') as f:
    for l in f:
        if out is None:
            m0 = re.match(r'\s+([0-9a-f]+):', l)
            if m0 and int(m0.group(1), 16) >= tgt:
                out = 0
        if out is not None:
            s = l.rstrip()
            # drop raw bytes column
            m = re.match(r'\s*([0-9a-f]+):\t(?:[0-9a-f]{2} )+\s*\t?(.*)', s)
            print((m.group(1) + ': ' + m.group(2)) if m else s[:140])
            out += 1
            if out >= cnt: break
