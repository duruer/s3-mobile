#!/usr/bin/env python3
"""Compare two `s3run --mp-hash` files (one per lockstep peer) step by step.

usage: python3 -I tools/mp_hash_compare.py host.hash join.hash
Lines look like `step=TICK rng=.. n=a/b/c settlers=.. buildings=.. ... [world=..] frame=F local=P`
(TICK = simulation ticks run so far). `frame=` (the game's frame counter, keeps running while the lockstep layer
pauses the simulation) and `local=` (local player) differ per peer by design and are not compared.
`exec at=TICK frame=F msg=HEX` lines (logic messages executed before tick TICK) are compared in order, except
local class-0x02/0x22 messages. Exit code 0 = no differing step, 1 = desync, 2 = nothing to compare, 3 = only world tiles differ.
"""
import sys


def load_exec(path):
    out = []
    with open(path, encoding="ascii", errors="replace") as f:
        for line in f:
            if line.startswith("exec "):
                kv = dict(p.split("=", 1) for p in line.split()[1:])
                kv.pop("frame", None)
                # Class 0x02 (timing) and 0x22 (a 0x58-byte message with a per-peer value in bytes 4..5, executed at
                # slightly different ticks on each peer) are local messages, not lockstep traffic (OBSERVED;
                # meaning HYPOTHESIS). Everything else (class 0x01: player commands, 0xfb sync) must match exactly.
                msg = kv["msg"]
                if msg.startswith("02") or msg.startswith("22"):
                    continue
                out.append((int(kv["at"]), msg))
    return out


def load(path):
    out = {}
    with open(path, encoding="ascii", errors="replace") as f:
        for line in f:
            if line.startswith("exec "):
                continue
            kv = dict(p.split("=", 1) for p in line.split() if "=" in p)
            if "step" not in kv:
                continue
            step = int(kv.pop("step"))
            kv.pop("local", None)
            kv.pop("frame", None)
            # Keep the first line per step (a step is hashed once per peer).
            out.setdefault(step, kv)
    return out


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    a, b = load(sys.argv[1]), load(sys.argv[2])
    ea, eb = load_exec(sys.argv[1]), load_exec(sys.argv[2])
    n = min(len(ea), len(eb))
    first_bad = next((i for i in range(n) if ea[i] != eb[i]), None)
    cmds = sum(1 for _, m in ea if m.startswith("01") and not m.startswith("01fb"))
    print(f"executed messages: a {len(ea)}, b {len(eb)} ({cmds} player commands in a); "
          + ("same order and steps" if first_bad is None else
             f"FIRST DIFFERENCE at #{first_bad}: a {ea[first_bad]} b {eb[first_bad]}"))
    common = sorted(set(a) & set(b))
    if not common:
        print(f"no common steps (a: {len(a)} lines, b: {len(b)} lines)")
        return 2
    bad = []
    for s in common:
        diff = [k for k in a[s] if k in b[s] and a[s][k] != b[s][k]]
        if diff:
            bad.append((s, diff))
    print(f"steps compared: {len(common)} (from {common[0]} to {common[-1]}, {len(common)} hashed ticks); "
          f"only in a: {len(set(a) - set(b))}, only in b: {len(set(b) - set(a))}")
    worlds = sum(1 for s in common if "world" in a[s] and "world" in b[s])
    print(f"world hashes compared: {worlds}")
    if not bad and first_bad is None and len(ea) == len(eb):
        print("RESULT: IDENTICAL (no desync)")
        return 0
    if not bad:
        print("RESULT: STATE IDENTICAL, but the executed message lists differ")
        return 1
    if all(d == ["world"] for _, d in bad) and first_bad is None:
        print(f"RESULT: SIMULATION TABLES IDENTICAL; world tiles differ at {len(bad)} of {worlds} world hashes "
              f"(first at tick {bad[0][0]}); diff raw dumps with S3_MP_DUMP_STEP + tools/mp_dump_diff.py")
        return 3
    s, diff = bad[0]
    print(f"RESULT: DESYNC at {len(bad)} steps; first at step {s}: {', '.join(diff)}")
    for s, diff in bad[:10]:
        print(f"  step {s}: {', '.join(diff)}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
