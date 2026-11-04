#!/usr/bin/env python3
"""Print PE imports grouped by DLL as markdown. Usage: python3 -I pe_imports.py <file.exe>"""
import sys, pefile
pe = pefile.PE(sys.argv[1], fast_load=False)
for e in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
    names = [(i.name.decode() if i.name else f"#{i.ordinal}") for i in e.imports]
    print(f"## {e.dll.decode()} ({len(names)})\n\n" + " ".join(names) + "\n")
