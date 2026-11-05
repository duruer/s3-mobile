"""peek.py ADDR NWORDS DIR... : print i32 words at ADDR from dumpall snapshot dirs."""
import struct, os, sys
addr = int(sys.argv[1], 16); n = int(sys.argv[2])
for d in sys.argv[3:]:
    for f in sorted(os.listdir(d)):
        b = int(f[:8], 16); data = open(os.path.join(d, f), 'rb').read()
        if b <= addr < b + len(data):
            print(d.ljust(3), hex(addr), ' '.join(str(x) for x in struct.unpack_from('<%di' % n, data, addr - b)))
