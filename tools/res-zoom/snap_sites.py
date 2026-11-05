#!/usr/bin/env python3
"""List the code sites that reference the trailing fields of the render snapshot (offsets 0xc65c0..0xc7720)
in the GOG S3.EXE, with the byte offset of the 32-bit displacement/immediate inside each instruction.
Usage: python3 -I tools/res-zoom/snap_sites.py S3.EXE scratch/re-ui/s3.asm
Output: Rust table lines `(addr, disp_off, value)`. Reads the exe as data only."""
import re, struct, sys

exe = open(sys.argv[1], 'rb').read()
pe = struct.unpack_from('<I', exe, 0x3c)[0]
nsec = struct.unpack_from('<H', exe, pe + 6)[0]
optsz = struct.unpack_from('<H', exe, pe + 20)[0]
base = struct.unpack_from('<I', exe, pe + 24 + 28)[0]
secs = []
for i in range(nsec):
    o = pe + 24 + optsz + i * 40
    vsz, va, rsz, raw = struct.unpack_from('<IIII', exe, o + 8)
    secs.append((base + va, max(vsz, rsz), raw))

def rd(va, n):
    for s, sz, raw in secs:
        if s <= va < s + sz:
            return exe[raw + va - s: raw + va - s + n]
    raise KeyError(hex(va))

lines = open(sys.argv[2]).read().splitlines()
pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
insns = []
for l in lines:
    m = pat.match(l)
    if m:
        insns.append((int(m.group(1), 16), m.group(2)))
val = re.compile(r'0x(c6[5-9a-f][0-9a-f]{2}|c7[0-7][0-9a-f]{2})\b')
for i, (a, txt) in enumerate(insns):
    if not (0x401000 <= a < 0x630000):
        continue
    for v in val.findall(txt):
        v = int(v, 16)
        if not (0xc65c0 <= v <= 0xc7720):
            continue
        if 'fc],0xc82828' in txt:
            continue
        n = insns[i + 1][0] - a
        b = rd(a, n)
        offs = [k for k in range(n - 3) if struct.unpack_from('<I', b, k)[0] == v]
        if len(offs) != 1:
            print(f'// AMBIGUOUS {a:#x} {txt} {offs}')
            continue
        print(f'    ({a:#x}, {offs[0]}, {v:#x}), // {txt.strip()}')
