#!/usr/bin/env python3
"""Print length and FNV-1a 64 hash (s3_cpu::code_hash) of guest code ranges of a PE (for NativeBlock entries).

usage: python3 -I tools/perf/code_hash.py S3.EXE START END [START END ...]   (hex VAs, END exclusive)
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
    b = open(sys.argv[1], "rb").read()
    secs = sections(b)
    a = sys.argv[2:]
    for i in range(0, len(a), 2):
        s, e = int(a[i], 16), int(a[i + 1], 16)
        print(f"{s:#x}..{e:#x} len {e - s} hash {fnv(read(b, secs, s, e - s)):#018x}")


if __name__ == "__main__":
    main()
