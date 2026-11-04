#!/usr/bin/env python3
"""Disassemble a VA range of S3.EXE with IAT slots replaced by API names (local analysis aid; prints to stdout).
usage: python3 -I tools/dis.py <S3.EXE> <start_va> <end_va>"""
import sys, re, subprocess, pefile
exe, s, e = sys.argv[1], int(sys.argv[2], 16), int(sys.argv[3], 16)
pe = pefile.PE(exe, fast_load=True); pe.parse_data_directories([1])
iat = {i.address: i.name.decode() for d in pe.DIRECTORY_ENTRY_IMPORT for i in d.imports if i.name}
out = subprocess.run(['objdump', '-d', '-M', 'intel', '--no-show-raw-insn', '--start-address=%d' % s, '--stop-address=%d' % e, exe],
                     capture_output=True, text=True).stdout.splitlines()[7:]
for l in out:
    if re.search(r'\tnop$', l) or 'int3' in l: continue
    l = re.sub(r'ds:0x([0-9a-f]+)', lambda m: '[' + iat[int(m.group(1), 16)] + ']' if int(m.group(1), 16) in iat else m.group(0), l)
    print(l.strip())
