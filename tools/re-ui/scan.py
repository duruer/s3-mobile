"""Find words in dumpall snapshots matching a change pattern.
python3 -I scan.py DIR_A DIR_B ... -- PATTERN  where PATTERN is a string of letters per snapshot:
same letter = equal value; for 'lin' mode values must step by a constant: use 'L' (e.g. 'aaLLL' means
snap0==snap1 then snap1..4 form a non-constant arithmetic progression).
Simple mode: pattern like 'aabcc' (equal letters == equal values, different letters == different values)."""
import sys, os, numpy as np
args = sys.argv[1:]
k = args.index('--'); dirs = args[:k]; pat = args[k+1]; width = int(args[k+2]) if len(args) > k+2 else 4
files = sorted(os.listdir(dirs[0]))
dt = {4: '<i4', 2: '<i2'}[width]
hits = 0
for f in files:
    base = int(f[:8], 16)
    raws = [open(os.path.join(d, f), 'rb').read() for d in dirs]
    for align in range(0, 4, width):
        arrs = []
        for r in raws:
            n = (len(r) - align) // width
            arrs.append(np.frombuffer(r, dtype=dt, count=n, offset=align).astype(np.int64))
        m = np.ones(len(arrs[0]), bool)
        for i in range(len(pat)):
            for j in range(i+1, len(pat)):
                if pat[i] == pat[j]:
                    m &= arrs[i] == arrs[j]
                else:
                    m &= arrs[i] != arrs[j]
        idx = np.nonzero(m)[0]
        for i in idx[:200]:
            print(f"{base + align + i*width:#x}: " + " ".join(str(a[i]) for a in arrs))
        hits += len(idx)
print("hits", hits, file=sys.stderr)
