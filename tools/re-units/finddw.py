#!/usr/bin/env python3
"""Find little-endian dwords in a PE image (S3.EXE) and print their VAs. Usage: finddw.py EXE 0xVAL [...]"""
import struct, sys
data = open(sys.argv[1], 'rb').read()
# section table: VA = file offset + 0x400000 for S3.EXE (all sections file-aligned to VA - 0x400000)
for v in sys.argv[2:]:
    pat = struct.pack('<I', int(v, 16))
    i = data.find(pat)
    hits = []
    while i >= 0:
        hits.append(hex(i + 0x400000))
        i = data.find(pat, i + 1)
    print(v, hits[:20])
