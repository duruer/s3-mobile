"""findstock.py DUMPDIR : search snapshot for a window of small counters matching the panel inventory multiset."""
import os, sys, numpy as np
from collections import Counter
need = Counter({25: 2, 15: 2, 10: 1, 6: 1, 8: 1, 7: 1, 4: 1, 2: 1, 5: 2, 1: 2})
d = sys.argv[1]
for f in sorted(os.listdir(d)):
    base = int(f[:8], 16); raw = open(os.path.join(d, f), 'rb').read()
    for dt, w in (('<i4', 4), ('<i2', 2), ('u1', 1)):
        a = np.frombuffer(raw[:len(raw) // w * w], dtype=dt).astype(np.int64)
        n = 40
        if len(a) < n: continue
        # quick filter: positions where value 25 occurs
        for i in np.nonzero(a == 25)[0]:
            lo = max(0, i - n); win = a[lo:lo + 2 * n]
            if win.max() > 40 or win.min() < 0: continue
            c = Counter(win.tolist())
            if all(c[k] >= v for k, v in need.items()):
                print(f, dt, hex(base + lo * w), win.tolist()); break
