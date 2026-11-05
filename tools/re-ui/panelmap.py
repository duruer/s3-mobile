"""panelmap.py ASM START END : walk the HandleInput panel-click code linearly and print, for every write of a
panel-state field (gd+0x2d284 page, +0x2d288 category, +0x2d28c slot, +0x2d298 subpage, +0x2d2f0 building type,
logic+0x54 tooltip id), the most recent grid column/row range checks (edx = column, ebp = row) and race checks.
Heuristic (linear, ignores control flow) - verify individual entries by reading the code."""
import re, sys
asm, lo, hi = sys.argv[1], int(sys.argv[2], 16), int(sys.argv[3], 16)
st = {'col': [None, None], 'row': [None, None], 'race': None, 'page': None, 'cat': None}
line_re = re.compile(r'^\s+([0-9a-f]+):\s+(.*)$')
for ln in open(asm):
    m = line_re.match(ln)
    if not m: continue
    a = int(m.group(1), 16)
    if a < lo: continue
    if a > hi: break
    ins = m.group(2).split('  ')[-1].strip() if False else m.group(2)
    ins = re.sub(r'\s+', ' ', ins)
    c = re.search(r'cmp (edx|ebp|eax|ebx),(0x[0-9a-f]+|\w+)$', ins)
    if c and c.group(2).startswith('0x'):
        v = int(c.group(2), 16)
        k = 'col' if c.group(1) == 'edx' else 'row' if c.group(1) == 'ebp' else None
        if k: st[k + '_last'] = v
    j = re.search(r'(jl|jg|jge|jle) ', ins)
    if j and 'col_last' in st:
        pass
    cm = re.search(r'cmp (\w+),(0x[0-9a-f]+)', ins)
    if cm and cm.group(1) in ('edx', 'ebp'):
        k = 'col' if cm.group(1) == 'edx' else 'row'
        pend = (k, int(cm.group(2), 16)); st['pend'] = pend
    elif j and st.get('pend'):
        k, v = st['pend']
        if j.group(1) in ('jl',): st[k][0] = v
        elif j.group(1) in ('jg',): st[k][1] = v
        st['pend'] = None
    if '0x11943e0]' in ins and 'cmp' in ins:
        r = re.search(r',(0x[0-9a-f]+|ebx|edi)$', ins); st['race'] = r.group(1) if r else '?'
    if re.search(r'cmp DWORD PTR \[e\w\w\+0x2d284\]', ins): st['page'] = ins.split(',')[-1]
    if re.search(r'cmp DWORD PTR \[e\w\w\+0x2d288\]', ins): st['cat'] = ins.split(',')[-1]
    w = re.search(r'mov DWORD PTR \[e\w\w\+(0x2d284|0x2d288|0x2d28c|0x2d298|0x2d2f0|0x54)\],(\w+)', ins)
    if w:
        print(f"{a:x} {w.group(1)}={w.group(2):6} col{st['col']} row{st['row']} page?{st['page']} cat?{st['cat']} race?{st['race']}")
