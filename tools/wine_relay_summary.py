# SPDX-License-Identifier: GPL-3.0-or-later
"""Summarize a wine +relay log: keep calls whose ret= is inside S3.EXE (0x400000-0x800000) or _INMM.dll.
Usage: python3 -I wine_relay_summary.py <log> [start_byte] [end_byte] [out.txt] [regex]  (counts go to stderr)
See docs/reverse-engineering/wine-trace.md."""
import re,sys,collections
log=sys.argv[1]; start=int(sys.argv[2]) if len(sys.argv)>2 else 0; end=int(sys.argv[3]) if len(sys.argv)>3 else 1<<60
pat=re.compile(r'^([0-9.]+):([0-9a-f]+):Call ([\w.]+)\((.*)\) ret=([0-9a-f]+)$')
rpat=re.compile(r'^([0-9.]+):([0-9a-f]+):Ret  ([\w.]+)\(\) retval=([0-9a-f]+) ret=([0-9a-f]+)$')
def ours(r):
    r=int(r,16); return 0x400000<=r<0x800000 or 0x6AD40000<=r<0x6AE00000
cnt=collections.Counter(); out=open(sys.argv[4] if len(sys.argv)>4 else '/dev/stdout','w')
interesting=re.compile(sys.argv[5] if len(sys.argv)>5 else r'Reg|CreateFile|FindFirst|LoadLibrary|GetProcAddress|CreateThread|timeSetEvent|mci|GetDrive|Volume|CoCreate|Create\w*Window|SetThreadPriority|CreateProcess|DirectSound|DirectInput|DirectDraw|DirectPlay|MessageBox|CreateDirectory|DeleteFile|MoveFile|CopyFile|SetCurrentDirectory|GetModuleFileName|ChangeDisplay|GetSystemMetrics|GetDeviceCaps|CreateFont|ShowCursor|ClipCursor|wsock|socket|gethost|CreateDIBSection')
pos=0; pend={}
with open(log,'rb') as f:
    f.seek(start)
    for raw in f:
        pos+=len(raw)
        if start+pos>end: break
        l=raw.decode('latin1').rstrip('\n')
        m=pat.match(l)
        if m:
            if ours(m.group(5)):
                fn=m.group(3); cnt[fn]+=1
                if interesting.search(fn): out.write('%s %s %s(%s) ret=%s\n'%(m.group(1),m.group(2),fn,m.group(4)[:300],m.group(5))); pend[(m.group(2),fn)]=1
            continue
        m=rpat.match(l)
        if m and (m.group(2),m.group(3)) in pend:
            del pend[(m.group(2),m.group(3))]; out.write('    -> %s\n'%m.group(4))
print('=== counts',file=sys.stderr)
for k,v in sorted(cnt.items(),key=lambda x:-x[1]): print('%7d %s'%(v,k),file=sys.stderr)
