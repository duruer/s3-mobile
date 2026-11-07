#!/usr/bin/env python3
"""Print length and FNV-1a 64 hash (s3_cpu::code_hash) of guest code ranges of a PE (for NativeBlock entries).

usage: python3 -I tools/perf/code_hash.py [--concat] S3.EXE START END [START END ...]   (hex VAs, END exclusive)
--concat: one hash over all ranges concatenated (fast_tile.rs OBJ_HASH).
Prints no code bytes (game code must not end up in our sources)."""
import struct
import sys


def sections(b):
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    n = struct.unpack_from("<H", b, pe + 6)[0]
    opt = struct.unpack_from("<H", b, pe + 20)[0]
    base = struct.unpack_from("<I", b, pe + 24 + 28)[0]
    off = pe + 24 + opt
    out = []
    for i in range(n):
        s = off + 40 * i
        vsize, va, rsize, raw = struct.unpack_from("<IIII", b, s + 8)
        out.append((base + va, max(vsize, rsize), raw, rsize))
    return out


def read(b, secs, va, n):
    for sva, size, raw, rsize in secs:
        if sva <= va and va + n <= sva + size:
            return b[raw + va - sva: raw + va - sva + n]
    raise SystemExit(f"{va:#x} not in a section")


def fnv(data):
    h = 0xCBF29CE484222325
    for x in data:
        h ^= x
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def main():
    args = sys.argv[1:]
    concat = args[:1] == ["--concat"]
    if concat:
        args = args[1:]
    b = open(args[0], "rb").read()
    secs = sections(b)
    a = args[1:]
    parts = []
    for i in range(0, len(a), 2):
        s, e = int(a[i], 16), int(a[i + 1], 16)
        parts.append(read(b, secs, s, e - s))
        if not concat:
            print(f"{s:#x}..{e:#x} len {e - s} hash {fnv(parts[-1]):#018x}")
    if concat:
        print(f"{len(parts)} ranges, {sum(map(len, parts))} bytes, hash {fnv(b''.join(parts)):#018x}")


if __name__ == "__main__":
    main()
