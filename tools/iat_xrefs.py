#!/usr/bin/env python3
"""List every instruction in S3.EXE .text that references an IAT slot (call [iat], mov reg,[iat], jmp [iat]).
usage: python3 -I tools/iat_xrefs.py <S3.EXE> [name-regex]   (reads local exe; prints addresses only)"""
import sys, re, subprocess, pefile
exe = sys.argv[1]
flt = re.compile(sys.argv[2]) if len(sys.argv) > 2 else None
pe = pefile.PE(exe)
iat = {}
for d in pe.DIRECTORY_ENTRY_IMPORT:
    for i in d.imports:
        iat[i.address] = (d.dll.decode() + '!' + (i.name.decode() if i.name else '#%d' % i.ordinal))
text = pe.sections[0]
start = pe.OPTIONAL_HEADER.ImageBase + text.VirtualAddress
p = subprocess.Popen(['objdump', '-d', '-M', 'intel', '--start-address=%d' % start,
                      '--stop-address=%d' % (start + text.Misc_VirtualSize), exe], stdout=subprocess.PIPE, text=True)
rx = re.compile(r'ds:0x([0-9a-f]+)')
for line in p.stdout:
    m = rx.search(line)
    if not m: continue
    a = int(m.group(1), 16)
    if a in iat and (flt is None or flt.search(iat[a])):
        parts = line.split('\t')
        print(parts[0].strip().rstrip(':'), iat[a], parts[-1].strip())
