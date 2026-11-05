"""strings.py DUMPDIR GFXOBJ ID... : resolve S3 text ids (table<<16 | index) like 0x5faaa0 does, from a dumpall
snapshot. GFXOBJ = render screen object (vtable 0x65a520); string table = [GFXOBJ+0x5f6b8].
ID 'all:T' prints every string of table T. Output is game text (data) - local use only, do not commit it."""
import os, struct, sys
d = sys.argv[1]
regs = []
for f in os.listdir(d):
    regs.append((int(f[:8], 16), open(os.path.join(d, f), 'rb').read()))
def rd(a, n=4):
    for b, data in regs:
        if b <= a and a + n <= b + len(data):
            return data[a - b:a - b + n]
    raise KeyError(hex(a))
u32 = lambda a: struct.unpack('<I', rd(a))[0]
u16 = lambda a: struct.unpack('<H', rd(a, 2))[0]
def cstr(a):
    out = b''
    while True:
        c = rd(a + len(out), 1)
        if c == b'\0' or len(out) > 300: return out.decode('cp1252', 'replace')
        out += c
gfx = int(sys.argv[2], 16); st = u32(gfx + 0x5f6b8); lang = u32(st + 0xc40)
def text(i):
    t, ix = i >> 16, i & 0xffff
    hdr = u32(st + 0x37c + t * 4); n = u16(hdr + 8)
    return cstr(u32(hdr + 0xc + (n * lang + ix) * 4) + u32(st + 0x264 + t * 4))
print('strtab', hex(st), 'lang', lang)
for a in sys.argv[3:]:
    if a.startswith('all:'):
        t = int(a[4:], 0); hdr = u32(st + 0x37c + t * 4); n = u16(hdr + 8)
        for ix in range(n):
            try: print(hex((t << 16) | ix), repr(text((t << 16) | ix)))
            except KeyError as e: print(hex((t << 16) | ix), 'unmapped', e)
    else:
        i = int(a, 16); print(hex(i), repr(text(i)))
