#!/usr/bin/env python3
"""Diff raw table dumps of two lockstep peers (`S3_MP_DUMP_STEP`, see port/crates/s3-desktop/src/mp_hash.rs).

usage: python3 -I tools/mp_dump_diff.py DIR STEP   (reads DIR/host.STEP.<table>.bin and DIR/join.STEP.<table>.bin)
Prints, per table, the differing byte offsets within an entry and a few examples (entry, host, join).
"""
import os
import sys

ENTRY = {"settlers": 0x40, "buildings": 0x3a, "econ": 0x2e0, "players": 0x148, "world": 0x18}


def main():
    d, step = sys.argv[1], sys.argv[2]
    for name, sz in ENTRY.items():
        pa, pb = os.path.join(d, f"host.{step}.{name}.bin"), os.path.join(d, f"join.{step}.{name}.bin")
        if not (os.path.exists(pa) and os.path.exists(pb)):
            continue
        a, b = open(pa, "rb").read(), open(pb, "rb").read()
        diffs = {}
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                diffs.setdefault(i % sz, []).append((i // sz, x, y))
        print(f"{name}: {len(a)} vs {len(b)} bytes, differing offsets: "
              + (", ".join(f"{k:#x}({len(v)})" for k, v in sorted(diffs.items())) or "none"))
        for k, v in sorted(diffs.items())[:12]:
            print(f"   +{k:#05x}: " + " ".join(f"[{e}] {x:#x}/{y:#x}" for e, x, y in v[:6]))


if __name__ == "__main__":
    main()
