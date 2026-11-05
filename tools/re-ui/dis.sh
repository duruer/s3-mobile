#!/bin/sh
# dis.sh ADDR [N]  - print N lines of objdump disassembly from ADDR (scratch/re-ui/s3.asm)
grep -n -m1 "^  $1:" ~/Projects/settlers-3/scratch/re-ui/s3.asm | cut -d: -f1 | { read l; sed -n "${l},$((l+${2:-60}))p" ~/Projects/settlers-3/scratch/re-ui/s3.asm; }
