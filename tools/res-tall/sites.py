#!/usr/bin/env python3
"""For each (address, value) pair print the byte offset of the 32-bit little-endian `value` inside the
instruction at `address` of the GOG S3.EXE (instruction length from the objdump listing). `value` may be negative.
For `shl r,6` sites use value `shl6`: prints the offset of the imm8 6 (`c1 /4 06`). Reads the exe as data only;
prints no code bytes.
Usage: python -I tools/res-tall/sites.py S3.EXE s3.asm addr:value [addr:value ...]"""
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


pat = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
addrs = []
for l in open(sys.argv[2]):
    m = pat.match(l)
    if m:
        addrs.append(int(m.group(1), 16))
nxt = {addrs[i]: addrs[i + 1] for i in range(len(addrs) - 1)}
for item in sys.argv[3:]:
    a, v = item.split(':')
    a = int(a, 16)
    n = nxt[a] - a
    b = rd(a, n)
    if v == 'shl6':
        ok = n == 3 and b[0] == 0xc1 and (b[1] >> 3) & 7 == 4 and b[2] == 6
        print(f'{a:#x} shl6 len {n} {"OK" if ok else "MISMATCH"}')
        continue
    val = int(v, 16) & 0xffffffff
    offs = [k for k in range(n - 3) if struct.unpack_from('<I', b, k)[0] == val]
    print(f'{a:#x} off {offs} len {n} value {val:#x}')
