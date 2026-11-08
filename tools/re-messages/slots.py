#!/usr/bin/env python3
"""Decode the in-game message ring dumped from logic+0x414 (5 entries x 0x1c; s3run `xdump 0x700ffee0@0x414 0x8c F`).
usage: python -I tools/re-messages/slots.py FILE...
Entry: +0 string object (+4 ptr to UTF-16, +8 length, +0xc capacity), +0x10 timer (ticks), +0x14 icon, +0x18 dirty.
Only pointers/lengths are decoded (the text lives on the guest heap; read it with xpeek)."""
import struct, sys
for f in sys.argv[1:]:
    b = open(f, 'rb').read()
    print(f)
    for i in range(len(b) // 0x1c):
        o = i * 0x1c
        _, p, l, c, t, ic = struct.unpack_from('<IIIIii', b, o)
        print('  slot %d ptr=%08x len=%3d timer=%7d icon=%#x dirty=%d' % (i, p, l, t, ic & 0xffffffff, b[o + 0x18]))
