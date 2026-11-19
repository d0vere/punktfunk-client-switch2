#!/usr/bin/env python3
"""Strict, read-only ELF/NRO structural checks; not an execution test."""
from pathlib import Path
import argparse, hashlib, struct, sys

def check_elf(p: Path):
    b=p.read_bytes()
    if len(b)<64 or b[:4]!=b'\x7fELF': raise ValueError('Not an ELF')
    if b[4]!=2 or b[5]!=1: raise ValueError('ELF is not 64-bit little endian')
    machine=struct.unpack_from('<H',b,18)[0]
    if machine!=183: raise ValueError(f'ELF machine {machine} != AArch64 (183)')
    e_type=struct.unpack_from('<H',b,16)[0]
    if e_type not in (2,3): raise ValueError(f'ELF type {e_type} not executable/shared')
    ph_off=struct.unpack_from('<Q',b,32)[0]
    ph_count=struct.unpack_from('<H',b,56)[0]
    ph_size=struct.unpack_from('<H',b,54)[0]
    if not ph_count or ph_size<56 or ph_off+ph_count*ph_size>len(b):
        raise ValueError('Invalid program headers')
    for i in range(ph_count):
        base=ph_off+i*ph_size
        off=struct.unpack_from('<Q',b,base+8)[0]
        sz=struct.unpack_from('<Q',b,base+32)[0]
        if off+sz>len(b): raise ValueError(f'Program header #{i} outside file')

def check_nro(p: Path):
    b=p.read_bytes()
    if len(b)<0x80 or b[0x10:0x14]!=b'NRO0': raise ValueError('Missing NRO0 header at offset 0x10')
    size=struct.unpack_from('<I',b,0x18)[0]
    if size<0x80 or size>len(b): raise ValueError(f'NRO declared size invalid: {size}')
    if len(b)>1024*1024*1024: raise ValueError('NRO >1GiB; rejected')

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--elf',type=Path)
    ap.add_argument('--nro',type=Path)
    a=ap.parse_args()
    if not a.elf and not a.nro: ap.error('Specify --elf and/or --nro')
    try:
        for kind,fn in [('ELF',a.elf),('NRO',a.nro)]:
            if fn is None:continue
            if not fn.is_file():raise ValueError(f'{fn} not found')
            (check_elf if kind=='ELF' else check_nro)(fn)
            print(f'{kind}: structural pass; sha256={hashlib.sha256(fn.read_bytes()).hexdigest()}')
    except (ValueError,OSError) as exc:
        print('REJECTED:',exc,file=sys.stderr)
        return 1
    return 0
if __name__=='__main__':sys.exit(main())
