"""Diff guest memory dumps: python3 -I diff.py BASE d1 d2 d3 ... ; lists 16/32-bit words stable in d1==d2
but changed later, with their value sequence. BASE = guest address of dump start (hex)."""
import sys, struct
base = int(sys.argv[1], 16)
ds = [open(p, 'rb').read() for p in sys.argv[2:]]
maxn = int(__import__('os').environ.get('MAXN', '400'))
n = 0
for off in range(0, len(ds[0]) - 3, 4):
    vals = [struct.unpack_from('<i', d, off)[0] for d in ds]
    if vals[0] != vals[1]:
        continue
    if len(set(vals)) == 1:
        continue
    print(f"{base+off:#x}: " + " ".join(str(v) for v in vals))
    n += 1
    if n >= maxn:
        break
