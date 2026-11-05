"""findstock2.py DUMPDIR WIDTH: windows (64 items) containing the panel inventory values, others unconstrained."""
import os, sys, numpy as np
from collections import Counter
need = Counter({25: 2, 15: 2, 10: 1, 6: 1, 8: 1, 7: 1, 4: 1, 2: 1, 5: 2})
d = sys.argv[1]; w = int(sys.argv[2]); dt = {4: '<i4', 2: '<i2', 1: 'u1'}[w]
for f in sorted(os.listdir(d)):
    base = int(f[:8], 16); raw = open(os.path.join(d, f), 'rb').read()
    a = np.frombuffer(raw[:len(raw) // w * w], dtype=dt).astype(np.int64)
    pos = np.nonzero(a == 25)[0]; last = -999
    for i in pos:
        if i - last < 64: continue
        win = a[max(0, i - 63):i + 64]
        c = Counter(win.tolist())
        if all(c[k] >= v for k, v in need.items()) and c[0] < 100 and sum(1 for x in win if 0 <= x <= 40) > 40:
            print(f, hex(base + max(0, i - 63) * w), win.tolist()[:80]); last = i
