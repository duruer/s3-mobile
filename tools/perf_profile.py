#!/usr/bin/env python3
"""Function-level guest profile from an `S3_PROFILE` dump of s3run.

usage: python3 -I tools/perf_profile.py PROFILE FUNCS [TOP]

PROFILE: lines `thread start_eip block_len entries` (s3run, env S3_PROFILE=<file>, optional S3_PROFILE_FROM_MS).
FUNCS:   lines `0xADDR name` with function start addresses (e.g. exported from REA list_procedures; keep it in
         scratch/, it is derived from the game binary). A block is attributed to the closest function start <= it.
Prints the top functions by guest instructions (entries * block length; an approximation because blocks that
exit early still count their full length).
"""
import bisect
import sys


def main():
    prof, funcs = sys.argv[1], sys.argv[2]
    top = int(sys.argv[3]) if len(sys.argv) > 3 else 40
    starts, names = [], []
    for line in open(funcs):
        a, *n = line.split()
        starts.append(int(a, 16))
        names.append(n[0] if n else a)
    order = sorted(range(len(starts)), key=lambda i: starts[i])
    starts = [starts[i] for i in order]
    names = [names[i] for i in order]
    agg, per_thread, total = {}, {}, 0
    for line in open(prof):
        t, s, ln, n = line.split()
        s, ins = int(s, 16), int(ln) * int(n)
        total += ins
        i = bisect.bisect_right(starts, s) - 1
        key = (starts[i], names[i]) if i >= 0 and s < 0x1000000 else (s, "?")
        e = agg.setdefault(key, [0, set()])
        e[0] += ins
        e[1].add(t)
        per_thread[t] = per_thread.get(t, 0) + ins
    print(f"total {total / 1e6:.1f} M instructions; per thread (M): "
          + ", ".join(f"t{k}={v / 1e6:.0f}" for k, v in sorted(per_thread.items())))
    cum = 0
    for (a, name), (ins, ts) in sorted(agg.items(), key=lambda kv: -kv[1][0])[:top]:
        cum += ins
        print(f"{100 * ins / total:6.2f}% cum {100 * cum / total:5.1f}%  {a:#08x} {name:<24} threads {','.join(sorted(ts))}")


if __name__ == "__main__":
    main()
