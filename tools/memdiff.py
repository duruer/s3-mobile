#!/usr/bin/env python3
"""Compare two s3det --dump files ([u32 addr][4096 bytes] records); print differing byte ranges.

usage: python3 -I tools/memdiff.py a.bin b.bin [max_ranges]
"""
import struct
import sys


def load(p):
    pages = {}
    with open(p, 'rb') as f:
        data = f.read()
    for off in range(0, len(data), 4100):
        addr = struct.unpack_from('<I', data, off)[0]
        pages[addr] = data[off + 4:off + 4100]
    return pages


def main():
    a, b = load(sys.argv[1]), load(sys.argv[2])
    limit = int(sys.argv[3]) if len(sys.argv) > 3 else 60
    only = sorted(set(a) ^ set(b))
    if only:
        print(f'pages only in one dump: {len(only)} (first {[hex(x) for x in only[:8]]})')
    ranges = []
    for addr in sorted(set(a) & set(b)):
        pa, pb = a[addr], b[addr]
        if pa == pb:
            continue
        i = 0
        while i < 4096:
            if pa[i] != pb[i]:
                j = i
                while j < 4096 and pa[j] != pb[j]:
                    j += 1
                ranges.append((addr + i, pa[i:j], pb[i:j]))
                i = j
            else:
                i += 1
    print(f'differing ranges: {len(ranges)}, bytes: {sum(len(r[1]) for r in ranges)}')
    for addr, x, y in ranges[:limit]:
        print(f'{addr:#010x} len={len(x):4}  a={x[:24].hex()}  b={y[:24].hex()}')


main()
