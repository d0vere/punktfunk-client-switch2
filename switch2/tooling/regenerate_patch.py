#!/usr/bin/env python3
"""Emit only source-addition git patch, not a claim of complete frontend integration."""
from pathlib import Path
import difflib, hashlib

ROOT=Path(__file__).resolve().parents[1]
chunks=[]
for f in sorted((ROOT/'native').iterdir()):
    if not f.is_file(): continue
    relative='ports/switch2/native/'+f.name
    contents=f.read_text()
    raw=f.read_bytes()
    blob=hashlib.sha1(f'blob {len(raw)}\0'.encode()+raw).hexdigest()[:7]
    header=f'diff --git a/{relative} b/{relative}\nnew file mode 100644\nindex 0000000..{blob}\n'
    diff=''.join(difflib.unified_diff([], contents.splitlines(keepends=True),
                                      fromfile='/dev/null', tofile=f'b/{relative}', lineterm='\n'))
    chunks.append(header+diff)
( ROOT/'patches'/'native-additions.patch').write_text(''.join(chunks))
print('Patch written: patches/native-additions.patch')
