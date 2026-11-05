"""rtti.py VTABLE... : print MSVC RTTI class name for a vtable address in S3.EXE (and first vtable slots)."""
import struct, sys
b = open('~/Projects/settlers-3/game-files/gog-s3-ultimate/S3.EXE', 'rb').read()
def u32(va): return struct.unpack_from('<I', b, va - 0x400000)[0]
def cstr(va):
    o = va - 0x400000; e = b.index(b'\0', o); return b[o:e].decode('latin1')
for a in sys.argv[1:]:
    vt = int(a, 16)
    try:
        col = u32(vt - 4); td = u32(col + 12); name = cstr(td + 8)
    except Exception as e:
        name = f'? ({e})'
    slots = [hex(u32(vt + 4 * i)) for i in range(int(__import__('os').environ.get('N', '6')))]
    print(hex(vt), name, ' '.join(slots))
